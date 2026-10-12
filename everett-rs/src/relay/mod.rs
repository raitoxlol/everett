pub mod crypto;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use convex::{ConvexClient, FunctionResult};
use futures::StreamExt;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::error::{EverettError, Result};
use crate::{auth, inbox, peer, send, session};
use crypto::{Device, Envelope, Keys};

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
    peer::save(path, &serde_json::to_vec_pretty(value).map_err(bad)?)
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
    let claims = |token: &str| -> Result<Value> {
        let part = token.split('.').nth(1).ok_or_else(|| bad("login did not return an OIDC ID token; run everett login --force with an OIDC provider"))?;
        serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(part)
                .map_err(|_| bad("invalid OIDC ID token"))?,
        )
        .map_err(|_| bad("invalid OIDC ID token"))
    };
    let mut claim = claims(&stored.id_token)?;
    if claim["exp"].as_f64().unwrap_or(0.0) - session::now() < 60.0 {
        stored.expires_at = 0.0;
        stored = auth::ensure_fresh(&stored)?;
        claim = claims(&stored.id_token)?;
    }
    let issuer = claim["iss"]
        .as_str()
        .ok_or_else(|| bad("ID token has no issuer"))?;
    if issuer.trim_end_matches('/') != stored.issuer.trim_end_matches('/')
        || claim["sub"] != stored.sub
        || claim["exp"].as_f64().unwrap_or(0.0) <= session::now()
    {
        return Err(bad(
            "expired or mismatched ID token; run everett login --force",
        ));
    }
    // Claims select local account storage; Convex independently verifies the JWT signature and audience.
    let owner = json!([issuer, stored.sub]).to_string();
    Ok((stored, owner))
}

fn keys(owner: &str) -> Result<Keys> {
    initialize()?;
    let identity = peer::identity()?;
    let _guard = peer::lock(&root().join("keys.lock"))?;
    let ssh =
        ssh_key::PrivateKey::read_openssh_file(&session::home().join(".everett/peer/id_ed25519"))
            .map_err(bad)?;
    let signing = ssh
        .key_data()
        .ed25519()
        .ok_or_else(|| bad("peer identity is not Ed25519"))?
        .private
        .to_bytes();
    let path = root().join("x25519");
    let encryption = if path.exists() {
        crypto::decode(&fs::read_to_string(&path).map_err(bad)?)?
    } else {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        peer::save(&path, hex::encode(key).as_bytes())?;
        key
    };
    Ok(Keys::new(
        owner.into(),
        identity["device_id"]
            .as_str()
            .ok_or_else(|| bad("missing device identity"))?
            .into(),
        signing,
        encryption,
    ))
}

fn paired(name: &str, owner: &str) -> Result<Paired> {
    let paired: Paired = read(&peer_path(name)?)?;
    paired.device.verify()?;
    if paired.device.owner != owner || paired.deployment != deployment()? {
        return Err(bad(
            "peer is pinned to another account or Convex deployment",
        ));
    }
    Ok(paired)
}

fn trusted(device: &str, owner: &str) -> Result<(String, Paired)> {
    for path in crate::paths::glob(&root().join("peers"), "*.json") {
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if let Ok(pin) = paired(&name, owner) {
            if pin.device.device_id == device {
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
        result.insert(path.file_stem().unwrap().to_string_lossy().into(), json!({"transport":"convex", "device_id":pin.device.device_id, "fingerprint":pin.device.fingerprint(), "deployment":pin.deployment}));
    }
    Ok(Value::Object(result))
}

pub fn remove(name: &str) -> Result<Value> {
    fs::remove_file(peer_path(name)?).map_err(bad)?;
    Ok(
        json!({"removed":name,"transport":"convex","note":"Local trust removed; revoke a lost device's OIDC credentials at the provider."}),
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
    let (_, current) = account()?;
    if current != owner {
        return Err(bad("signed-in owner changed; restart the relay"));
    }
    let expected = owner.to_string();
    let mut client = ConvexClient::new(&deployment()?)
        .await
        .map_err(|_| bad("cannot connect to Convex"))?;
    client
        .set_auth_callback(Some(Box::new(move |force| {
            let expected = expected.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    let (stored, current) = account_refresh(force)?;
                    if current != expected {
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

pub fn register() -> Result<Value> {
    let (_, owner) = account()?;
    let key = keys(&owner)?;
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

pub fn add(name: &str, device_id: &str, fingerprint: &str) -> Result<Value> {
    valid(name)?;
    valid(device_id)?;
    initialize()?;
    let (_, owner) = account()?;
    let rows = devices()?;
    let device: Device = serde_json::from_value(
        rows.as_array()
            .ok_or_else(|| bad("invalid device registry"))?
            .iter()
            .find(|d| d["deviceId"] == device_id)
            .ok_or_else(|| bad("device is not registered to this owner"))?
            .clone(),
    )
    .map_err(bad)?;
    device.verify()?;
    if device.owner != owner || device.fingerprint() != fingerprint {
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
    };
    let path = peer_path(name)?;
    if path.exists() {
        let existing: Paired = read(&path)?;
        if existing.device != pin.device || existing.deployment != pin.deployment {
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

fn prepare(name: &str, key: &str, payload: &Payload, keys: &Keys) -> Result<(PathBuf, Outgoing)> {
    valid(key)?;
    payload.validate()?;
    let pin = paired(name, &keys.device.owner)?;
    let digest = format!("{:x}", Sha256::digest(json!([name, payload]).to_string()));
    let path = outgoing_path(&keys.device.owner, key);
    let _guard = peer::lock(&root().join("outgoing.lock"))?;
    if path.exists() {
        let existing: Outgoing = read(&path)?;
        if existing.digest != digest || existing.envelope.to_device != pin.device.device_id {
            return Err(bad("idempotency key already used for a different request"));
        }
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
        envelope: keys.seal(&pin.device, key, payload, milliseconds(), TTL)?,
        digest,
        session,
        harness,
        posted: false,
        state: State::Queued,
    };
    write(&path, &outgoing)?;
    Ok((path, outgoing))
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

pub fn send(target: &str, text: &str, key: Option<&str>, caller: Option<&str>) -> Result<Value> {
    let (name, harness, destination) = peer::target(target)?;
    let (_, owner) = account()?;
    let keys = keys(&owner)?;
    let who = send::sender_info(caller);
    let from = who["sender"].as_str().unwrap_or(inbox::HUMAN);
    let hops = send::current_hops().max(inbox::session_hops(from)).max(0) + 1;
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
    };
    let (path, outgoing) = prepare(name, key, &payload, &keys)?;
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
        let state = if observed.state != State::Queued { json!(observed.state) } else if response["state"] == "expired" { json!("expired") } else { json!("queued") };
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
    let keys = keys(&owner)?;
    if original["relay_owner"] != owner {
        return Err(bad("message belongs to another signed-in account"));
    }
    let (name, _) = trusted(
        original["relay_device"]
            .as_str()
            .ok_or_else(|| bad("missing sender device"))?,
        &owner,
    )?;
    let mid = original["id"]
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
    };
    let (path, outgoing) = prepare(&name, &format!("reply-{mid}"), &payload, &keys)?;
    let mid = mid.to_string();
    runtime(async move {
        let mut client = client(&owner).await?;
        post(&mut client, &path, &outgoing).await?;
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
    if outgoing.peer != name {
        return Err(bad("message is for another peer"));
    }
    if outgoing.state != State::Queued {
        return Ok(json!({"message_id":mid,"state":outgoing.state}));
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
            json!({"message_id":outgoing.envelope.message_id,"state":if status["state"] == "accepted" {json!("queued")} else {status["state"].clone()},"relay_state":status["state"],"expires_at":outgoing.envelope.expires_at,"note":"Delivered/replied require authenticated encrypted receipts observed by the sender relay. Accepted means local inbox write only."}),
        )
    })
}

fn observe(owner: &str, device: &str, mid: &str, state: State) -> Result<()> {
    let _guard = peer::lock(&root().join("outgoing.lock"))?;
    let (path, mut out) = find_outgoing(owner, mid)?;
    if out.envelope.to_device != device {
        return Err(bad("receipt is from the wrong device"));
    }
    if out.state != State::Replied {
        out.state = state;
        write(&path, &out)?;
    }
    Ok(())
}

pub fn import(keys: &Keys, envelope: &Envelope, now: i64) -> Result<()> {
    let (_, pin) = trusted(&envelope.from_device, &keys.device.owner)?;
    let payload = keys.open(&pin.device, envelope, now)?;
    let _guard = peer::lock(&root().join("inbox.lock"))?;
    match payload {
        Payload::Delivered { message_id } => observe(
            &keys.device.owner,
            &envelope.from_device,
            &message_id,
            State::Delivered,
        )?,
        Payload::Inbox {
            session,
            harness,
            text,
            from,
            from_harness,
            from_card,
            reply_to,
            hops,
        } => {
            if !reply_to.is_empty() {
                let (_, out) = find_outgoing(&keys.device.owner, &reply_to)?;
                if out.envelope.to_device != envelope.from_device
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
            let digest = format!("{:x}", Sha256::digest(envelope.signed_bytes()));
            if let Some(existing) = inbox::find(&envelope.message_id) {
                if existing["relay_digest"] != digest {
                    return Err(bad("duplicate message id has different ciphertext"));
                }
            } else {
                let extra = serde_json::from_value(json!({"id":envelope.message_id,"relay_owner":keys.device.owner,"relay_device":envelope.from_device,"relay_sender":from,"relay_digest":digest})).map_err(bad)?;
                inbox::post(
                    &session,
                    &text,
                    &format!("{}/{from_harness}/{from}", envelope.from_device),
                    if reply_to.is_empty() {
                        "message"
                    } else {
                        "reply"
                    },
                    &reply_to,
                    hops,
                    &from_harness,
                    &from_card,
                    Some(extra),
                )?;
                let file = fs::OpenOptions::new()
                    .write(true)
                    .open(inbox::inbox_dir().join(format!("{session}.jsonl")))
                    .map_err(bad)?;
                file.sync_all().map_err(bad)?;
            }
            if !reply_to.is_empty() {
                observe(
                    &keys.device.owner,
                    &envelope.from_device,
                    &reply_to,
                    State::Replied,
                )?;
            }
        }
    }
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
            if message["relay_owner"] != keys.device.owner || !done.contains(mid) {
                continue;
            }
            let Some(device) = message["relay_device"].as_str() else {
                continue;
            };
            let Ok((name, _)) = trusted(device, &keys.device.owner) else {
                continue;
            };
            let (path, out) = prepare(
                &name,
                &format!("delivered-{mid}"),
                &Payload::Delivered {
                    message_id: mid.into(),
                },
                keys,
            )?;
            if !out.posted && out.envelope.expires_at > milliseconds() {
                post(client, &path, &out).await?;
            }
        }
    }
    for path in crate::paths::glob(&root().join("outgoing"), "*.json") {
        let out: Outgoing = read(&path)?;
        if out.envelope.owner == keys.device.owner
            && !out.posted
            && out.envelope.expires_at > milliseconds()
        {
            paired(&out.peer, &keys.device.owner)?;
            post(client, &path, &out).await?;
        }
    }
    Ok(())
}

pub fn run() -> Result<Value> {
    let (_, owner) = account()?;
    let keys = keys(&owner)?;
    let _guard = peer::lock(&root().join("daemon.lock"))?;
    tokio::runtime::Runtime::new().map_err(bad)?.block_on(async {
        let mut client = client(&owner).await?;
        mutation(&mut client,"relay:register",json!({"device":keys.device})).await?;
        let mut subscription = client.subscribe("relay:pending",args(json!({"deviceId":keys.device.device_id}))?).await.map_err(|_| bad("Convex subscription failed"))?;
        eprintln!("everett relay: subscribed; inbox-only, no session wake-up");
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut pending = Vec::<Envelope>::new();
        let mut rejected = std::collections::HashSet::new();
        loop {
            tokio::select! {
                update = subscription.next() => {
                    let value = result(update.ok_or_else(|| bad("Convex subscription ended"))?)?;
                    pending = serde_json::from_value(value).map_err(bad)?;
                    tick.reset_immediately();
                }
                _ = tick.tick() => {
                    let (_,current) = account()?;
                    if current != owner { return Err(bad("signed-in owner changed; restart the relay")); }
                    for envelope in &pending {
                        if let Err(error) = import(&keys,envelope,milliseconds()) {
                            if rejected.insert(envelope.message_id.clone()) { eprintln!("everett relay: {}: {}",envelope.message_id,error.message); }
                            continue;
                        }
                        let now = milliseconds();
                        let bytes = json!(["everett-accept-v1",owner,keys.device.device_id,envelope.message_id,now]).to_string();
                        if let Err(error) = mutation(&mut client,"relay:accept",json!({"deviceId":keys.device.device_id,"messageId":envelope.message_id,"at":now,"signature":keys.sign(bytes.as_bytes())})).await { eprintln!("everett relay: {}", error.message); }
                    }
                    if let Err(error) = reconcile(&mut client,&keys).await { eprintln!("everett relay: {}",error.message); }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests;
