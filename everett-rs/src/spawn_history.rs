use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::session::{home, now};

fn path() -> PathBuf {
    home().join(".everett").join("spawned.jsonl")
}

/// Sessions Everett started headless: shown like interactive ones.
pub(crate) fn ids() -> HashSet<String> {
    let mut ids = HashSet::new();
    let Ok(text) = fs::read_to_string(path()) else {
        return ids;
    };
    for line in text.lines() {
        if let Ok(Value::Object(d)) = serde_json::from_str::<Value>(line) {
            if let Some(id) = d.get("id").and_then(|v| v.as_str()) {
                ids.insert(id.to_string());
            }
        }
    }
    ids
}

/// Remember sessions Everett started, so ls/route show them although they ran headless.
pub(crate) fn record(harness: &str, session_id: &str, cwd: &str, text: &str) {
    let path = path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let text: String = text.chars().take(200).collect();
        let line = serde_json::to_string(&json!({
            "ts": now(),
            "harness": harness,
            "id": session_id,
            "cwd": cwd,
            "text": text,
        }))
        .unwrap();
        let _ = f.write_all((line + "\n").as_bytes());
    }
}
