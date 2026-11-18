mod common;

use common::{fixture, jsonl, mcp_calls, tool_json};
use serde_json::json;

#[test]
fn local_mcp_sender_can_follow_pickup_and_recover_a_consumed_reply() {
    let fx = fixture();
    fx.write(
        ".claude/projects/p/worker.jsonl",
        &jsonl(&[json!({
            "type": "user", "sessionId": "worker", "cwd": fx.home(),
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "message": {"role": "user", "content": "Review upload retries"}
        })]),
    );
    let sender = [("EVERETT_SESSION_ID", "sender")];
    let worker = [("EVERETT_SESSION_ID", "worker")];
    let call = |identity: &[(&str, &str)], name, args| {
        mcp_calls(&fx, &[], identity, &[(name, args)]).remove(0)
    };
    let sent = call(&sender, "everett_send", json!({"to": "worker", "text": "Check the retry cap", "mode": "inbox"}));
    let receipt = tool_json(&sent);
    let id = receipt["message_id"].as_str().unwrap();
    let args = json!({"message_id": id});
    let status = || tool_json(&call(&sender, "everett_status", args.clone()));
    assert_eq!(status()["state"], "queued");

    for (identity, request) in [
        (worker.as_slice(), args.clone()),
        (worker.as_slice(), json!({"message_id": id, "session_id": "sender"})),
        ([].as_slice(), json!({"message_id": id, "session_id": "sender"})),
        ([].as_slice(), args.clone()),
    ] {
        let denied = call(identity, "everett_status", request);
        assert_eq!(denied["isError"], true, "{denied}");
    }

    let picked_up = call(&worker, "everett_inbox", args.clone());
    assert_eq!(tool_json(&picked_up)["messages"][0]["id"], id);
    assert_eq!(status()["state"], "picked_up");
    assert_eq!(status()["picked_up"], true);

    let reply = "The retry cap is five.";
    let result = call(&worker, "everett_send", json!({"reply_to": id, "text": reply}));
    assert_ne!(result["isError"], true, "{result}");
    let before = status();
    assert_eq!(before["state"], "replied");
    assert_eq!(before["reply"]["reply_to"], id);
    assert_eq!(before["reply"]["text"], reply);
    let inbox = call(&sender, "everett_inbox", json!({}));
    assert_eq!(tool_json(&inbox)["messages"][0]["reply_to"], id);
    assert_eq!(status(), before);

    let sent = call(&sender, "everett_send", json!({"to": "worker", "text": "Send the build output", "mode": "inbox"}));
    let receipt = tool_json(&sent);
    let id = receipt["message_id"].as_str().unwrap();
    let reply = "abc\\\n".repeat(24_000);
    let result = call(&worker, "everett_send", json!({"reply_to": id, "text": reply}));
    assert_ne!(result["isError"], true, "{result}");
    let status = call(&sender, "everett_status", json!({"message_id": id}));
    assert_eq!(status["isError"], true, "{status}");
    assert!(status.to_string().contains("exceeds the MCP response limit"), "{status}");
    let cli = fx.run_stdin(&["inbox", "--peek", "--json"], "", &sender);
    let inbox: serde_json::Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(inbox["messages"][0]["reply_to"], id);
    assert_eq!(inbox["messages"][0]["text"], reply.trim());
}
