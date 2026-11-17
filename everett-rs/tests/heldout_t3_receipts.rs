//! HELD-OUT verifier cases for native T3 dispatch receipts (ping_as + control::send_t3).
//! Synthetic fake `t3` only; disposable HOME. Each case prints HELDOUT <name> <PASS|FAIL> via assert.
mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::lib_home;
use everett::{control, inbox, send, session::Session};
use rusqlite::Connection;
use serde_json::json;

const SESSION: &str = "ho-provider-session";
const THREAD: &str = "ho-thread";
const OTHER_SESSION: &str = "ho-other-session";
const OTHER_THREAD: &str = "ho-other-thread";

fn session(id: &str) -> Session {
    let mut s = Session::new("codex", id, "/work/project", "", "", 0.0);
    s.source = "t3code".into();
    s
}

fn plant(home: &Path) {
    let path = home.join(".t3/userdata/statev2.sqlite");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let con = Connection::open(path).unwrap();
    con.execute_batch(include_str!("../../tests/fixtures/t3_v2.sql")).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    con.execute("INSERT INTO projection_projects VALUES ('p','Project','/work/project',NULL,'[]',?1,?1,NULL)", [&now]).unwrap();
    for (thread, prov, native) in [(THREAD, "prov-a", SESSION), (OTHER_THREAD, "prov-b", OTHER_SESSION)] {
        con.execute(
            "INSERT INTO orchestration_v2_projection_threads \
             (thread_id,project_id,title,default_provider,runtime_mode,interaction_mode,active_provider_thread_id,created_at,updated_at,payload_json) \
             VALUES (?1,'p','held-out','codex','full-access','default',?2,?3,?3,'{}')",
            [thread, prov, &now],
        ).unwrap();
        con.execute(
            "INSERT INTO orchestration_v2_projection_provider_threads \
             (provider_thread_id,thread_id,provider,driver,status,updated_at,payload_json) \
             VALUES (?1,?2,'codex','codex','idle',?3,?4)",
            [prov, thread, &now, &json!({"driver":"codex","nativeThreadRef":{"driver":"codex","nativeId":native,"strength":"strong"}}).to_string()],
        ).unwrap();
    }
}

/// Fake t3: appends one line per invocation to $HOME/t3-calls, then prints $HOME/t3-out
/// (one stdout per call, consumed in order from t3-out.N, falling back to t3-out) and exits $HOME/t3-exit.
fn fake(home: &Path) -> PathBuf {
    let p = home.join("fake-t3");
    fs::write(&p, r#"#!/bin/sh
n=$(( $(cat "$HOME/t3-calls" 2>/dev/null | wc -l) + 1 ))
printf '%s\n' "$*" | tr '\n' ' ' >> "$HOME/t3-calls"; echo >> "$HOME/t3-calls"
if [ -f "$HOME/t3-out.$n" ]; then cat "$HOME/t3-out.$n"; elif [ -f "$HOME/t3-out" ]; then cat "$HOME/t3-out"; fi
exit $(cat "$HOME/t3-exit" 2>/dev/null || echo 0)
"#).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var("EVERETT_T3_BIN", &p);
    p
}

fn out(home: &Path, n: Option<usize>, s: &str) {
    let name = n.map(|n| format!("t3-out.{n}")).unwrap_or("t3-out".into());
    fs::write(home.join(name), s).unwrap();
}
fn calls(home: &Path) -> Vec<String> {
    fs::read_to_string(home.join("t3-calls")).unwrap_or_default().lines().map(str::to_string).collect()
}
fn receipt(thread: &str, delivery: &str, status: &str) -> String {
    json!({"threadId":thread,"messageId":"m-1","runId":"r-1","status":status,"delivery":delivery}).to_string()
}
fn setup() -> (common::LibHome, PathBuf) {
    let h = lib_home();
    let p = h.path();
    plant(&p);
    fake(&p);
    (h, p)
}

// 1. normal
#[test]
fn ho_normal_single_dispatch_single_inbox() {
    let (_h, p) = setup();
    out(&p, None, &receipt(THREAD, "started", "running"));
    let r = send::ping_as(&session(SESSION), "task A", 30.0, Some("caller"), None).unwrap();
    assert_eq!(r["wake"]["thread_id"], THREAD);
    assert_eq!(calls(&p).len(), 1, "exactly one t3 dispatch");
    assert_eq!(inbox::pending(SESSION, None).len(), 1);
    assert!(inbox::pending(OTHER_SESSION, None).is_empty(), "no cross-thread misdelivery");
}

// 2. busy thread -> queued is success, not duplicated
#[test]
fn ho_busy_thread_queued_receipt_accepted_once() {
    let (_h, p) = setup();
    out(&p, None, &receipt(THREAD, "queued", "queued"));
    let r = send::ping_as(&session(SESSION), "task B", 30.0, Some("caller"), None).unwrap();
    assert_eq!(r["pinged"], true);
    assert_eq!(calls(&p).len(), 1);
    assert_eq!(inbox::pending(SESSION, None).len(), 1);
}

// 3. lost response: dispatch happened (exit 0) but no stdout -> must not report success
#[test]
fn ho_lost_response_is_unconfirmed_not_success() {
    let (_h, p) = setup();
    out(&p, None, "");
    let e = send::ping_as(&session(SESSION), "task C", 30.0, Some("caller"), None).unwrap_err();
    assert_eq!(e.code, 4);
    assert!(e.message.contains("not blindly retry") || e.message.contains("unconfirmed"), "{}", e.message);
    let pend = inbox::pending(SESSION, None);
    assert_eq!(pend.len(), 1, "message retained exactly once");
    assert!(e.message.contains(pend[0]["id"].as_str().unwrap()), "error names retained id");
    assert_eq!(calls(&p).len(), 1, "no hidden internal retry");
}

// 4. malformed receipts -> rejected, never success
#[test]
fn ho_malformed_receipts_rejected() {
    let bad = [
        "not json".to_string(),
        "{\"threadId\":\"ho-thread\"".to_string(),
        json!({"threadId":THREAD,"runId":"r","status":"running","delivery":"started"}).to_string(), // no messageId
        json!({"threadId":THREAD,"messageId":"m","runId":"","status":"running","delivery":"started"}).to_string(),
        json!({"threadId":THREAD,"messageId":"m","runId":"r","status":"running","delivery":"rejected"}).to_string(),
        json!({"threadId":THREAD,"messageId":"m","runId":"r","status":"bogus","delivery":"started"}).to_string(),
        json!({"threadId":THREAD,"messageId":"m","runId":"r","status":"running","delivery":"started","error":"x"}).to_string(),
        json!({"threadId":THREAD,"messageId":"m","runId":"r","status":"running","delivery":"started","accepted":false}).to_string(),
        json!([{"threadId":THREAD}]).to_string(),
    ];
    for b in bad {
        let (_h, p) = setup();
        out(&p, None, &b);
        let r = send::ping_as(&session(SESSION), "task D", 30.0, Some("caller"), None);
        assert!(r.is_err(), "false success on {b}");
        assert_eq!(calls(&p).len(), 1);
        drop(_h);
    }
}

// 5. wrong-target receipt (other thread's thread) -> rejected
#[test]
fn ho_wrong_target_receipt_rejected_ping_and_control() {
    let (_h, p) = setup();
    out(&p, None, &receipt(OTHER_THREAD, "started", "running"));
    assert!(send::ping_as(&session(SESSION), "task E", 30.0, Some("caller"), None).is_err());
    assert!(control::send_t3(THREAD, None, "task E2", false, Some(10.0), Some("caller")).is_err());
    assert!(inbox::pending(OTHER_SESSION, None).is_empty(), "no cross-thread misdelivery");
    for c in calls(&p) {
        assert!(c.contains(THREAD) && !c.contains(OTHER_THREAD), "argv targeted wrong thread: {c}");
    }
}

// 6. nonzero exit with valid-looking receipt -> not success
#[test]
fn ho_nonzero_exit_with_receipt_rejected() {
    let (_h, p) = setup();
    out(&p, None, &receipt(THREAD, "started", "running"));
    fs::write(p.join("t3-exit"), "1").unwrap();
    assert!(send::ping_as(&session(SESSION), "task F", 30.0, Some("caller"), None).is_err());
    assert_eq!(inbox::pending(SESSION, None).len(), 1);
}

// 7. Independent identical-text calls are distinct requests (no text dedup in the contract).
// Baseline protocol carries no request/idempotency id, so a true same-ID retry is not expressible here.
#[test]
fn ho_independent_identical_text_calls_are_distinct_requests() {
    let (_h, p) = setup();
    out(&p, Some(1), "");
    out(&p, Some(2), &receipt(THREAD, "started", "running"));
    let _ = send::ping_as(&session(SESSION), "same text", 30.0, Some("caller"), None).unwrap_err();
    let ok = send::ping_as(&session(SESSION), "same text", 30.0, Some("caller"), None).unwrap();
    let pend = inbox::pending(SESSION, None);
    assert_eq!(pend.len(), 2, "each independent call keeps its own durable message");
    assert_eq!(calls(&p).len(), 2, "one dispatch per call, no hidden retry");
    assert_ne!(pend[0]["id"], pend[1]["id"]);
    assert!(calls(&p)[1].contains(ok["message_id"].as_str().unwrap()), "dispatch correlates to its own attempt");
}

// 8. Restart: unconfirmed attempt stays durable and findable by id exactly once (no promised attempt ledger).
#[test]
fn ho_restart_unconfirmed_message_durable_once() {
    let (_h, p) = setup();
    out(&p, None, "");
    let e = send::ping_as(&session(SESSION), "task R", 30.0, Some("caller"), None).unwrap_err();
    let pend = inbox::pending(SESSION, None);
    assert_eq!(pend.len(), 1);
    let mid = pend[0]["id"].as_str().unwrap().to_string();
    assert!(e.message.contains(&mid));
    assert!(inbox::find(&mid).is_some(), "durable record survives a fresh lookup");
    assert_eq!(calls(&p).len(), 1);
}

// 9. correlated completions: two threads (same HOME = same owner) receive their own receipts.
#[test]
fn ho_two_threads_receipts_correlate_per_attempt() {
    let (_h, p) = setup();
    out(&p, Some(1), &receipt(THREAD, "started", "running"));
    out(&p, Some(2), &receipt(OTHER_THREAD, "queued", "queued"));
    let a = send::ping_as(&session(SESSION), "for A", 30.0, Some("caller"), None).unwrap();
    let b = send::ping_as(&session(OTHER_SESSION), "for B", 30.0, Some("caller"), None).unwrap();
    assert_eq!(a["wake"]["thread_id"], THREAD);
    assert_eq!(b["wake"]["thread_id"], OTHER_THREAD);
    assert_eq!(inbox::pending(SESSION, None).len(), 1);
    assert_eq!(inbox::pending(OTHER_SESSION, None).len(), 1);
    assert_ne!(a["message_id"], b["message_id"]);
    let c = calls(&p);
    assert!(c[0].contains(THREAD) && c[0].contains(a["message_id"].as_str().unwrap()));
    assert!(c[1].contains(OTHER_THREAD) && c[1].contains(b["message_id"].as_str().unwrap()));
}

// 10. receipt correlation: receipt for wrong thread on second thread's attempt (swapped responses)
#[test]
fn ho_swapped_receipts_rejected() {
    let (_h, p) = setup();
    out(&p, Some(1), &receipt(OTHER_THREAD, "started", "running"));
    out(&p, Some(2), &receipt(THREAD, "started", "running"));
    assert!(send::ping_as(&session(SESSION), "for A", 30.0, Some("caller"), None).is_err());
    assert!(send::ping_as(&session(OTHER_SESSION), "for B", 30.0, Some("caller"), None).is_err());
}
