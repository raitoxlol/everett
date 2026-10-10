use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::config;
use crate::core::{find_secret, project_for, slug};
use crate::error::{EverettError, Result};
use crate::inbox;
use crate::paths;
use crate::send::caller_identity;
use crate::session::{file_mtime, home, now};

pub const KINDS: &[&str] = &["done", "blocked", "needs-input", "info"];
pub const HUMAN_KINDS: &[&str] = &["blocked", "needs-input"];
pub const DEBOUNCE: f64 = 600.0;
pub const MAX_TEXT: usize = 300;
pub const ESCALATE_MINUTES: f64 = 30.0;
pub const ESCALATE_THROTTLE: f64 = 60.0;

pub fn events_path() -> PathBuf {
    home().join(".everett").join("events.jsonl")
}

pub fn state_dir() -> PathBuf {
    home().join(".everett").join("state")
}

pub fn subs_path() -> PathBuf {
    home().join(".everett").join("subscriptions.json")
}

fn valid_sid(sid: &str) -> bool {
    inbox::valid_id(sid)
}

fn write_json(target: &PathBuf, data: &Value) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            EverettError::new(2, format!("cannot create {}: {}", parent.display(), e))
        })?;
    }
    let tmp = target.with_file_name(format!(
        ".{}.{}.tmp",
        target.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let w = || {
        fs::write(&tmp, serde_json::to_string(data).unwrap_or_default())
            .and_then(|_| fs::rename(&tmp, target))
    };
    w().map_err(|e| EverettError::new(2, format!("cannot write {}: {}", target.display(), e)))
}

pub fn state(session_id: &str) -> Option<Map<String, Value>> {
    if !valid_sid(session_id) {
        return None;
    }
    fs::read_to_string(state_dir().join(format!("{}.json", session_id)))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
}

// ---- record ----

#[allow(clippy::too_many_arguments)]
pub fn record(
    kind: &str,
    text: &str,
    session: Option<&str>,
    project: Option<&str>,
    harness: &str,
    cwd: Option<&str>,
    source: &str,
    at: Option<f64>,
) -> Result<Option<Map<String, Value>>> {
    if !KINDS.contains(&kind) {
        return Err(EverettError::new(2, format!("kind must be one of {}.", KINDS.join(", "))));
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err(EverettError::new(2, "The event message is empty."));
    }
    let text: String = text.chars().take(MAX_TEXT).collect();
    let secret = find_secret(&text);
    if !secret.is_empty() {
        return Err(EverettError::new(2, format!("Rejected: this looks like it contains {}.", secret)));
    }
    let ident = caller_identity();
    let ident_sid = ident.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
    let session = session.map(|s| s.to_string()).unwrap_or_else(|| ident_sid.to_string());
    let harness = if !harness.is_empty() {
        harness.to_string()
    } else if session == ident_sid {
        ident.get("harness").and_then(|v| v.as_str()).unwrap_or("").to_string()
    } else {
        String::new()
    };
    let cwd = cwd
        .map(|c| c.to_string())
        .unwrap_or_else(|| std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default());
    let project = match project {
        Some(p) => slug(p),
        None => project_for(&cwd),
    };
    let now = at.unwrap_or_else(now);
    let prev = if session.is_empty() { None } else { state(&session) };
    if source == "auto"
        && prev.as_ref().map(|p| {
            p.get("kind").and_then(|v| v.as_str()) == Some(kind)
                && (p.get("text").and_then(|v| v.as_str()) == Some(text.as_str())
                    || now - p.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0) < DEBOUNCE)
        }) == Some(true)
    {
        return Ok(None);
    }
    let mut event = Map::new();
    event.insert("id".into(), json!(format!("e{}", &uuid::Uuid::new_v4().simple().to_string()[..10])));
    event.insert("ts".into(), json!(now));
    event.insert("kind".into(), json!(kind));
    event.insert("text".into(), json!(text));
    event.insert("session".into(), json!(session));
    event.insert("project".into(), json!(project));
    event.insert("harness".into(), json!(harness));
    event.insert("cwd".into(), json!(cwd));
    event.insert("source".into(), json!(source));
    let events_file = events_path();
    if let Some(parent) = events_file.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            EverettError::new(2, format!("cannot create {}: {}", parent.display(), e))
        })?;
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_file)
        .map_err(|e| EverettError::new(2, format!("cannot write {}: {}", events_file.display(), e)))?;
    let _ = f.set_permissions(PermissionsExt::from_mode(0o600));
    f.write_all((serde_json::to_string(&event).unwrap() + "\n").as_bytes())
        .map_err(|e| EverettError::new(2, format!("cannot write {}: {}", events_file.display(), e)))?;
    if !session.is_empty() && valid_sid(&session) {
        let same = prev
            .as_ref()
            .map(|p| p.get("kind").and_then(|v| v.as_str()) == Some(kind))
            .unwrap_or(false);
        let mut state_doc = event.clone();
        state_doc.insert(
            "since".into(),
            if same {
                prev.as_ref()
                    .and_then(|p| p.get("since").cloned().or_else(|| p.get("ts").cloned()))
                    .unwrap_or(json!(now))
            } else {
                json!(now)
            },
        );
        state_doc.insert(
            "escalated".into(),
            if same {
                prev.and_then(|p| p.get("escalated").cloned()).unwrap_or(json!(false))
            } else {
                json!(false)
            },
        );
        write_json(&state_dir().join(format!("{}.json", session)), &Value::Object(state_doc))?;
    }
    route(&event);
    if HUMAN_KINDS.contains(&kind) {
        notify(&event, "", None);
    }
    Ok(Some(event))
}

// ---- subscriptions ----

pub fn subscriptions() -> HashMap<String, Vec<String>> {
    let Ok(text) = fs::read_to_string(subs_path()) else {
        return HashMap::new();
    };
    let Ok(Value::Object(data)) = serde_json::from_str::<Value>(&text) else {
        return HashMap::new();
    };
    data.iter()
        .filter(|(_, v)| v.is_array())
        .map(|(k, v)| {
            (
                k.clone(),
                v.as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|t| t.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

pub fn subscribe(subscriber: &str, target: &str, remove: bool) -> Result<Vec<String>> {
    if !valid_sid(subscriber) {
        return Err(EverettError::new(
            2,
            "No subscriber session: run this inside a session or pass --as <session id>.",
        ));
    }
    let target = target.trim();
    if target.is_empty() {
        return Err(EverettError::new(2, "Name a session id, \"project:<name>\", or \"*\"."));
    }
    let mut subs = subscriptions();
    let mut current = subs.get(subscriber).cloned().unwrap_or_default();
    if remove {
        current.retain(|t| t != target);
    } else if !current.contains(&target.to_string()) {
        current.push(target.to_string());
    }
    if current.is_empty() {
        subs.remove(subscriber);
    } else {
        subs.insert(subscriber.to_string(), current.clone());
    }
    write_json(&subs_path(), &serde_json::to_value(&subs).unwrap_or(Value::Null))?;
    Ok(current)
}

/// CLI/MCP target -> stored form: a session id, "project:<slug>", or "*".
pub fn resolve_target(target: &str) -> String {
    let target = target.trim();
    if target == "*" {
        return "*".to_string();
    }
    if let Some(rest) = target.strip_prefix("project:") {
        return format!("project:{}", slug(rest));
    }
    match crate::registry::find(target, &crate::registry::scan(72.0 * 7.0, true, None, "")) {
        Ok(s) => s.id,
        Err(_) => format!("project:{}", slug(target)),
    }
}

pub fn matches(target: &str, event: &Map<String, Value>) -> bool {
    if target == "*" {
        return true;
    }
    if let Some(name) = target.strip_prefix("project:") {
        return event
            .get("project")
            .and_then(|v| v.as_str())
            .map(|p| !p.is_empty() && name == p)
            .unwrap_or(false);
    }
    let sid = event.get("session").and_then(|v| v.as_str()).unwrap_or("");
    !sid.is_empty() && (sid == target || (target.chars().count() >= 6 && sid.starts_with(target)))
}

pub fn render(event: &Map<String, Value>) -> String {
    let session = event.get("session").and_then(|v| v.as_str()).unwrap_or("");
    let who = if !session.is_empty() {
        let harness = event.get("harness").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("session");
        format!("{} {}", harness, session.chars().take(12).collect::<String>())
    } else {
        "the human".to_string()
    };
    let where_ = event
        .get("project")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|p| format!(" in {}", p))
        .unwrap_or_default();
    format!(
        "{} from {}{}: {}",
        event.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_uppercase(),
        who,
        where_,
        event.get("text").and_then(|v| v.as_str()).unwrap_or("")
    )
}

/// Post the event to every subscribed session's inbox (never to the session it came from).
pub fn route(event: &Map<String, Value>) -> Vec<String> {
    let mut delivered = Vec::new();
    let session = event.get("session").and_then(|v| v.as_str()).unwrap_or("");
    for (subscriber, targets) in subscriptions() {
        if subscriber == session || !targets.iter().any(|t| matches(t, event)) {
            continue;
        }
        let mut extra = Map::new();
        extra.insert("event".into(), event.get("id").cloned().unwrap_or(Value::Null));
        if inbox::post(
            &subscriber,
            &render(event),
            if session.is_empty() { inbox::HUMAN } else { session },
            "event",
            "",
            1,
            event.get("harness").and_then(|v| v.as_str()).unwrap_or(""),
            "",
            Some(extra),
        )
        .is_ok()
        {
            delivered.push(subscriber);
        }
    }
    delivered
}

// ---- the human's notifier ----

fn applescript(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn notify_line(event: &Map<String, Value>, reason: &str) -> String {
    let project = event.get("project").and_then(|v| v.as_str()).unwrap_or("");
    let session = event.get("session").and_then(|v| v.as_str()).unwrap_or("");
    let where_ = if !project.is_empty() {
        project.to_string()
    } else if !session.is_empty() {
        session.chars().take(8).collect()
    } else {
        "everett".to_string()
    };
    format!(
        "{}{} [{}] {}",
        if reason.is_empty() { String::new() } else { format!("{}: ", reason) },
        event.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
        where_,
        event.get("text").and_then(|v| v.as_str()).unwrap_or("")
    )
}

/// Tell the human. Mode: config `notify` (osascript | command | both | none; env EVERETT_NOTIFY).
/// `runner` replaces the launcher (tests).
pub fn notify(
    event: &Map<String, Value>,
    reason: &str,
    runner: Option<&dyn Fn(&[String], Option<&HashMap<String, String>>)>,
) -> Vec<Vec<String>> {
    let command = config::get("notify_command", Some("EVERETT_NOTIFY_COMMAND"), "");
    let default_mode = if !command.is_empty() { "both" } else { "osascript" };
    let mode = config::get("notify", Some("EVERETT_NOTIFY"), default_mode);
    let line = notify_line(event, reason);
    let mut runs: Vec<(Vec<String>, Option<HashMap<String, String>>)> = Vec::new();
    if (mode == "command" || mode == "both") && !command.is_empty() {
        let mut env: HashMap<String, String> = std::env::vars().collect();
        env.insert("EVERETT_EVENT_KIND".into(), event.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string());
        env.insert("EVERETT_EVENT_TEXT".into(), event.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string());
        env.insert("EVERETT_EVENT_SESSION".into(), session_str(event, "session"));
        env.insert("EVERETT_EVENT_PROJECT".into(), session_str(event, "project"));
        env.insert("EVERETT_EVENT_REASON".into(), reason.to_string());
        env.insert("EVERETT_EVENT_JSON".into(), serde_json::to_string(event).unwrap_or_default());
        runs.push((vec!["/bin/sh".into(), "-c".into(), command, "everett".into(), line], Some(env)));
    }
    if (mode == "osascript" || mode == "both") && cfg!(target_os = "macos") {
        let title = format!(
            "Everett: {}{}",
            if reason.is_empty() { "" } else { "still " },
            event.get("kind").and_then(|v| v.as_str()).unwrap_or("")
        );
        let text: String = event
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(200)
            .collect();
        let mut script = format!(
            "display notification {} with title {}",
            applescript(&text),
            applescript(&title)
        );
        let sub = event.get("project").and_then(|v| v.as_str()).unwrap_or("");
        if !sub.is_empty() {
            script += &format!(" subtitle {}", applescript(sub));
        }
        runs.push((vec!["osascript".into(), "-e".into(), script], None));
    }
    let mut argv_list = Vec::new();
    for (argv, env) in &runs {
        if let Some(runner) = runner {
            runner(argv, env.as_ref());
        } else {
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(env) = env {
                cmd.envs(env);
            }
            if let Ok(mut child) = cmd.spawn() {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
        }
        argv_list.push(argv.clone());
    }
    argv_list
}

fn session_str(event: &Map<String, Value>, key: &str) -> String {
    event.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn escalate_minutes() -> f64 {
    config::get("escalate_minutes", Some("EVERETT_ESCALATE_MINUTES"), &ESCALATE_MINUTES.to_string())
        .parse::<f64>()
        .unwrap_or(ESCALATE_MINUTES)
}

/// Notify once for every session blocked / waiting on input longer than escalate_minutes.
pub fn check_escalations(at: Option<f64>, runner: Option<&dyn Fn(&[String], Option<&HashMap<String, String>>)>) -> Vec<Map<String, Value>> {
    let now = at.unwrap_or_else(now);
    let limit = escalate_minutes() * 60.0;
    let mut out = Vec::new();
    if limit <= 0.0 || !state_dir().is_dir() {
        return out;
    }
    for file in paths::glob(&state_dir(), "*.json") {
        let Ok(text) = fs::read_to_string(&file) else { continue };
        let Ok(Value::Object(mut data)) = serde_json::from_str::<Value>(&text) else { continue };
        let kind = data.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let escalated = data.get("escalated").and_then(|v| v.as_bool()).unwrap_or(false);
        let since = data
            .get("since")
            .and_then(|v| v.as_f64())
            .or_else(|| data.get("ts").and_then(|v| v.as_f64()))
            .unwrap_or(now);
        if !HUMAN_KINDS.contains(&kind) || escalated || now - since < limit {
            continue;
        }
        let minutes = ((now - since) / 60.0) as i64;
        notify(
            &data,
            &format!("{} for {} min", kind, minutes),
            runner,
        );
        data.insert("escalated".into(), json!(true));
        let _ = write_json(&file, &Value::Object(data.clone()));
        out.push(data);
    }
    out
}

/// Throttled escalation check for hooks: at most once per ESCALATE_THROTTLE seconds.
pub fn maybe_escalate(at: Option<f64>) {
    let now = at.unwrap_or_else(now);
    let stamp = state_dir().join(".escalate-check");
    let mtime = file_mtime(&stamp);
    if mtime > 0.0 && now - mtime < ESCALATE_THROTTLE {
        return;
    }
    if mtime == 0.0 && !state_dir().is_dir() {
        return;
    }
    // Actually advance the stamp's mtime: open+drop alone does not, and an
    // unchanged mtime makes the scan run on every hook invocation.
    if let Ok(mut f) = OpenOptions::new().create(true).write(true).truncate(true).open(&stamp) {
        let _ = f.write_all(now.to_string().as_bytes());
    }
    let _ = check_escalations(Some(now), None);
}

// ---- reading ----

pub fn read(since: f64) -> Vec<Map<String, Value>> {
    let mut out = Vec::new();
    let Ok(text) = fs::read_to_string(events_path()) else {
        return out;
    };
    for line in text.lines() {
        if let Ok(Value::Object(item)) = serde_json::from_str::<Value>(line) {
            let ts = item.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
            if ts >= since && item.get("kind").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false) {
                out.push(item);
            }
        }
    }
    out
}

/// '24h', '30m', '2d', or a number of hours -> seconds.
pub fn parse_since(value: &str) -> Result<f64> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^\s*(\d+(?:\.\d+)?)\s*([smhd]?)\s*$").unwrap());
    let m = re
        .captures(value)
        .ok_or_else(|| EverettError::new(2, "--since takes a duration like 30m, 24h, or 7d."))?;
    let amount: f64 = m[1].parse().unwrap_or(0.0);
    let multiplier = match &m[2] {
        "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        _ => 3600.0,
    };
    Ok(amount * multiplier)
}

pub fn age(seconds: f64) -> String {
    let s = seconds.max(0.0) as i64;
    if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 172800 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86400)
    }
}

/// 'blocked 32h: waiting on staging key'.
pub fn describe(data: &Map<String, Value>, at: Option<f64>) -> String {
    let now = at.unwrap_or_else(now);
    let since = data
        .get("since")
        .and_then(|v| v.as_f64())
        .or_else(|| data.get("ts").and_then(|v| v.as_f64()))
        .unwrap_or(now);
    format!(
        "{} {}: {}",
        data.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
        age(now - since),
        data.get("text").and_then(|v| v.as_str()).unwrap_or("")
    )
}

/// Attach each session's latest event state (for ls / everett_ls).
pub fn apply(sessions: &mut [crate::session::Session]) {
    let now = now();
    for s in sessions.iter_mut() {
        if let Some(data) = state(&s.id) {
            if data.get("kind").and_then(|v| v.as_str()).map(|k| !k.is_empty()).unwrap_or(false) {
                s.state = describe(&data, Some(now));
                s.state_kind = data.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string();
            }
        }
    }
}

// ---- automatic detection (Stop hooks) ----

fn blocked_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:blocked|stuck|waiting (?:on|for)|can(?:no|')t (?:proceed|continue)|unable to (?:proceed|continue))\b",
        )
        .unwrap()
    })
}

fn needs_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:need you to|needs? your (?:input|approval|decision|confirmation|go-ahead|answer)|please (?:confirm|approve|choose|decide|let me know|advise)|let me know (?:if|whether|which|how|what)|should i|do you want|would you like|want me to|shall i)\b",
        )
        .unwrap()
    })
}

fn negated_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(?:\bnot|\bno longer|\bnever|\bun|\bwithout|n't)\s*$").unwrap())
}

/// The last few prose lines of a reply (code blocks, quotes, and tables removed).
fn tail_lines(text: &str, lines: usize, chars: usize) -> Vec<String> {
    static FENCE: OnceLock<Regex> = OnceLock::new();
    let fence_re = FENCE.get_or_init(|| Regex::new(r"^\s*(```|~~~)").unwrap());
    static MARKS: OnceLock<Regex> = OnceLock::new();
    let marks = MARKS.get_or_init(|| Regex::new(r"[*_`]+").unwrap());
    let mut kept: Vec<String> = Vec::new();
    let mut fence = false;
    for raw in text.lines() {
        if fence_re.is_match(raw) {
            fence = !fence;
            continue;
        }
        let line = raw.trim();
        if fence || line.is_empty() || line.starts_with('>') || line.starts_with('|') {
            continue;
        }
        kept.push(marks.replace_all(line, "").trim().to_string());
    }
    let mut tail: Vec<String> = Vec::new();
    let mut total = 0usize;
    for line in kept.iter().rev() {
        if tail.len() >= lines || (total + line.chars().count() > chars && !tail.is_empty()) {
            break;
        }
        tail.insert(0, line.clone());
        total += line.chars().count();
    }
    tail
}

fn sentence(line: &str, start: usize) -> String {
    let prefix = &line[..start];
    let left = [". ", "! ", "? "]
        .iter()
        .filter_map(|pat| prefix.rfind(pat))
        .max()
        .map(|i| i + 2)
        .unwrap_or(0);
    static SENT_END: OnceLock<Regex> = OnceLock::new();
    let sent_end = SENT_END.get_or_init(|| Regex::new(r"[.!?](\s|$)").unwrap());
    let right = sent_end.find(&line[start..]).map(|m| start + m.end());
    let end = right.unwrap_or(line.len());
    line[left.min(end)..end].trim().to_string()
}

fn hit(pattern: &Regex, lines: &[String], statements_only: bool) -> String {
    for line in lines.iter().rev() {
        for m in pattern.find_iter(line) {
            let before: String = line[..m.start()].chars().rev().take(14).collect::<String>().chars().rev().collect();
            if negated_re().is_match(&before) {
                continue;
            }
            let sentence = sentence(line, m.start());
            if statements_only && sentence.ends_with('?') {
                continue;
            }
            return sentence;
        }
    }
    String::new()
}

/// Deterministic: (kind, summary) for the last assistant message of a turn, or None if empty.
pub fn classify(message: &str) -> Option<(String, String)> {
    let lines = tail_lines(message, 4, 700);
    if lines.is_empty() {
        return None;
    }
    let hit_result = hit(blocked_re(), &lines, true);
    if !hit_result.is_empty() {
        return Some(("blocked".into(), hit_result.chars().take(MAX_TEXT).collect()));
    }
    let question = lines
        .iter()
        .rev()
        .find(|l| l.trim_end_matches([' ', ')']).ends_with('?'))
        .cloned()
        .unwrap_or_default();
    if !question.is_empty() {
        let trimmed = question.trim_end();
        let start = trimmed.rfind('?').map(|i| i.saturating_sub(1)).unwrap_or(0);
        let s = sentence(&question, start);
        let out: String = s.chars().take(MAX_TEXT).collect();
        return Some((
            "needs-input".into(),
            if out.is_empty() { question.chars().take(MAX_TEXT).collect() } else { out },
        ));
    }
    let hit_result = hit(needs_re(), &lines, false);
    if !hit_result.is_empty() {
        return Some(("needs-input".into(), hit_result.chars().take(MAX_TEXT).collect()));
    }
    Some(("done".into(), lines.last().unwrap().chars().take(160).collect()))
}
