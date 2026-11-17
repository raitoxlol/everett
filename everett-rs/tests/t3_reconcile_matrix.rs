//! Deterministic fault-injection matrix for T3 request -> dispatch receipt ->
//! correlation, driven through `everett_control` against a scripted fake `t3`.
//! Synthetic only: it proves Everett's handling of receipts, not real-client behavior.
//!
//! Score per case: a correlated confirmation when the fault-free receipt matches the
//! request's thread, or an explicit "unconfirmed" error when it cannot. Invariants:
//! no extra CLI dispatches, no false successes, and refusal of remote-derived callers.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use common::{fixture, mcp_calls, tool_json};
use serde_json::{json, Value};

const THREAD: &str = "thread-123";
const OTHER: &str = "thread-other";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Expect {
    Confirmed,
    Unconfirmed,
    Refused,
}

struct Case {
    name: &'static str,
    mode: &'static str,
    request: Value,
    /// CLI invocations this request may make (a read after uncertainty is one `show`).
    invocations: usize,
    remote: bool,
    expect: Expect,
}

fn fake_t3(fx: &common::Fixture) -> String {
    let send = json!({"threadId":THREAD,"messageId":"message-1","runId":"run-1","status":"running","delivery":"started"});
    let queued = json!({"threadId":THREAD,"messageId":"message-2","runId":"run-0","status":"queued","delivery":"queued"});
    let wrong = json!({"threadId":OTHER,"messageId":"message-9","runId":"run-9","status":"running","delivery":"started"});
    let start = json!({"threadId":THREAD,"projectId":"project-1","runId":"run-1"});
    let start_wrong_shape = json!({"threadId":THREAD,"projectId":"project-1","runId":""});
    let waited = json!({"threadId":THREAD,"runId":"run-1","status":"completed","reply":"done","error":null});
    let stale = json!({"threadId":OTHER,"runId":"run-old","status":"completed","reply":"old reply","error":null});
    let show = json!({"threadId":THREAD,"messages":[{"role":"user","text":"next step"}]});
    let script = format!(r#"#!/bin/sh
mkdir -p "$HOME/t3-log"
printf '%s\n' "$@" > "$(mktemp "$HOME/t3-log/c.XXXXXX")"
mode=$(cat "$HOME/t3-mode")
case "$2" in start) ok='{start}' ;; show) ok='{show}' ;; *) ok='{send}' ;; esac
for arg in "$@"; do [ "$arg" = --wait ] && ok='{waited}'; done
case "$mode" in
ok) printf '%s\n' "$ok" ;;
lost) sleep 4; printf '%s\n' "$ok" ;;
crash) printf '%s\n' "$ok"; exit 1 ;;
partial) printf '%s' '{{"threadId":"{THREAD}","messageId"' ;;
wrong) printf '%s\n' '{wrong}' ;;
queued) printf '%s\n' '{queued}' ;;
stale) printf '%s\n' '{stale}' ;;
two_docs) printf '%s\n%s\n' "$ok" '{wrong}' ;;
empty_run) printf '%s\n' '{start_wrong_shape}' ;;
exit2) printf '%s\n' "$ok"; exit 2 ;;
noise) printf 'WARN migration\n%s\n' "$ok" ;;
esac
"#);
    let path = fx.home().join("fake-t3");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path.to_string_lossy().into_owned()
}

fn invocations(fx: &common::Fixture) -> usize {
    fs::read_dir(fx.home().join("t3-log")).map(Iterator::count).unwrap_or(0)
}

#[derive(Default, Debug)]
struct Score {
    attempts: usize,
    correct: usize,
    duplicates: usize,
    false_successes: usize,
    leaks: usize,
    failures: Vec<String>,
}

fn run(cases: &[Case]) -> Score {
    let fx = fixture();
    let t3 = fake_t3(&fx);
    let mut score = Score::default();
    for case in cases {
        let _ = fs::remove_dir_all(fx.home().join("t3-log"));
        let relay = fx.home().join(".everett/inbox/relay-context/caller.json");
        if case.remote {
            fx.write(".everett/inbox/relay-context/caller.json", &json!({"hops":1}).to_string());
        } else {
            let _ = fs::remove_file(&relay);
        }
        fs::write(fx.home().join("t3-mode"), case.mode).unwrap();
        let r = mcp_calls(&fx, &[], &[("EVERETT_T3_BIN", t3.as_str()), ("EVERETT_SESSION_ID", "caller")],
            &[("everett_control", case.request.clone())])[0].clone();
        score.attempts += 1;
        let calls = invocations(&fx);
        let text = r.to_string();
        let body = tool_json(&r);
        let success = r["isError"] != true;
        let confirmed = success && match case.request["action"].as_str() {
            Some("start") => body["started"] == true && body["result"]["threadId"] == THREAD,
            Some("read") => body["thread"] == THREAD && body["result"]["threadId"] == THREAD,
            _ => body["sent"] == true && body["thread"] == THREAD && body["result"]["threadId"] == THREAD,
        };
        let got = if confirmed {
            Expect::Confirmed
        } else if !success && case.remote && text.contains("Remote-derived") {
            Expect::Refused
        } else if !success && text.to_lowercase().contains("unconfirmed") {
            Expect::Unconfirmed
        } else {
            score.failures.push(format!("{}: unclassified outcome {text}", case.name));
            continue;
        };
        if calls > case.invocations {
            score.duplicates += calls - case.invocations;
        }
        if case.remote && calls > 0 {
            score.leaks += 1;
        }
        if success && case.expect != Expect::Confirmed {
            score.false_successes += 1;
        }
        // Queued delivery is dispatch, never completion.
        if case.mode == "queued" && text.contains("\"completed\"") {
            score.false_successes += 1;
        }
        if got == case.expect {
            score.correct += 1;
        } else {
            score.failures.push(format!("{}: expected {:?}, got {got:?}: {text}", case.name, case.expect));
        }
    }
    eprintln!("t3 matrix: {} / {} correct; duplicates={} false_successes={} leaks={}",
        score.correct, score.attempts, score.duplicates, score.false_successes, score.leaks);
    score
}

fn case(name: &'static str, mode: &'static str, request: Value, expect: Expect) -> Case {
    Case { name, mode, request, invocations: 1, remote: false, expect }
}

fn assert_clean(score: &Score) {
    assert!(score.failures.is_empty(), "{:#?}", score.failures);
    assert_eq!((score.correct, score.duplicates, score.false_successes, score.leaks),
               (score.attempts, 0, 0, 0), "{score:?}");
}

#[test]
fn t3_reconciliation_matrix() {
    let start = json!({"action":"start","project":"/work/p","text":"build it","timeout":1.0});
    let send = json!({"action":"send","thread":THREAD,"text":"next step","timeout":1.0});
    let wake = json!({"action":"wake","thread":THREAD,"timeout":1.0});
    let read = json!({"action":"read","thread":THREAD,"messages":5});
    let cases = [
        // Normal flow.
        case("normal start", "ok", start.clone(), Expect::Confirmed),
        case("normal send", "ok", send.clone(), Expect::Confirmed),
        case("normal wake", "ok", wake.clone(), Expect::Confirmed),
        // Lost / ambiguous response.
        case("lost start ack", "lost", start.clone(), Expect::Unconfirmed),
        case("lost send ack", "lost", send.clone(), Expect::Unconfirmed),
        case("crash after start receipt", "crash", start.clone(), Expect::Unconfirmed),
        case("crash after send receipt", "crash", send.clone(), Expect::Unconfirmed),
        // Malformed / wrong-target receipts.
        case("partial send receipt", "partial", send.clone(), Expect::Unconfirmed),
        case("partial start receipt", "partial", start.clone(), Expect::Unconfirmed),
        case("wrong-thread send receipt", "wrong", send.clone(), Expect::Unconfirmed),
        case("wrong-thread wake receipt", "wrong", wake.clone(), Expect::Unconfirmed),
        // Busy thread: queued is a confirmed dispatch, not completion.
        case("busy thread queues send", "queued", send.clone(), Expect::Confirmed),
        // Independent caller attempt after uncertainty: one fresh dispatch per call,
        // never an internal replay.
        case("retry after lost ack", "ok", send.clone(), Expect::Confirmed),
        // Read-only inspection after uncertainty; this does not simulate a restart.
        case("reconcile by reading thread", "ok", read, Expect::Confirmed),
        // Remote-derived provenance: caller never reaches the CLI; no account isolation claim.
        Case { name: "remote caller start", mode: "ok", request: start, invocations: 0, remote: true, expect: Expect::Refused },
        Case { name: "remote caller send", mode: "ok", request: send, invocations: 0, remote: true, expect: Expect::Refused },
    ];
    assert_clean(&run(&cases));
}

/// Written before the matrix was run and not tuned against: extra receipt faults.
#[test]
fn t3_reconciliation_held_out() {
    let send = json!({"action":"send","thread":THREAD,"text":"next step","timeout":1.0});
    let waited = json!({"action":"send","thread":THREAD,"text":"next step","wait":true,"timeout":1.0});
    let start = json!({"action":"start","project":"/work/p","text":"build it","timeout":1.0});
    let cases = [
        case("waited send completes", "ok", waited.clone(), Expect::Confirmed),
        case("stale cross-thread waited reply", "stale", waited, Expect::Unconfirmed),
        case("correct then stale receipt", "two_docs", send.clone(), Expect::Unconfirmed),
        case("start with empty run id", "empty_run", start, Expect::Unconfirmed),
        case("non-wait send exits 2", "exit2", send.clone(), Expect::Unconfirmed),
        case("log noise before receipt", "noise", send, Expect::Confirmed),
    ];
    assert_clean(&run(&cases));
}
