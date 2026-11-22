//! Label authorized send destinations with readable context.
//!
//! A gateway binding allowlists exact destination IDs. Remote clients receive
//! only this labeled view of those IDs: never the owner's full session list.

use serde_json::{json, Map, Value};

use crate::adapters::{external, t3code};
use crate::session::{now, project_label, Session};

/// The window gateway sends and directory listings both resolve over. Inbox
/// delivery is durable, so it is wider than the interactive default.
pub(crate) const SCAN_HOURS: f64 = 24.0 * 30.0;

/// The T3 thread a bare thread ID drives: (harness, provider session, title).
fn t3_thread(id: &str) -> Option<(String, String, String)> {
    t3code::threads(&t3code::db_path())
        .into_iter()
        .find(|(_, (thread, _))| thread == id)
        .map(|((harness, sid), (_, title))| (harness, sid, title))
}

/// The local inbox a `to` address reaches. A bare T3 thread ID reaches its
/// provider session's inbox; anything else already names an inbox.
pub(crate) fn queue_target(to: &str) -> String {
    t3_thread(to)
        .map(|(_, sid, _)| sid)
        .unwrap_or_else(|| to.to_string())
}

/// A readable label: the title, else the card's What line, else the request.
fn label(s: &Session) -> String {
    if !s.title.is_empty() {
        return s.title.chars().take(120).collect();
    }
    let what = s
        .card
        .split("What:")
        .nth(1)
        .unwrap_or("")
        .split("State:")
        .next()
        .unwrap_or("")
        .trim();
    if !what.is_empty() {
        return what.chars().take(120).collect();
    }
    s.first_user.chars().take(80).collect()
}

fn entry(id: &str) -> Map<String, Value> {
    Map::from_iter([("id".into(), json!(id)), ("host".into(), json!("local"))])
}

fn session_entry(s: &Session) -> Map<String, Value> {
    let mut e = entry(&s.id);
    e.insert("label".into(), json!(label(s)));
    e.insert("project".into(), json!(project_label(&s.cwd)));
    e.insert("harness".into(), json!(s.harness));
    e.insert(
        "minutes_ago".into(),
        json!(((now() - s.last_active).max(0.0) / 60.0) as i64),
    );
    e.insert(
        "state".into(),
        json!(if external::is_external(&s.harness) {
            "registered"
        } else if s.running {
            "running"
        } else {
            "idle"
        }),
    );
    if let Some(thread) = &s.t3_thread_id {
        e.insert("t3_thread_id".into(), json!(thread));
    }
    e
}

fn resolve_one(id: &str, sessions: &[Session], conversation: bool) -> Value {
    let mut e = if !crate::gateway::valid_destination(id) {
        let mut e = entry(id);
        e.insert("kind".into(), json!("invalid"));
        e.insert("state".into(), json!("unknown"));
        e.insert("actions".into(), json!([]));
        e.insert(
            "note".into(),
            json!("not a valid Everett destination ID"),
        );
        e
    } else if let Some(s) = sessions.iter().find(|s| s.id == id) {
        let mut e = session_entry(s);
        if external::is_external(&s.harness) && external::device_target(&s.id).is_some() {
            e.insert("kind".into(), json!("conversation"));
            e.insert("actions".into(), json!([]));
            e.insert("note".into(), json!(
                "its inbox lives on a paired remote device; this connection delivers to local inboxes only"
            ));
        } else {
            e.insert(
                "kind".into(),
                json!(if external::is_external(&s.harness) {
                    "conversation"
                } else {
                    "session"
                }),
            );
            e.insert("actions".into(), json!(["send"]));
        }
        e
    } else if let Some((harness, sid, title)) = t3_thread(id) {
        let mut e = entry(id);
        e.insert("kind".into(), json!("t3_thread"));
        e.insert("label".into(), json!(title));
        e.insert("harness".into(), json!(harness));
        e.insert("session_id".into(), json!(sid));
        match sessions.iter().find(|s| s.id == sid) {
            Some(s) => {
                e.insert("project".into(), json!(project_label(&s.cwd)));
                e.insert(
                    "state".into(),
                    json!(if s.running { "running" } else { "idle" }),
                );
                e.insert(
                    "minutes_ago".into(),
                    json!(((now() - s.last_active).max(0.0) / 60.0) as i64),
                );
            }
            None => {
                e.insert("state".into(), json!("inactive"));
            }
        }
        e.insert("actions".into(), json!(["send"]));
        e.insert(
            "note".into(),
            json!("send reaches this thread's provider session inbox"),
        );
        e
    } else {
        let mut e = entry(id);
        if conversation {
            e.insert("kind".into(), json!("inbox"));
            e.insert("state".into(), json!("inactive"));
            e.insert("actions".into(), json!(["send"]));
            e.insert("note".into(), json!(
                "a bare local inbox address; no known session binds it, so nothing picks messages up until one does"
            ));
        } else {
            e.insert("kind".into(), json!("unknown"));
            e.insert("state".into(), json!("unknown"));
            e.insert("actions".into(), json!([]));
            e.insert("note".into(), json!(
                "no session, T3 thread or registered conversation has this exact ID; this connection's sends need a live session"
            ));
        }
        e
    };
    e.retain(|_, v| !v.is_null());
    Value::Object(e)
}

/// One labeled entry per authorized ID, preserving the allowlist order.
pub(crate) fn resolve_all(ids: &[String], conversation: bool) -> Vec<Value> {
    let sessions = crate::registry::scan(SCAN_HOURS, true, None, "");
    ids.iter()
        .map(|id| resolve_one(id, &sessions, conversation))
        .collect()
}
