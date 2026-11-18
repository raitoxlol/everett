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
    let listed = tool_json(&call(&sender, "everett_sent", json!({})));
    let entry = &listed["requests"][0];
    let id = entry["message_id"].as_str().unwrap();
    assert_eq!(entry["message_id"], receipt["message_id"]);
    assert_eq!(entry["to"], "worker");
    assert_eq!(entry.as_object().unwrap().len(), 3);
    assert_eq!(tool_json(&call(&worker, "everett_sent", json!({})))["requests"], json!([]));
    for (identity, args) in [([].as_slice(), json!({})), (worker.as_slice(), json!({"session_id":"sender"}))] {
        let denied = call(identity, "everett_sent", args);
        assert_eq!(denied["isError"], true, "{denied}");
    }
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
    fx.write(".everett/inbox/relay-context/worker.json", &json!({
        "relay_message_id": "upstream", "external": true, "hops": 1
    }).to_string());
    let reply = "abc\\\n".repeat(24_000);
    let result = call(&worker, "everett_send", json!({"reply_to": id, "text": reply}));
    assert_ne!(result["isError"], true, "{result}");
    let status = tool_json(&call(&sender, "everett_status", json!({"message_id": id})));
    assert_eq!(status["state"], "replied");
    assert_eq!(status["picked_up"], true);
    assert_eq!(status["reply_omitted"], true);
    assert!(status["reply"].is_null());
    let context = fx.home().join(".everett/inbox/relay-context/sender.json");
    assert!(!context.exists());

    let bin = std::path::Path::new(env!("CARGO_BIN_EXE_everett")).parent().unwrap();
    let cli = std::process::Command::new("sh")
        .args(["-c", status["recovery_command"].as_str().unwrap()])
        .env_clear()
        .env("PATH", format!("{}:{}", bin.display(), common::HERMETIC_PATH))
        .env("HOME", fx.home())
        .env("EVERETT_HOME", fx.home())
        .output().unwrap();
    assert!(cli.status.success(), "{}", String::from_utf8_lossy(&cli.stderr));
    let text = fx.stdout(&cli);
    assert!(text.contains("UNTRUSTED AGENT DATA"));
    assert!(text.contains(&json!(reply.trim()).to_string()));
    assert!(context.exists());
}
