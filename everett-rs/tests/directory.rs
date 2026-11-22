//! everett_directory labels exact destination IDs for the local MCP caller and
//! for owner-authorized gateway connections.
mod common;

use common::{fixture, jsonl, Fixture};
use serde_json::{json, Value};

fn tool(fx: &Fixture, name: &str, args: Value, caller: &str) -> Value {
    let input = jsonl(&[
        json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
            "protocolVersion":"2025-06-18", "capabilities":{},
            "clientInfo":{"name":"synthetic-directory", "version":"1"}}}),
        json!({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{
            "name":name, "arguments":args}}),
    ]);
    common::native_mcp_account(&fx.home());
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(["mcp"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", fx.home())
        .env("EVERETT_HOME", fx.home())
        .env("EVERETT_SESSION_ID", caller)
        .env("EVERETT_ROUTER", "local")
        .env("EVERETT_NOTIFY", "none")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let response: Value =
        serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(response["id"], 2);
    response
}

fn t3(fx: &Fixture, thread: &str, native: &str, title: &str) {
    let path = fx.home().join(".t3/userdata/statev2.sqlite");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let con = rusqlite::Connection::open(path).unwrap();
    con.execute_batch(include_str!("../../tests/fixtures/t3_v2.sql"))
        .unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    con.execute(
        "INSERT INTO projection_projects VALUES ('p','Project','/work/project',NULL,'[]',?1,?1,NULL)",
        [&now],
    )
    .unwrap();
    let provider = format!("provider-{thread}");
    con.execute(
        "INSERT INTO orchestration_v2_projection_threads \
         (thread_id,project_id,title,default_provider,provider_instance_id,runtime_mode,interaction_mode, \
          active_provider_thread_id,created_at,updated_at,payload_json) \
         VALUES (?1,'p',?2,'custom-instance','custom-instance','full-access','default',?3,?4,?4,?5)",
        rusqlite::params![thread, title, provider, now, json!({"worktreePath":"/work/tree"}).to_string()],
    )
    .unwrap();
    con.execute(
        "INSERT INTO orchestration_v2_projection_provider_threads \
         (provider_thread_id,thread_id,provider,driver,provider_instance_id,status,updated_at,payload_json) \
         VALUES (?1,?2,'custom-instance',?3,'custom-instance','idle',?4,?5)",
        rusqlite::params![
            provider,
            thread,
            "codex",
            now,
            json!({"driver":"codex","providerInstanceId":"custom-instance",
                   "nativeThreadRef":{"driver":"codex","nativeId":native,"strength":"strong"}})
            .to_string()
        ],
    )
    .unwrap();
}

#[test]
fn directory_labels_ids_for_the_local_caller() {
    let fx = fixture();
    t3(&fx, "chat-teresis", "native-teresis", "Teresis refactor");
    let missing = tool(&fx, "everett_directory", json!({}), "caller");
    assert!(
        missing["error"]["message"]
            .as_str()
            .unwrap()
            .contains("\"ids\""),
        "{missing}"
    );
    let result = tool(
        &fx,
        "everett_directory",
        json!({"ids": ["chat-teresis", "native-teresis", "unknown-x", "bad/id"]}),
        "caller",
    )["result"]
        .clone();
    assert_eq!(result["isError"], false, "{result}");
    let entries: std::collections::HashMap<String, Value> = result["structuredContent"]
        ["destinations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| (entry["id"].as_str().unwrap().to_string(), entry.clone()))
        .collect();
    let thread = &entries["chat-teresis"];
    assert_eq!(thread["kind"], "t3_thread");
    assert_eq!(thread["label"], "Teresis refactor");
    assert_eq!(thread["session_id"], "native-teresis");
    assert_eq!(thread["actions"], json!(["send"]));
    let session = &entries["native-teresis"];
    assert_eq!(session["kind"], "session");
    assert_eq!(session["t3_thread_id"], "chat-teresis");
    let unknown = &entries["unknown-x"];
    assert_eq!(unknown["kind"], "unknown");
    assert_eq!(unknown["actions"], json!([]));
    assert_eq!(entries["bad/id"]["kind"], "invalid");
}
