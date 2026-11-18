mod common;

use common::{fixture, jsonl, mcp_calls, tool_json};
use serde_json::{json, Value};

#[test]
fn failed_inbox_claim_returns_no_work_and_can_be_retried() {
    let fx = fixture();
    fx.write(".everett/inbox/worker.jsonl", &jsonl(&[json!({
        "id": "request", "kind": "message", "from": "caller", "to": "worker",
        "text": "Check the build", "ts": everett::session::now()
    })]));
    let done = fx.home().join(".everett/inbox/worker.done");
    std::fs::create_dir(&done).unwrap();
    let worker = [("EVERETT_SESSION_ID", "worker")];
    let cli = |args: &[&str]| fx.run_stdin(args, "", &worker);
    let mcp = |args| mcp_calls(&fx, &[], &worker, &[("everett_inbox", args)]).remove(0);

    for args in [
        vec!["inbox", "--session", "bad/session", "--json"],
        vec!["inbox", "--session", "bad/session", "--json", "--peek"],
    ] {
        let invalid = cli(&args);
        assert_eq!(invalid.status.code(), Some(2), "{invalid:?}");
        assert!(invalid.stdout.is_empty(), "{invalid:?}");
    }

    let rejected = cli(&["inbox", "--json"]);
    assert!(!rejected.status.success(), "{rejected:?}");
    assert!(rejected.stdout.is_empty(), "{rejected:?}");
    let rejected = mcp(json!({}));
    assert_eq!(rejected["isError"], true, "{rejected}");
    assert!(!rejected.to_string().contains("Check the build"), "{rejected}");

    let peek = cli(&["inbox", "--peek", "--json"]);
    assert!(peek.status.success(), "{peek:?}");
    let peek: Value = serde_json::from_slice(&peek.stdout).unwrap();
    assert_eq!(peek["messages"][0]["id"], "request");
    std::fs::remove_dir(&done).unwrap();

    let claimed = mcp(json!({}));
    assert_eq!(claimed["isError"], false, "{claimed}");
    assert_eq!(tool_json(&claimed)["messages"][0]["id"], "request");
    let empty = cli(&["inbox", "--json"]);
    let empty: Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(empty["messages"], json!([]));
    assert_eq!(std::fs::read_to_string(&done).unwrap(), "request\n");
}
