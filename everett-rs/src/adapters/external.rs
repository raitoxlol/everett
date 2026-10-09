use std::fs;
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::inbox::HUMAN;
use crate::session::{home, now, Session};

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

#[derive(Debug, Deserialize, Serialize)]
pub struct Registration {
    pub id: String,
    pub harness: String,
    pub title: String,
    pub cwd: String,
    pub updated: f64,
}

fn directory() -> PathBuf {
    home().join(".everett").join("external")
}

fn path(id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err(format!("Invalid or reserved external id: {id:?}"));
    }
    Ok(directory().join(format!("{id}.json")))
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

pub fn records() -> Vec<Registration> {
    let Ok(entries) = fs::read_dir(directory()) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(record) = load(&path) else {
            continue;
        };
        records.push(record);
    }
    records.sort_by(|a, b| a.id.cmp(&b.id));
    records
}

pub fn register(
    id: &str,
    harness: &str,
    title: &str,
    cwd: &str,
    replace: bool,
) -> Result<Registration, String> {
    let target = path(id)?;
    if !is_external(harness) {
        return Err(format!("External harness must be one of {}.", HARNESSES.join(", ")));
    }
    if replace {
        let previous = load(&target).ok_or("Existing registration must be a valid regular file.")?;
        if previous.harness != harness {
            return Err("Remove the existing registration before changing its harness.".into());
        }
    }
    let record = Registration {
        id: id.into(),
        harness: harness.into(),
        title: title.into(),
        cwd: cwd.into(),
        updated: now(),
    };
    let folder = directory();
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&folder).map_err(|e| e.to_string())?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".registration-")
        .tempfile_in(&folder)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut temporary, &record).map_err(|e| e.to_string())?;
    temporary.write_all(b"\n").map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    if replace {
        temporary.persist(&target).map_err(|e| e.error.to_string())?;
    } else {
        temporary.persist_noclobber(&target).map_err(|e| e.error.to_string())?;
    }
    Ok(record)
}

pub fn remove(id: &str) -> Result<(), String> {
    fs::remove_file(path(id)?).map_err(|e| e.to_string())
}

pub fn scan(harness: &str) -> Vec<Session> {
    let mut sessions = Vec::new();
    for record in records() {
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
