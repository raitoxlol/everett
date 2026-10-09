use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::inbox::HUMAN;
use crate::session::{home, Session};

pub const HARNESSES: &[&str] = &["openai-dot", "grok-bot"];
pub const RESERVED_IDS: &[&str] = &[HUMAN, "live"];

pub fn is_external(harness: &str) -> bool {
    HARNESSES.contains(&harness)
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        && !RESERVED_IDS.iter().any(|reserved| id.eq_ignore_ascii_case(reserved))
}

#[derive(Debug, Deserialize)]
pub struct Registration {
    pub id: String,
    pub harness: String,
    pub title: String,
    pub cwd: String,
    pub updated: f64,
}

pub fn load(path: &Path) -> Option<Registration> {
    if !fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
    let record: Registration = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    if !valid_id(&record.id)
        || !is_external(&record.harness)
        || !record.updated.is_finite()
        || record.updated < 0.0
        || path.file_stem()?.to_str()? != record.id
    {
        return None;
    }
    Some(record)
}

pub fn scan(harness: &str) -> Vec<Session> {
    let folder = home().join(".everett").join("external");
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(record) = load(&path) else {
            continue;
        };
        if !harness.is_empty() && record.harness != harness {
            continue;
        }
        let mut session = Session::new(&record.harness, &record.id, &record.cwd, "", "", record.updated);
        session.title = record.title;
        session.source = "external".into();
        sessions.push(session);
    }
    sessions
}
