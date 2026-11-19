mod common;

use common::{fixture, jsonl, Fixture};
use serde_json::{json, Value};

fn run(fx: &Fixture, caller: &str, args: &[&str]) -> Value {
    let result = fx.run_stdin(args, "", &[("EVERETT_SESSION_ID", caller)]);
    assert!(result.status.success(), "{args:?}: {result:?}");
    serde_json::from_str(&fx.stdout(&result)).unwrap()
}

#[test]
fn dashboard_tracks_real_local_pickup_reply_and_independent_card() {
    let fx = fixture();
    fx.write(".claude/projects/p/worker.jsonl", &jsonl(&[json!({
        "type":"user","sessionId":"worker","cwd":fx.home(),
        "timestamp":chrono::Utc::now().to_rfc3339(),
        "message":{"role":"user","content":"Test the dashboard"}
    })]));
    fx.write(".everett/cards/worker.md", "What: Retry audit\nState: Checking cases\nNext: Report the cap\n");
    let request = run(&fx, "sender", &["send", "Check retry cap", "--to", "worker", "--mode", "inbox", "--json"]);
    let id = request["message_id"].as_str().unwrap();
    let status = || run(&fx, "sender", &["status", id, "--json"]);
    let queued = status();
    assert_eq!(queued["schema_version"], 1);
    assert_eq!(queued["delivery"]["state"], "queued");
    let queue_time = queued["delivery"]["evidence"]["queued_at_ms"].as_i64().unwrap();
    assert_eq!(queued["agent"]["what"], "Retry audit");
    assert_eq!(queued["agent"]["freshness"], "current");
    assert_eq!(queued["agent"]["state"], "Checking cases");
    assert_eq!(queued["agent"]["next"], "Report the cap");
    assert!(queued["agent"]["updated_at_ms"].is_i64());
    assert_eq!(run(&fx, "worker", &["inbox", "--message", id, "--json"])["messages"][0]["id"], id);
    let pickup = status();
    assert_eq!(pickup["delivery"]["state"], "picked_up");
    let picked_up_at = pickup["delivery"]["evidence"]["picked_up_at_ms"].as_i64().unwrap();
    assert!(picked_up_at >= queue_time);
    let reply = fx.run_stdin(&["reply", id, "The retry cap is five."], "", &[("EVERETT_SESSION_ID", "worker")]);
    assert!(reply.status.success(), "{reply:?}");
    let replied = status();
    assert_eq!(replied["delivery"]["state"], "replied");
    assert_eq!(replied["reply"]["reply_to"], id);
    assert_eq!(replied["reply"]["text"], "The retry cap is five.");
    assert!(replied["delivery"]["evidence"]["replied_at_ms"].as_i64().unwrap() >= picked_up_at);
    assert_eq!(replied["agent"], queued["agent"]);

    let card = fx.home().join(".everett/cards/worker.md");
    let file = std::fs::File::options().write(true).open(card).unwrap();
    file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(90_000)).unwrap();
    let stale = status();
    assert_eq!(stale["delivery"], replied["delivery"]);
    assert_eq!(stale["agent"]["state"], "unknown");
    assert_eq!(stale["agent"]["freshness"], "stale");
    assert!(stale["agent"]["what"].is_null() && stale["agent"]["next"].is_null());
}

#[test]
fn dashboard_does_not_fabricate_legacy_or_expiry_evidence() {
    let fx = fixture();
    let now = everett::session::now();
    let request = |id, ts| json!({"id":id,"kind":"message","from":"sender","to":"worker","text":"check","ts":ts});
    fx.write(".everett/inbox/worker.jsonl", &jsonl(&[
        request("old", json!(now - 86400.0 * 8.0)),
        request("claimed", json!(now)),
        request("uncertain", Value::Null),
    ]));
    fx.write(".everett/inbox/worker.done", "claimed\n");
    let status = |id| run(&fx, "sender", &["status", id, "--json"]);
    assert_eq!(status("old")["delivery"]["state"], "expired");
    assert!(status("old")["delivery"]["evidence"]["expires_at_ms"].is_i64());
    let claimed = status("claimed");
    assert_eq!(claimed["delivery"]["state"], "picked_up");
    assert_eq!(claimed["delivery"]["evidence"].get("picked_up_at_ms"), Some(&Value::Null));
    assert_eq!(status("uncertain")["delivery"]["state"], "unknown");
    assert_eq!(status("uncertain")["agent"]["freshness"], "unknown");
}
