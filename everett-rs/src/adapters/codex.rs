use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Value};

use crate::paths;
use crate::session::{clean, file_mtime, home, is_injected, read_edges, recent_files, Session};

fn payload(d: &Map<String, Value>) -> Option<&Map<String, Value>> {
    if d.get("type").and_then(|v| v.as_str()) != Some("response_item") {
        return None;
    }
    d.get("payload").and_then(|p| p.as_object())
}

fn message_text(d: &Map<String, Value>, role: &str, only_text_parts: bool) -> String {
    let Some(p) = payload(d) else { return String::new() };
    if p.get("type").and_then(|v| v.as_str()) != Some("message")
        || p.get("role").and_then(|v| v.as_str()) != Some(role)
    {
        return String::new();
    }
    p.get("content")
        .and_then(|c| c.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|c| c.as_object())
                .filter(|c| {
                    !only_text_parts
                        || c.get("type")
                            .and_then(|t| t.as_str())
                            .map(|t| t == "text" || t == "output_text")
                            .unwrap_or(false)
                })
                .map(|c| c.get("text").and_then(|t| t.as_str()).unwrap_or(""))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

pub fn user_text(d: &Map<String, Value>, raw: bool) -> String {
    let text = message_text(d, "user", false);
    if is_injected(&text) {
        return String::new();
    }
    if raw { text } else { clean(&text, 200) }
}

pub fn assistant_text(d: &Map<String, Value>, raw: bool) -> String {
    let text = message_text(d, "assistant", true);
    if raw { text } else { clean(&text, 200) }
}

/// Codex Desktop thread names from session_index.jsonl (later lines win).
pub fn thread_names() -> HashMap<String, String> {
    let mut names = HashMap::new();
    let Ok(text) = fs::read_to_string(home().join(".codex").join("session_index.jsonl")) else {
        return names;
    };
    for line in text.lines() {
        let Ok(Value::Object(d)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let (Some(id), Some(name)) = (
            d.get("id").and_then(|v| v.as_str()),
            d.get("thread_name").and_then(|v| v.as_str()),
        ) {
            names.insert(id.to_string(), name.trim_start_matches(['✅', ' ']).trim().to_string());
        }
    }
    names
}

pub fn parse(path: &Path, names: &HashMap<String, String>) -> Option<Session> {
    let (head, tail) = read_edges(path);
    let meta = head
        .iter()
        .find(|d| d.get("type").and_then(|v| v.as_str()) == Some("session_meta"))
        .and_then(|d| d.get("payload").and_then(|p| p.as_object()))?;
    let users: Vec<String> = head.iter().map(|d| user_text(d, false)).filter(|t| !t.is_empty()).collect();
    let mut last: Vec<String> = tail.iter().map(|d| user_text(d, false)).filter(|t| !t.is_empty()).collect();
    if last.is_empty() {
        last = users.clone();
    }
    let src = meta.get("source");
    let auto = src.and_then(|v| v.as_str()) == Some("exec")
        || src.and_then(|v| v.as_object()).map(|o| o.contains_key("subagent")).unwrap_or(false);
    let sid = meta
        .get("id")
        .and_then(|v| v.as_str())
        .or_else(|| meta.get("session_id").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .or_else(|| {
            // `codex exec resume` needs the uuid, not the rollout-<ts>-<uuid> filename.
            static RE: OnceLock<Regex> = OnceLock::new();
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            RE.get_or_init(|| {
                Regex::new(r"([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})$")
                    .unwrap()
            })
            .captures(&stem)
            .map(|c| c[1].to_string())
        })?;
    let mut s = Session::new(
        "codex",
        &sid,
        meta.get("cwd").and_then(|v| v.as_str()).unwrap_or(""),
        &path.to_string_lossy(),
        meta.get("timestamp").and_then(|v| v.as_str()).unwrap_or(""),
        file_mtime(path),
    );
    s.first_user = users.first().cloned().unwrap_or_default();
    s.last_user = last.last().cloned().unwrap_or_default();
    s.auto = auto;
    s.title = names.get(&sid).map(|t| clean(t, 120)).unwrap_or_default();
    if meta
        .get("originator")
        .and_then(|v| v.as_str())
        .map(|o| o.starts_with("t3code"))
        .unwrap_or(false)
    {
        s.source = "t3code".to_string();
    }
    Some(s)
}

pub fn scan(since_hours: f64) -> Vec<Session> {
    let root = home().join(".codex").join("sessions");
    let names = thread_names();
    let mut out = Vec::new();
    for p in recent_files(paths::glob(&root, "*/*/*/*.jsonl"), since_hours) {
        if let Some(s) = parse(&p, &names) {
            out.push(s);
        }
    }
    out
}
