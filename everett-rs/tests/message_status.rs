mod common;

use common::{fixture, jsonl, Fixture};
use serde_json::{json, Value};

fn run(fx: &Fixture, caller: &str, args: &[&str]) -> String {
    let out = fx.run_stdin(args, "", &[("EVERETT_SESSION_ID", caller)]);
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    fx.stdout(&out)
}

fn run_json(fx: &Fixture, caller: &str, args: &[&str]) -> Value {
    serde_json::from_str(&run(fx, caller, args)).unwrap()
}

#[test]
fn sender_can_recover_lost_request_ids_without_reading_the_target_inbox() {
    let fx = fixture();
    let now = everett::session::now();
    let item = |id, sender, target, ts| json!({
        "id": id, "kind": "message", "from": sender, "to": target,
        "text": format!("private body {id}"), "ts": ts
    });
    let mut remote = item("remote", "sender", "worker", now);
    remote["relay_device"] = json!("peer-device");
    let mut reply = item("reply", "sender", "worker", now);
    reply["kind"] = json!("reply");
    fx.write(".everett/inbox/worker.jsonl", &jsonl(&[
        item("older", "sender", "worker", now - 60.0),
        item("newest", "sender", "worker", now),
        item("foreign", "other", "worker", now),
        item("expired", "sender", "worker", now - 8.0 * 86400.0),
        item("mismatched", "sender", "other", now),
        remote, reply,
    ]));
    let sent = run_json(&fx, "sender", &["sent", "--json"]);
    let requests = sent["requests"].as_array().unwrap();
    let ids: Vec<_> = requests.iter().map(|request| request["message_id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["newest", "older"]);
    assert_eq!(requests[0]["to"], "worker");
    assert!(!sent.to_string().contains("private body"));
    let status = run_json(&fx, "sender", &["status", ids[0], "--json"]);
    assert_eq!(status["state"], "queued");
    assert_eq!(run_json(&fx, "worker", &["sent", "--json"])["requests"], json!([]));
}

#[test]
fn sender_can_follow_a_local_request_and_recover_its_read_reply() {
    let fx = fixture();
    fx.write(
        ".claude/projects/p/worker.jsonl",
        &jsonl(&[json!({
            "type": "user", "sessionId": "worker", "cwd": fx.home(),
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "message": {"role": "user", "content": "Review upload retries"}
        })]),
    );
    let sessions = fx.ls_json();
    let worker = sessions[0]["id"].as_str().unwrap();
    assert_eq!(worker, "worker");

    let sent = run_json(&fx, "sender", &["send", "Check the retry cap", "--to", worker, "--mode", "inbox", "--json"]);
    let id = sent["message_id"].as_str().unwrap();
    let status = || run_json(&fx, "sender", &["status", id, "--json"]);
    let queued = status();
    assert_eq!(queued["message_id"], id);
    assert_eq!(queued["to"], worker);
    assert_eq!(queued["state"], "queued");
    assert_eq!(queued["picked_up"], false);
    assert!(queued["reply"].is_null());

    let denied = fx.run_stdin(&["status", id, "--json"], "", &[("EVERETT_SESSION_ID", worker)]);
    let unknown = fx.run_stdin(&["status", "m0000000000", "--json"], "", &[("EVERETT_SESSION_ID", worker)]);
    assert_eq!(denied.status.code(), Some(2));
    assert_eq!(denied.stderr, unknown.stderr);
    assert!(denied.stdout.is_empty());

    let read = run_json(&fx, worker, &["inbox", "--message", id, "--json"]);
    assert_eq!(read["messages"][0]["id"], id);
    let picked_up = status();
    assert_eq!(picked_up["state"], "picked_up");
    assert_eq!(picked_up["picked_up"], true);
    assert!(picked_up["reply"].is_null());

    let answer = "The retry cap is five.";
    run(&fx, worker, &["reply", id, answer]);
    let replied = status();
    assert_eq!(replied["state"], "replied");
    assert_eq!(replied["picked_up"], true);
    assert_eq!(replied["reply"]["reply_to"], id);
    assert_eq!(replied["reply"]["text"], answer);
    let reply_id = replied["reply"]["id"].as_str().unwrap();

    let read_reply = run_json(&fx, "sender", &["inbox", "--message", reply_id, "--json"]);
    assert_eq!(read_reply["messages"][0]["id"], reply_id);
    let empty = run_json(&fx, "sender", &["inbox", "--message", reply_id, "--json"]);
    assert_eq!(empty["messages"], json!([]));
    assert_eq!(status(), replied);
    let text = run(&fx, "sender", &["status", id]);
    assert!(text.contains("REPLIED") && text.contains(id) && text.contains(answer), "{text}");

    let sent = run_json(&fx, "sender", &["send", "Confirm the backoff", "--to", worker, "--mode", "inbox", "--json"]);
    let id = sent["message_id"].as_str().unwrap();
    let terminal_reply = "The backoff is exponential.";
    let replied = fx.run(&["reply", id, terminal_reply]);
    assert!(replied.status.success(), "{replied:?}");
    let inbox = run_json(&fx, "sender", &["inbox", "--json"]);
    assert_eq!(inbox["messages"][0]["reply_to"], id);
    assert_eq!(inbox["messages"][0]["from"], "human");
    let status = || run_json(&fx, "sender", &["status", id, "--json"]);
    let replied = status();
    assert_eq!(replied["state"], "replied");
    assert_eq!(replied["picked_up"], false);
    assert_eq!(replied["reply"]["from"], "human");
    assert_eq!(replied["reply"]["text"], terminal_reply);

    let before = run(&fx, "sender", &["status", id]);
    assert!(before.contains("Inbox pickup unconfirmed") && before.contains("Reply from human"), "{before}");
    run(&fx, worker, &["inbox", "--message", id, "--json"]);
    let after = run(&fx, "sender", &["status", id]);
    assert!(after.contains("Inbox pickup observed") && after.contains("Reply from human"), "{after}");

    run(&fx, worker, &["reply", id, "Confirmed in the code."]);
    let replied = status();
    assert_eq!(replied["reply"]["from"], worker);
    assert_eq!(replied["picked_up"], true);
}
