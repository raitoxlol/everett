use std::collections::HashSet;
use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};

use crate::paths;
use crate::session::{clean, home, is_injected, now, warn, Session};
use crate::timefmt::iso_from_epoch;

pub const INTERACTIVE: &[&str] = &["cli", "tui", "desktop", "webui"];
pub const AUTOMATED: &[&str] = &["cron", "oneshot", "webhook", "kanban", "delegate", "subagent"];

pub fn databases() -> Vec<(String, PathBuf)> {
    let root = home().join(".hermes");
    let mut found = Vec::new();
    if root.join("state.db").is_file() {
        found.push(("default".to_string(), root.join("state.db")));
    }
    let mut profiles = paths::glob(&root, "profiles/*/state.db");
    profiles.sort();
    for p in profiles {
        if p.is_file() {
            let name = p
                .parent()
                .and_then(|d| d.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            found.push((name, p));
        }
    }
    found
}

fn connect(path: &PathBuf) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
}

fn columns(con: &Connection, table: &str) -> HashSet<String> {
    let mut stmt = match con.prepare(&format!("PRAGMA table_info({})", table)) {
        Ok(s) => s,
        Err(_) => return HashSet::new(),
    };
    stmt.query_map([], |r| r.get::<_, String>(1))
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
}

use rusqlite::types::Value as SqlValue;

fn sql_text(v: &SqlValue) -> String {
    match v {
        SqlValue::Text(s) => s.clone(),
        SqlValue::Integer(i) => i.to_string(),
        _ => String::new(),
    }
}

fn sql_f64(v: &SqlValue) -> f64 {
    match v {
        SqlValue::Real(f) => *f,
        SqlValue::Integer(i) => *i as f64,
        _ => 0.0,
    }
}

fn text(content: &SqlValue) -> String {
    let s = sql_text(content);
    if s.is_empty() || is_injected(&s) {
        return String::new();
    }
    clean(&s, 200)
}

pub fn read_db(profile: &str, path: &PathBuf, since_hours: f64) -> Result<Vec<Session>, String> {
    let con = connect(path).map_err(|e| e.to_string())?;
    let cols = columns(&con, "sessions");
    for required in ["id", "source", "started_at"] {
        if !cols.contains(required) {
            return Ok(Vec::new());
        }
    }
    let mut active: Vec<&str> = ["last_activity_at", "ended_at"]
        .into_iter()
        .filter(|c| cols.contains(*c))
        .collect();
    active.push("started_at");
    let last_expr = format!("COALESCE({})", active.join(", "));
    let wanted: Vec<&str> = ["id", "source", "started_at", "cwd", "title", "profile_name"]
        .into_iter()
        .filter(|c| cols.contains(*c))
        .collect();
    let mut where_clause = format!("{} >= ?1", last_expr);
    for flag in ["hidden", "archived"] {
        if cols.contains(flag) {
            where_clause += &format!(" AND COALESCE({}, 0) = 0", flag);
        }
    }
    if cols.contains("parent_session_id") {
        where_clause += " AND parent_session_id IS NULL";
    }
    let sql = format!(
        "SELECT {}, {} AS last_active FROM sessions WHERE {} ORDER BY last_active DESC LIMIT 200",
        wanted.join(", "),
        last_expr,
        where_clause
    );
    let cutoff = now() - since_hours * 3600.0;
    let mut stmt = con.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([cutoff], |row| {
            let mut map = std::collections::HashMap::new();
            for (i, name) in wanted.iter().enumerate() {
                map.insert(name.to_string(), row.get::<_, SqlValue>(i)?);
            }
            map.insert("last_active".to_string(), row.get::<_, SqlValue>(wanted.len())?);
            Ok(map)
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows.flatten() {
        let id = row.get("id").map(sql_text).unwrap_or_default();
        let get_text = |k: &str| -> String { row.get(k).map(sql_text).unwrap_or_default() };
        let mut users: Vec<String> = con
            .prepare("SELECT content FROM messages WHERE session_id = ?1 AND role = 'user' ORDER BY id LIMIT 5")
            .and_then(|mut st| {
                st.query_map([&id], |r| r.get::<_, SqlValue>(0))
                    .map(|rows| {
                        rows.filter_map(|r| r.ok())
                            .map(|v| text(&v))
                            .filter(|t| !t.is_empty())
                            .collect::<Vec<String>>()
                    })
            })
            .unwrap_or_default();
        users.retain(|u| !u.is_empty());
        let last: String = con
            .prepare("SELECT content FROM messages WHERE session_id = ?1 AND role = 'user' ORDER BY id DESC LIMIT 1")
            .and_then(|mut st| st.query_row([&id], |r| r.get::<_, SqlValue>(0)))
            .map(|v| text(&v))
            .unwrap_or_default();
        let source = get_text("source");
        let started_at = row.get("started_at").map(sql_f64).unwrap_or(0.0);
        let last_active = row.get("last_active").map(sql_f64).unwrap_or(0.0);
        let mut s = Session::new(
            "hermes",
            &id,
            &get_text("cwd"),
            &path.to_string_lossy(),
            &if started_at != 0.0 { iso_from_epoch(started_at) } else { String::new() },
            last_active,
        );
        s.title = clean(&get_text("title"), 120);
        s.first_user = users.first().cloned().unwrap_or_default();
        s.last_user = last;
        s.auto = AUTOMATED.contains(&source.as_str());
        let prof = get_text("profile_name");
        s.profile = if prof.is_empty() { profile.to_string() } else { prof };
        s.source = source;
        out.push(s);
    }
    Ok(out)
}

pub fn scan(since_hours: f64) -> Vec<Session> {
    let mut out = Vec::new();
    for (profile, path) in databases() {
        match read_db(&profile, &path, since_hours) {
            Ok(sessions) => out.extend(sessions),
            Err(e) => warn(&format!("skip {}: {}", path.display(), e)),
        }
    }
    out
}
