mod common;

use common::{fixture, jsonl, mcp_calls, tool_json};
use serde_json::json;

fn call(fx: &common::Fixture, name: &str, args: serde_json::Value) -> serde_json::Value {
    mcp_calls(fx, &[], &[("EVERETT_SESSION_ID", "caller-1")], &[(name, args)])
        .remove(0)
}

#[test]
fn tasks_board_lifecycle() {
    let fx = fixture();
    let added = tool_json(&call(&fx, "everett_tasks", json!({"action": "add", "title": "fix login redirect"})));
    let id = added["added"]["id"].as_str().unwrap().to_string();
    assert_eq!(added["added"]["status"], "open");

    let listed = tool_json(&call(&fx, "everett_tasks", json!({})));
    assert_eq!(listed["tasks"].as_array().unwrap().len(), 1);
    assert!(listed["board"].as_str().unwrap().ends_with(".everett/tasks.json"));

    let claimed = tool_json(&call(&fx, "everett_tasks", json!({"action": "claim", "id": id})));
    assert_eq!(claimed["task"]["status"], "claimed");
    assert_eq!(claimed["task"]["owner"], "caller-1");

    let done = tool_json(&call(&fx, "everett_tasks", json!({"action": "done", "id": id})));
    assert_eq!(done["task"]["status"], "done");

    tool_json(&call(&fx, "everett_tasks", json!({"action": "remove", "id": id})));
    let listed = tool_json(&call(&fx, "everett_tasks", json!({})));
    assert_eq!(listed["tasks"].as_array().unwrap().len(), 0);

    let missing = call(&fx, "everett_tasks", json!({"action": "done", "id": "t99"}));
    assert_eq!(missing["isError"], true, "{missing}");
    assert!(missing["content"][0]["text"].as_str().unwrap_or("").contains("t99"), "{missing}");
}

#[test]
fn tasks_list_does_not_create_a_lock() {
    let fx = fixture();
    let listed = tool_json(&call(&fx, "everett_tasks", json!({})));
    assert_eq!(listed["tasks"], json!([]));
    let board = std::path::Path::new(listed["board"].as_str().unwrap());
    assert!(!board.with_extension("lock").exists());
}

#[test]
fn tasks_board_guards() {
    let fx = fixture();
    let added = tool_json(&call(&fx, "everett_tasks", json!({"action": "add", "title": "first"})));
    let id = added["added"]["id"].as_str().unwrap().to_string();
    tool_json(&call(&fx, "everett_tasks", json!({"action": "claim", "id": id})));

    // A second claim must fail rather than hand the task to another owner.
    let again = call(&fx, "everett_tasks", json!({"action": "claim", "id": id}));
    assert_eq!(again["isError"], true, "{again}");

    // Removing a task must not free its id for the next add.
    tool_json(&call(&fx, "everett_tasks", json!({"action": "remove", "id": id})));
    let readded = tool_json(&call(&fx, "everett_tasks", json!({"action": "add", "title": "second"})));
    assert_ne!(readded["added"]["id"].as_str().unwrap(), id, "{readded}");
}

#[test]
fn tasks_board_rejects_symlink() {
    let fx = fixture();
    // The MCP server's cwd is the fixture home; make it the project root and
    // point .everett at another directory.
    std::fs::create_dir_all(fx.home().join(".git")).unwrap();
    let outside = fx.home().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, fx.home().join(".everett")).unwrap();
    let out = call(&fx, "everett_tasks", json!({"action": "list"}));
    assert_eq!(out["isError"], true, "{out}");
    assert!(out["content"][0]["text"].as_str().unwrap_or("").contains("symlink"), "{out}");
}

#[test]
fn search_finds_session_by_topic() {
    let fx = fixture();
    fx.write(
        ".claude/projects/app/worker.jsonl",
        &jsonl(&[json!({
            "type": "user", "sessionId": "worker", "cwd": "/work/app",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "message": {"role": "user", "content": "debug the websocket reconnect loop"}
        })]),
    );
    fx.write(
        ".claude/projects/app/other.jsonl",
        &jsonl(&[json!({
            "type": "user", "sessionId": "other", "cwd": "/work/app",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "message": {"role": "user", "content": "rename the settings page"}
        })]),
    );
    let result = tool_json(&call(&fx, "everett_search", json!({"query": "websocket reconnect"})));
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{result}");
    assert_eq!(results[0]["id"], "worker");
    assert!(results[0]["score"].as_f64().unwrap() > 0.0);
    assert!(results[0]["matched"].as_array().unwrap().iter().any(|t| t == "websocket"));
}

#[test]
fn peers_list_empty_and_sessions_requires_peer() {
    let fx = fixture();
    let listed = tool_json(&call(&fx, "everett_peers", json!({})));
    assert!(listed["peers"].is_object(), "{listed}");
    let missing = call(&fx, "everett_peers", json!({"action": "sessions"}));
    assert!(missing["error"]["message"].as_str().unwrap_or("").contains("peer"), "{missing}");
}

#[test]
fn broadcast_reports_per_target() {
    let fx = fixture();
    fx.write(
        ".claude/projects/app/worker.jsonl",
        &jsonl(&[json!({
            "type": "user", "sessionId": "worker", "cwd": "/work/app",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "message": {"role": "user", "content": "own the retry logic"}
        })]),
    );
    let result = tool_json(&call(
        &fx,
        "everett_broadcast",
        json!({"text": "status?", "targets": ["worker", "no-such-session"], "mode": "inbox"}),
    ));
    assert_eq!(result["sent"], 1, "{result}");
    assert_eq!(result["failed"], 1, "{result}");
    assert_eq!(result["results"][0]["ok"], true);
    assert_eq!(result["results"][1]["ok"], false);

    let bad = call(&fx, "everett_broadcast", json!({"text": "x", "targets": []}));
    assert!(bad["error"]["message"].as_str().unwrap_or("").contains("targets"), "{bad}");
}
