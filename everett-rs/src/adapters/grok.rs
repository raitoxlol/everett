use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Value};

use crate::cards::card_path;
use crate::paths;
use crate::proc::pid_alive;
use crate::session::{clean, file_mtime, home, is_injected, now, read_edges, Session};

pub const ACTIVITY_FILES: &[&str] = &["updates.jsonl", "chat_history.jsonl", "summary.json"];

fn query_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<user_query>\s*(.*?)\s*(?:</user_query>|$)").unwrap())
}

fn unreserved(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.' || c == b'~'
}

/// `urllib.parse.quote(s, safe='')` — UTF-8 percent-encoding of non-unreserved bytes.
pub fn quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        if unreserved(*b) {
            out.push(*b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `urllib.parse.unquote` ('+' stays literal).
pub fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            if let (Some(hi), Some(lo)) = (hexval(bytes[i + 1]), hexval(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

pub fn sessions_root() -> PathBuf {
    home().join(".grok").join("sessions")
}

/// Newest write in a session directory (appends don't touch the directory's own mtime).
pub fn activity(session_dir: &Path) -> f64 {
    let mut times: Vec<f64> = ACTIVITY_FILES
        .iter()
        .map(|name| file_mtime(&session_dir.join(name)))
        .filter(|t| *t > 0.0)
        .collect();
    if times.is_empty() {
        times.push(file_mtime(session_dir));
    }
    times.into_iter().fold(0.0, f64::max)
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(list) => list
            .iter()
            .filter_map(|c| c.as_object())
            .filter(|c| c.get("type").and_then(|t| t.as_str()) == Some("text"))
            .map(|c| c.get("text").and_then(|t| t.as_str()).unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

pub fn user_text(d: &Map<String, Value>, raw: bool) -> String {
    if d.get("type").and_then(|v| v.as_str()) != Some("user")
        || d.get("synthetic_reason").map(|v| !v.is_null()).unwrap_or(false)
    {
        return String::new();
    }
    let mut text = content_text(d.get("content").unwrap_or(&Value::Null));
    if let Some(m) = query_re().captures(&text) {
        text = m[1].to_string();
    } else if is_injected(&text) {
        return String::new();
    }
    if text.trim().is_empty() {
        return String::new();
    }
    if raw { text } else { clean(&text, 200) }
}

pub fn assistant_text(d: &Map<String, Value>, raw: bool) -> String {
    if d.get("type").and_then(|v| v.as_str()) != Some("assistant") {
        return String::new();
    }
    let text = content_text(d.get("content").unwrap_or(&Value::Null));
    if raw { text } else { clean(&text, 200) }
}

/// Ids of sessions a live Grok process has open.
pub fn active_sessions() -> HashSet<String> {
    let mut live = HashSet::new();
    let Ok(text) = fs::read_to_string(home().join(".grok").join("active_sessions.json")) else {
        return live;
    };
    let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(&text) else {
        return live;
    };
    for row in &rows {
        let Some(obj) = row.as_object() else { continue };
        let Some(sid) = obj.get("session_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let pid = obj.get("pid").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        if pid_alive(pid) {
            live.insert(sid.to_string());
        }
    }
    live
}

/// {session id: typed prompts, oldest first} from a cwd folder's prompt_history.jsonl.
pub fn prompt_history(cwd_dir: &Path) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let Ok(text) = fs::read_to_string(cwd_dir.join("prompt_history.jsonl")) else {
        return out;
    };
    for line in text.lines() {
        let Ok(Value::Object(d)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if d.get("is_bash").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        let Some(prompt) = d.get("prompt").and_then(|v| v.as_str()) else {
            continue;
        };
        let text = clean(prompt, 200);
        if !text.is_empty() {
            if let Some(sid) = d.get("session_id").and_then(|v| v.as_str()) {
                out.entry(sid.to_string()).or_default().push(text);
            }
        }
    }
    out
}

/// chat_history.jsonl of a session, looked up by id (the cwd folder first).
pub fn transcript(session_id: &str, cwd: &str) -> Option<PathBuf> {
    let root = sessions_root();
    if !cwd.is_empty() {
        let direct = root.join(quote(cwd)).join(session_id).join("chat_history.jsonl");
        if direct.is_file() {
            return Some(direct);
        }
    }
    paths::glob(&root, &format!("*/{}/chat_history.jsonl", session_id))
        .into_iter()
        .next()
}

pub fn parse(
    session_dir: &Path,
    live: &HashSet<String>,
    prompts: Option<&HashMap<String, Vec<String>>>,
) -> Option<Session> {
    let summary: Map<String, Value> = fs::read_to_string(session_dir.join("summary.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let empty = Map::new();
    let info = summary.get("info").and_then(|v| v.as_object()).unwrap_or(&empty);
    let sid = info
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| session_dir.file_name().unwrap_or_default().to_string_lossy().to_string());
    let cwd = info
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            unquote(
                &session_dir
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
            )
        });
    let history = session_dir.join("chat_history.jsonl");
    let (head, tail) = if history.is_file() {
        read_edges(&history)
    } else {
        (Vec::new(), Vec::new())
    };
    let owned;
    let prompt_map = match prompts {
        Some(p) => p,
        None => {
            owned = prompt_history(session_dir.parent().unwrap_or(session_dir));
            &owned
        }
    };
    let typed = prompt_map.get(&sid).cloned().unwrap_or_default();
    let users = if !typed.is_empty() {
        typed.clone()
    } else {
        head.iter().map(|d| user_text(d, false)).filter(|t| !t.is_empty()).collect::<Vec<_>>()
    };
    let mut last = if !typed.is_empty() {
        typed
    } else {
        tail.iter().map(|d| user_text(d, false)).filter(|t| !t.is_empty()).collect::<Vec<_>>()
    };
    if last.is_empty() {
        last = users.clone();
    }
    let title = summary
        .get("title")
        .and_then(|v| v.as_str())
        .filter(|t| !t.is_empty())
        .or_else(|| summary.get("generated_title").and_then(|v| v.as_str()))
        .unwrap_or("");
    if users.is_empty() && title.is_empty() && card_path(&sid).is_none_or(|p| !p.exists()) {
        return None;
    }
    let mut s = Session::new(
        "grok",
        &sid,
        &cwd,
        &session_dir.to_string_lossy(),
        summary.get("created_at").and_then(|v| v.as_str()).unwrap_or(""),
        activity(session_dir),
    );
    s.title = clean(title, 120);
    s.first_user = users.first().cloned().unwrap_or_default();
    s.last_user = last.last().cloned().unwrap_or_default();
    s.running = live.contains(&sid);
    s.auto = summary.get("session_kind").and_then(|v| v.as_str()) == Some("headless");
    Some(s)
}

pub fn scan(since_hours: f64) -> Vec<Session> {
    let root = sessions_root();
    let cutoff = now() - since_hours * 3600.0;
    let live = active_sessions();
    let mut histories: HashMap<PathBuf, HashMap<String, Vec<String>>> = HashMap::new();
    let mut out = Vec::new();
    for d in paths::glob(&root, "*/*") {
        if !d.is_dir() || activity(&d) < cutoff {
            continue;
        }
        let parent = d.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        if !histories.contains_key(&parent) {
            histories.insert(parent.clone(), prompt_history(&parent));
        }
        if let Some(s) = parse(&d, &live, histories.get(&parent)) {
            out.push(s);
        }
    }
    out
}
