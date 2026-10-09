mod common;

use std::io::Write;
use std::process::{Command, Output, Stdio};

use chrono::{Duration, Utc};
use common::{fixture, jsonl, Fixture};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};

fn run(fx: &Fixture, args: &[&str], input: &str, caller: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", fx.home())
        .env("EVERETT_HOME", fx.home())
        .env("EVERETT_SESSION_ID", caller)
        .env("EVERETT_ROUTER", "local")
        .env("EVERETT_NOTIFY", "none")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn list(fx: &Fixture, harness: &str) -> Vec<Value> {
    let out = run(fx, &["ls", "--json", "--harness", harness], "", "");
    serde_json::from_slice(&out.stdout).unwrap()
}

fn tool(fx: &Fixture, name: &str, args: Value, caller: &str) -> Value {
    let input = jsonl(&[
        json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
            "protocolVersion":"2025-06-18", "capabilities":{},
            "clientInfo":{"name":"synthetic-t3", "version":"1"}}}),
        json!({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{
            "name":name, "arguments":args}}),
    ]);
    let out = run(fx, &["mcp"], &input, caller);
    let stdout = String::from_utf8(out.stdout).unwrap();
    let response: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(response["id"], 2);
    response["result"].clone()
}

fn v2(fx: &Fixture) -> Connection {
    let path = fx.home().join(".t3/userdata/statev2.sqlite");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let con = Connection::open(path).unwrap();
    con.execute_batch(include_str!("../../tests/fixtures/t3_v2.sql"))
        .unwrap();
    let now = Utc::now().to_rfc3339();
    con.execute("INSERT INTO projection_projects VALUES ('p','Project','/work/project',NULL,'[]',?1,?1,NULL)",
                [&now]).unwrap();
    con
}

fn add(con: &Connection, thread: &str, native: Option<&str>, driver: &str) {
    let now = Utc::now().to_rfc3339();
    let provider_id = format!("provider-{}", thread);
    let payload = json!({"worktreePath":"/work/tree"}).to_string();
    con.execute(
        "INSERT INTO orchestration_v2_projection_threads \
         (thread_id,project_id,title,default_provider,provider_instance_id,runtime_mode,interaction_mode, \
          active_provider_thread_id,created_at,updated_at,payload_json) \
         VALUES (?1,'p','T3 task','custom-instance','custom-instance','full-access','default',?2,?3,?3,?4)",
        [thread, &provider_id, &now, &payload]
    ).unwrap();
    let native_ref = native.map(|sid| json!({"driver":driver,"nativeId":sid,"strength":"strong"}));
    let payload = json!({"driver":driver,"providerInstanceId":"custom-instance",
                         "nativeThreadRef":native_ref})
    .to_string();
    con.execute(
        "INSERT INTO orchestration_v2_projection_provider_threads \
         (provider_thread_id,thread_id,provider,driver,provider_instance_id,status,updated_at,payload_json) \
         VALUES (?1,?2,'custom-instance',?3,'custom-instance','idle',?4,?5)",
        [&provider_id, thread, driver, &now, &payload]
    ).unwrap();
    for (index, text) in ["first real request", "latest real request"]
        .iter()
        .enumerate()
    {
        con.execute(
            "INSERT INTO orchestration_v2_projection_messages \
             (message_id,thread_id,role,streaming,created_at,updated_at,payload_json) \
             VALUES (?1,?2,'user',0,?3,?3,?4)",
            [
                format!("{}-{}", thread, index),
                thread.into(),
                now.clone(),
                json!({"text":text}).to_string(),
            ],
        )
        .unwrap();
    }
}

#[test]
fn current_t3_without_provider_transcript_uses_native_driver_and_readonly_projection() {
    let fx = fixture();
    let con = v2(&fx);
    add(&con, "app-1", Some("native-1"), "codex");
    let path = fx.home().join(".t3/userdata/statev2.sqlite");
    let before = std::fs::read(&path).unwrap();
    let sessions = list(&fx, "codex");
    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s["id"], "native-1");
    assert_eq!(s["harness"], "codex");
    assert_eq!(s["source"], "t3code");
    assert_eq!(s["cwd"], "/work/tree");
    assert_eq!(s["first_user"], "first real request");
    assert_eq!(s["last_user"], "latest real request");
    assert!(list(&fx, "claude").is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let ro = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert!(ro
        .execute("DELETE FROM orchestration_v2_projection_threads", [])
        .is_err());
}

#[test]
fn t3_filters_old_deleted_archived_unknown_missing_and_mismatched_native_references() {
    let fx = fixture();
    let con = v2(&fx);
    for id in ["old", "deleted", "archived", "bad", "valid"] {
        add(&con, id, Some(id), "codex");
    }
    add(&con, "empty", None, "codex");
    add(&con, "unsupported", Some("cursor-native"), "cursor");
    let old = (Utc::now() - Duration::hours(100)).to_rfc3339();
    con.execute(
        "UPDATE orchestration_v2_projection_threads SET updated_at=?1 WHERE thread_id='old'",
        [&old],
    )
    .unwrap();
    con.execute("UPDATE orchestration_v2_projection_threads SET deleted_at=updated_at WHERE thread_id='deleted'", []).unwrap();
    con.execute("UPDATE orchestration_v2_projection_threads SET archived_at=updated_at WHERE thread_id='archived'", []).unwrap();
    con.execute("UPDATE orchestration_v2_projection_provider_threads SET payload_json=?1 WHERE thread_id='bad'",
        [json!({"nativeThreadRef":{"driver":"claudeAgent","nativeId":"bad"}}).to_string()]).unwrap();
    con.execute(
        "UPDATE orchestration_v2_projection_threads SET payload_json='{}' WHERE thread_id='valid'",
        [],
    )
    .unwrap();
    let sessions = list(&fx, "codex");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], "valid");
    assert_eq!(sessions[0]["cwd"], "/work/project");
}

#[test]
fn provider_transcript_is_preserved_and_current_store_does_not_revive_legacy_rows() {
    let fx = fixture();
    let con = v2(&fx);
    add(&con, "app-1", Some("native-1"), "codex");
    add(&con, "app-2", Some("native-1"), "codex");
    fx.write(".codex/sessions/2026/10/09/rollout-native-1.jsonl", &jsonl(&[
        json!({"type":"session_meta","payload":{"id":"native-1","cwd":"/work/provider","timestamp":Utc::now().to_rfc3339()}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"provider request"}]}}),
    ]));
    let legacy = Connection::open(fx.home().join(".t3/userdata/state.sqlite")).unwrap();
    legacy.execute_batch("CREATE TABLE projection_threads (thread_id TEXT, project_id TEXT, title TEXT, created_at TEXT, updated_at TEXT, deleted_at TEXT);
        CREATE TABLE provider_session_runtime (thread_id TEXT, provider_name TEXT, resume_cursor_json TEXT);").unwrap();
    let now = Utc::now().to_rfc3339();
    legacy
        .execute(
            "INSERT INTO projection_threads VALUES ('stale','p','Old copy',?1,?1,NULL)",
            [&now],
        )
        .unwrap();
    legacy.execute("INSERT INTO provider_session_runtime VALUES ('stale','codex','{\"threadId\":\"stale-native\"}')", []).unwrap();
    let sessions = list(&fx, "codex");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], "native-1");
    assert_eq!(sessions[0]["cwd"], "/work/provider");
    assert_eq!(sessions[0]["first_user"], "provider request");
    assert_eq!(sessions[0]["source"], "t3code");
}

#[test]
fn legacy_t3_missing_transcript_uses_only_real_cursor() {
    let fx = fixture();
    let path = fx.home().join(".t3/userdata/state.sqlite");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let con = Connection::open(&path).unwrap();
    con.execute_batch("CREATE TABLE projection_threads (thread_id TEXT, project_id TEXT, title TEXT, created_at TEXT, updated_at TEXT, deleted_at TEXT);
        CREATE TABLE provider_session_runtime (thread_id TEXT, provider_name TEXT, resume_cursor_json TEXT);").unwrap();
    let now = Utc::now().to_rfc3339();
    for id in ["app-legacy", "no-native"] {
        con.execute(
            "INSERT INTO projection_threads VALUES (?1,'p','Legacy task',?2,?2,NULL)",
            [id, &now],
        )
        .unwrap();
    }
    con.execute("INSERT INTO provider_session_runtime VALUES ('app-legacy','claudeAgent','{\"resume\":\"legacy-native\"}')", []).unwrap();
    con.execute("INSERT INTO provider_session_runtime VALUES ('no-native','claudeAgent','{\"threadId\":\"app-only\"}')", []).unwrap();
    let sessions = list(&fx, "claude");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], "legacy-native");
    assert_eq!(sessions[0]["source"], "t3code");
    assert!(list(&fx, "codex").is_empty());
    assert_eq!(everett::adapters::t3code::threads(&path).len(), 1);
}

#[test]
fn synthetic_t3_mcp_routes_polls_replies_and_learns_without_provider_resume() {
    let fx = fixture();
    let con = v2(&fx);
    add(&con, "app-1", Some("native-1"), "codex");
    let routed = tool(
        &fx,
        "everett_route",
        json!({"text":"first real request", "router":"local"}),
        "sender",
    );
    assert_eq!(routed["isError"], false);
    let guidance = routed["structuredContent"]["command"].as_str().unwrap();
    assert!(guidance.contains("everett_inbox"));
    assert!(!guidance.contains("codex resume"));
    let queued = tool(
        &fx,
        "everett_send",
        json!({"to":"native-1","text":"Review the T3 task"}),
        "sender",
    );
    assert_eq!(queued["isError"], false);
    let queued = &queued["structuredContent"];
    assert_eq!(queued["pickup"], "poll");
    assert_eq!(queued["hooked"], false);
    assert_eq!(queued["poll_session_id"], "native-1");
    let self_send = tool(
        &fx,
        "everett_send",
        json!({"to":"native-1","text":"loop","session_id":"native-1"}),
        "native-1",
    );
    assert_eq!(self_send["isError"], true);
    let polled = tool(
        &fx,
        "everett_inbox",
        json!({"session_id":"native-1"}),
        "native-1",
    );
    assert_eq!(
        polled["structuredContent"]["messages"][0]["id"],
        queued["message_id"]
    );
    let empty = tool(
        &fx,
        "everett_inbox",
        json!({"session_id":"native-1"}),
        "native-1",
    );
    assert_eq!(empty["structuredContent"]["messages"], json!([]));
    let answered = tool(
        &fx,
        "everett_send",
        json!({"reply_to":queued["message_id"],"text":"Reviewed","session_id":"native-1"}),
        "native-1",
    );
    assert_eq!(answered["isError"], false);
    let response = tool(
        &fx,
        "everett_inbox",
        json!({"session_id":"sender"}),
        "sender",
    );
    assert_eq!(
        response["structuredContent"]["messages"][0]["text"],
        "Reviewed"
    );
    assert_eq!(
        response["structuredContent"]["messages"][0]["reply_to"],
        queued["message_id"]
    );
    let learned = tool(
        &fx,
        "everett_learn",
        json!({"fact":"T3 messages require explicit inbox polling","project":"/work/tree"}),
        "native-1",
    );
    assert_eq!(learned["isError"], false);
    let core = tool(
        &fx,
        "everett_core",
        json!({"project":"/work/tree"}),
        "native-1",
    );
    assert_eq!(core["structuredContent"]["pending_learnings"], 1);
}
