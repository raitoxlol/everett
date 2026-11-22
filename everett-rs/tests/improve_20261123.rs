mod common;

use common::{fixture, lib_home};
use everett::cards::auto::auto_card;
use everett::session::{now, Session};
use serde_json::{json, Value};
use std::path::Path;

fn write_jsonl(path: &Path, rows: &[Value]) {
    std::fs::write(path, rows.iter().map(|r| format!("{}\n", r)).collect::<String>()).unwrap();
}

fn card(rows: &[Value], cwd: &str, harness: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    write_jsonl(&transcript, rows);
    auto_card(&transcript, harness, cwd, "")
}

fn codex(role: &str, text: &str, kind: &str) -> Value {
    json!({"type": "response_item", "payload": {"type": "message", "role": role, "content": [{"type": kind, "text": text}]}})
}

fn next_line(card_text: &str) -> String {
    card_text
        .lines()
        .find(|l| l.starts_with("Next:"))
        .unwrap_or("")
        .to_string()
}

#[test]
fn next_steps_plural_keeps_the_step_text() {
    let _h = lib_home();
    let plural = card(&[
        json!({"type": "session_meta", "payload": {"cwd": "/work/orbit"}}),
        codex("user", "# Task\nAdd retry handling to the client.", "input_text"),
        codex("assistant", "The client change is ready.\nNext steps: run the tests.", "output_text"),
    ], "", "codex");
    let singular = card(&[
        json!({"type": "session_meta", "payload": {"cwd": "/work/orbit"}}),
        codex("user", "# Task\nAdd retry handling to the client.", "input_text"),
        codex("assistant", "The client change is ready.\nNext step: run the tests.", "output_text"),
    ], "", "codex");
    assert_eq!(next_line(&plural), next_line(&singular), "{plural}");
    assert!(!plural.contains("s: run"), "{plural}");
}

#[test]
fn huge_timeout_does_not_panic() {
    let out = everett::proc::run_capture(
        &["true".to_string()],
        None,
        &std::collections::HashMap::new(),
        1e20,
    )
    .unwrap();
    assert_eq!(out.code, 0);
    assert!(!out.timed_out);
}

#[test]
fn devin_hook_snippet_names_the_devin_config() {
    let _h = lib_home();
    let snippet = everett::install::snippet("devin");
    assert!(!snippet.contains(".grok"), "{snippet}");
    assert!(
        snippet.contains(&everett::install::settings_path("devin").display().to_string()),
        "{snippet}"
    );
    assert!(everett::install::snippet("grok").contains(".grok/hooks/everett.json"));
}

#[cfg(unix)]
#[test]
fn codex_mcp_install_keeps_config_mode() {
    use std::os::unix::fs::PermissionsExt;
    let _h = lib_home();
    let path = everett::install::mcp_path("codex");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[profile]\nname = \"default\"\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    everett::install::apply_mcp("codex", false).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("everett"), "{text}");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn skill_with_utf8_bom_parses_frontmatter() {
    let fx = fixture();
    fx.write(
        "skills/misc-dir/SKILL.md",
        "\u{feff}---\nname: bom-skill\ndescription: Does the thing\n---\nBody text\n",
    );
    let proj = fx.home().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let skills = everett::skills::scan(&proj);
    let skill = skills
        .iter()
        .find(|s| s.dir.ends_with("misc-dir"))
        .expect("planted skill not scanned");
    assert_eq!(skill.name, "bom-skill");
    assert_eq!(skill.description, "Does the thing");
    let payload = everett::skills::payload(skill);
    assert_eq!(payload["content"], "Body text");
    assert!(!payload["content"].as_str().unwrap().contains("description:"));
}

fn session(title: &str, card_text: &str, first: &str, last: &str) -> Session {
    let mut s = Session::new("codex", "w-1", "/work/atlas", "/tmp/w-1.jsonl", "", now() - 60.0);
    s.title = title.into();
    s.card = card_text.into();
    s.first_user = first.into();
    s.last_user = last.into();
    s
}

#[test]
fn coordinator_terms_match_whole_words_only() {
    assert!(!everett::route::is_coordinator(&session(
        "Masking the UI flashes",
        "Fixing the masking regression in the settings UI.",
        "the masking animation flashes",
        "still masking"
    )));
    assert!(!everett::route::is_coordinator(&session(
        "Bug triage",
        "Triaging filed bugs.",
        "triage the new bug",
        "Reopen the task the bot filed"
    )));
    assert!(everett::route::is_coordinator(&session(
        "Status check",
        "",
        "",
        "Ask the billing session for status"
    )));
    assert!(everett::route::is_coordinator(&session(
        "Sweep",
        "Sweep report: everett_send delivered to all workers.",
        "",
        ""
    )));
}

#[test]
fn hermes_last_user_skips_injected_newest_row() {
    let fx = fixture();
    let db = fx.home().join(".hermes/state.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let con = rusqlite::Connection::open(&db).unwrap();
    let now_secs = std::time::SystemTime::now()
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
        [now_secs - 60.0],
    )
    .unwrap();
    con.execute(
        "INSERT INTO messages (session_id, role, content) VALUES ('h-1','user','fix the login bug')",
        [],
    )
    .unwrap();
    con.execute(
        "INSERT INTO messages (session_id, role, content) VALUES ('h-1','user','<command-name>/clear</command-name>')",
        [],
    )
    .unwrap();
    drop(con);
    let list = fx.ls_json();
    let s = list.iter().find(|r| r["id"] == "h-1").expect("h-1 not listed");
    assert_eq!(s["last_user"], "fix the login bug");
}
