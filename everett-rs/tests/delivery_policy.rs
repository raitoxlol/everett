mod common;

use common::{fixture, jsonl, mcp_calls, tool_json, Fixture};
use serde_json::{json, Value};

fn live_target() -> Fixture {
    let fx = fixture();
    fx.write(
        ".claude/projects/p/policy-target.jsonl",
        &jsonl(&[json!({
            "type":"user", "sessionId":"policy-target", "cwd":fx.home(),
            "timestamp":chrono::Utc::now().to_rfc3339(),
            "message":{"role":"user", "content":"Review the policy task"}
        })]),
    );
    fx.write(
        ".everett/inbox/live/policy-target.json",
        &json!({"pid":std::process::id(), "harness":"claude", "ts":everett::session::now()})
            .to_string(),
    );
    fx
}

#[test]
fn automatic_cli_send_refuses_passive_delivery_without_enqueuing() {
    let fx = live_target();
    let out = fx.run(&[
        "send",
        "Review this request",
        "--to",
        "policy-target",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(4));
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(error.contains("Nothing was queued"), "{error}");
    assert!(error.contains("--mode inbox"), "{error}");
    assert!(error.contains("policy-target"), "{error}");
    assert!(!fx
        .home()
        .join(".everett/inbox/policy-target.jsonl")
        .exists());
}

#[test]
fn automatic_mcp_send_refuses_passive_delivery_without_enqueuing() {
    let fx = live_target();
    let results = mcp_calls(
        &fx,
        &[],
        &[("EVERETT_SESSION_ID", "policy-sender")],
        &[(
            "everett_send",
            json!({"to":"policy-target", "text":"Review this request"}),
        )],
    );
    assert_eq!(results[0]["isError"], true, "{}", results[0]);
    assert!(results[0].to_string().contains("Nothing was queued"));
    assert!(!fx
        .home()
        .join(".everett/inbox/policy-target.jsonl")
        .exists());
}

#[test]
fn explicit_queue_is_truthful_and_mcp_read_clears_it_once() {
    let fx = live_target();
    let out = fx.run(&[
        "send",
        "Review this request",
        "--to",
        "policy-target",
        "--mode",
        "inbox",
        "--json",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let queued: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(queued["queued"], true);
    assert_eq!(queued["delivered"], false);
    assert_eq!(queued["wake"], false);
    let calls = mcp_calls(
        &fx,
        &[],
        &[("EVERETT_SESSION_ID", "policy-target")],
        &[
            ("everett_inbox", json!({"peek":true})),
            ("everett_inbox", json!({})),
            ("everett_inbox", json!({})),
        ],
    );
    assert_eq!(
        tool_json(&calls[0])["messages"][0]["id"],
        queued["message_id"]
    );
    assert_eq!(
        tool_json(&calls[1])["messages"][0]["text"],
        "Review this request"
    );
    assert_eq!(tool_json(&calls[2])["messages"], json!([]));
}

#[test]
fn explicit_queue_dry_run_never_publishes_or_claims_pickup() {
    let fx = live_target();
    let out = fx.run(&[
        "send",
        "Review this request",
        "--to",
        "policy-target",
        "--mode",
        "inbox",
        "--wait",
        "60",
        "--dry-run",
        "--json",
    ]);
    assert!(out.status.success());
    let data: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(data["dry_run"], true);
    assert_eq!(data["queued"], false);
    assert_eq!(data["delivered"], false);
    assert!(!fx
        .home()
        .join(".everett/inbox/policy-target.jsonl")
        .exists());
}

#[test]
fn explicit_queue_wait_reports_observed_pickup_and_reply() {
    let fx = live_target();
    std::thread::scope(|scope| {
        let sending = scope.spawn(|| {
            fx.run(&[
                "send",
                "Review this request",
                "--to",
                "policy-target",
                "--mode",
                "inbox",
                "--wait",
                "5",
                "--json",
            ])
        });
        let path = fx.home().join(".everett/inbox/policy-target.jsonl");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::fs::read_to_string(&path)
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "send did not publish its explicit request"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let read = fx.run(&["inbox", "--session", "policy-target", "--json"]);
        assert!(read.status.success());
        let read: Value = serde_json::from_slice(&read.stdout).unwrap();
        let mid = read["messages"][0]["id"].as_str().unwrap();
        assert_eq!(read["messages"][0]["text"], "Review this request");
        let reply = fx.run_stdin(
            &["reply", mid, "Reviewed"],
            "",
            &[("EVERETT_SESSION_ID", "policy-target")],
        );
        assert!(reply.status.success());
        let out = sending.join().unwrap();
        assert!(out.status.success());
        let sent: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(sent["message_id"], mid);
        assert_eq!(sent["reply"], "Reviewed");
        assert_eq!(sent["delivered"], true);
        assert_eq!(sent["queued"], false);
        assert_eq!(sent["wake"], false);
        assert!(fx.stdout(&fx.run(&["inbox"])).contains("empty"));
    });
}
