use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::session::{clean, home, warn, Session};

pub const SOURCE: &str = "t3code";

pub fn db_path() -> PathBuf {
    home().join(".t3").join("userdata").join("state.sqlite")
}

fn cursor_key(provider: &str) -> Option<(&'static str, &'static str)> {
    match provider {
        "codex" => Some(("codex", "threadId")),
        "claudeAgent" => Some(("claude", "resume")),
        "grok" => Some(("grok", "sessionId")),
        _ => None,
    }
}

/// {(harness, session id): (thread_id, title)} for every T3 thread with a provider session.
pub fn threads(path: &Path) -> HashMap<(String, String), (String, String)> {
    let mut out = HashMap::new();
    if !path.is_file() {
        return out;
    }
    let con = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(e) => {
            warn(&format!("skip {}: {}", path.display(), e));
            return out;
        }
    };
    let rows: Vec<(String, String, Option<String>, Option<String>)> = (|| {
        let mut stmt = con.prepare(
            "SELECT r.thread_id, r.provider_name, r.resume_cursor_json, t.title \
             FROM provider_session_runtime r LEFT JOIN projection_threads t ON t.thread_id = r.thread_id",
        ).ok()?;
        let mapped = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .ok()?;
        Some(mapped.filter_map(|r| r.ok()).collect())
    })()
    .unwrap_or_else(|| {
        warn(&format!("skip {}", path.display()));
        Vec::new()
    });
    for (thread_id, provider, cursor, title) in rows {
        let Some((harness, key)) = cursor_key(&provider) else {
            continue;
        };
        let session_id = cursor
            .as_deref()
            .unwrap_or("{}")
            .parse::<Value>()
            .ok()
            .and_then(|v| v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string()));
        if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
            out.insert((harness.to_string(), sid), (thread_id, title.unwrap_or_default()));
        }
    }
    out
}

/// Mark sessions T3 Code drives; its thread title is the fallback headline.
pub fn annotate(sessions: &mut [Session], found: Option<&HashMap<(String, String), (String, String)>>) {
    let owned;
    let found = match found {
        Some(f) => f,
        None => {
            owned = threads(&db_path());
            &owned
        }
    };
    if found.is_empty() {
        return;
    }
    for s in sessions.iter_mut() {
        let Some((_, title)) = found.get(&(s.harness.clone(), s.id.clone())) else {
            continue;
        };
        s.source = SOURCE.to_string();
        if s.title.is_empty() && !title.is_empty() {
            s.title = clean(title, 120);
        }
    }
}
