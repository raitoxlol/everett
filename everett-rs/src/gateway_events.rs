use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use subtle::ConstantTimeEq;
use ureq::unversioned::{
    resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver},
    transport::{DefaultConnector, NextTimeout},
};

const EVENT: &str = "message.created";
const DAY: i64 = 86_400_000;
const MAX_SUBSCRIPTIONS: i64 = 64;

#[derive(Clone)]
pub(super) struct Events {
    db: Arc<Mutex<Connection>>,
    pending: Arc<Mutex<std::collections::HashMap<String, String>>>,
    acknowledgments: Arc<Mutex<Vec<Acknowledgment>>>,
    home: PathBuf,
    transport: Arc<dyn Webhook>,
}

struct Acknowledgment {
    subscription: String,
    message: String,
    claim: String,
}

struct Verification {
    pending: Arc<Mutex<std::collections::HashMap<String, String>>>,
    id: String,
    revision: String,
}
impl Drop for Verification {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock() {
            if pending.get(&self.id) == Some(&self.revision) {
                pending.remove(&self.id);
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct Principal {
    pub owner: String,
    pub agent: String,
    pub token: Option<String>,
}

#[derive(Clone)]
pub(super) struct Lease {
    pub id: String,
    pub principal: Principal,
}

#[derive(Debug)]
pub(super) struct Failure {
    pub code: i32,
    pub reason: &'static str,
}
fn invalid(reason: &'static str) -> Failure {
    Failure {
        code: -32602,
        reason,
    }
}
fn storage() -> Failure {
    Failure {
        code: -32603,
        reason: "event storage unavailable",
    }
}
fn callback(reason: &'static str) -> Failure {
    Failure {
        code: -32015,
        reason,
    }
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn digest(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}
fn iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

trait Webhook: Send + Sync {
    fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), &'static str>;
}

#[derive(Debug)]
struct PublicResolver;
impl Resolver for PublicResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let addresses = DefaultResolver::default().resolve(uri, config, timeout)?;
        if addresses.iter().any(|a| !public_ip(a.ip())) {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(addresses)
    }
}
fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
        || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
        || (a == 203 && b == 0 && c == 113))
}
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => {
            let s = ip.segments();
            // Only global unicast; exclude transition, documentation and special-use ranges.
            s[0] & 0xe000 == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
                && !(s[0] == 0x3fff && s[1] < 0x1000)
                && ip != Ipv6Addr::UNSPECIFIED
        }
    }
}
struct HttpsWebhook;
impl Webhook for HttpsWebhook {
    fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), &'static str> {
        validate_url(url).map_err(|_| "invalid_callback_url")?;
        let config = ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .max_idle_connections(0)
            .timeout_global(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .build();
        let agent = ureq::Agent::with_parts(config, DefaultConnector::default(), PublicResolver);
        let mut request = agent.post(url);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send(body).map_err(|e| {
            if matches!(e, ureq::Error::Timeout(_)) {
                "timeout"
            } else {
                "transport_failed"
            }
        })?;
        finish_response(response)
    }
}
fn finish_response(
    mut response: ureq::http::Response<ureq::Body>,
) -> Result<(u16, Vec<u8>), &'static str> {
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(8192)
        .read_to_vec()
        .unwrap_or_default();
    Ok((status, body))
}
fn validate_url(raw: &str) -> Result<(), Failure> {
    let u = url::Url::parse(raw).map_err(|_| invalid("invalid_callback_url"))?;
    if raw.len() > 2048
        || u.scheme() != "https"
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
        || u.port_or_known_default() != Some(443)
    {
        return Err(invalid("invalid_callback_url"));
    }
    match u.host() {
        Some(url::Host::Ipv4(ip)) if !public_v4(ip) => return Err(invalid("invalid_callback_url")),
        Some(url::Host::Ipv6(ip)) if !public_ip(IpAddr::V6(ip)) => {
            return Err(invalid("invalid_callback_url"))
        }
        _ => {}
    }
    Ok(())
}
fn secret(raw: &str) -> Result<Vec<u8>, Failure> {
    let bytes = raw
        .strip_prefix("whsec_")
        .and_then(|s| STANDARD.decode(s).ok())
        .ok_or_else(|| invalid("invalid_signing_secret"))?;
    if !(24..=64).contains(&bytes.len()) {
        return Err(invalid("invalid_signing_secret"));
    }
    Ok(bytes)
}
fn headers(
    id: &str,
    event: &str,
    body: &[u8],
    key: &str,
    previous: Option<&str>,
) -> Result<Vec<(String, String)>, Failure> {
    let timestamp = (now() / 1000).to_string();
    let prefix = format!("{event}.{timestamp}.");
    let mut signatures = Vec::new();
    for key in std::iter::once(key).chain(previous) {
        let mut mac = Hmac::<Sha256>::new_from_slice(&secret(key)?).map_err(|_| storage())?;
        mac.update(prefix.as_bytes());
        mac.update(body);
        signatures.push(format!(
            "v1,{}",
            STANDARD.encode(mac.finalize().into_bytes())
        ));
    }
    Ok(vec![
        ("Content-Type".into(), "application/json".into()),
        ("webhook-id".into(), event.into()),
        ("webhook-timestamp".into(), timestamp),
        ("webhook-signature".into(), signatures.join(" ")),
        ("X-MCP-Subscription-Id".into(), id.into()),
    ])
}

impl Events {
    pub fn open(home: &Path, scope: &str) -> Result<Self, String> {
        let directory = home.join(".everett/gateway-events");
        crate::storage::private_dir(&directory)
            .map_err(|_| "cannot create private event directory")?;
        let path = directory.join(format!("{}.sqlite3", digest(scope.as_bytes())));
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|_| "cannot open event store")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|_| "cannot protect event store")?;
        let db = Connection::open(path).map_err(|_| "cannot open event database")?;
        Self::with_db(db, home.to_path_buf(), Arc::new(HttpsWebhook))
            .map_err(|_| "cannot initialize event database".into())
    }
    fn with_db(
        mut db: Connection,
        home: PathBuf,
        transport: Arc<dyn Webhook>,
    ) -> Result<Self, Failure> {
        db.busy_timeout(Duration::from_secs(2))
            .map_err(|_| storage())?;
        db.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS subscriptions(id TEXT PRIMARY KEY,owner TEXT NOT NULL,agent TEXT NOT NULL,token TEXT,url TEXT NOT NULL,secret TEXT NOT NULL,previous TEXT,rotation_until INTEGER NOT NULL,created INTEGER NOT NULL,expires INTEGER NOT NULL,verified INTEGER NOT NULL,blocked TEXT NOT NULL DEFAULT ''); CREATE TABLE IF NOT EXISTS deliveries(subscription TEXT REFERENCES subscriptions(id) ON DELETE CASCADE,message TEXT,event TEXT NOT NULL,body TEXT NOT NULL,attempts INTEGER NOT NULL DEFAULT 0,next INTEGER NOT NULL DEFAULT 0,done INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(subscription,message));").map_err(|_| storage())?;
        let migration = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| storage())?;
        for (table, column, alter) in [
            ("subscriptions", "revision", "ALTER TABLE subscriptions ADD COLUMN revision TEXT NOT NULL DEFAULT ''"),
            ("deliveries", "claim", "ALTER TABLE deliveries ADD COLUMN claim TEXT NOT NULL DEFAULT ''"),
        ] {
            let exists: bool = migration.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?) WHERE name=?)",
                params![table, column], |row| row.get(0),
            ).map_err(|_| storage())?;
            if !exists {
                migration.execute(alter, []).map_err(|_| storage())?;
            }
        }
        migration.commit().map_err(|_| storage())?;
        Ok(Self {
            pending: Arc::new(Mutex::new(std::collections::HashMap::new())),
            db: Arc::new(Mutex::new(db)),
            acknowledgments: Arc::new(Mutex::new(Vec::new())),
            home,
            transport,
        })
    }
    pub fn catalog(agent: &str) -> Value {
        json!({"events":[{"name":EVENT,"description":"A new message is available in this connection's Everett inbox. Read the inbox tool for untrusted message content.","delivery":["webhook"],"inputSchema":{"type":"object","properties":{"session_id":{"type":"string","enum":[agent]}},"required":["session_id"],"additionalProperties":false},"payloadSchema":{"type":"object","properties":{"session_id":{"type":"string"},"message_id":{"type":"string"}},"required":["session_id","message_id"],"additionalProperties":false}}]})
    }
    fn identity(p: &Principal, args: &Value) -> Result<(String, String), Failure> {
        if args["name"] != EVENT
            || args["arguments"] != json!({"session_id":p.agent})
            || args["delivery"]["mode"] != "webhook"
            || !args.get("cursor").is_none_or(Value::is_null)
        {
            return Err(invalid("unsupported_event_or_filter"));
        }
        let url = args["delivery"]["url"]
            .as_str()
            .ok_or_else(|| invalid("invalid_callback_url"))?;
        validate_url(url)?;
        let id = format!(
            "sub_{}",
            digest(
                serde_json::to_string(&json!([p.owner, p.agent, EVENT, url]))
                    .unwrap()
                    .as_bytes()
            )
        );
        Ok((id, url.into()))
    }
    #[cfg(test)]
    pub fn subscribe(&self, p: &Principal, args: &Value) -> Result<Value, Failure> {
        self.subscribe_until(p, args, None)
    }
    pub fn subscribe_until(
        &self,
        p: &Principal,
        args: &Value,
        credential_expires_at: Option<i64>,
    ) -> Result<Value, Failure> {
        self.subscribe_with_clock(p, args, credential_expires_at, now)
    }

    fn subscribe_with_clock(
        &self,
        p: &Principal,
        args: &Value,
        credential_expires_at: Option<i64>,
        clock: impl Fn() -> i64,
    ) -> Result<Value, Failure> {
        let (id, url) = Self::identity(p, args)?;
        let key = args["delivery"]["secret"]
            .as_str()
            .ok_or_else(|| invalid("invalid_signing_secret"))?;
        secret(key)?;
        let ttl = match args.get("ttlMs") {
            None | Some(Value::Null) => DAY,
            Some(v) => v
                .as_i64()
                .filter(|n| *n > 0)
                .ok_or_else(|| invalid("invalid_ttl"))?
                .min(DAY),
        };
        let db = self.db.lock().map_err(|_| storage())?;
        let at = clock();
        let expires = credential_expires_at.map_or(at + ttl, |deadline| deadline.min(at + ttl));
        if expires <= at {
            return Err(invalid("token_expired"));
        }
        db.execute("DELETE FROM subscriptions WHERE expires<=?", [at])
            .map_err(|_| storage())?;
        let old: Option<(String, i64, i64, String)> = db
            .query_row(
                "SELECT secret,created,verified,revision FROM subscriptions WHERE id=? AND owner=?",
                params![id, p.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|_| storage())?;
        if old.is_none()
            && db
                .query_row("SELECT count(*) FROM subscriptions", [], |r| {
                    r.get::<_, i64>(0)
                })
                .map_err(|_| storage())?
                >= MAX_SUBSCRIPTIONS
        {
            return Err(invalid("subscription_limit"));
        }
        let blocked: Option<String> = db
            .query_row("SELECT blocked FROM subscriptions WHERE id=?", [&id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|_| storage())?;
        if blocked.is_some_and(|b| !b.is_empty()) {
            return Err(invalid(
                "event_capacity_exceeded_unsubscribe_and_resubscribe",
            ));
        }
        let cached = old
            .as_ref()
            .is_some_and(|o| o.0 == key && at - o.2 < 300_000);
        let revision = uuid::Uuid::new_v4().to_string();
        let verification = {
            let mut pending = self.pending.lock().map_err(|_| storage())?;
            if !pending.contains_key(&id) && pending.len() >= MAX_SUBSCRIPTIONS as usize {
                return Err(invalid("verification_capacity"));
            }
            pending.insert(id.clone(), revision.clone());
            Verification {
                pending: self.pending.clone(),
                id: id.clone(),
                revision: revision.clone(),
            }
        };
        drop(db);
        if !cached {
            let challenge = uuid::Uuid::new_v4().to_string();
            let body =
                serde_json::to_vec(&json!({"type":"verification","challenge":challenge})).unwrap();
            let event = format!("verification_{}", uuid::Uuid::new_v4());
            let (status, response) = self
                .transport
                .post(&url, &headers(&id, &event, &body, key, None)?, &body)
                .map_err(callback)?;
            let reply: Value =
                serde_json::from_slice(&response).map_err(|_| callback("challenge_failed"))?;
            if !(200..300).contains(&status)
                || !reply["challenge"]
                    .as_str()
                    .is_some_and(|r| bool::from(r.as_bytes().ct_eq(challenge.as_bytes())))
            {
                return Err(callback("challenge_failed"));
            }
        }
        let mut db = self.db.lock().map_err(|_| storage())?;
        if self.pending.lock().map_err(|_| storage())?.get(&id) != Some(&verification.revision) {
            return Err(invalid("subscription_changed_retry"));
        }
        if expires <= clock() {
            return Err(invalid("token_expired"));
        }
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| storage())?;
        if expires <= clock() {
            return Err(invalid("token_expired"));
        }
        let current: Option<(String, String)> = tx
            .query_row(
                "SELECT revision,blocked FROM subscriptions WHERE id=?",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| storage())?;
        if current.as_ref().map(|r| &r.0) != old.as_ref().map(|r| &r.3) {
            return Err(invalid("subscription_changed_retry"));
        }
        if current.as_ref().is_some_and(|r| !r.1.is_empty()) {
            return Err(invalid(
                "event_capacity_exceeded_unsubscribe_and_resubscribe",
            ));
        }
        if current.is_none()
            && tx
                .query_row("SELECT count(*) FROM subscriptions", [], |r| {
                    r.get::<_, i64>(0)
                })
                .map_err(|_| storage())?
                >= MAX_SUBSCRIPTIONS
        {
            return Err(invalid("subscription_limit"));
        }
        let verified = if cached { old.as_ref().unwrap().2 } else { at };
        let created = old.as_ref().map_or(at, |o| o.1);
        let previous = old.as_ref().filter(|o| o.0 != key).map(|o| o.0.clone());
        tx.execute("INSERT INTO subscriptions(id,owner,agent,token,url,secret,previous,rotation_until,created,expires,verified,revision) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET token=excluded.token,secret=excluded.secret,previous=CASE WHEN excluded.previous IS NULL THEN subscriptions.previous ELSE excluded.previous END,rotation_until=CASE WHEN excluded.previous IS NULL THEN subscriptions.rotation_until ELSE excluded.rotation_until END,expires=excluded.expires,verified=excluded.verified,revision=excluded.revision",params![id,p.owner,p.agent,p.token,url,key,previous,at+300_000,created,expires,verified,revision]).map_err(|_|storage())?;
        tx.commit().map_err(|_| storage())?;
        Ok(json!({"id":id,"refreshBefore":iso(expires),"cursor":null,"truncated":false}))
    }
    pub fn unsubscribe(&self, p: &Principal, args: &Value) -> Result<Value, Failure> {
        let (id, _) = Self::identity(p, args)?;
        let db = self.db.lock().map_err(|_| storage())?;
        self.pending.lock().map_err(|_| storage())?.remove(&id);
        db.execute(
            "DELETE FROM subscriptions WHERE id=? AND owner=?",
            params![id, p.owner],
        )
        .map_err(|_| storage())?;
        Ok(json!({}))
    }
    pub fn leases(&self) -> Result<Vec<Lease>, Failure> {
        let db = self.db.lock().map_err(|_| storage())?;
        self.flush_acknowledgments(&db, None)?;
        db.execute("DELETE FROM subscriptions WHERE expires<=?", [now()])
            .map_err(|_| storage())?;
        let mut s = db
            .prepare("SELECT id,owner,agent,token FROM subscriptions WHERE blocked=''")
            .map_err(|_| storage())?;
        let rows = s
            .query_map([], |r| {
                Ok(Lease {
                    id: r.get(0)?,
                    principal: Principal {
                        owner: r.get(1)?,
                        agent: r.get(2)?,
                        token: r.get(3)?,
                    },
                })
            })
            .map_err(|_| storage())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|_| storage())
    }
    pub fn revoke(&self, lease: &Lease) {
        if let Ok(db) = self.db.lock() {
            let removed = db
                .execute(
                    "DELETE FROM subscriptions WHERE id=? AND token IS ?",
                    params![lease.id, lease.principal.token],
                )
                .unwrap_or(0);
            if removed > 0 {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&lease.id);
                }
            }
        }
    }
    fn flush_acknowledgments(
        &self,
        db: &Connection,
        accepted: Option<Acknowledgment>,
    ) -> Result<(), Failure> {
        let mut receipts = self.acknowledgments.lock().map_err(|_| storage())?;
        receipts.extend(accepted);
        while let Some(receipt) = receipts.last() {
            // Refresh keeps the row. A replacement or newer claim invalidates this receipt.
            db.execute(
                "UPDATE deliveries SET done=1, attempts=attempts+1
                 WHERE subscription=?1 AND message=?2 AND claim=?3 AND done=0",
                params![receipt.subscription, receipt.message, receipt.claim],
            ).map_err(|_| storage())?;
            receipts.pop();
        }
        Ok(())
    }
    pub fn deliver(&self, lease: &Lease) -> Result<(), Failure> {
        let mut db = self.db.lock().map_err(|_| storage())?;
        self.flush_acknowledgments(&db, None)?;
        let record:Option<(String,String,Option<String>,i64,i64,String)>=db.query_row("SELECT url,secret,previous,rotation_until,created,revision FROM subscriptions WHERE id=? AND owner=? AND expires>? AND token IS ? AND blocked=''",params![lease.id,lease.principal.owner,now(),lease.principal.token],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(|_|storage())?;
        let Some((url, key, previous, rotation, created, revision)) = record else {
            return Ok(());
        };
        if !crate::inbox::valid_id(&lease.principal.agent) {
            return Err(storage());
        }
        let inbox = crate::inbox::path_in(&self.home, &lease.principal.agent);
        let mut reader: Box<dyn BufRead> = match File::open(inbox) {
            Ok(file) => Box::new(BufReader::new(file)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Box::new(std::io::empty()),
            Err(_) => return Err(storage()),
        };
        let tx = db.transaction().map_err(|_| storage())?;
        let count: i64 = tx
            .query_row(
                "SELECT count(*) FROM deliveries WHERE subscription=?",
                [&lease.id],
                |r| r.get(0),
            )
            .map_err(|_| storage())?;
        let mut remaining = 10000 - count;
        loop {
            let mut line = Vec::new();
            let size = (&mut reader)
                .take(8 * 1024 * 1024 + 1)
                .read_until(b'\n', &mut line)
                .map_err(|_| storage())?;
            if size == 0 {
                break;
            }
            if size > 8 * 1024 * 1024 {
                tx.execute(
                    "UPDATE subscriptions SET blocked='record_capacity' WHERE id=?",
                    [&lease.id],
                )
                .map_err(|_| storage())?;
                tx.commit().map_err(|_| storage())?;
                return Err(invalid(
                    "event_capacity_exceeded_unsubscribe_and_resubscribe",
                ));
            }
            let Ok(m) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            let Some(mid) = m["id"].as_str().filter(|id| crate::inbox::valid_id(id)) else {
                continue;
            };
            let Some(ts) = m["ts"].as_f64().filter(|ts| ts.is_finite() && *ts >= 0.) else {
                continue;
            };
            let ms = (ts * 1000.) as i64;
            if ms < created || ms > now() + 60_000 || m["to"] != lease.principal.agent {
                continue;
            }
            let event = format!("evt_{}", digest(format!("{}:{mid}", lease.id).as_bytes()));
            let body=serde_json::to_string(&json!({"eventId":event,"name":EVENT,"timestamp":iso(ms),"data":{"session_id":lease.principal.agent,"message_id":mid},"cursor":null})).unwrap();
            if remaining <= 0 {
                let known:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM deliveries WHERE subscription=? AND message=?)",params![lease.id,mid],|r|r.get(0)).map_err(|_|storage())?;
                if known {
                    continue;
                }
                tx.execute(
                    "UPDATE subscriptions SET blocked='capacity' WHERE id=?",
                    [&lease.id],
                )
                .map_err(|_| storage())?;
                tx.commit().map_err(|_| storage())?;
                return Err(invalid(
                    "event_capacity_exceeded_unsubscribe_and_resubscribe",
                ));
            }
            remaining-=tx.execute("INSERT OR IGNORE INTO deliveries(subscription,message,event,body) VALUES(?,?,?,?)",params![lease.id,mid,event,body]).map_err(|_|storage())? as i64;
        }
        tx.commit().map_err(|_| storage())?;
        let rows = {
            let mut s=db.prepare("SELECT message,event,body FROM deliveries WHERE subscription=? AND done=0 AND next<=? LIMIT 1").map_err(|_|storage())?;
            let rows = s
                .query_map(params![lease.id, now()], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(|_| storage())?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|_| storage())?
        };
        for (mid, event, body) in rows {
            let signed = headers(
                &lease.id,
                &event,
                body.as_bytes(),
                &key,
                previous.as_deref().filter(|_| rotation > now()),
            )?;
            let claim = uuid::Uuid::new_v4().to_string();
            let at = now();
            let attempts: i64 = {
                let mut statement = db.prepare(
                    "UPDATE deliveries SET next=?1, claim=?2
                     WHERE subscription=?3 AND message=?4 AND done=0 AND next<=?5
                     AND EXISTS (SELECT 1 FROM subscriptions WHERE id=?3 AND revision=?6 AND token IS ?7 AND expires>?5 AND blocked='')
                     RETURNING attempts",
                ).map_err(|_| storage())?;
                let mut rows = statement.query(params![at + 30_000, claim, lease.id, mid, at, revision, lease.principal.token])
                    .map_err(|_| storage())?;
                let Some(row) = rows.next().map_err(|_| storage())? else { return Ok(()); };
                let attempts = row.get(0).map_err(|_| storage())?;
                // SQLite can still fail to commit after returning the updated row.
                if rows.next().map_err(|_| storage())?.is_some() {
                    return Err(storage());
                }
                attempts
            };
            drop(db);
            let status = self
                .transport
                .post(&url, &signed, body.as_bytes())
                .map(|r| r.0)
                .unwrap_or(0);
            db = self.db.lock().map_err(|_| storage())?;
            if (200..300).contains(&status) {
                self.flush_acknowledgments(&db, Some(Acknowledgment {
                    subscription: lease.id.clone(),
                    message: mid,
                    claim,
                }))?;
                continue;
            }
            let at = now();
            if status == 410 {
                db.execute(
                    "DELETE FROM subscriptions WHERE id=?1 AND revision=?2 AND token IS ?3 AND expires>?4 AND blocked=''
                     AND EXISTS (SELECT 1 FROM deliveries WHERE subscription=?1 AND message=?5 AND claim=?6 AND done=0)",
                    params![lease.id, revision, lease.principal.token, at, mid, claim],
                ).map_err(|_| storage())?;
                break;
            }
            let done = status == 413
                || (400..500).contains(&status) && status != 408 && status != 429
                || attempts >= 7;
            db.execute(
                "UPDATE deliveries SET done=?1, attempts=attempts+1, next=?2
                 WHERE subscription=?3 AND message=?4 AND claim=?5 AND done=0
                 AND EXISTS (SELECT 1 FROM subscriptions WHERE id=?3 AND revision=?6 AND token IS ?7 AND expires>?8 AND blocked='')",
                params![done, at + 1000 * (1i64 << attempts.min(8)), lease.id, mid, claim, revision, lease.principal.token, at],
            ).map_err(|_| storage())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::{TcpListener, TcpStream},
        sync::atomic::{AtomicU16, Ordering},
    };

    type ReceivedRequest = (Value, Vec<(String, String)>);

    struct Fixture {
        port: u16,
        requests: Arc<Mutex<Vec<ReceivedRequest>>>,
        status: Arc<AtomicU16>,
    }
    impl Fixture {
        fn new() -> Self {
            let server = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = server.local_addr().unwrap().port();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let received = requests.clone();
            let status = Arc::new(AtomicU16::new(204));
            let reply_status = status.clone();
            std::thread::spawn(move || {
                for stream in server.incoming() {
                    let mut stream = stream.unwrap();
                    let mut input = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    input.read_line(&mut first).unwrap();
                    assert!(first.starts_with("POST /callback "));
                    let mut hs = Vec::new();
                    let mut size = 0;
                    loop {
                        let mut line = String::new();
                        input.read_line(&mut line).unwrap();
                        if line == "\r\n" {
                            break;
                        }
                        let (k, v) = line.trim().split_once(':').unwrap();
                        if k.eq_ignore_ascii_case("content-length") {
                            size = v.trim().parse().unwrap();
                        }
                        hs.push((k.to_ascii_lowercase(), v.trim().to_owned()));
                    }
                    let mut raw = vec![0; size];
                    input.read_exact(&mut raw).unwrap();
                    let body: Value = serde_json::from_slice(&raw).unwrap();
                    let get = |k: &str| hs.iter().find(|(n, _)| n == k).unwrap().1.clone();
                    let sig = get("webhook-signature");
                    assert!([7, 9].iter().any(|key| {
                        let mut mac = Hmac::<Sha256>::new_from_slice(&[*key; 32]).unwrap();
                        mac.update(
                            format!("{}.{}.", get("webhook-id"), get("webhook-timestamp"))
                                .as_bytes(),
                        );
                        mac.update(&raw);
                        sig.split_whitespace().any(|s| {
                            s == format!(
                                "v1,{}",
                                STANDARD.encode(mac.clone().finalize().into_bytes())
                            )
                        })
                    }));
                    let (code, response) = if body["type"] == "verification" {
                        (
                            if reply_status.load(Ordering::SeqCst) == 400 {
                                400
                            } else {
                                200
                            },
                            json!({"challenge":body["challenge"]}).to_string(),
                        )
                    } else {
                        (reply_status.load(Ordering::SeqCst), String::new())
                    };
                    received.lock().unwrap().push((body, hs));
                    write!(stream,"HTTP/1.1 {code} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
                }
            });
            Self {
                port,
                requests,
                status,
            }
        }
    }
    impl Webhook for Fixture {
        fn post(
            &self,
            _: &str,
            hs: &[(String, String)],
            body: &[u8],
        ) -> Result<(u16, Vec<u8>), &'static str> {
            let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            write!(
                s,
                "POST /callback HTTP/1.1\r\nHost: callback.example\r\nContent-Length: {}\r\n",
                body.len()
            )
            .unwrap();
            for (k, v) in hs {
                write!(s, "{k}: {v}\r\n").unwrap();
            }
            s.write_all(b"\r\n").unwrap();
            s.write_all(body).unwrap();
            let mut response = Vec::new();
            s.read_to_end(&mut response).unwrap();
            let split = response.windows(4).position(|s| s == b"\r\n\r\n").unwrap();
            let code = String::from_utf8_lossy(&response[..split])
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse()
                .unwrap();
            Ok((code, response[split + 4..].to_vec()))
        }
    }
    fn principal() -> Principal {
        Principal {
            owner: "owner-a".into(),
            agent: "dot-a".into(),
            token: None,
        }
    }
    fn args() -> Value {
        json!({"name":EVENT,"arguments":{"session_id":"dot-a"},"delivery":{"mode":"webhook","url":"https://callback.example/callback","secret":format!("whsec_{}",STANDARD.encode([7;32]))}})
    }
    fn store(home: &Path, fixture: Arc<Fixture>) -> Events {
        Events::with_db(
            Connection::open(home.join("events.sqlite")).unwrap(),
            home.into(),
            fixture,
        )
        .unwrap()
    }
    fn message(home: &Path, id: &str, agent: &str, ts: i64) {
        let path = home.join(".everett/inbox");
        fs::create_dir_all(&path).unwrap();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path.join(format!("{agent}.jsonl")))
            .unwrap();
        writeln!(
            f,
            "{}",
            json!({"id":id,"to":agent,"ts":ts as f64/1000.,"text":"untrusted test input"})
        )
        .unwrap();
    }

    #[test]
    fn callback_verification_delivery_retry_restart_and_unsubscribe() {
        let home = tempfile::tempdir().unwrap();
        let fixture = Arc::new(Fixture::new());
        let events = store(home.path(), fixture.clone());
        message(home.path(), "old", "dot-a", now() - 10000);
        let sub = events.subscribe(&principal(), &args()).unwrap();
        assert_eq!(
            events.subscribe(&principal(), &args()).unwrap()["id"],
            sub["id"]
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        message(home.path(), "new", "dot-a", now());
        message(home.path(), "foreign", "dot-b", now());
        fixture.status.store(503, Ordering::SeqCst);
        let lease = events.leases().unwrap().remove(0);
        events.deliver(&lease).unwrap();
        let first = fixture.requests.lock().unwrap()[1].0.clone();
        assert_eq!(
            first["data"],
            json!({"session_id":"dot-a","message_id":"new"})
        );
        assert!(first["data"].get("text").is_none());
        drop(events);
        let events = store(home.path(), fixture.clone());
        events
            .db
            .lock()
            .unwrap()
            .execute("UPDATE deliveries SET next=0", [])
            .unwrap();
        fixture.status.store(204, Ordering::SeqCst);
        events.deliver(&events.leases().unwrap()[0]).unwrap();
        assert_eq!(fixture.requests.lock().unwrap()[2].0, first);
        events.deliver(&lease).unwrap();
        assert_eq!(fixture.requests.lock().unwrap().len(), 3);
        assert!(!home.path().join(".everett/inbox/dot-a.done").exists());
        assert_eq!(
            events.unsubscribe(&principal(), &args()).unwrap(),
            json!({})
        );
        message(home.path(), "later", "dot-a", now());
        events.deliver(&lease).unwrap();
        assert_eq!(fixture.requests.lock().unwrap().len(), 3);
    }
    #[test]
    fn owner_filter_expiry_rotation_revocation_and_terminal_delivery() {
        let home = tempfile::tempdir().unwrap();
        let fixture = Arc::new(Fixture::new());
        let events = store(home.path(), fixture.clone());
        let mut wrong = args();
        wrong["arguments"]["session_id"] = json!("foreign");
        assert_eq!(
            events.subscribe(&principal(), &wrong).unwrap_err().code,
            -32602
        );
        events.subscribe(&principal(), &args()).unwrap();
        let lease = events.leases().unwrap().remove(0);
        let mut other = principal();
        other.owner = "owner-b".into();
        events.unsubscribe(&other, &args()).unwrap();
        assert_eq!(events.leases().unwrap().len(), 1);
        let mut rotated = args();
        rotated["delivery"]["secret"] = json!(format!("whsec_{}", STANDARD.encode([9; 32])));
        events.subscribe(&principal(), &rotated).unwrap();
        message(home.path(), "rotated", "dot-a", now());
        events.deliver(&lease).unwrap();
        let received = fixture.requests.lock().unwrap();
        let signatures = &received
            .last()
            .unwrap()
            .1
            .iter()
            .find(|(k, _)| k == "webhook-signature")
            .unwrap()
            .1;
        assert_eq!(signatures.split_whitespace().count(), 2);
        drop(received);
        let ttl_ms = 60_000;
        rotated["ttlMs"] = json!(ttl_ms);
        let before = now();
        events.subscribe(&principal(), &rotated).unwrap();
        let after = now();
        {
            let db = events.db.lock().unwrap();
            let expires: i64 = db
                .query_row(
                    "SELECT expires FROM subscriptions WHERE id=?",
                    [&lease.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert!((before + ttl_ms..=after + ttl_ms).contains(&expires));
            assert_eq!(
                db.execute(
                    "UPDATE subscriptions SET expires=? WHERE id=?",
                    params![before, lease.id]
                )
                .unwrap(),
                1
            );
        }
        assert!(events.leases().unwrap().is_empty());
        events.subscribe(&principal(), &args()).unwrap();
        message(home.path(), "gone", "dot-a", now());
        fixture.status.store(410, Ordering::SeqCst);
        events.deliver(&lease).unwrap();
        assert!(events.leases().unwrap().is_empty());
        events.subscribe(&principal(), &args()).unwrap();
        events.revoke(&lease);
        assert!(events.leases().unwrap().is_empty());
    }
    #[test]
    fn failed_challenge_and_stale_credential_cannot_activate_or_revoke_new_lease() {
        let home = tempfile::tempdir().unwrap();
        let fixture = Arc::new(Fixture::new());
        let events = store(home.path(), fixture.clone());
        fixture.status.store(400, Ordering::SeqCst);
        let error = events.subscribe(&principal(), &args()).unwrap_err();
        assert_eq!(error.code, -32015);
        assert_eq!(error.reason, "challenge_failed");
        assert!(events.leases().unwrap().is_empty());
        fixture.status.store(204, Ordering::SeqCst);
        let mut p = principal();
        p.token = Some("old-token".into());
        events.subscribe(&p, &args()).unwrap();
        let stale = events.leases().unwrap().remove(0);
        p.token = Some("new-token".into());
        events.subscribe(&p, &args()).unwrap();
        events.revoke(&stale);
        assert_eq!(
            events.leases().unwrap()[0].principal.token.as_deref(),
            Some("new-token")
        );
        message(home.path(), "fresh", "dot-a", now());
        events.deliver(&stale).unwrap();
        assert_eq!(fixture.requests.lock().unwrap().len(), 2);
        events.deliver(&events.leases().unwrap()[0]).unwrap();
        assert_eq!(
            fixture.requests.lock().unwrap()[2].0["data"]["message_id"],
            "fresh"
        );
    }
    #[test]
    fn verification_cache_has_fixed_age_and_capacity_deactivates_explicitly() {
        let home = tempfile::tempdir().unwrap();
        let fixture = Arc::new(Fixture::new());
        let events = store(home.path(), fixture.clone());
        events.subscribe(&principal(), &args()).unwrap();
        let lease = events.leases().unwrap().remove(0);
        let verified = now() - 200_000;
        events
            .db
            .lock()
            .unwrap()
            .execute("UPDATE subscriptions SET verified=?", [verified])
            .unwrap();
        events.subscribe(&principal(), &args()).unwrap();
        let actual: i64 = events
            .db
            .lock()
            .unwrap()
            .query_row("SELECT verified FROM subscriptions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(actual, verified);
        events
            .db
            .lock()
            .unwrap()
            .execute("UPDATE subscriptions SET verified=?", [now() - 301_000])
            .unwrap();
        events.subscribe(&principal(), &args()).unwrap();
        assert_eq!(fixture.requests.lock().unwrap().len(), 2);
        {
            let mut db = events.db.lock().unwrap();
            let tx = db.transaction().unwrap();
            for n in 0..10000 {
                tx.execute("INSERT INTO deliveries(subscription,message,event,body,done) VALUES(?,?,?,'{}',1)",params![lease.id,format!("old{n}"),format!("evt{n}")]).unwrap();
            }
            tx.commit().unwrap();
        }
        message(home.path(), "overflow", "dot-a", now());
        assert_eq!(
            events.deliver(&lease).unwrap_err().reason,
            "event_capacity_exceeded_unsubscribe_and_resubscribe"
        );
        assert!(events.leases().unwrap().is_empty());
        assert!(events.subscribe(&principal(), &args()).is_err());
        events.unsubscribe(&principal(), &args()).unwrap();
        assert!(events.subscribe(&principal(), &args()).is_ok());
        assert_eq!(events.leases().unwrap().len(), 1);
    }
    #[test]
    fn production_resolver_refuses_private_dns_and_literal_addresses() {
        let config = ureq::Agent::config_builder().proxy(None).build();
        for uri in ["https://localhost/", "https://127.0.0.1/", "https://[::1]/"] {
            let result = PublicResolver.resolve(
                &uri.parse().unwrap(),
                &config,
                NextTimeout {
                    after: ureq::unversioned::transport::time::Duration::from_secs(2),
                    reason: ureq::Timeout::Global,
                },
            );
            assert!(result.is_err(), "{uri}");
            assert!(HttpsWebhook.post(uri, &[], b"{}").is_err(), "{uri}");
        }
    }
    #[test]
    fn callbacks_reject_non_public_networks_and_invalid_secrets() {
        for raw in [
            "http://example.com",
            "https://127.0.0.1/",
            "https://169.254.169.254/",
            "https://[::1]/",
            "https://[::ffff:127.0.0.1]/",
            "https://user@example.com",
            "https://example.com:8443",
            "https://example.com/#fragment",
        ] {
            assert!(validate_url(raw).is_err(), "{raw}");
        }
        for ip in [
            "0.1.2.3",
            "100.64.1.1",
            "192.0.2.1",
            "198.19.0.1",
            "203.0.113.2",
            "224.0.0.1",
            "2001:db8::1",
            "2002:0808:0808::1",
            "64:ff9b::7f00:1",
            "fc00::1",
            "fe80::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(secret("whsec_AAAA").is_err());
        assert!(secret(&format!("whsec_{}", STANDARD.encode([7; 32]))).is_ok());
    }
    #[test]
    fn forged_and_replayed_challenges_never_activate_delivery() {
        struct Replay(Mutex<Option<Vec<u8>>>);
        impl Webhook for Replay {
            fn post(
                &self,
                _: &str,
                _: &[(String, String)],
                body: &[u8],
            ) -> Result<(u16, Vec<u8>), &'static str> {
                let mut prior = self.0.lock().unwrap();
                let response = prior
                    .clone()
                    .unwrap_or_else(|| b"{\"challenge\":\"forged\"}".to_vec());
                let request: Value = serde_json::from_slice(body).unwrap();
                *prior =
                    Some(serde_json::to_vec(&json!({"challenge":request["challenge"]})).unwrap());
                Ok((200, response))
            }
        }
        let home = tempfile::tempdir().unwrap();
        let events = Events::with_db(
            Connection::open_in_memory().unwrap(),
            home.path().into(),
            Arc::new(Replay(Mutex::new(None))),
        )
        .unwrap();
        for _ in 0..3 {
            assert_eq!(
                events.subscribe(&principal(), &args()).unwrap_err().reason,
                "challenge_failed"
            );
            assert!(events.leases().unwrap().is_empty());
        }
    }

    #[test]
    fn retries_are_bounded_and_payload_too_large_is_terminal() {
        for status in [503, 413] {
            let home = tempfile::tempdir().unwrap();
            let fixture = Arc::new(Fixture::new());
            let events = store(home.path(), fixture.clone());
            events.subscribe(&principal(), &args()).unwrap();
            message(home.path(), "bounded", "dot-a", now());
            fixture.status.store(status, Ordering::SeqCst);
            let lease = events.leases().unwrap().remove(0);
            for _ in 0..12 {
                events
                    .db
                    .lock()
                    .unwrap()
                    .execute("UPDATE deliveries SET next=0", [])
                    .unwrap();
                events.deliver(&lease).unwrap();
            }
            assert_eq!(
                fixture.requests.lock().unwrap().len(),
                if status == 503 { 9 } else { 2 }
            );
        }
    }
    #[test]
    fn receipt_retry_signature_and_burst_contract() {
        let home = tempfile::tempdir().unwrap();
        let fixture = Arc::new(Fixture::new());
        let events = store(home.path(), fixture.clone());
        events.subscribe(&principal(), &args()).unwrap();
        let lease = events.leases().unwrap().remove(0);
        message(home.path(), "one", "dot-a", now());
        message(home.path(), "one", "dot-a", now());
        message(home.path(), "two", "dot-a", now());
        message(home.path(), "foreign", "dot-b", now());
        fixture.status.store(503, Ordering::SeqCst);
        events.deliver(&lease).unwrap();
        let first = fixture.requests.lock().unwrap()[1].clone();
        std::thread::sleep(Duration::from_millis(1100));
        events
            .db
            .lock()
            .unwrap()
            .execute("UPDATE deliveries SET next=0", [])
            .unwrap();
        fixture.status.store(200, Ordering::SeqCst);
        events.deliver(&lease).unwrap();
        let second = fixture.requests.lock().unwrap()[2].clone();
        assert_eq!(
            first.0, second.0,
            "retry preserves event ID and exact payload"
        );
        for name in ["webhook-timestamp", "webhook-signature"] {
            let value =
                |r: &ReceivedRequest| r.1.iter().find(|(n, _)| n == name).unwrap().1.clone();
            assert_ne!(value(&first), value(&second), "fresh signing metadata");
        }
        events.deliver(&lease).unwrap();
        events.deliver(&lease).unwrap();
        let requests = fixture.requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            4,
            "burst sends separate requests and suppresses duplicates"
        );
        assert_eq!(requests[3].0["data"]["message_id"], "two");
        drop(requests);
        assert!(
            !home.path().join(".everett/inbox/dot-a.done").exists(),
            "2xx only acknowledges webhook receipt, never inbox processing"
        );
        events.unsubscribe(&principal(), &args()).unwrap();
        events.deliver(&lease).unwrap();
        assert_eq!(fixture.requests.lock().unwrap().len(), 4);
    }

}

#[cfg(test)]
#[path = "gateway_events_regressions.rs"]
mod regressions;

#[cfg(test)]
#[path = "gateway_events_concurrency.rs"]
mod concurrency_tests;
