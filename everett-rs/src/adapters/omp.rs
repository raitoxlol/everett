use std::path::Path;

use serde_json::{Map, Value};

use crate::paths;
use crate::session::{clean, file_mtime, home, is_injected, read_edges, recent_files, Session};

fn user_text(d: &Map<String, Value>) -> String {
    if d.get("type").and_then(|v| v.as_str()) != Some("message") {
        return String::new();
    }
    let Some(m) = d.get("message").and_then(|m| m.as_object()) else {
        return String::new();
    };
    if m.get("role").and_then(|v| v.as_str()) != Some("user") {
        return String::new();
    }
    let text = match m.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(|c| c.as_object())
            .filter(|c| c.get("type").and_then(|t| t.as_str()) == Some("text"))
            .map(|c| c.get("text").and_then(|t| t.as_str()).unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return String::new(),
    };
    if is_injected(&text) {
        return String::new();
    }
    clean(&text, 200)
}

/// Parse an OMP or Pi session file (Pi's JSONL format, which OMP extends).
pub fn parse(path: &Path, harness: &str) -> Option<Session> {
    let (head, tail) = read_edges(path);
    let sess = head.iter().find(|d| d.get("type").and_then(|v| v.as_str()) == Some("session"))?;
    let title = head
        .iter()
        .find(|d| d.get("type").and_then(|v| v.as_str()) == Some("title"))
        .and_then(|d| d.get("title").and_then(|v| v.as_str()))
        .filter(|t| !t.is_empty())
        .or_else(|| sess.get("title").and_then(|v| v.as_str()))
        .unwrap_or("");
    let users: Vec<String> = head.iter().map(user_text).filter(|t| !t.is_empty()).collect();
    let mut last: Vec<String> = tail.iter().map(user_text).filter(|t| !t.is_empty()).collect();
    if last.is_empty() {
        last = users.clone();
    }
    let mut s = Session::new(
        harness,
        sess.get("id").and_then(|v| v.as_str()).unwrap_or(""),
        sess.get("cwd").and_then(|v| v.as_str()).unwrap_or(""),
        &path.to_string_lossy(),
        sess.get("timestamp").and_then(|v| v.as_str()).unwrap_or(""),
        file_mtime(path),
    );
    s.title = clean(title, 120);
    s.first_user = users.first().cloned().unwrap_or_default();
    s.last_user = last.last().cloned().unwrap_or_default();
    Some(s)
}

pub fn scan_root(root: &Path, harness: &str, since_hours: f64) -> Vec<Session> {
    let mut out = Vec::new();
    for p in recent_files(paths::glob(root, "*/*.jsonl"), since_hours) {
        if let Some(s) = parse(&p, harness) {
            out.push(s);
        }
    }
    out
}

pub fn scan(since_hours: f64) -> Vec<Session> {
    scan_root(&home().join(".omp").join("agent").join("sessions"), "omp", since_hours)
}
