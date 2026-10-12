pub mod crypto;

#[cfg(test)]
use self::tests::ConvexClient;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
#[cfg(not(test))]
use convex::ConvexClient;
use convex::FunctionResult;
use fs2::FileExt;
use futures::StreamExt;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::os::unix::fs::OpenOptionsExt;
use std::{
    collections::BTreeMap,
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

fn root() -> PathBuf {
    session::home().join(".everett/relay")
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

fn deployment() -> Result<String> {
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

fn account() -> Result<(auth::Stored, String)> {
    account_refresh(false)
}

fn account_refresh(force: bool) -> Result<(auth::Stored, String)> {
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

fn claims(token: &str) -> Result<Value> {
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

#[derive(Serialize, Deserialize)]
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
        let (stored, current) = account()?;
        if current != owner {
            return Err(bad("owner changed"));
        }
        let expected_agent = claims(&stored.id_token)?["everett_agent"].clone();
        // Creating/reusing secret keys only follows Convex verification of the JWT
        // and authoritative membership/credential binding, not unverified claims alone.
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
    let (stored, current) = account()?;
    if current != owner {
        return Err(bad("owner changed"));
    }
    let claim = claims(&stored.id_token)?;
    let agent = claim["everett_agent"]
        .as_str()
        .ok_or_else(|| bad("missing agent claim"))?;
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

fn paired(name: &str, owner: &str) -> Result<Paired> {
    let paired: Paired = read(&peer_path(name)?)?;
    paired.device.verify()?;
    if (paired.device.owner != owner && paired.grant_id.is_none())
        || paired.deployment != deployment()?
    {
        return Err(bad(
            "peer is pinned to another account or Convex deployment",
        ));
    }
    Ok(paired)
}

fn trusted(device: &str, sender_owner: &str, grant_id: Option<&str>) -> Result<(String, Paired)> {
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
    let mut result = Map::new();
    for path in crate::paths::glob(&root().join("peers"), "*.json") {
        let pin: Paired = read(&path)?;
        result.insert(path.file_stem().unwrap().to_string_lossy().into(), json!({"transport":"convex", "device_id":pin.device.device_id, "owner":pin.device.owner, "grant_id":pin.grant_id, "fingerprint":pin.device.fingerprint(), "deployment":pin.deployment}));
    }
    Ok(Value::Object(result))
}

pub fn remove(name: &str) -> Result<Value> {
    fs::remove_file(peer_path(name)?).map_err(bad)?;
    Ok(
        json!({"removed":name,"transport":"convex","note":"Local trust removed; permanent backend revocation is everett relay revoke-agent AGENT_ID."}),
    )
}

fn runtime<T: Send + 'static>(
    future: impl std::future::Future<Output = Result<T>> + Send + 'static,
) -> Result<T> {
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new().map_err(bad)?.block_on(async {
            tokio::time::timeout(DEADLINE, future).await.map_err(|_| EverettError::new(5, "Convex request timed out; delivery is unconfirmed. Retry with the same idempotency key."))?
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

fn result(value: FunctionResult) -> Result<Value> {
    match value {
        FunctionResult::Value(v) => Ok(v.into()),
        _ => Err(bad("Convex rejected the request; check OIDC configuration, device registration, TTL, and idempotency key")),
    }
}

async fn client(owner: &str) -> Result<ConvexClient> {
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
    let mut client = ConvexClient::new(&deployment()?)
        .await
        .map_err(|_| bad("cannot connect to Convex"))?;
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

async fn mutation(client: &mut ConvexClient, name: &str, value: Value) -> Result<Value> {
    result(
        tokio::time::timeout(DEADLINE, client.mutation(name, args(value)?))
            .await
            .map_err(|_| bad("Convex mutation timed out; delivery unconfirmed"))?
            .map_err(|_| bad("Convex mutation failed; delivery unconfirmed"))?,
    )
}

async fn query(client: &mut ConvexClient, name: &str, value: Value) -> Result<Value> {
    result(
        tokio::time::timeout(DEADLINE, client.query(name, args(value)?))
            .await
            .map_err(|_| bad("Convex query timed out"))?
            .map_err(|_| bad("Convex query failed"))?,
    )
}

pub fn admin_call(name: &str, value: Value, is_query: bool) -> Result<Value> {
    let (_, owner) = account()?;
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

pub fn rotate() -> Result<Value> {
    let (_, owner) = account()?;
    let _daemon_guard = daemon_lock()?;
    let old = verified_keys(&owner)?;
    let path = key_path(&owner, &old.device.device_id);
    let _guard = peer::lock(&root().join("keys.lock"))?;
    // Persist staged keys before contacting the server; retries recover a committed rotation.
    let stage = path.with_extension("rotation");
    let private = if stage.exists() {
        read::<PrivateKeys>(&stage)?
    } else {
        let mut signing = [0u8; 32];
        let mut encryption = [0u8; 32];
        OsRng.fill_bytes(&mut signing);
        OsRng.fill_bytes(&mut encryption);
        let private = PrivateKeys {
            signing: hex::encode(signing),
            encryption: hex::encode(encryption),
        };
        write(&stage, &private)?;
        private
    };
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
        let registry = query(&mut client, "relay:devices", json!({})).await?;
        if registry
            .as_array()
            .is_some_and(|rows| rows.iter().any(|d| *d == json!(new.device)))
        {
            return Ok(json!({"fingerprint":new.device.fingerprint()}));
        }
        mutation(&mut client,"relay:rotate",json!({"device":new.device,"oldFingerprint":old.device.fingerprint(),"signature":old.sign(bytes.as_bytes())})).await
    })?;
    write(&path, &private)?;
    fs::remove_file(stage).map_err(bad)?;
    Ok(
        json!({"device":device,"result":response,"note":"Peers must explicitly repin; both org admins must reconfirm cross-org grants."}),
    )
}

pub fn register() -> Result<Value> {
    let (_, owner) = account()?;
    let key = verified_keys(&owner)?;
    let device = key.device.clone();
    runtime(async move {
        let mut client = client(&owner).await?;
        mutation(&mut client, "relay:register", json!({"device":device})).await?;
        Ok(json!({"device":device, "fingerprint":device.fingerprint()}))
    })
}

pub fn devices() -> Result<Value> {
    let (_, owner) = account()?;
    runtime(async move {
        let mut client = client(&owner).await?;
        result(
            client
                .query("relay:devices", BTreeMap::new())
                .await
                .map_err(|_| bad("Convex registry query failed"))?,
        )
    })
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
    let (_, owner) = account()?;
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
        let rows = devices()?;
        serde_json::from_value(
            rows.as_array()
                .ok_or_else(|| bad("invalid device registry"))?
                .iter()
                .find(|d| d["deviceId"] == device_id)
                .ok_or_else(|| bad("device is not registered to this owner"))?
                .clone(),
        )
        .map_err(bad)?
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
    let _guard = peer::lock(&root().join("peers.lock"))?;
    if peer::peers()?.contains_key(name) {
        return Err(bad("alias already selects SSH; use a different alias"));
    }
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
    Ok(json!({"peer":name,"transport":"convex","device_id":device_id,"fingerprint":fingerprint}))
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

async fn post(client: &mut ConvexClient, path: &Path, outgoing: &Outgoing) -> Result<Value> {
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

async fn observe_reply(client: &mut ConvexClient, keys: &Keys, path: &Path) -> Result<()> {
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
    let (name, harness, destination) = peer::target(target)?;
    let (_, owner) = account()?;
    let keys = verified_keys(&owner)?;
    let who = send::sender_info(caller);
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
        Ok(json!({"peer":name,"transport":"convex","message_id":outgoing.envelope.message_id,"idempotency_key":outgoing.envelope.idempotency_key,"state":state,"expires_at":outgoing.envelope.expires_at,"pickup":"live relay; hooks/MCP must consume local inbox"}))
    }).map_err(|e| EverettError::new(e.code, format!("{}; idempotency key {key}. A local retry record exists, but Convex queueing is not confirmed.",e.message)))
}

pub fn retain_reply(
    original: &Map<String, Value>,
    text: &str,
    who: &Map<String, Value>,
    hops: i64,
) -> Result<Map<String, Value>> {
    let (_, owner) = account()?;
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
    let (_, owner) = account()?;
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
            json!({"message_id":mid,"state":"unconfirmed","expires_at":outgoing.envelope.expires_at,"note":"A local retry record exists; Convex queueing has not been confirmed. Run everett relay to retry before expiry."}),
        );
    }
    runtime(async move {
        let mut client = client(&owner).await?;
        let status = result(client.query("relay:status",args(json!({"fromDevice":outgoing.envelope.from_device,"messageId":outgoing.envelope.message_id}))?).await.map_err(|_| bad("Convex status query failed"))?)?;
        Ok(
            json!({"message_id":outgoing.envelope.message_id,"state":status["state"],"expires_at":outgoing.envelope.expires_at,"note":"Server state reflects signed observations, never session wake-up."}),
        )
    })
}

pub fn agent_status(mid: &str) -> Result<Value> {
    let (_, owner) = account()?;
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
            if !reply_to.is_empty() {
                observe(&keys.device.owner, envelope, &reply_to, State::Replied)?;
            }
        }
    }
    Ok(())
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
    client: &mut ConvexClient,
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

async fn reconcile(client: &mut ConvexClient, keys: &Keys) -> Result<()> {
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
            let pin = paired(&out.peer, &keys.device.owner)?;
            if pin.device.fingerprint() != out.envelope.recipient_fingerprint
                || pin.device.owner != out.envelope.recipient_owner
                || pin.grant_id != out.envelope.grant_id
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
async fn check_own_identity(client: &mut ConvexClient, keys: &Keys) -> Result<bool> {
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
    client: &mut ConvexClient,
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
        eprintln!("everett relay: refusing staged row: {}", error.message);
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

pub fn run() -> Result<Value> {
    let (_, owner) = account()?;
    let keys = verified_keys(&owner)?;
    let _guard = daemon_lock()?;
    let ingress = root().join("ingress");
    peer::private_dir(&ingress)?;
    let outcome = tokio::runtime::Runtime::new().map_err(bad)?.block_on(async {
        let mut client = client(&owner).await?;
        mutation(&mut client,"relay:register",json!({"device":keys.device})).await?;
        let mut final_cursor: Option<String> = None;
        let mut subscription = client.subscribe("relay:pending",args(json!({"deviceId":keys.device.device_id,"cursor":final_cursor}))?).await.map_err(|_| bad("Convex subscription failed"))?;
        eprintln!("everett relay: subscribed; inbox-only, no session wake-up");
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut rejected = std::collections::HashSet::new();
        loop {
            tokio::select! {
                update = subscription.next() => {
                    // No pending result cache: a denied subscription terminates delivery.
                    result(update.ok_or_else(|| bad("Convex subscription ended"))?)?;
                    tick.reset_immediately();
                }
                _ = tick.tick() => {
                    let (stored,current) = account()?;
                    if current != owner || claims(&stored.id_token)?["everett_agent"] != keys.device.device_id { return Err(bad("signed-in identity changed; restart the relay")); }
                    // Mutation barrier rejects a revoked credential even if a query snapshot is stale.
                    mutation(&mut client,"relay:register",json!({"device":keys.device})).await?;
                    for path in crate::paths::glob(&ingress,"*.json") {
                        let envelope: Envelope = read(&path)?;
                        if envelope.recipient_owner == owner && envelope.to_device == keys.device.device_id {
                            publish_staged(&mut client,&keys,&path,&envelope).await?;
                        }
                    }
                    let mut cursor: Option<String> = None;
                    loop {
                        let page: PendingPage = serde_json::from_value(query(&mut client,"relay:pending",json!({"deviceId":keys.device.device_id,"cursor":cursor})).await?).map_err(bad)?;
                        for row in page.envelopes {
                            let envelope = row.envelope;
                            if let Err(error) = import_inner(&keys,&envelope,page.server_now,false) {
                                if rejected.insert(local_message_id(&envelope)) { eprintln!("everett relay: refusing inbox row: {}",error.message); }
                                continue;
                            }
                            // Persist authenticated encrypted Body in the private ingress inbox first.
                            let path = ingress.join(format!("{}.json",local_message_id(&envelope)));
                            write(&path,&envelope)?;
                            publish_staged(&mut client,&keys,&path,&envelope).await?;
                        }
                        if page.is_done {
                            if final_cursor != cursor {
                                subscription = client.subscribe("relay:pending",args(json!({"deviceId":keys.device.device_id,"cursor":cursor}))?).await.map_err(|_| bad("Convex subscription failed"))?;
                                final_cursor = cursor;
                            }
                            break;
                        }
                        if cursor.as_ref() == Some(&page.continue_cursor) { return Err(bad("pending cursor did not advance")); }
                        cursor = Some(page.continue_cursor);
                    }
                    reconcile(&mut client,&keys).await?;
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
