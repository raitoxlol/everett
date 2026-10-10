use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{types::Value as SqlValue, Connection, OpenFlags};
use serde_json::{Map, Value};

use crate::paths;
use crate::session::{clean, home, is_injected, now, warn, Session};
use crate::timefmt::{epoch_from_any, epoch_from_iso, iso_from_epoch};

#[cfg(target_os = "macos")]
pub const DEFAULT_REL: &str = "Library/Application Support/devin/cli";
#[cfg(target_os = "windows")]
pub const DEFAULT_REL: &str = "AppData/Roaming/devin/cli";
#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
pub const DEFAULT_REL: &str = ".local/share/devin/cli";

/// Primary data dir: $DEVIN_HOME when set, else the platform default.
pub fn data_dir() -> PathBuf {
    if let Ok(override_dir) = std::env::var("DEVIN_HOME") {
        if !override_dir.is_empty() {
            return paths::expanduser(&override_dir);
        }
    }
    home().join(DEFAULT_REL)
}

/// Existing candidate dirs: $DEVIN_HOME, the platform default, and the XDG path.
pub fn data_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for cand in [
        std::env::var("DEVIN_HOME").unwrap_or_default(),
        home().join(DEFAULT_REL).to_string_lossy().to_string(),
        home().join(".local/share/devin/cli").to_string_lossy().to_string(),
    ] {
        if cand.is_empty() {
            continue;
        }
        let p = paths::expanduser(&cand);
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

fn connect(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
}

fn columns(con: &Connection, table: &str) -> HashSet<String> {
    con.prepare(&format!("PRAGMA table_info({})", table))
        .and_then(|mut stmt| {
            stmt.query_map([], |r| r.get::<_, String>(1))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
        })
        .unwrap_or_default()
}

fn tables(con: &Connection) -> HashSet<String> {
    con.prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .and_then(|mut stmt| {
            stmt.query_map([], |r| r.get::<_, String>(0))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
        })
        .unwrap_or_default()
}

fn epoch(v: &SqlValue) -> f64 {
    let x = match v {
        SqlValue::Integer(i) => *i as f64,
        SqlValue::Real(f) => *f,
        SqlValue::Text(s) => match epoch_from_iso(s) {
            Some(ts) => return ts,
            None => s.trim().parse::<f64>().unwrap_or(0.0),
        },
        _ => return 0.0,
    };
    if x > 1e12 { x / 1000.0 } else { x }
}

fn sql_text(v: &SqlValue) -> String {
    match v {
        SqlValue::Text(s) => s.clone(),
        SqlValue::Integer(i) => i.to_string(),
        SqlValue::Real(f) => f.to_string(),
        _ => String::new(),
    }
}

/// ATIF message content: a plain string or a list of ContentPart objects.
fn parts_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(list) => list
            .iter()
            .filter_map(|p| p.as_object())
            .filter_map(|p| p.get("text"))
            .map(|t| match t {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn json_value(raw: &Value) -> Option<Map<String, Value>> {
    match raw {
        Value::String(s) => serde_json::from_str::<Value>(s).ok().and_then(|v| v.as_object().cloned()),
        Value::Object(o) => Some(o.clone()),
        _ => None,
    }
}

/// A node/step plus its nested chat_message/message payloads, if any.
fn layers(obj: &Map<String, Value>) -> Vec<Map<String, Value>> {
    let mut out = vec![obj.clone()];
    for key in ["chat_message", "message"] {
        if let Some(nested) = obj.get(key).and_then(json_value) {
            out.push(nested);
        }
    }
    out
}

fn is_user(obj: &Map<String, Value>) -> bool {
    layers(obj).iter().any(|layer| {
        layer.get("source").and_then(|v| v.as_str()) == Some("user")
            || layer.get("role").and_then(|v| v.as_str()) == Some("user")
            || layer
                .get("metadata")
                .and_then(|m| m.get("is_user_input"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
    })
}

fn node_text(obj: &Map<String, Value>) -> String {
    for layer in layers(obj) {
        for key in ["content", "text", "message"] {
            let text = parts_text(layer.get(key).unwrap_or(&Value::Null));
            if !text.trim().is_empty() {
                return text;
            }
        }
    }
    String::new()
}

/// First and last user texts in message_nodes; the schema shifts between CLI builds,
/// so columns are probed.
fn users(con: &Connection, cols: &HashSet<String>, session_id: &str) -> (String, String) {
    let sid = ["session_id", "session"].iter().find(|c| cols.contains(**c));
    let body = ["chat_message", "message", "content", "data"].iter().find(|c| cols.contains(**c));
    let order = ["node_id", "created_at", "id"].iter().find(|c| cols.contains(**c));
    let (Some(sid), Some(body), Some(order)) = (sid, body, order) else {
        return (String::new(), String::new());
    };
    let nearest = |desc: bool| -> String {
        let sql = format!(
            "SELECT {} AS body FROM message_nodes WHERE {} = ?1 ORDER BY {} {} LIMIT 1000",
            body,
            sid,
            order,
            if desc { "DESC" } else { "ASC" }
        );
        let Ok(mut stmt) = con.prepare(&sql) else {
            return String::new();
        };
        let Ok(rows) = stmt.query_map([session_id], |r| r.get::<_, SqlValue>(0)) else {
            return String::new();
        };
        for r in rows.flatten() {
            let raw = sql_text(&r);
            let Some(obj) = serde_json::from_str::<Value>(&raw)
                .ok()
                .and_then(|v| v.as_object().cloned())
            else {
                continue;
            };
            if is_user(&obj) {
                let text = node_text(&obj);
                if !text.is_empty() && !is_injected(&text) {
                    return clean(&text, 200);
                }
            }
        }
        String::new()
    };
    (nearest(false), nearest(true))
}

/// Sessions from sessions.db; the bool says whether message_nodes was readable
/// (when it is, transcripts stay unread so the two never double count).
pub fn read_db(path: &Path, since_hours: f64) -> Result<(Vec<Session>, bool), String> {
    let con = connect(path).map_err(|e| e.to_string())?;
    let cols = columns(&con, "sessions");
    if !cols.contains("id") {
        return Ok((Vec::new(), false));
    }
    let have_nodes = tables(&con).contains("message_nodes");
    let node_cols = if have_nodes { columns(&con, "message_nodes") } else { HashSet::new() };
    let wanted: Vec<&str> = [
        "id",
        "working_directory",
        "cwd",
        "title",
        "model",
        "created_at",
        "last_activity_at",
        "updated_at",
    ]
    .into_iter()
    .filter(|c| cols.contains(*c))
    .collect();
    let clauses: Vec<String> = ["hidden", "archived"]
        .iter()
        .filter(|f| cols.contains(**f))
        .map(|f| format!("COALESCE({}, 0) = 0", f))
        .collect();
    let where_clause = if clauses.is_empty() { "1=1".to_string() } else { clauses.join(" AND ") };
    let activity: Vec<&str> = ["last_activity_at", "updated_at", "created_at"]
        .iter()
        .copied()
        .filter(|c| cols.contains(*c))
        .collect();
    let order = if activity.is_empty() {
        String::new()
    } else {
        format!(" ORDER BY COALESCE({}) DESC", activity.join(", "))
    };
    let sql = format!(
        "SELECT {} FROM sessions WHERE {}{} LIMIT 400",
        wanted.join(", "),
        where_clause,
        order
    );
    let mut stmt = con.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let mut map = HashMap::new();
            for (i, name) in wanted.iter().enumerate() {
                map.insert(name.to_string(), row.get::<_, SqlValue>(i)?);
            }
            Ok(map)
        })
        .map_err(|e| e.to_string())?;
    let cutoff = now() - since_hours * 3600.0;
    let mut out = Vec::new();
    for row in rows.flatten() {
        let last_ts = ["last_activity_at", "updated_at", "created_at"]
            .iter()
            .filter_map(|c| row.get(*c))
            .map(epoch)
            .fold(0.0, f64::max);
        if last_ts != 0.0 && last_ts < cutoff {
            continue;
        }
        let id = row.get("id").map(sql_text).unwrap_or_default();
        let (first, last) = if have_nodes { users(&con, &node_cols, &id) } else { (String::new(), String::new()) };
        let transcript = path.parent().unwrap_or(path).join("transcripts").join(format!("{}.json", id));
        let mut cwd = String::new();
        for key in ["working_directory", "cwd"] {
            if let Some(v) = row.get(key) {
                let t = sql_text(v);
                if !t.is_empty() {
                    cwd = t;
                    break;
                }
            }
        }
        let mut s = Session::new(
            "devin",
            &id,
            &cwd,
            &if transcript.is_file() {
                transcript.to_string_lossy().to_string()
            } else {
                path.to_string_lossy().to_string()
            },
            &row
                .get("created_at")
                .map(|v| epoch(v))
                .map(|ts| if ts != 0.0 { iso_from_epoch(ts) } else { String::new() })
                .unwrap_or_default(),
            last_ts,
        );
        s.title = row.get("title").map(|v| clean(&sql_text(v), 120)).unwrap_or_default();
        s.first_user = first;
        s.last_user = last;
        s.source = "cli".to_string();
        out.push(s);
    }
    Ok((out, have_nodes))
}

/// One ATIF transcript file -> Session, for when sessions.db is missing or too old
/// to carry message_nodes.
pub fn read_transcript(path: &Path) -> Option<Session> {
    let obj: Map<String, Value> = fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())?;
    let steps = obj.get("steps")?.as_array()?;
    let mut users: Vec<String> = Vec::new();
    let mut stamps: Vec<f64> = Vec::new();
    for step in steps {
        let Some(step) = step.as_object() else { continue };
        let ts = step
            .get("metadata")
            .and_then(|m| m.get("created_at"))
            .map(epoch_from_any)
            .filter(|t| *t != 0.0)
            .or_else(|| step.get("created_at").map(epoch_from_any).filter(|t| *t != 0.0))
            .unwrap_or(0.0);
        if ts != 0.0 {
            stamps.push(ts);
        }
        if is_user(step) {
            let text = node_text(step);
            if !text.is_empty() && !is_injected(&text) {
                users.push(clean(&text, 200));
            }
        }
    }
    let empty = Map::new();
    let agent = obj.get("agent").and_then(|v| v.as_object()).unwrap_or(&empty);
    let extra = agent.get("extra").and_then(|v| v.as_object()).unwrap_or(&empty);
    let mut cwd = String::new();
    for key in ["working_directory", "cwd", "workdir"] {
        if let Some(v) = extra.get(key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                cwd = v.to_string();
                break;
            }
        }
    }
    let last_ts = stamps.iter().cloned().fold(0.0, f64::max);
    let started = stamps.iter().cloned().fold(f64::INFINITY, f64::min);
    let mut s = Session::new(
        "devin",
        &obj.get("session_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().to_string()),
        &cwd,
        &path.to_string_lossy(),
        &if started.is_finite() { iso_from_epoch(started) } else { String::new() },
        last_ts,
    );
    s.title = obj
        .get("title")
        .and_then(|v| v.as_str())
        .map(|t| clean(t, 120))
        .unwrap_or_default();
    s.first_user = users.first().cloned().unwrap_or_default();
    s.last_user = users.last().cloned().unwrap_or_default();
    s.source = "cli".to_string();
    Some(s)
}

pub fn scan(since_hours: f64) -> Vec<Session> {
    let cutoff = now() - since_hours * 3600.0;
    let mut by_id: HashMap<String, Session> = HashMap::new();
    for base in data_dirs() {
        let mut sessions = Vec::new();
        let mut have_nodes = false;
        let db = base.join("sessions.db");
        if db.is_file() {
            match read_db(&db, since_hours) {
                Ok((s, h)) => {
                    sessions = s;
                    have_nodes = h;
                }
                Err(e) => warn(&format!("skip {}: {}", db.display(), e)),
            }
        }
        for s in sessions {
            by_id.entry(s.id.clone()).or_insert(s);
        }
        if !have_nodes {
            let mut transcripts = paths::glob(&base.join("transcripts"), "*.json");
            transcripts.sort();
            for path in transcripts {
                let Some(s) = read_transcript(&path) else { continue };
                if let Some(known) = by_id.get_mut(&s.id) {
                    if known.first_user.is_empty() {
                        known.first_user = s.first_user.clone();
                    }
                    if known.last_user.is_empty() {
                        known.last_user = s.last_user.clone();
                    }
                    if known.cwd.is_empty() {
                        known.cwd = s.cwd.clone();
                    }
                    if known.title.is_empty() {
                        known.title = s.title.clone();
                    }
                    if known.started.is_empty() {
                        known.started = s.started.clone();
                    }
                    if known.last_active == 0.0 {
                        known.last_active = s.last_active;
                    }
                    known.path = s.path.clone();
                } else if s.last_active == 0.0 || s.last_active >= cutoff {
                    by_id.insert(s.id.clone(), s);
                }
            }
        }
    }
    let mut out: Vec<Session> = by_id.into_values().collect();
    out.sort_by(|a, b| b.last_active.partial_cmp(&a.last_active).unwrap_or(std::cmp::Ordering::Equal));
    out
}
