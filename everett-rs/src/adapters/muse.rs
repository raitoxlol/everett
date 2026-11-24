//! Muse Code sessions: `~/.local/share/muse/session-index.db` (SQLite, read-only)
//! plus per-session `session.jsonl` event logs. Muse honors XDG_DATA_HOME.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::{Map, Value};

use crate::session::{clean, home, is_injected, now, Session};
use crate::timefmt::iso_from_epoch;

pub fn data_dir() -> PathBuf {
    if let Ok(v) = std::env::var("MUSE_DATA_HOME") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Ok(v) = std::env::var("XDG_DATA_HOME") {
        if !v.is_empty() {
            return PathBuf::from(v).join("muse");
        }
    }
    home().join(".local").join("share").join("muse")
}

pub fn index_path() -> PathBuf {
    data_dir().join("session-index.db")
}

fn connect(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
}

fn sql_text(v: rusqlite::types::Value) -> String {
    match v {
        rusqlite::types::Value::Text(s) => s,
        rusqlite::types::Value::Integer(i) => i.to_string(),
        rusqlite::types::Value::Real(f) => f.to_string(),
        _ => String::new(),
    }
}

fn sql_f64(v: rusqlite::types::Value) -> f64 {
    match v {
        rusqlite::types::Value::Integer(i) => i as f64,
        rusqlite::types::Value::Real(f) => f,
        rusqlite::types::Value::Text(s) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Text of one `runtime.user_intent.accepted` record's model messages.
fn intent_text(payload: &Map<String, Value>) -> String {
    let mut parts = Vec::new();
    for source in ["model_messages", "refill_blocks"] {
        if let Some(Value::Array(items)) = payload.get(source) {
            for item in items {
                let texts: Vec<&str> = item
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map(|c| {
                        c.iter()
                            .filter(|p| p.get("kind").and_then(|k| k.as_str()) == Some("text"))
                            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                            .collect()
                    })
                    .unwrap_or_default();
                if texts.is_empty() {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        parts.push(t);
                    }
                } else {
                    parts.extend(texts);
                }
            }
        }
        if !parts.is_empty() {
            break;
        }
    }
    parts.join(" ")
}

/// Last user prompt recorded in a Muse session log, or "" when unreadable.
pub fn last_user_text(path: &Path) -> String {
    let Ok(file) = File::open(path) else { return String::new() };
    let mut last = String::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if !line.contains("user_intent.accepted") {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(&line) else { continue };
        if row.get("payload_type").and_then(|v| v.as_str()) != Some("runtime.user_intent.accepted") {
            continue;
        }
        let text = row
            .get("payload")
            .and_then(|p| p.as_object())
            .map(intent_text)
            .unwrap_or_default();
        let text = clean(&text, 200);
        if !text.is_empty() && !is_injected(&text) {
            last = text;
        }
    }
    last
}

/// `session_log_path` for a Muse session id, for the Stop hook's auto card.
pub fn transcript_path(session_id: &str) -> Option<PathBuf> {
    let con = connect(&index_path()).ok()?;
    con.query_row(
        "SELECT session_log_path FROM sessions WHERE session_id = ?1",
        [session_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .map(PathBuf::from)
    .filter(|p| p.is_file())
}

/// Card adapter hooks: user text of one session.jsonl row (auto.rs contract).
pub fn user_text(row: &Map<String, Value>, _with_context: bool) -> String {
    if row.get("payload_type").and_then(|v| v.as_str()) != Some("runtime.user_intent.accepted") {
        return String::new();
    }
    let text = row
        .get("payload")
        .and_then(|p| p.as_object())
        .map(intent_text)
        .unwrap_or_default();
    let text = clean(&text, 200);
    if text.is_empty() || is_injected(&text) {
        String::new()
    } else {
        text
    }
}

/// Muse Code has no stable assistant-text payload contract yet; cards still
/// get What/State from user intents.
pub fn assistant_text(_row: &Map<String, Value>, _with_context: bool) -> String {
    String::new()
}

pub fn scan(since_hours: f64) -> Vec<Session> {
    let path = index_path();
    if !path.is_file() {
        return Vec::new();
    }
    let con = match connect(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let cols: std::collections::HashSet<String> = {
        let mut stmt = match con.prepare("PRAGMA table_info(sessions)") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |r| r.get::<_, String>(1))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    };
    for required in ["session_id", "session_log_path"] {
        if !cols.contains(required) {
            return Vec::new();
        }
    }
    let wanted: Vec<&'static str> = [
        "session_id",
        "session_log_path",
        "workspace_root",
        "title",
        "first_user_prompt",
        "created_at_us",
        "updated_at_us",
        "msp_updated_at_us",
        "msp_last_activity_at_us",
    ]
    .into_iter()
    .filter(|c| cols.contains(*c))
    .collect();
    let active: Vec<&str> = ["updated_at_us", "msp_last_activity_at_us", "msp_updated_at_us", "created_at_us"]
        .into_iter()
        .filter(|c| cols.contains(*c))
        .collect();
    if active.is_empty() {
        return Vec::new();
    }
    let last_expr = if active.len() == 1 {
        active[0].to_string()
    } else {
        format!("COALESCE({})", active.join(", "))
    };
    let sql = format!(
        "SELECT {}, {} / 1000000.0 AS last_active FROM sessions \
         WHERE {} IS NOT NULL AND {} / 1000000.0 >= ?1 \
         ORDER BY last_active DESC",
        wanted.join(", "),
        last_expr,
        last_expr,
        last_expr
    );
    let cutoff = now() - since_hours * 3600.0;
    let mut stmt = match con.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt
        .query_map([cutoff], |row| {
            let mut map: HashMap<String, String> = HashMap::new();
            for (i, name) in wanted.iter().enumerate() {
                map.insert(name.to_string(), sql_text(row.get::<_, rusqlite::types::Value>(i)?));
            }
            map.insert("last_active".to_string(), sql_f64(row.get::<_, rusqlite::types::Value>(wanted.len())?).to_string());
            Ok(map)
        })
        .ok();
    let mut out = Vec::new();
    for row in rows.into_iter().flatten().flatten() {
        let id = row.get("session_id").cloned().unwrap_or_default();
        let log = row.get("session_log_path").cloned().unwrap_or_default();
        let cwd = row.get("workspace_root").cloned().unwrap_or_default();
        if id.is_empty() || log.is_empty() || cwd.is_empty() {
            continue;
        }
        let last_active = row
            .get("last_active")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
        let created = row
            .get("created_at_us")
            .and_then(|v| v.parse::<f64>().ok())
            .map(|us| us / 1e6)
            .unwrap_or(0.0);
        let log_path = PathBuf::from(&log);
        let mut s = Session::new(
            "muse",
            &id,
            &cwd,
            &log,
            &if created != 0.0 { iso_from_epoch(created) } else { String::new() },
            last_active,
        );
        s.title = clean(&row.get("title").cloned().unwrap_or_default(), 120);
        s.first_user = clean(&row.get("first_user_prompt").cloned().unwrap_or_default(), 200);
        s.last_user = last_user_text(&log_path);
        out.push(s);
    }
    out
}
