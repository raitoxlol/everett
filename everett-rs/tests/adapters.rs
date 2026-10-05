//! Session-store reads through `everett ls --json`: each harness's store is planted
//! under a temp HOME, never a real one. Mirrors tests/test_everett.py::Adapters.

mod common;

use common::{fixture, jsonl, plant};
use serde_json::json;

fn session<'a>(list: &'a [serde_json::Value], id: &str) -> &'a serde_json::Value {
    list.iter().find(|s| s["id"] == id).unwrap_or_else(|| panic!("{} not in {:?}", id, list))
}

#[test]
fn claude_store_listed() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    let _l = fx.ls_json(); let s = session(&_l, "c-1");
    assert_eq!(s["harness"], "claude");
    assert_eq!(s["cwd"], "/work/app");
    assert_eq!(s["first_user"], "fix the login bug");
    assert_eq!(s["last_user"], "now add tests");
    assert_eq!(s["title"], "Login bug fix");
}

#[test]
fn codex_store_skips_injected_context() {
    let fx = fixture();
    plant(&fx, "codex.jsonl", ".codex/sessions/2026/09/20/rollout-x-1.jsonl");
    let _l = fx.ls_json(); let s = session(&_l, "x-1");
    assert_eq!(s["harness"], "codex");
    assert_eq!(s["cwd"], "/work/atlas");
    assert_eq!(s["first_user"], "ship the atlas repo");
}

#[test]
fn omp_store_title() {
    let fx = fixture();
    plant(&fx, "omp.jsonl", ".omp/agent/sessions/work/omp.jsonl");
    let _l = fx.ls_json(); let s = session(&_l, "o-1");
    assert_eq!(s["harness"], "omp");
    assert_eq!(s["title"], "Repo health checks");
    assert_eq!(s["first_user"], "implement health checks");
}

#[test]
fn hermes_db_listed() {
    let fx = fixture();
    let db = fx.home().join(".hermes/state.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let con = rusqlite::Connection::open(&db).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    con.execute_batch(
        "CREATE TABLE sessions (id TEXT, source TEXT, started_at REAL, cwd TEXT, title TEXT,
            profile_name TEXT, last_activity_at REAL);
         CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT, role TEXT, content TEXT);",
    )
    .unwrap();
    con.execute(
        "INSERT INTO sessions VALUES ('h-1','cli',?1,'/work/herm','Hermes run','',?1)",
        [now - 60.0],
    )
    .unwrap();
    con.execute(
        "INSERT INTO messages (session_id, role, content) VALUES ('h-1','user','write the hermes adapter')",
        [],
    )
    .unwrap();
    drop(con);
    let _l = fx.ls_json(); let s = session(&_l, "h-1");
    assert_eq!(s["harness"], "hermes");
    assert_eq!(s["cwd"], "/work/herm");
    assert_eq!(s["first_user"], "write the hermes adapter");
    assert_eq!(s["auto"], false);
}

#[test]
fn devin_db_and_transcript_listed() {
    let fx = fixture();
    let base = fx.home().join(".local/share/devin/cli");
    std::fs::create_dir_all(base.join("transcripts")).unwrap();
    let db = base.join("sessions.db");
    let con = rusqlite::Connection::open(&db).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    con.execute_batch(
        "CREATE TABLE sessions (id TEXT, working_directory TEXT, title TEXT,
            created_at REAL, last_activity_at REAL);",
    )
    .unwrap();
    con.execute(
        "INSERT INTO sessions VALUES ('d-1','/work/cloud','Cloud task',?1,?1)",
        [now - 60.0],
    )
    .unwrap();
    drop(con);
    // Without message_nodes the ATIF transcripts are read directly, and fill in
    // user text for sessions the DB already knows.
    fx.write(
        ".local/share/devin/cli/transcripts/d-2.json",
        &json!({
            "session_id": "d-2",
            "title": "Transcript task",
            "agent": {"extra": {"working_directory": "/work/other"}},
            "steps": [{"source": "user", "content": "summarize the queue",
                        "metadata": {"created_at": now - 30.0}}]
        })
        .to_string(),
    );
    let list = fx.ls_json();
    let s = session(&list, "d-1");
    assert_eq!(s["harness"], "devin");
    assert_eq!(s["cwd"], "/work/cloud");
    let t = session(&list, "d-2");
    assert_eq!(t["first_user"], "summarize the queue");
    assert_eq!(t["title"], "Transcript task");
}

#[test]
fn devin_message_nodes_supply_user_text() {
    let fx = fixture();
    let base = fx.home().join(".local/share/devin/cli");
    std::fs::create_dir_all(base.join("transcripts")).unwrap();
    let con = rusqlite::Connection::open(base.join("sessions.db")).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    con.execute_batch(
        "CREATE TABLE sessions (id TEXT, working_directory TEXT, title TEXT,
            created_at REAL, last_activity_at REAL);
         CREATE TABLE message_nodes (node_id TEXT, session_id TEXT, chat_message TEXT);",
    )
    .unwrap();
    con.execute(
        "INSERT INTO sessions VALUES ('d-1','/work/cloud','Cloud task',?1,?1)",
        [now - 60.0],
    )
    .unwrap();
    con.execute(
        "INSERT INTO message_nodes VALUES ('n1','d-1',?1)",
        [json!({"source":"user","content":"audit the devin sessions"}).to_string()],
    )
    .unwrap();
    drop(con);
    // With message_nodes present, transcripts must not double-list sessions.
    fx.write(
        ".local/share/devin/cli/transcripts/d-9.json",
        &json!({
            "session_id": "d-9",
            "title": "Should be skipped",
            "agent": {"extra": {}},
            "steps": [{"source": "user", "content": "do not show me",
                        "metadata": {"created_at": now - 30.0}}]
        })
        .to_string(),
    );
    let list = fx.ls_json();
    let s = session(&list, "d-1");
    assert_eq!(s["first_user"], "audit the devin sessions");
    assert!(list.iter().all(|x| x["id"] != "d-9"));
}

#[test]
fn card_wins_over_title_and_stale_card_is_ignored() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    let card = fx.write(".everett/cards/c-1.md", "# Login fix\nFixing auth redirect; next: tests.\n");
    let _l = fx.ls_json(); let listed = session(&_l, "c-1");
    assert_eq!(listed["card"], "Login fix Fixing auth redirect; next: tests.");
    assert_eq!(listed["card_source"], "agent");

    // A card untouched for >24h of session activity stops applying.
    let stale = std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 3600);
    std::fs::File::options()
        .write(true)
        .open(&card)
        .unwrap()
        .set_modified(stale)
        .unwrap();
    let _l = fx.ls_json(); let listed = session(&_l, "c-1");
    assert_eq!(listed["card"], "");
}

#[test]
fn auto_card_is_tagged_auto() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    fx.write(
        ".everett/cards/c-1.md",
        "<!-- everett:auto -->\nWhat: Authentication tests\nState: Adding retries\nNext: Run the suite.\n",
    );
    let _l = fx.ls_json(); let listed = session(&_l, "c-1");
    assert_eq!(listed["card_source"], "auto");
    assert!(listed["card"].as_str().unwrap().contains("Authentication tests"));
}

#[test]
fn sessions_without_user_text_still_list_when_carded() {
    let fx = fixture();
    let rows = jsonl(&[json!({
        "type": "user", "sessionId": "p-1", "cwd": "/w", "timestamp": "t",
        "message": {"content": "<command-name>/clear</command-name>"}
    })]);
    fx.write(".claude/projects/app/p-1.jsonl", &rows);
    assert!(fx.ls_json().is_empty());
    fx.write(".everett/cards/p-1.md", "Orchestrating work");
    let list = fx.ls_json();
    session(&list, "p-1");
}

#[test]
fn pasted_content_counts_as_user_text() {
    let fx = fixture();
    let rows = jsonl(&[json!({
        "type": "user", "sessionId": "p-1", "cwd": "/w", "timestamp": "t",
        "message": {"content": "\n\n<pasted_content id=\"a\">\nHelp me orchestrate work\n</pasted_content>"}
    })]);
    fx.write(".claude/projects/app/p-1.jsonl", &rows);
    let _l = fx.ls_json(); let s = session(&_l, "p-1");
    assert_eq!(s["first_user"], "Help me orchestrate work");
}
