use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::error::{EverettError, Result};
use crate::paths;
use crate::proc::{parent_of, pid_alive};
use crate::session::{home, now};

pub const MAX_BATCH: usize = 5;
pub const MAX_TEXT: usize = 2000;
pub const MAX_INJECT: usize = 6000;
pub const TTL: f64 = 7.0 * 24.0 * 3600.0;
pub const MAX_HOPS: i64 = 3;
pub const HUMAN: &str = "human";

fn sid_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9._-]{1,128}$").unwrap())
}

pub fn inbox_dir() -> PathBuf {
    home().join(".everett").join("inbox")
}

pub fn valid_id(session_id: &str) -> bool {
    !session_id.is_empty() && sid_re().is_match(session_id) && session_id != "." && session_id != ".."
}

pub fn path(session_id: &str) -> Result<PathBuf> {
    if !valid_id(session_id) {
        return Err(EverettError::new(2, format!("not a valid session id: {:?}", session_id)));
    }
    Ok(inbox_dir().join(format!("{}.jsonl", session_id)))
}

fn done_path(session_id: &str) -> PathBuf {
    inbox_dir().join(format!("{}.done", session_id))
}

fn append(target: &PathBuf, line: &str) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            EverettError::new(2, format!("cannot create {}: {}", parent.display(), e))
        })?;
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(target)
        .map_err(|e| EverettError::new(2, format!("cannot write {}: {}", target.display(), e)))?;
    let _ = f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600));
    f.write_all(line.as_bytes())
        .map_err(|e| EverettError::new(2, format!("cannot write {}: {}", target.display(), e)))
}

pub fn new_id() -> String {
    format!("m{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

#[allow(clippy::too_many_arguments)]
pub fn post(
    to: &str,
    text: &str,
    sender: &str,
    kind: &str,
    reply_to: &str,
    hops: i64,
    from_harness: &str,
    from_card: &str,
    extra: Option<Map<String, Value>>,
) -> Result<Map<String, Value>> {
    let text = text.trim();
    if text.is_empty() {
        return Err(EverettError::new(2, "The message is empty."));
    }
    let mut message = Map::new();
    message.insert("id".into(), json!(new_id()));
    message.insert(
        "from".into(),
        json!(if sender.is_empty() { HUMAN } else { sender }),
    );
    message.insert("from_harness".into(), json!(from_harness));
    let card: String = from_card.chars().take(160).collect();
    message.insert("from_card".into(), json!(card));
    message.insert("to".into(), json!(to));
    message.insert("text".into(), json!(text));
    message.insert("ts".into(), json!(now()));
    message.insert("reply_to".into(), json!(reply_to));
    message.insert("hops".into(), json!(hops));
    message.insert("kind".into(), json!(kind));
    if let Some(extra) = extra {
        for (k, v) in extra {
            message.insert(k, v);
        }
    }
    let target = path(to)?;
    append(&target, &(serde_json::to_string(&message).unwrap() + "\n"))?;
    Ok(message)
}

pub fn read(session_id: &str) -> Vec<Map<String, Value>> {
    let mut out = Vec::new();
    let Ok(p) = path(session_id) else { return out };
    let Ok(raw) = fs::read_to_string(&p) else { return out };
    for line in raw.lines() {
        if let Ok(Value::Object(item)) = serde_json::from_str::<Value>(line) {
            if item.get("id").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false)
                && item.get("text").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false)
            {
                out.push(item);
            }
        }
    }
    out
}

pub fn done_ids(session_id: &str) -> HashSet<String> {
    fs::read_to_string(done_path(session_id))
        .map(|t| t.split_whitespace().map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

/// Undelivered, unexpired messages, oldest first.
pub fn pending(session_id: &str, at: Option<f64>) -> Vec<Map<String, Value>> {
    let now = at.unwrap_or_else(now);
    let done = done_ids(session_id);
    read(session_id)
        .into_iter()
        .filter(|m| {
            let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let ts = m.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
            !id.is_empty() && !done.contains(id) && now - ts < TTL
        })
        .collect()
}

pub fn mark_done(session_id: &str, ids: &[&str]) {
    let ids: Vec<&&str> = ids.iter().filter(|i| !i.is_empty()).collect();
    if ids.is_empty() {
        return;
    }
    let body: String = ids.iter().map(|i| format!("{}\n", i)).collect();
    let _ = append(&done_path(session_id), &body);
}

/// Look a message up by id across every inbox (for replies).
pub fn find(message_id: &str) -> Option<Map<String, Value>> {
    let folder = inbox_dir();
    if !folder.is_dir() || message_id.is_empty() {
        return None;
    }
    for file in paths::glob(&folder, "*.jsonl") {
        let Ok(text) = fs::read_to_string(&file) else { continue };
        if !text.contains(message_id) {
            continue;
        }
        for line in text.lines() {
            if let Ok(Value::Object(item)) = serde_json::from_str::<Value>(line) {
                if item.get("id").and_then(|v| v.as_str()) == Some(message_id) {
                    return Some(item);
                }
            }
        }
    }
    None
}

/// Look a message up by id in one session's inbox only; the message must be
/// addressed to that session. Used for replies bound to a specific inbox.
pub fn find_in(session_id: &str, message_id: &str) -> Option<Map<String, Value>> {
    if message_id.is_empty() {
        return None;
    }
    read(session_id).into_iter().find(|item| {
        item.get("id").and_then(|v| v.as_str()) == Some(message_id)
            && item.get("to").and_then(|v| v.as_str()) == Some(session_id)
    })
}

pub fn reply_hops(original: &Map<String, Value>, env_hops: i64) -> Result<i64> {
    let hops = original
        .get("hops")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(env_hops)
        + 1;
    if hops > MAX_HOPS {
        return Err(EverettError::new(
            7,
            format!(
                "Hop limit reached ({}/{}): this thread already went back and forth {} times. \
                 Answer here instead of replying again.",
                hops - 1,
                MAX_HOPS,
                hops - 1
            ),
        ));
    }
    Ok(hops)
}

/// Poll the sender's inbox for a reply to message_id; consume and return it, or None on timeout.
pub fn wait_reply(sender: &str, message_id: &str, wait: f64, poll: f64) -> Option<Map<String, Value>> {
    wait_reply_with(sender, message_id, wait, poll, || now(), |d| std::thread::sleep(std::time::Duration::from_secs_f64(d)))
}

pub fn wait_reply_with(
    sender: &str,
    message_id: &str,
    wait: f64,
    poll: f64,
    clock: impl Fn() -> f64,
    sleep: impl Fn(f64),
) -> Option<Map<String, Value>> {
    let deadline = clock() + wait.max(0.0);
    loop {
        for item in pending(sender, None) {
            if item.get("reply_to").and_then(|v| v.as_str()) == Some(message_id) {
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                    mark_done(sender, &[id]);
                }
                return Some(item);
            }
        }
        if clock() >= deadline {
            return None;
        }
        let left = (deadline - clock()).max(0.0);
        sleep(if poll.min(left) > 0.0 { poll.min(left) } else { poll });
    }
}

fn ago(ts: f64, now: f64) -> String {
    let d = (now - ts).max(0.0) as i64;
    if d < 60 {
        format!("{}s ago", d)
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else {
        format!("{}h ago", d / 3600)
    }
}

/// The injection text for a batch, and the ids it actually contains (bounded).
pub fn render(messages: &[Map<String, Value>], at: Option<f64>) -> (String, Vec<String>) {
    let now = at.unwrap_or_else(now);
    let mut parts: Vec<String> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    let mut total = 0usize;
    for m in messages.iter().take(MAX_BATCH) {
        let raw_text = m.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let text = if raw_text.chars().count() <= MAX_TEXT {
            raw_text.to_string()
        } else {
            format!("{} […clipped]", raw_text.chars().take(MAX_TEXT).collect::<String>())
        };
        let sender = m.get("from").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or(HUMAN);
        let who = if sender == HUMAN {
            "the human (terminal)".to_string()
        } else {
            let harness = m.get("from_harness").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("agent");
            format!("{} session {}", harness, sender.chars().take(12).collect::<String>())
        };
        let card = m
            .get("from_card")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|c| format!(" — card: {}", c))
            .unwrap_or_default();
        let kind = m.get("kind").and_then(|v| v.as_str()).unwrap_or("message");
        let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let head = match kind {
            "reply" => format!("REPLY to your message {}", m.get("reply_to").and_then(|v| v.as_str()).unwrap_or("")),
            "event" => "EVENT".to_string(),
            _ => "MESSAGE".to_string(),
        };
        let how = if kind == "reply" || kind == "event" {
            String::new()
        } else {
            format!(
                "\nAnswer with everett_send(reply_to=\"{}\", text=...) or `everett reply {} \"<text>\"`.",
                id, id
            )
        };
        let ts = m.get("ts").and_then(|v| v.as_f64()).unwrap_or(now);
        let hops = m.get("hops").and_then(|v| v.as_i64()).unwrap_or(1);
        let block = format!(
            "--- {} {} from {}{} ({}, hop {}/{})\n{}{}",
            head,
            id,
            who,
            card,
            ago(ts, now),
            hops,
            MAX_HOPS,
            text,
            how
        );
        if !parts.is_empty() && total + block.chars().count() > MAX_INJECT {
            break;
        }
        total += block.chars().count();
        parts.push(block);
        ids.push(id.to_string());
    }
    if parts.is_empty() {
        return (String::new(), Vec::new());
    }
    let more = messages.len() - ids.len();
    let header = format!(
        "[Everett] {} message(s) from other sessions on this machine, delivered by Everett \
         (not typed by the user). Handle them alongside your current task; the user can see this.",
        ids.len()
    );
    let tail = if more > 0 {
        format!("\n({} more waiting; they arrive at your next turn or tool call.)", more)
    } else {
        String::new()
    };
    (format!("{}\n{}{}", header, parts.join("\n"), tail), ids)
}

/// Render pending messages for injection, mark exactly those delivered, and
/// record the highest hop count seen so the session can't forward forever.
pub fn take(session_id: &str) -> String {
    let pending_msgs = pending(session_id, None);
    let (text, ids) = render(&pending_msgs, None);
    if !ids.is_empty() {
        mark_done(session_id, &ids.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let max_hops = ids
            .iter()
            .filter_map(|id| {
                pending_msgs
                    .iter()
                    .find(|m| m.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
            })
            .filter_map(|m| m.get("hops").and_then(|v| v.as_i64()))
            .max()
            .unwrap_or(0);
        record_hops(session_id, max_hops);
    }
    text
}

pub fn live_path(session_id: &str) -> PathBuf {
    inbox_dir().join("live").join(format!("{}.json", session_id))
}

/// The highest message hop count take() has delivered to this session (0 if none).
/// A live session inherits received hop counts for 15 minutes, not forever.
pub const HOP_MEMORY_SECONDS: f64 = 900.0;

/// The highest hop count take() delivered to this session within HOP_MEMORY_SECONDS.
pub fn session_hops(session_id: &str) -> i64 {
    if !valid_id(session_id) {
        return 0;
    }
    let data = fs::read_to_string(live_path(session_id))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or(Value::Null);
    let hops = data.get("hops").and_then(|h| h.as_i64()).unwrap_or(0).max(0);
    let stamp = data.get("hops_ts").and_then(|h| h.as_f64()).unwrap_or(0.0);
    if hops <= 0 || now() - stamp > HOP_MEMORY_SECONDS {
        return 0;
    }
    hops
}

/// Remember the highest hops delivered to this session in its live record (best effort).
pub fn record_hops(session_id: &str, hops: i64) {
    if !valid_id(session_id) {
        return;
    }
    let target = live_path(session_id);
    let mut current: Map<String, Value> = fs::read_to_string(&target)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let prev = current.get("hops").and_then(|v| v.as_i64()).unwrap_or(0);
    current.insert("hops".into(), json!(prev.max(hops)));
    current.insert("hops_ts".into(), json!(now()));
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = target.with_file_name(format!(
        ".{}.{}.tmp",
        target.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    if fs::write(&tmp, serde_json::to_string(&Value::Object(current)).unwrap()).is_ok() {
        let _ = fs::rename(&tmp, &target);
    }
}

const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "fish", "-sh", "-bash", "-zsh"];

/// The harness process that ran this hook: our parent, or its parent when a shell sits between.
pub fn harness_pid() -> i32 {
    let pid = std::os::unix::process::parent_id() as i32;
    if let Some((ppid, comm)) = parent_of(pid) {
        let base = comm.rsplit('/').next().unwrap_or(&comm);
        if SHELLS.contains(&base) && ppid > 0 {
            return ppid;
        }
    }
    pid
}

/// Record that this session's harness process is alive (called from hooks; cheap after the first).
pub fn touch_live(session_id: &str, harness: &str, state: &str) {
    let target = live_path(session_id);
    let current: Map<String, Value> = fs::read_to_string(&target)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let mut pid = current.get("pid").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let now = now();
    let cur_state = current.get("state").and_then(|v| v.as_str()).unwrap_or("");
    let cur_ts = current.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
    if pid != 0 && pid_alive(pid) && cur_state == state && now - cur_ts < 30.0 {
        return;
    }
    if pid == 0 || !pid_alive(pid) {
        pid = harness_pid();
        release_pid(pid, session_id);
    }
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = target.with_file_name(format!(
        ".{}.{}.tmp",
        target.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let mut body = current;
    body.insert("pid".into(), json!(pid));
    body.insert("harness".into(), json!(harness));
    body.insert("state".into(), json!(state));
    body.insert("ts".into(), json!(now));
    if fs::write(&tmp, serde_json::to_string(&Value::Object(body)).unwrap()).is_ok() {
        let _ = fs::rename(&tmp, &target);
    }
}

/// A harness process holds one session at a time: after /clear or /resume, the old session's
/// live record for the same process is stale, so drop it.
fn release_pid(pid: i32, keep: &str) {
    let folder = inbox_dir().join("live");
    if !folder.is_dir() {
        return;
    }
    for file in paths::glob(&folder, "*.json") {
        if file.file_stem().map(|s| s.to_string_lossy().to_string()) == Some(keep.to_string()) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&file) else { continue };
        let file_pid = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("pid").and_then(|p| p.as_i64()))
            .unwrap_or(0) as i32;
        if file_pid == pid {
            let _ = fs::remove_file(&file);
        }
    }
}

/// The live record of a session whose harness process is still running, else None.
pub fn live(session_id: &str) -> Option<Map<String, Value>> {
    if !valid_id(session_id) {
        return None;
    }
    let data: Map<String, Value> = fs::read_to_string(live_path(session_id))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())?;
    let pid = data.get("pid").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    if !pid_alive(pid) {
        return None;
    }
    Some(data)
}
