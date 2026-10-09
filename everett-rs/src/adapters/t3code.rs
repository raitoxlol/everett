//! Read-only projections from pingdotgg/t3code 43f8a8de17a7ac1baa7a3cf36d681856de2d8add.
//! V2 fields follow Migrations/055_OrchestrationV2.ts and contracts/src/orchestrationV2.ts.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::session::{clean, home, now, warn, Session};
use crate::timefmt::epoch_from_iso;

pub const SOURCE: &str = "t3code";
pub type ThreadMap = HashMap<(String, String), (String, String)>;

pub fn db_path() -> PathBuf {
    let state = home().join(".t3").join("userdata");
    let current = state.join("statev2.sqlite");
    if current.is_file() {
        current
    } else {
        state.join("state.sqlite")
    }
}

fn cursor_key(provider: &str) -> Option<(&'static str, &'static str)> {
    match provider {
        "codex" => Some(("codex", "threadId")),
        "claudeAgent" => Some(("claude", "resume")),
        "grok" => Some(("grok", "sessionId")),
        _ => None,
    }
}

struct Thread {
    id: String,
    session: Session,
}

fn columns(con: &Connection, table: &str) -> rusqlite::Result<HashSet<String>> {
    let mut stmt = con.prepare(&format!("PRAGMA table_info({})", table))?;
    let rows = stmt.query_map([], |r| r.get(1))?;
    rows.collect()
}

fn prompts(con: &Connection, thread_id: &str, v2: bool) -> rusqlite::Result<(String, String)> {
    let table = if v2 {
        "orchestration_v2_projection_messages"
    } else {
        "projection_thread_messages"
    };
    if columns(con, table)?.is_empty() {
        return Ok((String::new(), String::new()));
    }
    let field = if v2 { "payload_json" } else { "text" };
    let mut texts = Vec::new();
    for order in ["ASC", "DESC"] {
        let mut stmt = con.prepare(&format!(
            "SELECT {} FROM {} WHERE thread_id=?1 AND role='user' \
             ORDER BY created_at {}, message_id {} LIMIT 1",
            field, table, order, order
        ))?;
        let mut rows = stmt.query([thread_id])?;
        let text = match rows.next()? {
            Some(row) => {
                let raw: String = row.get(0)?;
                if v2 {
                    serde_json::from_str::<Value>(&raw)
                        .ok()
                        .and_then(|v| v.get("text").and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_default()
                } else {
                    raw
                }
            }
            None => String::new(),
        };
        texts.push(clean(&text, 200));
    }
    Ok((texts.remove(0), texts.remove(0)))
}

fn read(con: &Connection, path: &Path) -> rusqlite::Result<HashMap<(String, String), Thread>> {
    let v2 = !columns(con, "orchestration_v2_projection_threads")?.is_empty();
    let table = if v2 {
        "orchestration_v2_projection_threads"
    } else {
        "projection_threads"
    };
    let cols = columns(con, table)?;
    let projects = columns(con, "projection_projects")?;
    let workspace = if projects.contains("workspace_root") {
        "COALESCE(j.workspace_root, '')"
    } else {
        "''"
    };
    let project_join = if projects.is_empty() {
        ""
    } else {
        "LEFT JOIN projection_projects j ON j.project_id=t.project_id"
    };
    let project_filter = if projects.contains("deleted_at") {
        "AND j.deleted_at IS NULL"
    } else {
        ""
    };
    let archive_filter = if cols.contains("archived_at") {
        "AND t.archived_at IS NULL"
    } else {
        ""
    };
    let sql = if v2 {
        format!(
            "SELECT t.thread_id, p.driver, p.payload_json, t.title, t.created_at, t.updated_at, \
             t.payload_json, {} FROM {} t \
             JOIN orchestration_v2_projection_provider_threads p ON p.provider_thread_id=t.active_provider_thread_id \
             {} WHERE t.deleted_at IS NULL {} {} ORDER BY t.updated_at DESC, t.thread_id",
            workspace, table, project_join, archive_filter, project_filter
        )
    } else {
        let worktree = if cols.contains("worktree_path") {
            "COALESCE(t.worktree_path, '')"
        } else {
            "''"
        };
        format!(
            "SELECT t.thread_id, r.provider_name, COALESCE(r.resume_cursor_json, '{{}}'), \
             t.title, t.created_at, t.updated_at, {}, {} \
             FROM projection_threads t JOIN provider_session_runtime r ON r.thread_id=t.thread_id \
             {} WHERE t.deleted_at IS NULL {} {} ORDER BY t.updated_at DESC, t.thread_id",
            worktree, workspace, project_join, archive_filter, project_filter
        )
    };
    let mut stmt = con.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
        ))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let Ok((thread_id, provider, cursor, title, started, updated, worktree, workspace)) = row
        else {
            continue;
        };
        let Some((harness, key)) = cursor_key(&provider) else {
            continue;
        };
        let data = serde_json::from_str::<Value>(&cursor).unwrap_or(Value::Null);
        let sid = if v2 {
            let Some(reference) = data.get("nativeThreadRef") else {
                continue;
            };
            if reference.get("driver").and_then(Value::as_str) != Some(provider.as_str()) {
                continue;
            }
            reference.get("nativeId").and_then(Value::as_str)
        } else {
            data.get(key).and_then(Value::as_str)
        };
        let Some(sid) = sid.map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let cwd = if v2 {
            serde_json::from_str::<Value>(&worktree)
                .ok()
                .and_then(|v| {
                    v.get("worktreePath")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .filter(|s| !s.is_empty())
                .unwrap_or(workspace)
        } else if worktree.is_empty() {
            workspace
        } else {
            worktree
        };
        let (first, last) = prompts(con, &thread_id, v2)?;
        let mut session = Session::new(
            harness,
            sid,
            &cwd,
            &path.to_string_lossy(),
            &started,
            epoch_from_iso(&updated).unwrap_or(0.0),
        );
        session.title = clean(&title, 120);
        session.source = SOURCE.into();
        session.first_user = first;
        session.last_user = last;
        out.entry((harness.to_string(), sid.to_string()))
            .or_insert(Thread {
                id: thread_id,
                session,
            });
    }
    Ok(out)
}

fn read_threads(path: &Path) -> HashMap<(String, String), Thread> {
    if !path.is_file() {
        return HashMap::new();
    }
    let result = (|| {
        let con = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        con.busy_timeout(Duration::from_secs(1))?;
        read(&con, path)
    })();
    result.unwrap_or_else(|e| {
        warn(&format!("skip {}: {}", path.display(), e));
        HashMap::new()
    })
}

/// Active T3 metadata keyed by harness/native ID, never by provider instance ID.
pub fn threads(path: &Path) -> ThreadMap {
    read_threads(path)
        .into_iter()
        .map(|(key, thread)| (key, (thread.id, thread.session.title)))
        .collect()
}

/// Mark sessions T3 Code drives; its thread title is the fallback headline.
pub fn annotate(sessions: &mut [Session], found: Option<&ThreadMap>) {
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

pub fn overlay(sessions: &mut Vec<Session>, since_hours: f64, harness: &str) {
    let cutoff = now() - since_hours * 3600.0;
    for (_, thread) in read_threads(&db_path()) {
        let t = thread.session;
        if !harness.is_empty() && t.harness != harness {
            continue;
        }
        if let Some(s) = sessions
            .iter_mut()
            .find(|s| s.harness == t.harness && s.id == t.id)
        {
            s.source = SOURCE.into();
            if s.title.is_empty() {
                s.title = t.title
            }
            if t.last_active >= cutoff {
                s.last_active = s.last_active.max(t.last_active);
                if s.cwd.is_empty() {
                    s.cwd = t.cwd
                }
                if s.first_user.is_empty() {
                    s.first_user = t.first_user
                }
                if s.last_user.is_empty() {
                    s.last_user = t.last_user
                }
            }
        } else if t.last_active >= cutoff {
            sessions.push(t)
        }
    }
}
