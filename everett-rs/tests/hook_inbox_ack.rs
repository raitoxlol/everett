mod common;

use common::{fixture, jsonl};
use serde_json::{json, Value};
use std::fs;

fn assert_hook_retries_failed_acknowledgment(args: &[&str], stdin: &str) {
    let fx = fixture();
    fx.write(".everett/inbox/worker.jsonl", &jsonl(&[json!({
        "id": "request", "kind": "message", "from": "caller", "to": "worker",
        "text": "Check the build", "ts": everett::session::now()
    })]));
    let done = fx.home().join(".everett/inbox/worker.done");
    fs::create_dir(&done).unwrap();
    let hook = || {
        let out = fx.run_stdin(args, stdin, &[]);
        assert!(out.status.success(), "{out:?}");
        fx.stdout(&out)
    };
    let status = || {
        let out = fx.run_stdin(&["status", "request", "--json"], "", &[("EVERETT_SESSION_ID", "caller")]);
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };

    let blocked = hook();
    let queued = status();
    assert_eq!(queued["state"], "queued", "{queued}");
    assert_eq!(queued["picked_up"], false, "{queued}");
    assert!(blocked.is_empty(), "hook returned unacknowledged work: {blocked}");

    fs::remove_dir(&done).unwrap();
    let delivered = hook();
    assert_eq!(delivered.matches("Check the build").count(), 1, "{delivered}");
    let picked_up = status();
    assert_eq!(picked_up["state"], "picked_up", "{picked_up}");
    assert_eq!(picked_up["picked_up"], true, "{picked_up}");
    assert!(hook().is_empty());
    assert_eq!(fs::read_to_string(&done).unwrap(), "request\n");
}

#[test]
fn claude_hook_retries_failed_acknowledgment() {
    assert_hook_retries_failed_acknowledgment(
        &["hook", "claude_inbox"],
        r#"{"session_id":"worker","hook_event_name":"PostToolUse","tool_name":"Bash"}"#,
    );
}

#[test]
fn omp_hook_retries_failed_acknowledgment() {
    assert_hook_retries_failed_acknowledgment(&["hook", "omp_inbox", "worker"], "");
}
