use std::path::Path;

use serde_json::{Map, Value};

use crate::cards::card_path;
use crate::paths;
use crate::session::{clean, file_mtime, home, is_injected, read_edges, recent_files, Session};

fn content_text(d: &Map<String, Value>, types: &[&str]) -> Option<String> {
    let content = d.get("message").and_then(|m| m.get("content"))?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    let list = content.as_array()?;
    let parts: Vec<&str> = list
        .iter()
        .filter_map(|c| c.as_object())
        .filter(|c| c.get("type").and_then(|t| t.as_str()).map(|t| types.contains(&t)).unwrap_or(false))
        .map(|c| c.get("text").and_then(|t| t.as_str()).unwrap_or(""))
        .collect();
    Some(parts.join(" "))
}

pub fn user_text(d: &Map<String, Value>, raw: bool) -> String {
    if d.get("type").and_then(|v| v.as_str()) != Some("user") || d.get("isSidechain").and_then(|v| v.as_bool()).unwrap_or(false) {
        return String::new();
    }
    let Some(text) = content_text(d, &["text"]) else {
        return String::new();
    };
    if is_injected(&text) {
        return String::new();
    }
    if raw { text } else { clean(&text, 200) }
}

pub fn assistant_text(d: &Map<String, Value>, raw: bool) -> String {
    if d.get("type").and_then(|v| v.as_str()) != Some("assistant") || d.get("isSidechain").and_then(|v| v.as_bool()).unwrap_or(false) {
        return String::new();
    }
    let Some(text) = content_text(d, &["text", "output_text"]) else {
        return String::new();
    };
    if raw { text } else { clean(&text, 200) }
}

pub fn parse(path: &Path) -> Option<Session> {
    let (head, tail) = read_edges(path);
    let rows: Vec<&Map<String, Value>> = head.iter().chain(tail.iter()).collect();
    let cwd = rows.iter().find_map(|d| d.get("cwd").and_then(|v| v.as_str())).unwrap_or("").to_string();
    let sid = rows
        .iter()
        .find_map(|d| d.get("sessionId").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().to_string());
    let started = head.iter().find_map(|d| d.get("timestamp").and_then(|v| v.as_str())).unwrap_or("").to_string();
    let users: Vec<String> = head.iter().map(|d| user_text(d, false)).filter(|t| !t.is_empty()).collect();
    let mut last: Vec<String> = tail.iter().map(|d| user_text(d, false)).filter(|t| !t.is_empty()).collect();
    if last.is_empty() {
        last = users.clone();
    }
    let titles: Vec<&str> = rows
        .iter()
        .filter(|d| d.get("type").and_then(|v| v.as_str()) == Some("ai-title"))
        .filter_map(|d| d.get("aiTitle").and_then(|v| v.as_str()))
        .collect();
    if users.is_empty() && last.is_empty() && titles.is_empty() && !card_path(&sid).exists() {
        return None;
    }
    let auto = head.iter().any(|d| d.get("entrypoint").and_then(|v| v.as_str()) == Some("sdk-cli"));
    let mut s = Session::new("claude", &sid, &cwd, &path.to_string_lossy(), &started, file_mtime(path));
    s.first_user = users.first().cloned().unwrap_or_default();
    s.last_user = last.last().cloned().unwrap_or_default();
    s.auto = auto;
    s.title = titles.last().map(|t| clean(t, 120)).unwrap_or_default();
    Some(s)
}

pub fn scan(since_hours: f64) -> Vec<Session> {
    let root = home().join(".claude").join("projects");
    let mut out = Vec::new();
    for p in recent_files(paths::glob(&root, "*/*.jsonl"), since_hours) {
        if let Some(s) = parse(&p) {
            out.push(s);
        }
    }
    out
}
