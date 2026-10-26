pub mod crypto;
mod transport;

#[cfg(test)]
use self::tests::ConvexClient as RelayClient;
#[cfg(not(test))]
use self::transport::Client as RelayClient;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use convex::FunctionResult;
use fs2::FileExt;
use futures::StreamExt;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::os::unix::fs::OpenOptionsExt;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::error::{EverettError, Result};
use crate::{auth, inbox, peer, send, session};
use crypto::{Device, Envelope, Keys, Parent};

const TTL: i64 = 3_600_000;
const DEADLINE: Duration = Duration::from_secs(20);

pub(super) fn bad(error: impl std::fmt::Display) -> EverettError {
    EverettError::new(2, format!("relay: {error}"))
}

pub(super) fn valid(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 160
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
    {
        return Err(bad("invalid identifier"));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Payload {
    Inbox {
        session: String,
        harness: String,
        text: String,
        from: String,
        from_harness: String,
        from_card: String,
        reply_to: String,
        hops: i64,
        #[serde(default)]
        external_untrusted: bool,
    },
    Delivered {
        message_id: String,
    },
}

impl Payload {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Inbox {
                session,
                harness,
                text,
                from,
                from_harness,
                from_card,
                reply_to,
                hops,
                ..
            } => {
                for id in [session, from] {
                    valid(id)?;
                }
                if !harness.is_empty() {
                    valid(harness)?;
                }
                if !from_harness.is_empty() {
                    valid(from_harness)?;
                }
                if !reply_to.is_empty() {
                    valid(reply_to)?;
                }
                if !(1..=inbox::MAX_HOPS).contains(hops) {
                    return Err(EverettError::new(
                        7,
                        "cross-device hop limit reached (maximum 3)",
                    ));
                }
                if text.trim().is_empty() || text.len() > peer::BODY_CAP || from_card.len() > 640 {
                    return Err(bad("empty or oversized message"));
                }
            }
            Self::Delivered { message_id } => valid(message_id)?,
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Paired {
    device: Device,
    deployment: String,
    #[serde(default)]
    grant_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Outgoing {
    peer: String,
    envelope: Envelope,
    digest: String,
    session: String,
    harness: String,
    posted: bool,
    state: State,
    #[serde(default)]
    reply_pending: Option<Parent>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Queued,
    Delivered,
    Replied,
}

const REJECTED_CACHE_CAP: usize = 4096;

/// Diagnostic duplicate suppression only. Replay protection is authoritative
/// elsewhere, so rejected rows can be forgotten once the server can no longer
/// return them (or when the bounded cache is full).
struct RejectedCache {
    entries: HashMap<String, i64>,
}

impl RejectedCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    fn prune(&mut self, now: i64) {
        self.entries.retain(|_, expiry| *expiry > now);
    }

    /// Returns whether this rejection should be logged.
    fn insert(&mut self, id: String, expiry: i64, now: i64) -> bool {
        self.prune(now);
        if self.entries.get(&id).is_some_and(|known| *known > now) {
            return false;
        }
        while self.entries.len() >= REJECTED_CACHE_CAP {
            let evicted = self
                .entries
                .keys()
                .next()
                .cloned()
                .expect("cache is nonempty");
            self.entries.remove(&evicted);
        }
        self.entries.insert(id.clone(), expiry);
        true
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Provider {
    Oidc,
    Cloudflare,
}
impl Provider {
    fn selected() -> Result<Self> {
        match crate::config::get("auth.provider", Some("EVERETT_AUTH_PROVIDER"), "oidc").as_str() {
            "oidc" => Ok(Self::Oidc),
            "cloudflare" => Ok(Self::Cloudflare),
            _ => Err(bad("auth.provider must be oidc or cloudflare")),
        }
    }
    fn transport(self) -> &'static str {
        match self {
            Self::Oidc => "convex",
            Self::Cloudflare => "cloudflare",
        }
    }
}
struct RelayAccount {
    owner: String,
    agent: String,
    name: String,
}
fn native_credential() -> Result<crate::cloudflare_auth::Credential> {
    let stored = crate::cloudflare_auth::load()?
        .ok_or_else(|| bad("run everett login --provider cloudflare first"))?;
    stored.relay_origin()?;
    Ok(stored)
}
fn relay_account() -> Result<RelayAccount> {
    match Provider::selected()? {
        Provider::Cloudflare => {
            let stored = native_credential()?;
            Ok(RelayAccount {
                owner: stored.owner,
                agent: stored.agent_id,
                name: stored.device_name,
            })
        }
        Provider::Oidc => {
            let (stored, owner) = account()?;
            let agent = claims(&stored.id_token)?["everett_agent"]
                .as_str()
                .ok_or_else(|| bad("missing agent claim"))?
                .to_string();
            Ok(RelayAccount {
                owner,
                agent,
                name: stored.device_name,
            })
        }
    }
}

fn root() -> PathBuf {
    session::home().join(
        if matches!(Provider::selected(), Ok(Provider::Cloudflare)) {
            ".everett/relay-cloudflare"
        } else {
            ".everett/relay"
        },
    )
}

pub(crate) fn state_dir() -> PathBuf {
    root()
}
fn milliseconds() -> i64 {
    (session::now() * 1000.0) as i64
}

fn initialize() -> Result<()> {
    for dir in [root(), root().join("peers"), root().join("outgoing")] {
        peer::private_dir(&dir)?;
    }
    Ok(())
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&fs::read(path).map_err(bad)?).map_err(bad)
}

fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    peer::save(path, &serde_json::to_vec_pretty(value).map_err(bad)?)?;
    // Atomic rename alone is not durable across a power loss.
    fs::File::open(
        path.parent()
            .ok_or_else(|| bad("missing parent directory"))?,
    )
    .map_err(bad)?
    .sync_all()
    .map_err(bad)
}

fn peer_path(name: &str) -> Result<PathBuf> {
    valid(name)?;
    Ok(root().join("peers").join(format!("{name}.json")))
}

pub fn configured(name: &str) -> bool {
    peer_path(name).is_ok_and(|p| p.exists())
}

pub(crate) fn same_owner_enabled() -> bool {
    Provider::selected().is_ok_and(|provider| provider == Provider::Cloudflare)
}

fn deployment() -> Result<String> {
    if Provider::selected()? == Provider::Cloudflare {
        return native_credential()?.relay_origin();
    }
    let value = crate::config::get("relay.url", Some("EVERETT_CONVEX_URL"), "");
    let url = url::Url::parse(&value).map_err(|_| {
        bad("set EVERETT_CONVEX_URL or [relay] url to your HTTPS Convex deployment")
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(bad("Convex deployment must be an HTTPS origin"));
    }
    Ok(value.trim_end_matches('/').to_string())
}

pub(crate) fn account() -> Result<(auth::Stored, String)> {
    account_refresh(false)
}

fn account_refresh(force: bool) -> Result<(auth::Stored, String)> {
    if crate::config::get("auth.provider", Some("EVERETT_AUTH_PROVIDER"), "oidc") != "oidc" {
        return Err(bad("this operation requires OIDC/Convex; the Cloudflare relay does not support the Convex registry"));
    }
    initialize()?;
    let _guard = peer::lock(&root().join("auth.lock"))?;
    let stored = auth::load().ok_or_else(|| bad("run everett login first"))?;
    let mut stored = stored;
    if force && !stored.refresh_token.is_empty() {
        stored.expires_at = 0.0;
    }
    let mut stored = auth::ensure_fresh(&stored)?;
    let mut claim = claims(&stored.id_token)?;
    if claim["exp"].as_f64().unwrap_or(0.0) - session::now() < 60.0 {
        stored.expires_at = 0.0;
        stored = auth::ensure_fresh(&stored)?;
        claim = claims(&stored.id_token)?;
    }
    let owner = checked_identity(&stored, &claim, session::now())?;
    Ok((stored, owner))
}

fn checked_identity(stored: &auth::Stored, claim: &Value, now: f64) -> Result<String> {
    let issuer = claim["iss"]
        .as_str()
        .ok_or_else(|| bad("ID token has no issuer"))?;
    if issuer.is_empty()
        || issuer.len() > 2048
        || stored.sub.is_empty()
        || stored.sub.len() > 256
        || stored.client_id.is_empty()
        || issuer != stored.issuer
        || claim["sub"] != stored.sub
        || !(claim["aud"] == stored.client_id
            || claim["aud"]
                .as_array()
                .is_some_and(|aud| aud.iter().any(|a| *a == stored.client_id)))
        || claim["iat"].as_f64().is_some_and(|at| at > now + 30.0)
        || claim["nbf"].as_f64().is_some_and(|at| at > now)
        || (claim["aud"].as_array().is_some_and(|a| a.len() > 1)
            && claim["azp"] != stored.client_id)
        || (!claim["everett_org"].is_null() && !claim["everett_org"].is_string())
        || claim["exp"].as_f64().unwrap_or(0.0) <= now
        || claim["everett_agent"]
            .as_str()
            .is_none_or(|s| valid(s).is_err())
        || claim["everett_org"]
            .as_str()
            .is_some_and(|s| s.is_empty() || s.len() > 160)
    {
        return Err(bad(
            "expired or mismatched ID token; run everett login --force",
        ));
    }
    // Claims are local consistency checks; Convex verifies signature and membership.
    let owner = if let Some(org) = claim["everett_org"].as_str() {
        json!([issuer, "org", org]).to_string()
    } else {
        json!([issuer, "account", stored.sub]).to_string()
    };
    Ok(owner)
}

pub(crate) fn claims(token: &str) -> Result<Value> {
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
        return Err(bad("OIDC ID token required"));
    }
    let part = parts[1];
    serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(part)
            .map_err(|_| bad("invalid OIDC ID token"))?,
    )
    .map_err(|_| bad("invalid OIDC ID token"))
}

#[derive(Clone, Serialize, Deserialize)]
struct PrivateKeys {
    signing: String,
    encryption: String,
}

fn daemon_lock() -> Result<fs::File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(root().join("daemon.lock"))
        .map_err(bad)?;
    file.try_lock_exclusive()
        .map_err(|_| bad("relay already running; stop it before restarting or rotating keys"))?;
    Ok(file)
}

fn verified_keys(owner: &str) -> Result<Keys> {
    let owner = owner.to_string();
    runtime(async move {
        let identity = relay_account()?;
        if identity.owner != owner {
            return Err(bad("owner changed"));
        }
        let expected_agent = identity.agent;
        client(&owner).await?;
        let keys = keys(&owner)?;
        if expected_agent != keys.device.device_id {
            return Err(bad("agent changed"));
        }
        Ok(keys)
    })
}

fn key_path(owner: &str, agent: &str) -> PathBuf {
    root().join(format!(
        "keys-{:x}.json",
        Sha256::digest(json!([owner, agent]).to_string())
    ))
}

fn keys(owner: &str) -> Result<Keys> {
    initialize()?;
    let identity = relay_account()?;
    if identity.owner != owner {
        return Err(bad("owner changed"));
    }
    let agent = identity.agent.as_str();
    valid(agent)?;
    let _guard = peer::lock(&root().join("keys.lock"))?;
    let path = key_path(owner, agent);
    let private = if path.exists() {
        read::<PrivateKeys>(&path)?
    } else {
        let mut signing = [0u8; 32];
        let mut encryption = [0u8; 32];
        OsRng.fill_bytes(&mut signing);
        OsRng.fill_bytes(&mut encryption);
        let private = PrivateKeys {
            signing: hex::encode(signing),
            encryption: hex::encode(encryption),
        };
        write(&path, &private)?;
        private
    };
    Ok(Keys::new(
        owner.into(),
        agent.into(),
        crypto::decode(&private.signing)?,
        crypto::decode(&private.encryption)?,
    ))
}

fn same_owner_device(selector: &str, owner: &str, by_name: bool) -> Result<Paired> {
    let owner = owner.to_string();
    let selector = selector.to_string();
    runtime(async move {
        let mut client = client(&owner).await?;
        let id = if let Some(id) = selector.strip_prefix("id:") {
            valid(id)?;
            id.to_string()
        } else if by_name {
            let labels = query(&mut client, "relay:deviceLabels", json!({})).await?;
            resolve_device_name(&selector, &labels)?
        } else {
            selector
        };
        let device = active_same_owner_device(&mut client, &owner, &id).await?;
        if device.device_id == relay_account()?.agent {
            return Err(bad("cannot relay to this device; use the local inbox"));
        }
        Ok(Paired {
            device,
            deployment: deployment()?,
            grant_id: None,
        })
    })
}

fn resolve_device_name(selector: &str, labels: &Value) -> Result<String> {
    if labels["complete"] != true {
        return Err(bad(
            "device name directory is incomplete; use id:DEVICE_ID or update the backend",
        ));
    }
    let labels = labels["labels"]
        .as_object()
        .ok_or_else(|| bad("invalid device labels"))?;
    let matches: Vec<_> = labels
        .iter()
        .filter(|(_, name)| name.as_str() == Some(selector))
        .collect();
    match matches.as_slice() {
        [(id, _)] if !labels.contains_key(selector) || *id == selector => Ok((*id).clone()),
        [] => {
            valid(selector)?;
            Ok(selector.to_string())
        }
        _ => Err(bad(
            "ambiguous device name; use a unique name or exact device ID",
        )),
    }
}

fn paired(name: &str, owner: &str) -> Result<Paired> {
    let path = peer_path(name).ok();
    if let Some(path) = path.filter(|path| path.exists()) {
        peer::assert_alias_unambiguous(name)?;
        let paired: Paired = read(&path)?;
        paired.device.verify()?;
        if (paired.device.owner != owner && paired.grant_id.is_none())
            || paired.deployment != deployment()?
        {
            return Err(bad("peer is pinned to another account or relay deployment"));
        }
        if Provider::selected()? == Provider::Cloudflare
            && paired.device.owner == owner
            && paired.grant_id.is_none()
        {
            return same_owner_device(&paired.device.device_id, owner, false);
        }
        return Ok(paired);
    }
    if Provider::selected()? == Provider::Cloudflare {
        return same_owner_device(name, owner, true);
    }
    Err(bad(
        "peer is not pinned; confirm its fingerprint and pair it",
    ))
}

fn trusted(device: &str, sender_owner: &str, grant_id: Option<&str>) -> Result<(String, Paired)> {
    if Provider::selected()? == Provider::Cloudflare && grant_id.is_none() {
        let local = relay_account()?;
        if local.owner == sender_owner {
            let pin = same_owner_device(device, sender_owner, false).map_err(|error| {
                if denied(&error) {
                    error
                } else {
                    EverettError::new(6, error.message)
                }
            })?;
            return Ok((format!("id:{device}"), pin));
        }
    }
    for path in crate::paths::glob(&root().join("peers"), "*.json") {
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if let Ok(pin) = paired(&name, sender_owner) {
            if pin.device.device_id == device
                && pin.device.owner == sender_owner
                && pin.grant_id.as_deref() == grant_id
            {
                return Ok((name, pin));
            }
        }
    }
    Err(bad(
        "sender device has not been pinned; confirm its fingerprint and pair it",
    ))
}

pub fn list() -> Result<Value> {
    initialize()?;
    let ssh = peer::peers()?;
    let mut result = Map::new();
    for path in crate::paths::glob(&root().join("peers"), "*.json") {
        let pin: Paired = read(&path)?;
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        if ssh.contains_key(&name) {
            return Err(bad("alias is configured for both SSH and Convex; remove one binding explicitly before use"));
        }
        result.insert(name, json!({"transport":Provider::selected()?.transport(), "device_id":pin.device.device_id, "owner":pin.device.owner, "grant_id":pin.grant_id, "fingerprint":pin.device.fingerprint(), "deployment":pin.deployment}));
    }
    Ok(Value::Object(result))
}

pub(crate) fn remove_locked(name: &str) -> Result<Value> {
    fs::remove_file(peer_path(name)?).map_err(bad)?;
    Ok(
        json!({"removed":name,"transport":Provider::selected()?.transport(),"note":"Local trust removed; permanent backend revocation is everett relay revoke-agent AGENT_ID."}),
    )
}

pub fn remove(name: &str) -> Result<Value> {
    let _alias_guard = peer::alias_lock()?;
    peer::assert_alias_unambiguous(name)?;
    remove_locked(name)
}

fn runtime<T: Send + 'static>(
    future: impl std::future::Future<Output = Result<T>> + Send + 'static,
) -> Result<T> {
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new().map_err(bad)?.block_on(async {
            tokio::time::timeout(DEADLINE, future).await.map_err(|_| EverettError::new(5, "relay request timed out; delivery is unconfirmed. Retry with the same idempotency key."))?
        })
    }).join().map_err(|_| bad("relay worker failed"))?
}

fn args(value: Value) -> Result<BTreeMap<String, convex::Value>> {
    value
        .as_object()
        .ok_or_else(|| bad("invalid Convex arguments"))?
        .iter()
        .map(|(k, v)| Ok((k.clone(), convex::Value::try_from(v.clone()).map_err(bad)?)))
        .collect()
}

/// The message of a backend `userError({code, message})`; any other failure stays generic.
pub fn user_error(data: &Value) -> Option<String> {
    data["code"].as_str().filter(|code| !code.is_empty())?;
    data["message"].as_str().map(str::to_string)
}

fn result(value: FunctionResult) -> Result<Value> {
    match value {
        FunctionResult::Value(v) => Ok(v.into()),
        _ => Err(bad("Convex rejected the request; check OIDC configuration, device registration, TTL, and idempotency key")),
    }
}

async fn client(owner: &str) -> Result<RelayClient> {
    if Provider::selected()? == Provider::Cloudflare {
        let stored = native_credential()?;
        if stored.owner != owner {
            return Err(bad("signed-in owner changed; restart the relay"));
        }
        let agent = stored.agent_id.clone();
        let client = RelayClient::native(stored);
        let identity = client.native_call("relay:whoami", json!({})).await?;
        if identity["owner"] != owner || identity["deviceId"] != agent {
            return Err(EverettError::new(
                1,
                "Cloudflare relay denied: verified identity changed",
            ));
        }
        return Ok(client);
    }
    let (stored, current) = account()?;
    let agent = claims(&stored.id_token)?["everett_agent"]
        .as_str()
        .ok_or_else(|| bad("missing agent claim"))?
        .to_string();
    if current != owner {
        return Err(bad("signed-in owner changed; restart the relay"));
    }
    let expected = owner.to_string();
    let expected_agent = agent.clone();
    let mut client = RelayClient::new(&deployment()?)
        .await
        .map_err(|e| bad(format!("cannot connect to Convex: {e:#}")))?;
    client
        .set_auth_callback(Some(Box::new(move |force| {
            let expected = expected.clone();
            let expected_agent = expected_agent.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    let (stored, current) = account_refresh(force)?;
                    if current != expected
                        || claims(&stored.id_token)?["everett_agent"] != expected_agent
                    {
                        return Err(bad("signed-in owner changed"));
                    }
                    Ok(convex::AuthenticationToken::User(stored.id_token))
                })
                .await
                .map_err(|_| anyhow_error())?
                .map_err(|_| anyhow_error())
            })
        })))
        .await;
    let identity = query(&mut client, "relay:whoami", json!({})).await?;
    if identity["owner"] != owner || identity["deviceId"] != agent {
        return Err(bad(
            "verified backend identity differs from local token claims",
        ));
    }
    Ok(client)
}

fn anyhow_error() -> anyhow::Error {
    anyhow::anyhow!("Everett OIDC token unavailable; sign in again")
}

async fn mutation(client: &mut RelayClient, name: &str, value: Value) -> Result<Value> {
    if client.is_native() {
        return client.native_call(name, value).await;
    }
    let response = tokio::time::timeout(DEADLINE, client.mutation(name, args(value)?))
        .await
        .map_err(|_| bad("Convex mutation timed out; delivery unconfirmed"))?
        .map_err(|e| {
            bad(format!(
                "Convex mutation failed; delivery unconfirmed: {e:#}"
            ))
        })?;
    if let FunctionResult::ConvexError(error) = &response {
        if let Some(message) = user_error(&error.data.clone().into()) {
            return Err(bad(message));
        }
    }
    result(response)
}

async fn query(client: &mut RelayClient, name: &str, value: Value) -> Result<Value> {
    if client.is_native() {
        return client.native_call(name, value).await;
    }
    result(
        tokio::time::timeout(DEADLINE, client.query(name, args(value)?))
            .await
            .map_err(|_| bad("Convex query timed out"))?
            .map_err(|e| bad(format!("Convex query failed: {e:#}")))?,
    )
}

/// The presentation directory is bounded. Existence, key pinning, and
/// rotation recovery must use the owner-authorized exact-device query instead.
async fn exact_device(client: &mut RelayClient, device_id: &str) -> Result<Device> {
    valid(device_id)?;
    serde_json::from_value(query(client, "relay:device", json!({"deviceId":device_id})).await?)
        .map_err(bad)
}

async fn active_same_owner_device(
    client: &mut RelayClient,
    owner: &str,
    device_id: &str,
) -> Result<Device> {
    let device = exact_device(client, device_id).await?;
    device.verify()?;
    if device.owner != owner || device.device_id != device_id {
        return Err(bad("device owner or identifier mismatch"));
    }
    Ok(device)
}

async fn commit_rotation(
    client: &mut RelayClient,
    old: &Keys,
    new: &Keys,
    bytes: &str,
) -> Result<Value> {
    let registered = exact_device(client, &old.device.device_id).await?;
    if registered == new.device {
        return Ok(json!({"fingerprint":new.device.fingerprint()}));
    }
    if registered != old.device {
        return Err(bad(
            "registered device changed; refuse to overwrite an unknown key bundle",
        ));
    }
    mutation(client,"relay:rotate",json!({"device":new.device,"oldFingerprint":old.device.fingerprint(),"signature":old.sign(bytes.as_bytes())})).await
}

/// Keep the old primary key file authoritative until an exact-device check and
/// rotation commit have completed. A failed or lost response leaves the staged
/// bundle in place so the next attempt can discover a committed rotation.
fn staged_rotation<T>(
    path: &Path,
    stage: &Path,
    generate: impl FnOnce() -> PrivateKeys,
    commit: impl FnOnce(&PrivateKeys) -> Result<T>,
) -> Result<T> {
    let private = if stage.exists() {
        read::<PrivateKeys>(stage)?
    } else {
        let private = generate();
        write(stage, &private)?;
        private
    };
    let response = commit(&private)?;
    write(path, &private)?;
    fs::remove_file(stage).map_err(bad)?;
    Ok(response)
}

pub fn admin_call(name: &str, value: Value, is_query: bool) -> Result<Value> {
    let owner = relay_account()?.owner;
    let name = name.to_string();
    runtime(async move {
        let mut client = client(&owner).await?;
        if is_query {
            query(&mut client, &name, value).await
        } else {
            mutation(&mut client, &name, value).await
        }
    })
}

/// The owner namespace of the stored sign-in, without refreshing or contacting the backend.
pub fn signed_in_owner() -> Result<String> {
    Ok(relay_account()?.owner)
}
pub fn signed_in_device_id() -> Result<String> {
    Ok(relay_account()?.agent)
}

pub fn revoke_device(device_id: &str) -> Result<Value> {
    let owner = relay_account()?.owner;
    let deployment = deployment()?;
    revoke_device_with(device_id, &owner, &deployment, admin_call)
}

pub fn revoke_device_with(
    device_id: &str,
    owner: &str,
    deployment: &str,
    call: impl FnOnce(&str, Value, bool) -> Result<Value>,
) -> Result<Value> {
    valid(device_id)?;
    call("relay:revokeDevice", json!({"deviceId":device_id}), false)?;
    let cleanup = || -> Result<Vec<String>> {
        let _guard = peer::alias_lock()?;
        let mut removed = peer::forget_device_locked(device_id)?;
        for path in crate::paths::glob(&root().join("peers"), "*.json") {
            let pin: Paired = read(&path)?;
            if pin.device.device_id == device_id
                && pin.device.owner == owner
                && pin.deployment == deployment
            {
                let name = path.file_stem().unwrap().to_string_lossy().to_string();
                remove_locked(&name)?;
                removed.push(name);
            }
        }
        Ok(removed)
    };
    let removed =
        cleanup().map_err(|e| bad(format!("device revoked; local peer cleanup failed: {e}")))?;
    Ok(json!({"revoked":device_id,"removed_peers":removed}))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevokedDevice {
    pub device_id: String,
    pub revoked_at: f64,
    /// The revoked device's SSH peer id when it registered one; its SSH authorization carries this tag.
    #[serde(default)]
    pub ssh_device_id: Option<String>,
    /// Base64 field of the `ssh-ed25519` key that signed the binding.
    #[serde(default)]
    pub ssh_public_key: Option<String>,
    /// Hex Ed25519 signature over `ssh_binding_bytes` by `ssh_public_key`.
    #[serde(default)]
    pub ssh_signature: Option<String>,
}

pub fn ssh_binding_bytes(
    owner: &str,
    device_id: &str,
    ssh_device_id: &str,
    ssh_public_key: &str,
) -> Vec<u8> {
    json!([
        "everett-ssh-bind-v1",
        owner,
        device_id,
        ssh_device_id,
        ssh_public_key
    ])
    .to_string()
    .into_bytes()
}

const SSH_ED25519_PREFIX: &[u8] = b"\0\0\0\x0bssh-ed25519\0\0\0\x20";

/// The SSH id and key a revoked row may prune, only when its binding is signed by that key for this owner.
pub fn verified_ssh_binding<'a>(
    owner: &str,
    row: &'a RevokedDevice,
) -> Result<Option<(&'a str, &'a str)>> {
    let (Some(ssh), Some(key), Some(signature)) =
        (&row.ssh_device_id, &row.ssh_public_key, &row.ssh_signature)
    else {
        return Ok(None);
    };
    let invalid = || {
        bad(format!(
            "invalid SSH binding proof for revoked device {}",
            row.device_id
        ))
    };
    let blob = base64::engine::general_purpose::STANDARD
        .decode(key)
        .map_err(|_| invalid())?;
    let raw: [u8; 32] = blob
        .strip_prefix(SSH_ED25519_PREFIX)
        .and_then(|raw| raw.try_into().ok())
        .ok_or_else(invalid)?;
    let signature: [u8; 64] = hex::decode(signature)
        .ok()
        .and_then(|sig| sig.try_into().ok())
        .ok_or_else(invalid)?;
    ed25519_dalek::VerifyingKey::from_bytes(&raw)
        .and_then(|vk| {
            vk.verify_strict(
                &ssh_binding_bytes(owner, &row.device_id, ssh, key),
                &ed25519_dalek::Signature::from_bytes(&signature),
            )
        })
        .map_err(|_| invalid())?;
    Ok(Some((ssh, key)))
}

const MAX_REVOKED_PAGES: usize = 64;

pub fn revoked_devices() -> Result<Vec<RevokedDevice>> {
    revoked_devices_with(|cursor| {
        admin_call("relay:revokedDevices", json!({"cursor": cursor}), true)
    })
}

/// Follow the query's cursor until the backend reports the last page.
pub fn revoked_devices_with(
    mut fetch: impl FnMut(Option<String>) -> Result<Value>,
) -> Result<Vec<RevokedDevice>> {
    let mut all = Vec::new();
    let mut cursor = None;
    for _ in 0..MAX_REVOKED_PAGES {
        let mut page = fetch(cursor)?;
        all.extend(parse_revoked_devices(page["devices"].take())?);
        cursor = match page["cursor"].take() {
            Value::Null => return Ok(all),
            Value::String(next) => Some(next),
            _ => return Err(bad("invalid revocation cursor")),
        };
    }
    Err(bad(format!(
        "revocation list exceeds {MAX_REVOKED_PAGES} pages"
    )))
}

pub fn parse_revoked_devices(value: Value) -> Result<Vec<RevokedDevice>> {
    let rows: Vec<RevokedDevice> = serde_json::from_value(value).map_err(bad)?;
    if rows.len() > 256 {
        return Err(bad("revocation response exceeds 256 devices"));
    }
    for row in &rows {
        valid(&row.device_id)?;
        if let Some(ssh) = &row.ssh_device_id {
            valid(ssh)?;
        }
        if row.ssh_device_id.is_some() != row.ssh_public_key.is_some()
            || row.ssh_device_id.is_some() != row.ssh_signature.is_some()
        {
            return Err(bad("incomplete SSH binding in revocation"));
        }
        if !row.revoked_at.is_finite()
            || row.revoked_at.fract() != 0.0
            || row.revoked_at < 0.0
            || row.revoked_at > 9_007_199_254_740_991.0
        {
            return Err(bad("invalid revocation timestamp"));
        }
    }
    Ok(rows)
}

pub fn rotate() -> Result<Value> {
    let owner = relay_account()?.owner;
    let _daemon_guard = daemon_lock()?;
    let old = verified_keys(&owner)?;
    let path = key_path(&owner, &old.device.device_id);
    let _guard = peer::lock(&root().join("keys.lock"))?;
    let stage = path.with_extension("rotation");
    let (device, response) = staged_rotation(
        &path,
        &stage,
        || {
            let mut signing = [0u8; 32];
            let mut encryption = [0u8; 32];
            OsRng.fill_bytes(&mut signing);
            OsRng.fill_bytes(&mut encryption);
            PrivateKeys {
                signing: hex::encode(signing),
                encryption: hex::encode(encryption),
            }
        },
        |private| {
            let new = Keys::new(
                owner.clone(),
                old.device.device_id.clone(),
                crypto::decode(&private.signing)?,
                crypto::decode(&private.encryption)?,
            );
            let bytes = json!([
                "everett-rotate-v2",
                owner,
                old.device.device_id,
                old.device.fingerprint(),
                new.device.fingerprint()
            ])
            .to_string();
            let device = new.device.clone();
            let response = runtime(async move {
                let mut client = client(&owner).await?;
                commit_rotation(&mut client, &old, &new, &bytes).await
            })?;
            Ok((device, response))
        },
    )?;
    Ok(
        json!({"device":device,"result":response,"note":"Same-owner Cloudflare peers discover current keys automatically; explicit pins and cross-org grants require reconfirmation."}),
    )
}

/// Register args carry this machine's SSH peer id and key, signed by that key, so a later
/// revocation can prune exactly this device's SSH authorization.
fn register_args(device: &Device) -> Result<Value> {
    if Provider::selected()? == Provider::Cloudflare {
        return Ok(json!({"device":device}));
    }
    let mut args = json!({"device": device});
    if let Some(ssh) = ssh_binding(device)? {
        args["ssh"] = ssh;
    }
    Ok(args)
}

pub fn ssh_binding(device: &Device) -> Result<Option<Value>> {
    use ed25519_dalek::Signer;
    let Some((ssh, key, signing)) = peer::local_ssh_key()? else {
        return Ok(None);
    };
    let signature = signing.sign(&ssh_binding_bytes(
        &device.owner,
        &device.device_id,
        &ssh,
        &key,
    ));
    Ok(Some(
        json!({"sshDeviceId": ssh, "sshPublicKey": key, "sshSignature": hex::encode(signature.to_bytes())}),
    ))
}

pub fn register() -> Result<Value> {
    register_with_address(None)
}

pub fn register_with_address(address: Option<&str>) -> Result<Value> {
    let identity = relay_account()?;
    let (owner, name) = (identity.owner, identity.name);
    if Provider::selected()? == Provider::Cloudflare && address.is_some() {
        return Err(bad("Cloudflare relay does not publish SSH addresses; use its device ID and explicit fingerprint"));
    }
    let address = if Provider::selected()? == Provider::Cloudflare {
        None
    } else {
        valid(&name).map_err(|_| bad("device name must use 1-160 letters, digits, '.', '_' or '-'; run everett login --device NAME --force"))?;
        crate::device_registry::declared_address(address)?
    };
    let key = verified_keys(&owner)?;
    let device = key.device.clone();
    let args = register_args(&device)?;
    runtime(async move {
        let mut client = client(&owner).await?;
        mutation(&mut client, "relay:register", args).await?;
        if client.is_native() {
            return Ok(
                json!({"device":device,"fingerprint":device.fingerprint(),"transport":"cloudflare"}),
            );
        }
        let signature = key.sign(&crate::device_registry::metadata_bytes(
            &owner,
            &device.device_id,
            &name,
            address.as_deref(),
        ));
        mutation(&mut client, "registry:publish", json!({"deviceId":device.device_id,"name":name,"sshAddress":address,"signature":signature})).await?;
        Ok(json!({"device":device, "fingerprint":device.fingerprint()}))
    })
}

pub fn devices() -> Result<Value> {
    let owner = relay_account()?.owner;
    runtime(async move {
        let mut client = client(&owner).await?;
        query(&mut client, "relay:devices", json!({})).await
    })
}

pub fn device_names() -> Result<String> {
    let owner = relay_account()?.owner;
    runtime(async move {
        let mut client = client(&owner).await?;
        let bundles = query(&mut client, "relay:devices", json!({})).await?;
        let labels = if client.is_native() {
            query(&mut client, "relay:deviceLabels", json!({}))
                .await
                .ok()
        } else {
            None
        };
        format_device_names(bundles, labels, &owner)
    })
}

fn format_device_names(bundles: Value, labels: Option<Value>, owner: &str) -> Result<String> {
    let devices: Vec<Device> = serde_json::from_value(bundles).map_err(bad)?;
    let labels = labels.filter(|v| v.get("labels").is_some_and(Value::is_object));
    let mut text = String::from(
        "Names are advisory hosted metadata (not signed). Same-owner Cloudflare sends verify active signed bundles automatically.\n",
    );
    if labels.is_none() {
        text.push_str("Names unavailable: backend metadata unsupported/unavailable; IDs remain authoritative.\n");
    }
    text.push_str("NAME\tDEVICE ID\tFINGERPRINT\n");
    for device in devices {
        device.verify()?;
        if device.owner != owner {
            return Err(bad("device owner mismatch"));
        }
        let name = labels
            .as_ref()
            .and_then(|v| v["labels"][&device.device_id].as_str())
            .filter(|name| {
                !name.trim().is_empty()
                    && name.chars().count() <= 100
                    && !name.chars().any(char::is_control)
            });
        // JSON quoting escapes user-authored labels without terminal control sequences.
        let label = name
            .map(|n| json!(n).to_string())
            .unwrap_or_else(|| "(unnamed)".into());
        text.push_str(&format!(
            "{label}\t{}\t{}\n",
            device.device_id,
            device.fingerprint()
        ));
    }
    Ok(text)
}

pub fn rename_device(name: &str) -> Result<Value> {
    if name.trim().is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(bad("device name must contain 1-100 printable characters"));
    }
    if Provider::selected()? != Provider::Cloudflare {
        return Err(bad(
            "relay rename requires hosted Cloudflare metadata; Convex registry names are separate",
        ));
    }
    let stored = native_credential()?;
    stored.relay_request("relay:renameDevice", &json!({"name":name.trim()}))
}

pub fn add(
    name: &str,
    device_id: &str,
    fingerprint: &str,
    grant_id: Option<&str>,
) -> Result<Value> {
    valid(name)?;
    valid(device_id)?;
    initialize()?;
    let owner = relay_account()?.owner;
    let device: Device = if let Some(grant) = grant_id {
        valid(grant)?;
        let view = admin_call("grants:get", json!({"grantId":grant}), true)?;
        if view["access"] != "active" {
            return Err(bad("grant is not active"));
        }
        let local = keys(&owner)?.device;
        let (own, remote) = if view["aOwner"] == owner {
            (&view["aBundle"], &view["bBundle"])
        } else {
            (&view["bBundle"], &view["aBundle"])
        };
        if *own != json!(local) {
            return Err(bad("grant does not pin this agent's current keys"));
        }
        serde_json::from_value(remote.clone()).map_err(bad)?
    } else {
        let owner_for_query = owner.clone();
        let device_id = device_id.to_string();
        runtime(async move {
            let mut client = client(&owner_for_query).await?;
            exact_device(&mut client, &device_id).await
        })?
    };
    device.verify()?;
    if (grant_id.is_none() && device.owner != owner)
        || device.device_id != device_id
        || device.fingerprint() != fingerprint
    {
        return Err(bad(
            "device fingerprint or owner mismatch; confirm out of band",
        ));
    }
    if device.device_id == keys(&owner)?.device.device_id {
        return Err(bad("cannot pair this device with itself"));
    }
    peer::claim_alias(
        name,
        "SSH",
        || Ok(peer::peers()?.contains_key(name)),
        || {
            let _guard = peer::lock(&root().join("peers.lock"))?;
            let pin = Paired {
                device,
                deployment: deployment()?,
                grant_id: grant_id.map(String::from),
            };
            let path = peer_path(name)?;
            if path.exists() {
                let existing: Paired = read(&path)?;
                if existing.device != pin.device
                    || existing.deployment != pin.deployment
                    || existing.grant_id != pin.grant_id
                {
                    return Err(bad(
                        "peer pin changed; explicitly remove and confirm the new fingerprint",
                    ));
                }
            }
            write(&path, &pin)?;
            Ok(
                json!({"peer":name,"transport":Provider::selected()?.transport(),"device_id":device_id,"fingerprint":fingerprint}),
            )
        },
    )
}

fn outgoing_path(owner: &str, key: &str) -> PathBuf {
    root().join("outgoing").join(format!(
        "{:x}.json",
        Sha256::digest(json!([owner, key]).to_string())
    ))
}

fn prepare(
    name: &str,
    key: &str,
    payload: &Payload,
    keys: &Keys,
    parent: Option<Parent>,
) -> Result<(PathBuf, Outgoing)> {
    valid(key)?;
    payload.validate()?;
    let pin = paired(name, &keys.device.owner)?;
    let digest = format!(
        "{:x}",
        Sha256::digest(json!([name, payload, parent, pin.device.owner, pin.grant_id]).to_string())
    );
    let path = outgoing_path(
        &json!([keys.device.owner, keys.device.device_id]).to_string(),
        key,
    );
    let _guard = peer::lock(&root().join("outgoing.lock"))?;
    if let Some(existing) = reuse_outgoing(&path, &digest, keys, &pin)? {
        return Ok((path, existing));
    }
    let (session, harness) = match payload {
        Payload::Inbox {
            from, from_harness, ..
        } => (from.clone(), from_harness.clone()),
        _ => (String::new(), String::new()),
    };
    let outgoing = Outgoing {
        peer: name.into(),
        envelope: keys.seal(
            &pin.device,
            key,
            payload,
            milliseconds(),
            TTL,
            pin.grant_id,
            parent.clone(),
        )?,
        digest,
        session,
        harness,
        posted: false,
        state: State::Queued,
        reply_pending: match payload {
            Payload::Inbox { reply_to, .. } if !reply_to.is_empty() => parent,
            _ => None,
        },
    };
    write(&path, &outgoing)?;
    Ok((path, outgoing))
}

fn reuse_outgoing(
    path: &Path,
    digest: &str,
    keys: &Keys,
    pin: &Paired,
) -> Result<Option<Outgoing>> {
    if !path.exists() {
        return Ok(None);
    }
    let existing: Outgoing = read(path)?;
    if existing.digest != digest
        || existing.envelope.owner != keys.device.owner
        || existing.envelope.recipient_owner != pin.device.owner
        || existing.envelope.grant_id != pin.grant_id
        || existing.envelope.to_device != pin.device.device_id
        || existing.envelope.from_device != keys.device.device_id
        || existing.envelope.sender_fingerprint != keys.device.fingerprint()
        || existing.envelope.recipient_fingerprint != pin.device.fingerprint()
    {
        return Err(bad(
            "idempotency key already used for a different request or key bundle",
        ));
    }
    Ok(Some(existing))
}

async fn post(client: &mut RelayClient, path: &Path, outgoing: &Outgoing) -> Result<Value> {
    let result = mutation(
        client,
        "relay:enqueue",
        json!({"envelope":outgoing.envelope}),
    )
    .await?;
    let _guard = peer::lock(&root().join("outgoing.lock"))?;
    let mut current: Outgoing = read(path)?;
    current.posted = true;
    write(path, &current)?;
    Ok(result)
}

async fn observe_reply(client: &mut RelayClient, keys: &Keys, path: &Path) -> Result<()> {
    let out: Outgoing = read(path)?;
    if out.posted {
        if let Some(parent) = out.reply_pending {
            signed_observation(client, keys, &parent.owner, &parent.message_id, "replied").await?;
            let _guard = peer::lock(&root().join("outgoing.lock"))?;
            let mut current: Outgoing = read(path)?;
            current.reply_pending = None;
            write(path, &current)?;
        }
    }
    Ok(())
}

pub fn send(target: &str, text: &str, key: Option<&str>, caller: Option<&str>) -> Result<Value> {
    send_as(target, text, key, caller, None)
}

pub fn send_as(
    target: &str,
    text: &str,
    key: Option<&str>,
    caller: Option<&str>,
    caller_harness: Option<&str>,
) -> Result<Value> {
    let (name, harness, destination) = peer::target(target)?;
    let owner = relay_account()?.owner;
    let keys = verified_keys(&owner)?;
    let who = send::sender_info_as(caller, caller_harness);
    let from = who["sender"].as_str().unwrap_or(inbox::HUMAN);
    let context = inbox::relay_context(from);
    if context.as_ref().is_some_and(|m| {
        m["relay_owner"] != owner || m["relay_recipient_device"] != keys.device.device_id
    }) {
        return Err(bad("remote lineage belongs to another org/account"));
    }
    let parent = context.as_ref().map(|m| Parent {
        owner: m["relay_sender_owner"].as_str().unwrap_or("").into(),
        message_id: m["relay_message_id"].as_str().unwrap_or("").into(),
    });
    let hops = context
        .as_ref()
        .and_then(|m| m["hops"].as_i64())
        .unwrap_or(0)
        .max(send::current_hops())
        .max(inbox::session_hops(from))
        .max(0)
        + 1;
    if parent.is_none() && hops != 1 {
        return Err(bad(
            "remote-derived send needs signed parent context; use explicit reply",
        ));
    }
    let generated = uuid::Uuid::new_v4().to_string();
    let key = key.unwrap_or(&generated);
    let payload = Payload::Inbox {
        session: destination.into(),
        harness: harness.into(),
        text: text.trim().into(),
        from: from.into(),
        from_harness: who["from_harness"].as_str().unwrap_or("").into(),
        from_card: who["from_card"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(160)
            .collect(),
        reply_to: String::new(),
        hops,
        external_untrusted: context.as_ref().is_some_and(inbox::external),
    };
    let (path, outgoing) = prepare(name, key, &payload, &keys, parent)?;
    if outgoing.envelope.expires_at <= milliseconds() {
        return Err(bad(
            "request expired; use a new idempotency key only if you intend a new send",
        ));
    }
    let name = name.to_string();
    runtime(async move {
        let mut client = client(&owner).await?;
        let response = post(&mut client, &path, &outgoing).await?;
        let (_, observed) = find_outgoing(&owner, &outgoing.envelope.message_id)?;
        let state = if observed.state != State::Queued { json!(observed.state) } else { response["state"].clone() };
        Ok(json!({"peer":name,"transport":Provider::selected()?.transport(),"message_id":outgoing.envelope.message_id,"idempotency_key":outgoing.envelope.idempotency_key,"state":state,"expires_at":outgoing.envelope.expires_at,"pickup":"live relay; hooks/MCP must consume local inbox"}))
    }).map_err(|e| EverettError::new(e.code, format!("{}; idempotency key {key}. A local retry record exists, but relay queueing is not confirmed.",e.message)))
}

pub fn retain_reply(
    original: &Map<String, Value>,
    text: &str,
    who: &Map<String, Value>,
    hops: i64,
) -> Result<Map<String, Value>> {
    let owner = relay_account()?.owner;
    let keys = verified_keys(&owner)?;
    if original["relay_owner"] != owner
        || original["relay_recipient_device"] != keys.device.device_id
    {
        return Err(bad("message belongs to another signed-in account"));
    }
    let (name, _) = trusted(
        original["relay_device"]
            .as_str()
            .ok_or_else(|| bad("missing sender device"))?,
        original["relay_sender_owner"]
            .as_str()
            .ok_or_else(|| bad("missing sender owner"))?,
        original["relay_grant_id"].as_str(),
    )?;
    let mid = original["relay_message_id"]
        .as_str()
        .ok_or_else(|| bad("missing message id"))?;
    let payload = Payload::Inbox {
        session: original["relay_sender"]
            .as_str()
            .ok_or_else(|| bad("missing reply destination"))?
            .into(),
        harness: original["from_harness"].as_str().unwrap_or("").into(),
        text: text.trim().into(),
        from: who["sender"].as_str().unwrap_or(inbox::HUMAN).into(),
        from_harness: who["from_harness"].as_str().unwrap_or("").into(),
        from_card: who["from_card"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(160)
            .collect(),
        reply_to: mid.into(),
        hops,
        external_untrusted: inbox::external(original)
            || inbox::relay_context(who["sender"].as_str().unwrap_or(inbox::HUMAN))
                .as_ref()
                .is_some_and(inbox::external),
    };
    let parent = Parent {
        owner: original["relay_sender_owner"]
            .as_str()
            .ok_or_else(|| bad("missing sender owner"))?
            .into(),
        message_id: mid.into(),
    };
    let reply_key = format!(
        "reply-{:x}",
        Sha256::digest(json!([parent.owner, mid]).to_string())
    );
    let (path, outgoing) = prepare(&name, &reply_key, &payload, &keys, Some(parent))?;
    let mid = mid.to_string();
    runtime(async move {
        let mut client = client(&owner).await?;
        post(&mut client, &path, &outgoing).await?;
        observe_reply(&mut client, &keys, &path).await?;
        Ok(serde_json::from_value(json!({"mode":"relay-reply","message_id":outgoing.envelope.message_id,"reply_to":mid,"state":"queued","pickup":"live relay"})).unwrap())
    })
}

fn find_outgoing(owner: &str, mid: &str) -> Result<(PathBuf, Outgoing)> {
    valid(mid)?;
    for path in crate::paths::glob(&root().join("outgoing"), "*.json") {
        let out: Outgoing = read(&path)?;
        if out.envelope.owner == owner && out.envelope.message_id == mid {
            return Ok((path, out));
        }
    }
    Err(bad("no outgoing message for this account"))
}

pub fn status(name: &str, mid: &str) -> Result<Value> {
    let owner = relay_account()?.owner;
    paired(name, &owner)?;
    let (_, outgoing) = find_outgoing(&owner, mid)?;
    let keys = verified_keys(&owner)?;
    if outgoing.envelope.from_device != keys.device.device_id
        || outgoing.envelope.sender_fingerprint != keys.device.fingerprint()
    {
        return Err(bad("outgoing message belongs to another agent/key"));
    }
    if outgoing.peer != name {
        return Err(bad("message is for another peer"));
    }
    if !outgoing.posted {
        return Ok(
            json!({"message_id":mid,"state":"unconfirmed","expires_at":outgoing.envelope.expires_at,"note":"A local retry record exists; relay queueing has not been confirmed. Run everett relay to retry before expiry."}),
        );
    }
    runtime(async move {
        let mut client = client(&owner).await?;
        let status = query(&mut client,"relay:status",json!({"fromDevice":outgoing.envelope.from_device,"messageId":outgoing.envelope.message_id})).await?;
        Ok(
            json!({"message_id":outgoing.envelope.message_id,"state":status["state"],"expires_at":outgoing.envelope.expires_at,"note":"Server state reflects signed observations, never session wake-up."}),
        )
    })
}

pub fn agent_status(mid: &str) -> Result<Value> {
    let owner = relay_account()?.owner;
    let (_, outgoing) = find_outgoing(&owner, mid)?;
    status(&outgoing.peer, mid)
}

fn observe(owner: &str, envelope: &Envelope, mid: &str, state: State) -> Result<()> {
    let _guard = peer::lock(&root().join("outgoing.lock"))?;
    let (path, mut out) = find_outgoing(owner, mid)?;
    if out.envelope.from_device != envelope.to_device
        || out.envelope.to_device != envelope.from_device
        || out.envelope.recipient_owner != envelope.owner
        || out.envelope.grant_id != envelope.grant_id
        || out.envelope.recipient_fingerprint != envelope.sender_fingerprint
        || out.envelope.sender_fingerprint != envelope.recipient_fingerprint
    {
        return Err(bad("receipt is from the wrong device"));
    }
    if out.state != State::Replied {
        out.state = state;
        write(&path, &out)?;
    }
    Ok(())
}

pub fn import(keys: &Keys, envelope: &Envelope, now: i64) -> Result<()> {
    import_inner(keys, envelope, now, true)
}

fn import_inner(keys: &Keys, envelope: &Envelope, now: i64, publish: bool) -> Result<()> {
    let (_, pin) = trusted(
        &envelope.from_device,
        &envelope.owner,
        envelope.grant_id.as_deref(),
    )?;
    let payload = keys.open(&pin.device, envelope, now)?;
    let _guard = peer::lock(&root().join("inbox.lock"))?;
    match payload {
        Payload::Delivered { .. } => {
            return Err(bad(
                "v2 uses signed backend observations, not encrypted receipt messages",
            ))
        }
        Payload::Inbox {
            session,
            harness,
            text,
            from,
            from_harness,
            from_card,
            reply_to,
            hops,
            external_untrusted,
        } => {
            if !reply_to.is_empty() {
                let (_, out) = find_outgoing(&keys.device.owner, &reply_to)?;
                if out.envelope.from_device != keys.device.device_id
                    || out.envelope.to_device != envelope.from_device
                    || out.envelope.recipient_owner != envelope.owner
                    || out.envelope.grant_id != envelope.grant_id
                    || out.envelope.recipient_fingerprint != envelope.sender_fingerprint
                    || out.envelope.sender_fingerprint != envelope.recipient_fingerprint
                    || envelope.parent
                        != Some(Parent {
                            owner: keys.device.owner.clone(),
                            message_id: reply_to.clone(),
                        })
                    || envelope.hop <= out.envelope.hop
                    || envelope.hop > inbox::MAX_HOPS
                    || out.session != session
                    || out.harness != harness
                {
                    return Err(bad("reply does not match the original request"));
                }
            } else if !crate::registry::scan(24.0 * 365.0, true, None, &harness)
                .iter()
                .any(|s| s.id == session && s.harness == harness)
                && !receiver_policy_allows(envelope, keys, &session, &harness, external_untrusted)
            {
                return Err(bad(
                    "recipient session not found; row remains queued until TTL",
                ));
            }
            if !publish {
                return Ok(());
            }
            let digest = format!("{:x}", Sha256::digest(envelope.signed_bytes()));
            let local_id = local_message_id(envelope);
            let local_reply = if reply_to.is_empty() {
                String::new()
            } else {
                reply_to.clone()
            };
            if let Some(existing) = inbox::find(&local_id) {
                if existing["relay_digest"] != digest {
                    return Err(bad("duplicate message id has different ciphertext"));
                }
            } else {
                let external = envelope.owner != envelope.recipient_owner || external_untrusted;
                let extra = serde_json::from_value(json!({"id":local_id,"relay_message_id":envelope.message_id,"relay_owner":keys.device.owner,"relay_sender_owner":envelope.owner,"relay_recipient_owner":envelope.recipient_owner,"relay_grant_id":envelope.grant_id,"relay_device":envelope.from_device,"relay_recipient_device":keys.device.device_id,"relay_sender":from,"relay_digest":digest,"relay_parent":envelope.parent,"external":external,"untrusted":external,"trust":if external {"external-untrusted"} else {"same-org"},"instruction_source":"agent-data"})).map_err(bad)?;
                inbox::post(
                    &session,
                    &text,
                    &format!("{}/{from_harness}/{from}", envelope.from_device),
                    if reply_to.is_empty() {
                        "message"
                    } else {
                        "reply"
                    },
                    &local_reply,
                    hops,
                    &from_harness,
                    &from_card,
                    Some(extra),
                )?;
            }
            // Retry paths also sync: a previous append may have succeeded before
            // a file/directory fsync failed. Never discard its sole encrypted stage
            // until BOTH the plaintext file and its directory entry are durable.
            let file = fs::OpenOptions::new()
                .write(true)
                .open(inbox::path(&session)?)
                .map_err(bad)?;
            file.sync_all().map_err(bad)?;
            fs::File::open(inbox::inbox_dir())
                .and_then(|f| f.sync_all())
                .map_err(bad)?;
            if reply_to.is_empty() {
                record_receiver_import(
                    keys,
                    envelope,
                    ImportedReceiverContext {
                        session_id: &session,
                        harness: &harness,
                        sender_session: &from,
                        sender_harness: &from_harness,
                        local_id: &local_id,
                        digest: &digest,
                        external_untrusted,
                        hops,
                    },
                )?;
                note_receiver_webhook(crate::receiver_webhook::on_imported(
                    &crate::receiver_webhook::Arrival {
                        owner: keys.device.owner.clone(),
                        sender_owner: envelope.owner.clone(),
                        recipient_owner: envelope.recipient_owner.clone(),
                        recipient_device: keys.device.device_id.clone(),
                        sender_device: envelope.from_device.clone(),
                        session: session.clone(),
                        harness: harness.clone(),
                        sender_session: from.clone(),
                        sender_harness: from_harness.clone(),
                        message_id: local_id.clone(),
                        created_at_ms: envelope.created_at,
                        is_reply: false,
                        external: envelope.owner != envelope.recipient_owner
                            || external_untrusted
                            || crate::adapters::external::is_external(&from_harness),
                    },
                ));
            }
            if !reply_to.is_empty() {
                observe(&keys.device.owner, envelope, &reply_to, State::Replied)?;
            }
        }
    }
    Ok(())
}

fn receiver_policy_allows(
    envelope: &Envelope,
    keys: &Keys,
    session_id: &str,
    harness: &str,
    external_untrusted: bool,
) -> bool {
    if harness != "claude"
        || external_untrusted
        || envelope.owner != keys.device.owner
        || envelope.recipient_owner != keys.device.owner
        || envelope.grant_id.is_some()
    {
        return false;
    }
    crate::receiver_wake::load_policy(&root()).is_ok_and(|policy| {
        policy.enabled
            && policy.allow.iter().any(|rule| {
                rule.session_id == session_id && rule.allows_sender(&envelope.from_device)
            })
    })
}

struct ImportedReceiverContext<'a> {
    session_id: &'a str,
    harness: &'a str,
    sender_session: &'a str,
    sender_harness: &'a str,
    local_id: &'a str,
    digest: &'a str,
    external_untrusted: bool,
    hops: i64,
}

fn record_receiver_import(keys: &Keys, envelope: &Envelope, context: ImportedReceiverContext<'_>) -> Result<()> {
    let receiver_root = root();
    if let Ok(policy) = crate::receiver_wake::load_policy(&receiver_root) {
        if let Some(rule) = policy.allow.iter().find(|rule| {
            rule.session_id == context.session_id
                && rule.allows_sender(&envelope.from_device)
                && context.harness == "claude"
        }) {
            let imported = crate::receiver_wake::Imported {
                local_id: context.local_id.into(),
                digest: context.digest.into(),
                relay_message_id: envelope.message_id.clone(),
                reply_to: None,
                sender_session: context.sender_session.into(),
                sender_harness: context.sender_harness.into(),
                relay_parent_owner: envelope.parent.as_ref().map(|parent| parent.owner.clone()),
                relay_parent_message_id: envelope
                    .parent
                    .as_ref()
                    .map(|parent| parent.message_id.clone()),
                owner: keys.device.owner.clone(),
                sender_owner: envelope.owner.clone(),
                recipient_owner: envelope.recipient_owner.clone(),
                sender_device_id: envelope.from_device.clone(),
                recipient_device_id: keys.device.device_id.clone(),
                grant_id: envelope.grant_id.clone(),
                session_id: context.session_id.into(),
                harness: context.harness.into(),
                cwd: rule.cwd.clone(),
                expires_at: envelope.expires_at,
                hops: context.hops,
                external: envelope.owner != envelope.recipient_owner || context.external_untrusted,
            };
            crate::receiver_wake::schedule(&receiver_root, imported, milliseconds())?;
        }
    }
    Ok(())
}

fn drain_receiver_wakes(keys: &Keys) -> Result<()> {
    let identity = match relay_account() {
        Ok(identity) if identity.owner == keys.device.owner && identity.agent == keys.device.device_id => identity,
        _ => return Ok(()),
    };
    let owner = identity.owner;
    let receiver_root = root();
    let policy = match crate::receiver_wake::load_policy(&receiver_root) {
        Ok(policy) if policy.enabled => policy,
        _ => return Ok(()),
    };
    let now = milliseconds();
    for job in crate::receiver_wake::pending(&receiver_root, now)?
        .into_iter()
        .take(32)
    {
        let imported = &job.imported;
        if imported.owner != owner
            || imported.recipient_device_id != keys.device.device_id
            || imported.sender_owner != owner
            || imported.recipient_owner != owner
            || imported.grant_id.is_some()
            || imported.external
            || imported.harness != "claude"
        {
            continue;
        }
        if !policy.allow.iter().any(|rule| {
            rule.session_id == imported.session_id
                && rule.cwd == imported.cwd
                && rule.allows_sender(&imported.sender_device_id)
        }) {
            continue;
        }
        if !receiver_inbox_pending(&job) {
            crate::receiver_wake::skip(
                &receiver_root,
                &imported.local_id,
                "verified inbox row was already consumed or its digest changed",
                now,
            )?;
            continue;
        }
        if trusted(&imported.sender_device_id, &imported.sender_owner, None).is_err() {
            continue;
        }
        let session = session::Session::new("claude", &imported.session_id, &imported.cwd, "", "", 0.0);
        let prepared = match crate::claude_wake::prepare(&session) {
            Ok(prepared) => prepared,
            Err(error) => {
                crate::receiver_wake::defer(&receiver_root, &imported.local_id, &error.message, now)?;
                continue;
            }
        };
        let current_owner = match relay_account() {
            Ok(account) if account.owner == owner && account.agent == keys.device.device_id => account.owner,
            _ => continue,
        };
        if current_owner != imported.owner || keys.device.device_id != imported.recipient_device_id { continue; }
        let current_policy = match crate::receiver_wake::load_policy(&receiver_root) {
            Ok(policy) if policy.enabled => policy,
            _ => continue,
        };
        if !current_policy.allow.iter().any(|rule| {
            rule.session_id == imported.session_id
                && rule.cwd == imported.cwd
                && rule.allows_sender(&imported.sender_device_id)
        }) || trusted(&imported.sender_device_id, &imported.sender_owner, None).is_err()
            || keys.device.owner != owner
        {
            continue;
        }
        if !receiver_inbox_pending(&job) {
            crate::receiver_wake::skip(
                &receiver_root,
                &imported.local_id,
                "verified inbox row was already consumed or its digest changed",
                milliseconds(),
            )?;
            continue;
        }
        let outcome = crate::receiver_wake::attempt(
            &receiver_root,
            &imported.local_id,
            milliseconds(),
            || Ok(prepared),
            |prepared, local_id| match crate::claude_wake::wake(&prepared, local_id) {
                crate::claude_wake::WakeOutcome::PromptSent { client_pid, target_pid } => crate::receiver_wake::AttemptOutcome::PromptSent { client_pid, target_pid },
                crate::claude_wake::WakeOutcome::NotAttached { detail } => crate::receiver_wake::AttemptOutcome::Deferred(detail),
                crate::claude_wake::WakeOutcome::Cancelled { .. } => crate::receiver_wake::AttemptOutcome::Cancelled,
                crate::claude_wake::WakeOutcome::TimedOut { .. } => crate::receiver_wake::AttemptOutcome::Uncertain("Claude attach timed out after it may have received the nudge".into()),
                crate::claude_wake::WakeOutcome::Failed { detail } => crate::receiver_wake::AttemptOutcome::Uncertain(detail),
            },
        )?;
        match outcome {
            Some(crate::receiver_wake::AttemptOutcome::Cancelled) => {
                return Err(EverettError::new(130, "relay receiver wake cancelled; stopping relay"));
            }
            Some(crate::receiver_wake::AttemptOutcome::PromptSent { .. } | crate::receiver_wake::AttemptOutcome::Uncertain(_)) => break,
            Some(crate::receiver_wake::AttemptOutcome::Deferred(_)) | None => continue,
        }
    }
    Ok(())
}

fn receiver_inbox_pending(job: &crate::receiver_wake::Job) -> bool {
    crate::inbox::pending(&job.imported.session_id, None)
        .iter()
        .any(|message| {
            message.get("id").and_then(Value::as_str) == Some(&job.imported.local_id)
                && message.get("relay_digest").and_then(Value::as_str)
                    == Some(&job.imported.digest)
        })
}

fn local_message_id(envelope: &Envelope) -> String {
    format!(
        "relay-{:x}",
        Sha256::digest(
            json!([
                envelope.owner,
                envelope.from_device,
                envelope.grant_id,
                envelope.message_id
            ])
            .to_string()
        )
    )
}

async fn signed_observation(
    client: &mut RelayClient,
    keys: &Keys,
    sender_owner: &str,
    mid: &str,
    state: &str,
) -> Result<()> {
    let now = milliseconds();
    let bytes = json!([
        "everett-observe-v2",
        keys.device.owner,
        keys.device.device_id,
        sender_owner,
        mid,
        state,
        now
    ])
    .to_string();
    mutation(client, "relay:observe", json!({"deviceId":keys.device.device_id,"senderOwner":sender_owner,"messageId":mid,"state":state,"at":now,"signature":keys.sign(bytes.as_bytes())})).await?;
    Ok(())
}

async fn reconcile(client: &mut RelayClient, keys: &Keys) -> Result<()> {
    for path in crate::paths::glob(&inbox::inbox_dir(), "*.jsonl") {
        let session = path.file_stem().unwrap_or_default().to_string_lossy();
        let done = inbox::done_ids(&session);
        for message in inbox::read(&session) {
            let Some(mid) = message["id"].as_str() else {
                continue;
            };
            if message["relay_owner"] != keys.device.owner
                || message["relay_recipient_device"] != keys.device.device_id
                || !done.contains(mid)
            {
                continue;
            }
            let Some(sender_owner) = message["relay_sender_owner"].as_str() else {
                continue;
            };
            let Some(remote_mid) = message["relay_message_id"].as_str() else {
                continue;
            };
            let marker = root().join(format!(
                "claimed-{:x}",
                Sha256::digest(
                    json!([
                        keys.device.owner,
                        keys.device.device_id,
                        sender_owner,
                        remote_mid
                    ])
                    .to_string()
                )
            ));
            if !marker.exists() {
                if let Err(error) =
                    signed_observation(client, keys, sender_owner, remote_mid, "claimed").await
                {
                    if denied(&error) {
                        check_own_identity(client, keys).await?;
                    }
                    continue;
                }
                peer::save(&marker, b"observed")?;
            }
        }
    }
    for path in crate::paths::glob(&root().join("outgoing"), "*.json") {
        let out: Outgoing = read(&path)?;
        if out.envelope.owner == keys.device.owner
            && out.envelope.from_device == keys.device.device_id
            && out.envelope.sender_fingerprint == keys.device.fingerprint()
            && ((!out.posted && out.envelope.expires_at > milliseconds())
                || (out.posted && out.reply_pending.is_some()))
        {
            let pin = if client.is_native() && out.envelope.recipient_owner == keys.device.owner && out.envelope.grant_id.is_none() {
                active_same_owner_device(client, &keys.device.owner, &out.envelope.to_device).await.map(|device| (device, None))
            } else {
                paired(&out.peer, &keys.device.owner).map(|pin| (pin.device, pin.grant_id))
            };
            let (device, grant_id) = match pin {
                Ok(pin) => pin,
                Err(error) => {
                    if denied(&error) { check_own_identity(client, keys).await?; }
                    continue;
                }
            };
            if device.fingerprint() != out.envelope.recipient_fingerprint
                || device.owner != out.envelope.recipient_owner
                || grant_id != out.envelope.grant_id
            {
                continue;
            }
            let outcome = async {
                if !out.posted {
                    post(client, &path, &out).await?;
                }
                observe_reply(client, keys, &path).await
            }
            .await;
            if let Err(error) = outcome {
                if denied(&error) {
                    check_own_identity(client, keys).await?;
                }
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingPage {
    #[serde(deserialize_with = "crypto::timestamp")]
    server_now: i64,
    envelopes: Vec<PendingRow>,
    is_done: bool,
    continue_cursor: String,
}
#[derive(Deserialize)]
struct PendingRow {
    envelope: Envelope,
}

/// False means transient uncertainty, not revoked auth. Callers retain staging.
async fn check_own_identity(client: &mut RelayClient, keys: &Keys) -> Result<bool> {
    match query(client, "relay:whoami", json!({})).await {
        Ok(identity)
            if identity["owner"] == keys.device.owner
                && identity["deviceId"] == keys.device.device_id =>
        {
            Ok(true)
        }
        Ok(_) => Err(bad("verified backend identity changed")),
        Err(error) if denied(&error) => Err(error),
        Err(_) => Ok(false),
    }
}

async fn publish_staged(
    client: &mut RelayClient,
    keys: &Keys,
    path: &Path,
    envelope: &Envelope,
) -> Result<()> {
    // The private durable ingress inbox is not visible to hooks/MCP until this mutation
    // reauthorizes BOTH agents, membership, current key bundles, grant and TTL.
    // A prior committed delivery can recover after envelope TTL: the authoritative
    // observation rejects an expired QUEUED attempt but permits already delivered data.
    // We still authenticate every bound field before asking that authorization barrier.
    let validation_at = milliseconds().min(envelope.expires_at.saturating_sub(1));
    if let Err(error) = import_inner(keys, envelope, validation_at, false) {
        // A stale sender pin or invalid destination is specific to this row,
        // not evidence that our own agent has been revoked.
        eprintln!("everett relay: staged row not published: {}", error.message);
        if client.is_native() && error.code == 6 { return Ok(()); }
        remove_stage(path, envelope)?;
        return Ok(());
    }
    let authorization = signed_observation(
        client,
        keys,
        &envelope.owner,
        &envelope.message_id,
        "delivered",
    )
    .await;
    if let Err(error) = &authorization {
        if denied(error) {
            // A sender rotation/revocation or grant revocation must not shut down
            // unrelated chats. Independently verify OUR authoritative identity.
            if check_own_identity(client, keys).await? {
                remove_stage(path, envelope)?;
            }
            return Ok(());
        }
        // A transient mutation failure can have committed. Retain this encrypted
        // stage, but continue handling independently authorized envelopes.
        return Ok(());
    }
    finish_staged(keys, path, envelope, authorization)
}

fn remove_stage(path: &Path, envelope: &Envelope) -> Result<()> {
    let parent = path.parent().ok_or_else(|| bad("missing stage parent"))?;
    // Fail before unlinking if the directory cannot be durably flushed.
    fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(bad)?;
    fs::remove_file(path).map_err(bad)?;
    let flushed = fs::File::open(parent).and_then(|f| f.sync_all());
    #[cfg(test)]
    let flushed = if tests::FAIL_STAGE_DELETE_SYNC.with(|f| f.replace(false)) {
        Err(std::io::Error::other(
            "injected stage-directory fsync failure",
        ))
    } else {
        flushed
    };
    if let Err(error) = flushed {
        // Keep recoverable ciphertext on a failed deletion fsync. Publication is
        // idempotent, so retrying this stage cannot duplicate the local message.
        write(path, envelope)?;
        return Err(bad(error));
    }
    Ok(())
}
fn denied(error: &EverettError) -> bool {
    error.message.contains("Convex rejected the request")
        || error.message.starts_with("Cloudflare relay denied")
}

fn finish_staged(
    keys: &Keys,
    path: &Path,
    envelope: &Envelope,
    authorization: Result<()>,
) -> Result<()> {
    if let Err(error) = authorization {
        // Auth/grant/key revocation is terminal for cached query data. Network/timeouts
        // retain only encrypted staging, so an uncertain committed delivery can recover.
        if denied(&error) {
            remove_stage(path, envelope)?;
        }
        return Err(error);
    }
    // Authorization above is signed observed delivery, so TTL is no longer an
    // undelivered-row constraint. This also recovers a crash after server acceptance.
    import(
        keys,
        envelope,
        milliseconds().min(envelope.expires_at.saturating_sub(1)),
    )?;
    remove_stage(path, envelope)?;
    Ok(())
}

async fn receive(
    client: &mut RelayClient,
    keys: &Keys,
    ingress: &Path,
    register: &Value,
    rejected: &mut RejectedCache,
) -> Result<Option<String>> {
    mutation(client, "relay:register", register.clone()).await?;
    for path in crate::paths::glob(ingress, "*.json") {
        let envelope: Envelope = read(&path)?;
        if envelope.recipient_owner == keys.device.owner
            && envelope.to_device == keys.device.device_id
        {
            publish_staged(client, keys, &path, &envelope).await?;
        }
    }
    let mut cursor: Option<String> = None;
    let mut cursors = std::collections::HashSet::new();
    loop {
        let page: PendingPage = serde_json::from_value(
            query(
                client,
                "relay:pending",
                json!({"deviceId":keys.device.device_id,"cursor":cursor}),
            )
            .await?,
        )
        .map_err(bad)?;
        if page.envelopes.len() > 256 {
            return Err(bad("pending page exceeds envelope limit"));
        }
        for row in page.envelopes {
            let envelope = row.envelope;
            if let Err(error) = import_inner(keys, &envelope, page.server_now, false) {
                if rejected.insert(
                    local_message_id(&envelope),
                    envelope.expires_at,
                    page.server_now,
                ) {
                    eprintln!("everett relay: refusing inbox row: {}", error.message);
                }
                continue;
            }
            let path = ingress.join(format!("{}.json", local_message_id(&envelope)));
            write(&path, &envelope)?;
            publish_staged(client, keys, &path, &envelope).await?;
        }
        if page.is_done {
            break;
        }
        if page.continue_cursor.is_empty()
            || cursor.as_ref() == Some(&page.continue_cursor)
            || !cursors.insert(page.continue_cursor.clone())
            || cursors.len() > 256
        {
            return Err(bad("pending cursor did not advance within bounds"));
        }
        cursor = Some(page.continue_cursor);
    }
    reconcile(client, keys).await?;
    drain_receiver_wakes(keys)?;
    note_receiver_webhook(crate::receiver_webhook::poll());
    Ok(cursor)
}

fn note_receiver_webhook(outcome: Result<crate::receiver_webhook::Outcome>) {
    match outcome {
        Ok(outcome) => {
            for line in outcome.logs {
                eprintln!("everett relay: {line}");
            }
        }
        Err(error) => {
            eprintln!("everett relay: receiver webhook not posted: {}", error.message);
        }
    }
}

async fn run_native(
    mut client: RelayClient,
    keys: &Keys,
    ingress: &Path,
    register: &Value,
) -> Result<Value> {
    eprintln!(
        "everett relay: Cloudflare outbound polling; receiver wake and webhook follow the local owner policy"
    );
    let mut rejected = RejectedCache::new();
    let mut delay = 5;
    loop {
        match receive(&mut client, keys, ingress, register, &mut rejected).await {
            Ok(_) => {
                delay = 5;
            }
            Err(error) if denied(&error) || !matches!(error.code, 5 | 6) => return Err(error),
            Err(error) => {
                eprintln!(
                    "everett relay: {}; retrying in {delay}s; encrypted staging kept",
                    error.message
                );
                tokio::time::sleep(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(30);
                continue;
            }
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
}

pub fn run() -> Result<Value> {
    let owner = relay_account()?.owner;
    let keys = verified_keys(&owner)?;
    let register = register_args(&keys.device)?;
    let _guard = daemon_lock()?;
    let ingress = root().join("ingress");
    peer::private_dir(&ingress)?;
    let outcome = tokio::runtime::Runtime::new().map_err(bad)?.block_on(async {
        let mut client = client(&owner).await?;
        if client.is_native() { return run_native(client, &keys, &ingress, &register).await; }
        mutation(&mut client,"relay:register",register.clone()).await?;
        let mut final_cursor: Option<String> = None;
        let mut subscription = client.subscribe("relay:pending",args(json!({"deviceId":keys.device.device_id,"cursor":final_cursor}))?).await.map_err(|_| bad("Convex subscription failed"))?;
        eprintln!("everett relay: subscribed; receiver wake and webhook follow the local owner policy");
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut rejected = RejectedCache::new();
        loop {
            tokio::select! {
                error = client.tls_error() => return Err(bad(error)),
                update = subscription.next() => {
                    // No pending result cache: a denied subscription terminates delivery.
                    result(update.ok_or_else(|| bad("Convex subscription ended"))?)?;
                    tick.reset_immediately();
                }
                _ = tick.tick() => {
                    let (stored,current) = account()?;
                    if current != owner || claims(&stored.id_token)?["everett_agent"] != keys.device.device_id { return Err(bad("signed-in identity changed; restart the relay")); }
                    let cursor = receive(&mut client, &keys, &ingress, &register, &mut rejected).await?;
                    if final_cursor != cursor {
                        subscription = client.subscribe("relay:pending",args(json!({"deviceId":keys.device.device_id,"cursor":cursor}))?).await.map_err(|_| bad("Convex subscription failed"))?;
                        final_cursor = cursor;
                    }
                }
            }
        }
    });
    if outcome.as_ref().err().is_some_and(denied) {
        for path in crate::paths::glob(&ingress, "*.json") {
            if let Ok(envelope) = read::<Envelope>(&path) {
                if envelope.recipient_owner == owner && envelope.to_device == keys.device.device_id
                {
                    remove_stage(&path, &envelope)?;
                }
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests;
