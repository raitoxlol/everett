//! Owner-configured HTTPS wake for a registered external inbox.
//!
//! The policy file lives next to `receiver-wake.json` in the selected relay
//! directory. A match posts a small JSON nudge to the owner's webhook URL.
//! The inbox text is not included. HTTP 200 means the endpoint accepted a run.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::error::{EverettError, Result};

const MAX_ATTEMPTS: u32 = 5;
const DIRECTORY: &str = "receiver-webhook";
const POLICY_FILE: &str = "receiver-webhook.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arrival {
    pub owner: String,
    pub sender_owner: String,
    pub recipient_owner: String,
    pub recipient_device: String,
    pub sender_device: String,
    pub session: String,
    pub harness: String,
    pub sender_session: String,
    pub sender_harness: String,
    pub message_id: String,
    pub created_at_ms: i64,
    pub is_reply: bool,
    pub external: bool,
}

#[derive(Debug)]
pub struct Outcome {
    pub value: Option<Value>,
    pub logs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    version: u8,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    activated_at: i64,
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    id: String,
    owner: String,
    recipient_device: String,
    session: String,
    harness: String,
    sender_device: String,
    sender_session: String,
    sender_harness: String,
    cooldown_seconds: u64,
    url: String,
    bearer_file: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    rule_id: String,
    session: String,
    harness: String,
    state: String,
    attempts: u32,
    at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    version: u8,
    #[serde(default)]
    rules: BTreeMap<String, i64>,
    #[serde(default)]
    messages: BTreeMap<String, Saved>,
}

struct Request {
    url: String,
    bearer: String,
    body: Value,
}

struct Attempt<'a> {
    rule: &'a Rule,
    session: &'a str,
    harness: &'a str,
    message_id: &'a str,
}

enum Response {
    Status(u16),
    Transport,
}

#[derive(Clone)]
enum Report {
    Idle,
    NoMatch,
    Accepted {
        rule_id: String,
        status: u16,
    },
    AlreadyPosted {
        rule_id: String,
    },
    Rejected {
        rule_id: String,
        status: Option<u16>,
        reason: String,
    },
    Deferred {
        rule_id: String,
        reason: String,
    },
    Skipped {
        reason: String,
    },
}

fn bad(message: impl Into<String>) -> EverettError {
    EverettError::new(2, format!("receiver webhook: {}", message.into()))
}

fn policy_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = ROOT_OVERRIDE.with(|slot| slot.borrow().clone()) {
        return path;
    }
    crate::relay::state_dir()
}

pub fn on_imported(arrival: &Arrival) -> Result<Outcome> {
    evaluate(
        &policy_dir(),
        Some(arrival),
        unix_secs(),
        &mut default_poster,
    )
}

pub fn poll() -> Result<Outcome> {
    evaluate(&policy_dir(), None, unix_secs(), &mut default_poster)
}

pub fn after_local_queue(
    session_id: &str,
    harness: &str,
    message: &Map<String, Value>,
) -> Result<Option<Value>> {
    let root = policy_dir();
    let policy = load_policy(&root)?;
    if !policy.enabled {
        return Ok(None);
    }
    let Some(identity) = local_identity()? else {
        return Ok(Some(json!({
            "state": "skipped",
            "reason": "sign in on this computer and copy owner and agent_id from `everett whoami --json`"
        })));
    };
    let arrival = local_arrival(session_id, harness, message, &identity)?;
    Ok(evaluate(&root, Some(&arrival), unix_secs(), &mut default_poster)?.value)
}

fn local_arrival(
    session_id: &str,
    harness: &str,
    message: &Map<String, Value>,
    identity: &Identity,
) -> Result<Arrival> {
    let message_id = message.get("id").and_then(Value::as_str).unwrap_or("");
    if !message_ok(message_id) {
        return Err(bad("inbox message id is invalid"));
    }
    let created_at_ms = message
        .get("ts")
        .and_then(Value::as_f64)
        .filter(|stamp| stamp.is_finite() && *stamp >= 0.0)
        .map(|stamp| (stamp * 1000.0) as i64)
        .unwrap_or_else(|| unix_secs().saturating_mul(1000));
    Ok(Arrival {
        owner: identity.owner.clone(),
        sender_owner: identity.owner.clone(),
        recipient_owner: identity.owner.clone(),
        recipient_device: identity.device_id.clone(),
        sender_device: identity.device_id.clone(),
        session: session_id.to_string(),
        harness: harness.to_string(),
        sender_session: message
            .get("from")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        sender_harness: message
            .get("from_harness")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        message_id: message_id.to_string(),
        created_at_ms,
        is_reply: message
            .get("reply_to")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
            || message.get("kind").and_then(Value::as_str) == Some("reply"),
        external: crate::inbox::external(message),
    })
}

#[derive(Clone)]
struct Identity {
    owner: String,
    device_id: String,
}

fn local_identity() -> Result<Option<Identity>> {
    #[cfg(test)]
    if let Some(identity) = IDENTITY_OVERRIDE.with(|slot| slot.borrow().clone()) {
        return Ok(Some(identity));
    }
    match crate::config::get("auth.provider", Some("EVERETT_AUTH_PROVIDER"), "oidc").as_str() {
        "cloudflare" => match crate::cloudflare_auth::load() {
            Ok(Some(stored)) => Ok(checked_identity(stored.owner, stored.agent_id)),
            Ok(None) => Ok(None),
            Err(error) => Err(bad(error.message)),
        },
        "oidc" => Ok(oidc_identity()),
        _ => Ok(None),
    }
}

fn checked_identity(owner: String, device_id: String) -> Option<Identity> {
    if owner_ok(&owner) && ident_ok(&device_id) {
        Some(Identity { owner, device_id })
    } else {
        None
    }
}

fn oidc_identity() -> Option<Identity> {
    let stored = crate::auth::load()?;
    let claim = crate::relay::claims(&stored.id_token).ok()?;
    if claim.get("iss").and_then(Value::as_str) != Some(stored.issuer.as_str()) {
        return None;
    }
    let owner = if let Some(org) = claim.get("everett_org").and_then(Value::as_str) {
        if org.is_empty() {
            return None;
        }
        serde_json::json!([stored.issuer, "org", org]).to_string()
    } else if !stored.sub.is_empty() {
        serde_json::json!([stored.issuer, "account", stored.sub]).to_string()
    } else {
        return None;
    };
    let agent = claim
        .get("everett_agent")
        .and_then(Value::as_str)
        .unwrap_or("");
    checked_identity(owner, agent.to_string())
}

fn unix_secs() -> i64 {
    crate::session::now() as i64
}

fn evaluate(
    root: &Path,
    arrival: Option<&Arrival>,
    now: i64,
    post: &mut dyn FnMut(&Request) -> Response,
) -> Result<Outcome> {
    let policy = load_policy(root)?;
    if !policy.enabled {
        return Ok(Outcome {
            value: None,
            logs: Vec::new(),
        });
    }
    let directory = root.join(DIRECTORY);
    let ledger_path = directory.join("ledger.json");
    if !ledger_path.exists() {
        if arrival.is_none() {
            return Ok(Outcome {
                value: None,
                logs: Vec::new(),
            });
        }
        if let Some(report) = preview(&policy, arrival) {
            return Ok(report.outcome());
        }
    }
    crate::peer::private_dir(&directory)?;
    let _guard = crate::peer::lock(&directory.join("ledger.lock"))?;
    let policy = load_policy(root)?;
    if !policy.enabled {
        return Ok(Outcome {
            value: None,
            logs: Vec::new(),
        });
    }
    let mut ledger = load_ledger(&ledger_path)?;
    let mut logs = Vec::new();
    let mut dirty = false;
    let current_id = arrival.map(|item| item.message_id.as_str());
    if pump(&policy, current_id, now, &mut ledger, post, &mut logs)? {
        dirty = true;
    }
    let current = if let Some(arrival) = arrival {
        let (report, changed) = handle(&policy, arrival, now, &mut ledger, post, &mut logs)?;
        dirty |= changed;
        report
    } else {
        Report::Idle
    };
    if dirty {
        prune(&mut ledger);
        save_ledger(&ledger_path, &ledger)?;
    }
    let value = if arrival.is_some() {
        current.json()
    } else {
        None
    };
    Ok(Outcome { value, logs })
}

fn preview(policy: &Policy, arrival: Option<&Arrival>) -> Option<Report> {
    let arrival = arrival?;
    if matching(policy, arrival).is_none() {
        return Some(Report::NoMatch);
    }
    gate(policy, arrival).map(|reason| Report::Skipped { reason })
}

fn handle(
    policy: &Policy,
    arrival: &Arrival,
    now: i64,
    ledger: &mut Ledger,
    post: &mut dyn FnMut(&Request) -> Response,
    logs: &mut Vec<String>,
) -> Result<(Report, bool)> {
    let Some(rule) = matching(policy, arrival) else {
        return Ok((Report::NoMatch, false));
    };
    if let Some(reason) = gate(policy, arrival) {
        return Ok((Report::Skipped { reason }, false));
    }
    if !message_ok(&arrival.message_id) {
        return Err(bad("inbox message id is invalid"));
    }
    if let Some(saved) = ledger.messages.get(&arrival.message_id) {
        if saved.state == "posted" {
            return Ok((Report::AlreadyPosted { rule_id: rule.id }, false));
        }
        if saved.state == "rejected" {
            return Ok((
                Report::Rejected {
                    rule_id: rule.id,
                    status: None,
                    reason: "webhook was not accepted".into(),
                },
                false,
            ));
        }
    }
    if cooling(&rule, ledger, now) {
        remember_pending(
            ledger,
            &rule,
            &arrival.session,
            &arrival.harness,
            &arrival.message_id,
            now,
        );
        return Ok((
            Report::Deferred {
                rule_id: rule.id,
                reason: "cooldown".into(),
            },
            true,
        ));
    }
    let attempts = ledger
        .messages
        .get(&arrival.message_id)
        .map(|saved| saved.attempts)
        .unwrap_or(0);
    let report = attempt(
        &Attempt {
            rule: &rule,
            session: &arrival.session,
            harness: &arrival.harness,
            message_id: &arrival.message_id,
        },
        attempts,
        now,
        ledger,
        post,
        logs,
    )?;
    Ok((report, true))
}

fn pump(
    policy: &Policy,
    skip: Option<&str>,
    now: i64,
    ledger: &mut Ledger,
    post: &mut dyn FnMut(&Request) -> Response,
    logs: &mut Vec<String>,
) -> Result<bool> {
    let due = ledger.messages.iter().find_map(|(id, saved)| {
        if Some(id.as_str()) == skip || saved.state != "pending" || saved.attempts >= MAX_ATTEMPTS {
            return None;
        }
        let rule = policy.rules.iter().find(|rule| {
            rule.id == saved.rule_id
                && rule.session == saved.session
                && rule.harness == saved.harness
        })?;
        if cooling(rule, ledger, now) {
            return None;
        }
        Some((
            id.clone(),
            rule.id.clone(),
            saved.session.clone(),
            saved.harness.clone(),
            saved.attempts,
        ))
    });
    let Some((id, rule_id, session, harness, attempts)) = due else {
        return Ok(false);
    };
    let Some(rule) = policy.rules.iter().find(|rule| rule.id == rule_id).cloned() else {
        ledger.messages.remove(&id);
        return Ok(true);
    };
    attempt(
        &Attempt {
            rule: &rule,
            session: &session,
            harness: &harness,
            message_id: &id,
        },
        attempts,
        now,
        ledger,
        post,
        logs,
    )?;
    Ok(true)
}

fn remember_pending(
    ledger: &mut Ledger,
    rule: &Rule,
    session: &str,
    harness: &str,
    message_id: &str,
    now: i64,
) {
    let attempts = ledger
        .messages
        .get(message_id)
        .map(|saved| saved.attempts)
        .unwrap_or(0);
    ledger.messages.insert(
        message_id.to_string(),
        Saved {
            rule_id: rule.id.clone(),
            session: session.to_string(),
            harness: harness.to_string(),
            state: "pending".into(),
            attempts,
            at: now,
        },
    );
}

fn attempt(
    target: &Attempt<'_>,
    attempts: u32,
    now: i64,
    ledger: &mut Ledger,
    post: &mut dyn FnMut(&Request) -> Response,
    logs: &mut Vec<String>,
) -> Result<Report> {
    let Attempt {
        rule,
        session,
        harness,
        message_id,
    } = *target;
    let bearer = read_bearer(Path::new(&rule.bearer_file))?;
    let request = Request {
        url: rule.url.clone(),
        bearer,
        body: json!({
            "source": "everett",
            "event": "inbox",
            "session": session,
            "harness": harness,
            "message_id": message_id,
            "rule_id": rule.id,
        }),
    };
    let response = post(&request);
    ledger.rules.insert(rule.id.clone(), now);
    let attempts = attempts.saturating_add(1);
    let (state, report) = match response {
        Response::Status(200) => (
            "posted",
            Report::Accepted {
                rule_id: rule.id.clone(),
                status: 200,
            },
        ),
        Response::Status(status) if retryable(status) && attempts < MAX_ATTEMPTS => (
            "pending",
            Report::Deferred {
                rule_id: rule.id.clone(),
                reason: format!("HTTP {status}"),
            },
        ),
        Response::Status(status) => (
            "rejected",
            Report::Rejected {
                rule_id: rule.id.clone(),
                status: Some(status),
                reason: "webhook did not accept the wake".into(),
            },
        ),
        Response::Transport if attempts < MAX_ATTEMPTS => (
            "pending",
            Report::Deferred {
                rule_id: rule.id.clone(),
                reason: "webhook endpoint unreachable".into(),
            },
        ),
        Response::Transport => (
            "rejected",
            Report::Rejected {
                rule_id: rule.id.clone(),
                status: None,
                reason: "webhook endpoint unreachable".into(),
            },
        ),
    };
    ledger.messages.insert(
        message_id.to_string(),
        Saved {
            rule_id: rule.id.clone(),
            session: session.to_string(),
            harness: harness.to_string(),
            state: state.into(),
            attempts,
            at: now,
        },
    );
    if let Some(line) = report.log_line(session) {
        logs.push(line);
    }
    Ok(report)
}

fn cooling(rule: &Rule, ledger: &Ledger, now: i64) -> bool {
    ledger.rules.get(&rule.id).is_some_and(|last| {
        now < last.saturating_add(i64::try_from(rule.cooldown_seconds).unwrap_or(i64::MAX))
    })
}

fn matching(policy: &Policy, arrival: &Arrival) -> Option<Rule> {
    policy
        .rules
        .iter()
        .find(|rule| {
            rule.owner == arrival.owner
                && arrival.sender_owner == arrival.owner
                && arrival.recipient_owner == arrival.owner
                && rule.recipient_device == arrival.recipient_device
                && rule.sender_device == arrival.sender_device
                && rule.session == arrival.session
                && rule.harness == arrival.harness
                && rule.sender_session == arrival.sender_session
                && rule.sender_harness == arrival.sender_harness
        })
        .cloned()
}

fn gate(policy: &Policy, arrival: &Arrival) -> Option<String> {
    if arrival.is_reply {
        return Some("replies do not post the webhook".into());
    }
    if arrival.external {
        return Some("external-agent messages do not post the webhook".into());
    }
    if arrival.created_at_ms < policy.activated_at.saturating_mul(1000) {
        return Some("message is older than activated_at".into());
    }
    None
}

impl Report {
    fn outcome(self) -> Outcome {
        Outcome {
            value: self.json(),
            logs: Vec::new(),
        }
    }

    fn json(&self) -> Option<Value> {
        Some(match self {
            Self::Idle => return None,
            Self::NoMatch => json!({"state": "no_match"}),
            Self::Accepted { rule_id, status } => {
                json!({"state": "accepted", "rule_id": rule_id, "http_status": status})
            }
            Self::AlreadyPosted { rule_id } => {
                json!({"state": "already_posted", "rule_id": rule_id})
            }
            Self::Rejected {
                rule_id,
                status,
                reason,
            } => {
                let mut value = json!({"state": "rejected", "rule_id": rule_id, "reason": reason});
                if let (Some(status), Some(object)) = (status, value.as_object_mut()) {
                    object.insert("http_status".into(), json!(status));
                }
                value
            }
            Self::Deferred { rule_id, reason } => {
                json!({"state": "deferred", "rule_id": rule_id, "reason": reason})
            }
            Self::Skipped { reason } => json!({"state": "skipped", "reason": reason}),
        })
    }

    fn log_line(&self, session: &str) -> Option<String> {
        match self {
            Self::Accepted { rule_id, status } => Some(format!(
                "receiver webhook accepted HTTP {status} for session {session} rule {rule_id}"
            )),
            Self::Rejected {
                rule_id,
                status,
                reason,
            } => Some(match status {
                Some(status) => format!(
                    "receiver webhook rejected HTTP {status} for session {session} rule {rule_id}"
                ),
                None => format!(
                    "receiver webhook rejected for session {session} rule {rule_id}: {reason}"
                ),
            }),
            Self::Deferred { rule_id, reason } => Some(format!(
                "receiver webhook deferred for session {session} rule {rule_id}: {reason}"
            )),
            _ => None,
        }
    }
}

fn retryable(status: u16) -> bool {
    status == 408 || status == 429 || (500..600).contains(&status)
}

fn prune(ledger: &mut Ledger) {
    if ledger.messages.len() <= 256 {
        return;
    }
    let mut finished: Vec<_> = ledger
        .messages
        .iter()
        .filter(|(_, saved)| saved.state != "pending")
        .map(|(id, saved)| (saved.at, id.clone()))
        .collect();
    finished.sort_unstable();
    for (_, id) in finished {
        if ledger.messages.len() <= 256 {
            break;
        }
        ledger.messages.remove(&id);
    }
}

fn load_policy(root: &Path) -> Result<Policy> {
    let path = root.join(POLICY_FILE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Policy {
                version: 1,
                enabled: false,
                activated_at: 0,
                rules: Vec::new(),
            });
        }
        Err(_) => return Err(bad("policy could not be read")),
    };
    if !metadata.file_type().is_file() || metadata.mode() & 0o077 != 0 {
        return Err(bad(
            "policy must be a regular file with mode 0600 or stricter",
        ));
    }
    let policy: Policy =
        serde_json::from_slice(&fs::read(&path).map_err(|_| bad("policy could not be read"))?)
            .map_err(|_| bad("policy JSON is invalid"))?;
    if policy.version != 1 {
        return Err(bad("unsupported policy version"));
    }
    if !policy.enabled {
        return Ok(policy);
    }
    if !(1..=4_000_000_000).contains(&policy.activated_at) {
        return Err(bad("activated_at must be a Unix timestamp in seconds"));
    }
    if policy.rules.is_empty() || policy.rules.len() > 16 {
        return Err(bad("enabled policy needs 1 to 16 rules"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for rule in &policy.rules {
        validate_rule(rule)?;
        if !seen.insert(rule.id.clone()) {
            return Err(bad("duplicate rule id"));
        }
    }
    Ok(policy)
}

fn validate_rule(rule: &Rule) -> Result<()> {
    if !rule_id_ok(&rule.id) {
        return Err(bad("rule id is invalid"));
    }
    if !owner_ok(&rule.owner)
        || !ident_ok(&rule.recipient_device)
        || !ident_ok(&rule.sender_device)
        || !crate::adapters::external::valid_id(&rule.session)
        || !matches!(rule.harness.as_str(), "grok-bot" | "openai-dot")
        || !ident_ok(&rule.sender_session)
        || !(rule.sender_harness.is_empty() || ident_ok(&rule.sender_harness))
        || !(1..=86_400).contains(&rule.cooldown_seconds)
    {
        return Err(bad("rule fields are invalid"));
    }
    validate_url(&rule.url)?;
    if !Path::new(&rule.bearer_file).is_absolute() {
        return Err(bad("bearer_file must be an absolute path"));
    }
    Ok(())
}

fn validate_url(raw: &str) -> Result<()> {
    if raw.len() > 2048 {
        return Err(bad("webhook url is invalid"));
    }
    let url = url::Url::parse(raw).map_err(|_| bad("webhook url is invalid"))?;
    let host = url.host_str().unwrap_or("");
    if url.scheme() != "https"
        || !host_ok(host)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port().is_some_and(|port| port != 443)
        || url.path().len() <= 1
    {
        return Err(bad(
            "webhook url must be https on a public hostname, with no userinfo, query, fragment, or non-443 port",
        ));
    }
    Ok(())
}

fn host_ok(host: &str) -> bool {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2
        || labels
            .iter()
            .any(|label| label.is_empty() || label.len() > 63)
    {
        return false;
    }
    labels.iter().all(|label| {
        let bytes = label.as_bytes();
        bytes
            .first()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
            && bytes
                .last()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    })
}

fn read_bearer(path: &Path) -> Result<String> {
    if !path.is_absolute() {
        return Err(bad("bearer_file must be an absolute path"));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| bad("bearer_file must be a regular file with mode 0600 or stricter"))?;
    if !metadata.file_type().is_file()
        || metadata.mode() & 0o077 != 0
        || metadata.len() == 0
        || metadata.len() > 4096
    {
        return Err(bad(
            "bearer_file must be a regular file with mode 0600 or stricter",
        ));
    }
    let text = String::from_utf8(fs::read(path).map_err(|_| bad("bearer_file could not be read"))?)
        .map_err(|_| bad("bearer_file must contain one ASCII token"))?;
    let mut token = text.trim_end_matches(['\n', '\r']).to_string();
    if let Some(rest) = token.strip_prefix("Bearer ") {
        token = rest.to_string();
    }
    if (8..=4096).contains(&token.len()) && token.bytes().all(|byte| byte.is_ascii_graphic()) {
        Ok(token)
    } else {
        Err(bad("bearer_file must contain one ASCII token"))
    }
}

fn load_ledger(path: &Path) -> Result<Ledger> {
    if !path.exists() {
        return Ok(Ledger {
            version: 1,
            rules: BTreeMap::new(),
            messages: BTreeMap::new(),
        });
    }
    let ledger: Ledger =
        serde_json::from_slice(&fs::read(path).map_err(|_| bad("ledger could not be read"))?)
            .map_err(|_| bad("ledger JSON is invalid"))?;
    if ledger.version != 1 {
        return Err(bad("unsupported ledger version"));
    }
    Ok(ledger)
}

fn save_ledger(path: &Path, ledger: &Ledger) -> Result<()> {
    crate::peer::save(
        path,
        &serde_json::to_vec_pretty(ledger).map_err(|_| bad("ledger could not be encoded"))?,
    )
}

fn ident_ok(value: &str) -> bool {
    (1..=160).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

fn message_ok(value: &str) -> bool {
    (1..=128).contains(&value.len()) && ident_ok(value)
}

fn rule_id_ok(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn owner_ok(value: &str) -> bool {
    (1..=512).contains(&value.len()) && value.bytes().all(|byte| (0x20..0x7f).contains(&byte))
}

fn default_poster(request: &Request) -> Response {
    #[cfg(test)]
    if let Some(status) = capture(request) {
        return Response::Status(status);
    }
    live_post(request)
}

fn live_post(request: &Request) -> Response {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into();
    let mut response = match agent
        .post(&request.url)
        .header("Authorization", &format!("Bearer {}", request.bearer))
        .header("User-Agent", "everett-receiver-webhook")
        .send_json(&request.body)
    {
        Ok(response) => response,
        Err(_) => return Response::Transport,
    };
    let status = response.status().as_u16();
    let _ = response.body_mut().with_config().limit(4096).read_to_vec();
    Response::Status(status)
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct Posted {
    pub bearer: String,
    pub body: String,
}

#[cfg(test)]
struct Capture {
    status: u16,
    posts: Vec<Posted>,
}

#[cfg(test)]
thread_local! {
    static ROOT_OVERRIDE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    static IDENTITY_OVERRIDE: std::cell::RefCell<Option<Identity>> = const { std::cell::RefCell::new(None) };
    static CAPTURE: std::cell::RefCell<Option<Capture>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn capture(request: &Request) -> Option<u16> {
    CAPTURE.with(|slot| {
        let mut guard = slot.borrow_mut();
        let capture = guard.as_mut()?;
        capture.posts.push(Posted {
            bearer: request.bearer.clone(),
            body: request.body.to_string(),
        });
        Some(capture.status)
    })
}

#[cfg(test)]
pub(crate) struct CaptureGuard;

#[cfg(test)]
impl Drop for CaptureGuard {
    fn drop(&mut self) {
        CAPTURE.with(|slot| *slot.borrow_mut() = None);
    }
}

#[cfg(test)]
pub(crate) fn testing_begin(status: u16) -> CaptureGuard {
    CAPTURE.with(|slot| {
        *slot.borrow_mut() = Some(Capture {
            status,
            posts: Vec::new(),
        });
    });
    CaptureGuard
}

#[cfg(test)]
pub(crate) fn testing_posts() -> Vec<Posted> {
    CAPTURE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|capture| capture.posts.clone())
            .unwrap_or_default()
    })
}

#[cfg(test)]
struct OverrideGuard;

#[cfg(test)]
impl OverrideGuard {
    fn set(root: PathBuf, identity: Identity) -> Self {
        ROOT_OVERRIDE.with(|slot| *slot.borrow_mut() = Some(root));
        IDENTITY_OVERRIDE.with(|slot| *slot.borrow_mut() = Some(identity));
        Self
    }
}

#[cfg(test)]
impl Drop for OverrideGuard {
    fn drop(&mut self) {
        ROOT_OVERRIDE.with(|slot| *slot.borrow_mut() = None);
        IDENTITY_OVERRIDE.with(|slot| *slot.borrow_mut() = None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const SECRET: &str = "test-sender-key-0123456789";
    const PHRASE: &str = "super-secret-phrase";

    struct Posts(Vec<Response>);

    impl Posts {
        fn call(&mut self, request: &Request) -> Response {
            assert!(
                !request.body.to_string().contains(PHRASE),
                "{}",
                request.body
            );
            assert!(
                !request.body.to_string().contains(SECRET),
                "{}",
                request.body
            );
            assert_eq!(request.bearer, SECRET);
            assert_eq!(
                request.body,
                json!({
                    "source": "everett",
                    "event": "inbox",
                    "session": "ext-grok-acceptance",
                    "harness": "grok-bot",
                    "message_id": request.body["message_id"],
                    "rule_id": "grok-inbox"
                })
            );
            self.0.pop().unwrap_or(Response::Transport)
        }
    }

    fn private(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn fixture(root: &Path) -> Arrival {
        let bearer = root.join("webhook.key");
        private(&bearer, format!("Bearer {SECRET}\n").as_bytes());
        let policy = json!({
            "version": 1,
            "enabled": true,
            "activated_at": 1_700_000_000,
            "rules": [{
                "id": "grok-inbox",
                "owner": "[\"https://everett.raitox.tech\",\"account\",\"github:1\"]",
                "recipient_device": "agent-receiver",
                "session": "ext-grok-acceptance",
                "harness": "grok-bot",
                "sender_device": "agent-receiver",
                "sender_session": "human",
                "sender_harness": "",
                "cooldown_seconds": 30,
                "url": "https://api2.cursor.sh/automations/webhook/routine",
                "bearer_file": bearer
            }]
        });
        private(
            &root.join(POLICY_FILE),
            &serde_json::to_vec(&policy).unwrap(),
        );
        Arrival {
            owner: "[\"https://everett.raitox.tech\",\"account\",\"github:1\"]".into(),
            sender_owner: "[\"https://everett.raitox.tech\",\"account\",\"github:1\"]".into(),
            recipient_owner: "[\"https://everett.raitox.tech\",\"account\",\"github:1\"]".into(),
            recipient_device: "agent-receiver".into(),
            sender_device: "agent-receiver".into(),
            session: "ext-grok-acceptance".into(),
            harness: "grok-bot".into(),
            sender_session: "human".into(),
            sender_harness: String::new(),
            message_id: "m0123456789".into(),
            created_at_ms: 1_700_000_000_000,
            is_reply: false,
            external: false,
        }
    }

    fn deliver(root: &Path, arrival: &Arrival, now: i64, posts: &mut Posts) -> Outcome {
        evaluate(root, Some(arrival), now, &mut |request| posts.call(request)).unwrap()
    }

    #[test]
    fn missing_policy_stays_quiet_and_bad_files_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let outcome =
            evaluate(temp.path(), None, 1_700_000_100, &mut |_| panic!("no post")).unwrap();
        assert!(outcome.value.is_none());
        let arrival = fixture(temp.path());
        fs::set_permissions(
            temp.path().join(POLICY_FILE),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let error = evaluate(temp.path(), Some(&arrival), 1_700_000_100, &mut |_| {
            panic!("no post")
        })
        .unwrap_err();
        assert!(!error.message.contains(SECRET));
        assert!(error.message.contains("0600"));
    }

    #[test]
    fn exact_match_posts_once_without_the_inbox_text() {
        let temp = tempfile::tempdir().unwrap();
        let arrival = fixture(temp.path());
        let mut posts = Posts(vec![Response::Status(200)]);
        let outcome = deliver(temp.path(), &arrival, 1_700_000_100, &mut posts);
        let value = outcome.value.unwrap();
        assert_eq!(value["state"], "accepted");
        assert_eq!(value["http_status"], 200);
        assert!(outcome.logs[0].contains("HTTP 200"));
        assert!(!outcome.logs[0].contains(SECRET));
        let again = deliver(
            temp.path(),
            &arrival,
            1_700_000_101,
            &mut Posts(vec![Response::Status(200)]),
        );
        assert_eq!(again.value.unwrap()["state"], "already_posted");
    }

    #[test]
    fn cooldown_defers_then_a_later_message_posts() {
        let temp = tempfile::tempdir().unwrap();
        let arrival = fixture(temp.path());
        deliver(
            temp.path(),
            &arrival,
            1_700_000_100,
            &mut Posts(vec![Response::Status(200)]),
        );
        let mut second = arrival.clone();
        second.message_id = "mabcdef1234".into();
        let deferred = deliver(temp.path(), &second, 1_700_000_110, &mut Posts(Vec::new()));
        assert_eq!(deferred.value.unwrap()["reason"], "cooldown");
        let posted = deliver(
            temp.path(),
            &second,
            1_700_000_140,
            &mut Posts(vec![Response::Status(200)]),
        );
        assert_eq!(posted.value.unwrap()["state"], "accepted");
    }

    #[test]
    fn client_errors_do_not_retry_and_transport_errors_stop_at_five() {
        let temp = tempfile::tempdir().unwrap();
        let arrival = fixture(temp.path());
        let rejected = deliver(
            temp.path(),
            &arrival,
            1_700_000_100,
            &mut Posts(vec![Response::Status(401)]),
        );
        assert_eq!(rejected.value.unwrap()["http_status"], 401);
        let again = deliver(
            temp.path(),
            &arrival,
            1_700_000_200,
            &mut Posts(vec![Response::Status(200)]),
        );
        assert_eq!(again.value.unwrap()["state"], "rejected");

        let temp = tempfile::tempdir().unwrap();
        let arrival = fixture(temp.path());
        for attempt in 0..MAX_ATTEMPTS {
            let outcome = deliver(
                temp.path(),
                &arrival,
                1_700_000_100 + i64::from(attempt) * 31,
                &mut Posts(vec![Response::Transport]),
            );
            let state = outcome.value.unwrap()["state"]
                .as_str()
                .unwrap()
                .to_string();
            if attempt + 1 < MAX_ATTEMPTS {
                assert_eq!(state, "deferred");
            } else {
                assert_eq!(state, "rejected");
            }
        }
        let stopped = deliver(
            temp.path(),
            &arrival,
            1_700_000_400,
            &mut Posts(vec![Response::Status(200)]),
        );
        assert_eq!(stopped.value.unwrap()["state"], "rejected");
    }

    #[test]
    fn mismatches_replies_external_messages_and_old_messages_do_not_post() {
        let temp = tempfile::tempdir().unwrap();
        let arrival = fixture(temp.path());
        let mut wrong = arrival.clone();
        wrong.sender_device = "agent-other".into();
        let outcome = deliver(temp.path(), &wrong, 1_700_000_100, &mut Posts(Vec::new()));
        assert_eq!(outcome.value.unwrap()["state"], "no_match");
        let mut reply = arrival.clone();
        reply.is_reply = true;
        assert_eq!(
            deliver(temp.path(), &reply, 1_700_000_100, &mut Posts(Vec::new()))
                .value
                .unwrap()["state"],
            "skipped"
        );
        let mut external = arrival.clone();
        external.external = true;
        assert_eq!(
            deliver(
                temp.path(),
                &external,
                1_700_000_100,
                &mut Posts(Vec::new())
            )
            .value
            .unwrap()["state"],
            "skipped"
        );
        let mut old = arrival.clone();
        old.created_at_ms = 1_600_000_000_000;
        assert_eq!(
            deliver(temp.path(), &old, 1_700_000_100, &mut Posts(Vec::new()))
                .value
                .unwrap()["reason"],
            "message is older than activated_at"
        );
    }

    #[test]
    fn invalid_urls_unknown_fields_and_loose_bearer_files_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let mut arrival = fixture(temp.path());
        let mut policy: Value =
            serde_json::from_slice(&fs::read(temp.path().join(POLICY_FILE)).unwrap()).unwrap();
        policy["rules"][0]["url"] = json!("http://api2.cursor.sh/automations/webhook/routine");
        private(
            &temp.path().join(POLICY_FILE),
            &serde_json::to_vec(&policy).unwrap(),
        );
        let error = evaluate(temp.path(), Some(&arrival), 1_700_000_100, &mut |_| {
            panic!("no post")
        })
        .unwrap_err();
        assert!(error.message.contains("https"));
        policy["rules"][0]["url"] = json!("https://api2.cursor.sh/automations/webhook/routine");
        policy["wildcard"] = json!("*");
        private(
            &temp.path().join(POLICY_FILE),
            &serde_json::to_vec(&policy).unwrap(),
        );
        assert!(
            evaluate(temp.path(), Some(&arrival), 1_700_000_100, &mut |_| panic!(
                "no post"
            ))
            .is_err()
        );
        arrival.message_id = "mloose12345".into();
        let bearer = temp.path().join("loose.key");
        fs::write(&bearer, SECRET).unwrap();
        fs::set_permissions(&bearer, fs::Permissions::from_mode(0o644)).unwrap();
        policy.as_object_mut().unwrap().remove("wildcard");
        policy["rules"][0]["bearer_file"] = json!(bearer);
        private(
            &temp.path().join(POLICY_FILE),
            &serde_json::to_vec(&policy).unwrap(),
        );
        let error = evaluate(temp.path(), Some(&arrival), 1_700_000_100, &mut |_| {
            panic!("no post")
        })
        .unwrap_err();
        assert!(!error.message.contains(SECRET));
    }

    #[test]
    fn local_queue_uses_this_device_for_both_sender_and_recipient() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = testing_begin(200);
        let _override = OverrideGuard::set(
            temp.path().to_path_buf(),
            Identity {
                owner: "[\"https://everett.raitox.tech\",\"account\",\"github:1\"]".into(),
                device_id: "agent-receiver".into(),
            },
        );
        let arrival = fixture(temp.path());
        let mut message = Map::new();
        message.insert("id".into(), json!(arrival.message_id));
        message.insert("from".into(), json!("human"));
        message.insert("from_harness".into(), json!(""));
        message.insert("text".into(), json!(PHRASE));
        message.insert("ts".into(), json!(1_700_000_100.0));
        message.insert("kind".into(), json!("message"));
        let value = after_local_queue(&arrival.session, &arrival.harness, &message)
            .unwrap()
            .unwrap();
        assert_eq!(value["state"], "accepted");
        let posts = testing_posts();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].bearer, SECRET);
        assert!(!posts[0].body.contains(PHRASE));
    }
}
