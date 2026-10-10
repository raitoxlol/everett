mod common;

use common::{fixture, Fixture};
use everett::adapters::{grok, t3code};
use everett::send::command_for;
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::PathBuf;

const SCHEMA: &str = "
CREATE TABLE projection_threads (thread_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, title TEXT NOT NULL,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT);
CREATE TABLE provider_session_runtime (thread_id TEXT PRIMARY KEY, provider_name TEXT NOT NULL,
  adapter_key TEXT NOT NULL, status TEXT NOT NULL, last_seen_at TEXT NOT NULL, resume_cursor_json TEXT,
  runtime_payload_json TEXT);";

fn write(path: PathBuf, rows: &[Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, rows.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
}

/// Claude, Codex and Grok sessions driven by a legacy-schema T3 store, plus one pruned thread.
fn legacy(fx: &Fixture) -> PathBuf {
    let home = fx.home();
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    write(home.join(".claude/projects/-work-api/cl-1.jsonl"), &[json!({"type": "user", "sessionId": "cl-1", "cwd": "/work/api",
        "timestamp": now, "entrypoint": "sdk-ts", "message": {"role": "user", "content": "add retries"}})]);
    write(home.join(".codex/sessions/2026/09/24/rollout-cx-1.jsonl"), &[
        json!({"type": "session_meta", "payload": {"id": "cx-1", "cwd": "/work/web", "timestamp": now, "originator": "t3code_desktop", "source": "vscode"}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "dark mode"}]}})]);
    let folder = home.join(".grok/sessions").join(grok::quote("/work/infra")).join("gk-1");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("summary.json"), json!({"info": {"id": "gk-1", "cwd": "/work/infra"}, "created_at": now, "last_active_at": now}).to_string()).unwrap();
    write(folder.join("chat_history.jsonl"), &[
        json!({"type": "user", "prompt_index": 0, "content": [{"type": "text", "text": "<user_query>\nterraform bucket\n</user_query>"}]}),
        json!({"type": "assistant", "content": "Done."})]);
    let db = home.join(".t3/userdata/state.sqlite");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let con = Connection::open(&db).unwrap();
    con.execute_batch(SCHEMA).unwrap();
    for (id, title) in [("t-cl", "API retry thread"), ("t-cx", "Dark mode"), ("t-gk", "Infra bucket"), ("t-gone", "Pruned")] {
        con.execute("INSERT INTO projection_threads VALUES (?1,'p',?2,?3,?3,NULL)", [id, title, &now]).unwrap();
    }
    for (id, provider, cursor) in [
        ("t-cl", "claudeAgent", json!({"threadId": "t-cl", "resume": "cl-1", "resumeSessionAt": "m-9", "turnCount": 1})),
        ("t-cx", "codex", json!({"threadId": "cx-1"})),
        ("t-gk", "grok", json!({"schemaVersion": 1, "sessionId": "gk-1"})),
        ("t-gone", "codex", json!({"threadId": "no-such-session"})),
    ] {
        con.execute("INSERT INTO provider_session_runtime VALUES (?1,?2,?2,'stopped',?3,?4,'{}')", [id, provider, &now, &cursor.to_string()]).unwrap();
    }
    db
}

fn ls(fx: &Fixture) -> Vec<Value> {
    let out = fx.run(&["ls", "--json", "--hours", "72"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn by_id(rows: &[Value], id: &str) -> Value {
    rows.iter().find(|s| s["id"] == id).cloned().unwrap_or_else(|| panic!("{id} missing from {rows:?}"))
}

#[test]
fn legacy_threads_map_to_harness_sessions_read_only() {
    let fx = fixture();
    let db = legacy(&fx);
    let before = std::fs::read(&db).unwrap();
    let found = t3code::threads(&db);
    let get = |h: &str, id: &str| found.get(&(h.to_string(), id.to_string())).cloned().unwrap_or_else(|| panic!("{h}/{id}"));
    let (thread, title) = get("claude", "cl-1");
    assert!([thread.as_str(), title.as_str()].contains(&"API retry thread"), "{thread} {title}");
    let (thread, title) = get("codex", "cx-1");
    assert!([thread.as_str(), title.as_str()].contains(&"t-cx"), "{thread} {title}");
    get("grok", "gk-1");
    assert_eq!(std::fs::read(&db).unwrap(), before);
    let con = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert!(con.execute("DELETE FROM projection_threads", []).is_err());
}

#[test]
fn legacy_sessions_annotated_not_double_listed_and_route_only() {
    let fx = fixture();
    legacy(&fx);
    let rows = ls(&fx);
    let mut ids: Vec<&str> = rows.iter().map(|s| s["id"].as_str().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, ["cl-1", "cx-1", "gk-1", "no-such-session"]);
    assert!(rows.iter().all(|s| s["source"] == t3code::SOURCE), "{rows:?}");
    assert_eq!(by_id(&rows, "cl-1")["title"], "API retry thread");
    assert_eq!(by_id(&rows, "cx-1")["harness"], "codex");
    let session: everett::session::Session = serde_json::from_value(by_id(&rows, "cx-1")).unwrap();
    let err = command_for(&session, "go").unwrap_err();
    assert_eq!(err.code, 2);
    assert!(err.to_string().contains("T3 Code"), "{err}");
}

#[test]
fn codex_originator_marks_t3_without_db_and_broken_db_is_ignored() {
    let fx = fixture();
    let db = legacy(&fx);
    std::fs::remove_file(&db).unwrap();
    let rows = ls(&fx);
    assert_eq!(by_id(&rows, "cx-1")["source"], t3code::SOURCE);
    assert_ne!(by_id(&rows, "cl-1")["source"], t3code::SOURCE);
    std::fs::write(&db, "not sqlite").unwrap();
    assert!(t3code::threads(&db).is_empty());
    assert_eq!(ls(&fx).len(), 3);
}

#[test]
fn doctor_shows_grok_and_t3code_stores() {
    let fx = fixture();
    let db = legacy(&fx);
    let text = fx.stdout(&fx.run(&["doctor", "--hours", "72"]));
    let grok_root = fx.home().join(".grok/sessions");
    for want in [format!("{} (found)", grok_root.display()), format!("{} (found); 4 thread(s) with a harness session id", db.display()),
                 "run `everett install-hooks --grok --apply`".to_string()] {
        assert!(text.contains(&want), "missing {want:?} in\n{text}");
    }
}
