mod common;

use common::{fixture, jsonl, mcp_calls, tool_json};
use serde_json::{json, Value};

#[test]
fn cli_and_stdio_can_read_a_reply_without_consuming_other_messages() {
    let fx = fixture();
    fx.write(
        ".claude/projects/p/worker.jsonl",
        &jsonl(&[json!({
            "type": "user", "sessionId": "worker", "cwd": fx.home(),
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "message": {"role": "user", "content": "Review the upload retries"}
        })]),
    );
    let sender = [("EVERETT_SESSION_ID", "sender")];
    let worker = [("EVERETT_SESSION_ID", "worker")];
    let mut ids = Vec::new();
    for subject in ["uploads", "retries", "timeouts"] {
        let sent = fx.run_stdin(&["send", subject, "--to", "worker", "--mode", "inbox", "--json"], "", &sender);
        assert!(sent.status.success(), "{sent:?}");
        let receipt: Value = serde_json::from_slice(&sent.stdout).unwrap();
        let id = receipt["message_id"].as_str().unwrap();
        let reply = fx.run_stdin(&["reply", id, subject], "", &worker);
        assert!(reply.status.success(), "{reply:?}");
        ids.push(id.to_string());
    }

    let cli = |args: &[&str]| {
        let out = fx.run_stdin(args, "", &sender);
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["messages"].clone()
    };
    let mcp = |args: Value| {
        let result = mcp_calls(&fx, &[], &sender, &[("everett_inbox", args)]).remove(0);
        assert_eq!(result["isError"], false, "{result}");
        tool_json(&result)["messages"].clone()
    };

    let cli_peek = cli(&["inbox", "--reply-to", &ids[0], "--peek", "--json"]);
    assert_eq!(cli_peek.as_array().unwrap().len(), 1);
    assert_eq!(cli_peek[0]["reply_to"], ids[0]);
    assert_eq!(cli(&["inbox", "--reply-to", &ids[0], "--json"]), cli_peek);
    assert_eq!(cli(&["inbox", "--reply-to", &ids[0], "--json"]), json!([]));

    let mcp_peek = mcp(json!({"reply_to": ids[1], "peek": true}));
    assert_eq!(mcp_peek.as_array().unwrap().len(), 1);
    assert_eq!(mcp_peek[0]["reply_to"], ids[1]);
    assert_eq!(mcp(json!({"reply_to": ids[1]})), mcp_peek);
    assert_eq!(mcp(json!({"reply_to": ids[1]})), json!([]));

    let pending = cli(&["inbox", "--peek", "--json"]);
    assert_eq!(pending.as_array().unwrap().len(), 1);
    assert_eq!(pending[0]["reply_to"], ids[2]);
}
