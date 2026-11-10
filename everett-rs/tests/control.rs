mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use common::{fixture, mcp_calls, tool_json};
use serde_json::{json, Value};

const THREAD: &str = "thread-123";
const CLAUDE: &str = "0f2c7f4e-1111-4a3b-9c1d-123456789abc";

/// Records argv and answers like `t3 ... --json`; exit 2 mimics "waiting for an answer".
fn fake_t3(fx: &common::Fixture, exit: i32) -> String {
    let path = fx.home().join("fake-t3");
    fs::write(&path, format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/t3-argv\"\necho 'noise' >&2\nprintf '%s\\n' '{{\"ok\":true,\"id\":\"{THREAD}\"}}'\nexit {exit}\n"
    )).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path.to_string_lossy().into_owned()
}

fn argv(fx: &common::Fixture) -> Vec<String> {
    fs::read_to_string(fx.home().join("t3-argv")).unwrap().lines().map(str::to_string).collect()
}

fn call(fx: &common::Fixture, t3: &str, args: Value) -> Value {
    let out = mcp_calls(fx, &[], &[("EVERETT_T3_BIN", t3), ("EVERETT_SESSION_ID", "caller")],
        &[("everett_control", args)]);
    out[0].clone()
}

#[test]
fn t3_threads_start_send_and_read_use_the_t3_cli() {
    let fx = fixture();
    let t3 = fake_t3(&fx, 0);

    let r = tool_json(&call(&fx, &t3, json!({"action":"threads","project":"/work/p"})));
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(argv(&fx), ["thread", "list", "--json", "--project", "/work/p"]);

    let r = tool_json(&call(&fx, &t3, json!({"action":"start","project":"/work/p","text":"build it",
        "title":"T","provider":"codex","model":"gpt","worktree":"main","branch":"b","wait":true})));
    assert_eq!((r["via"].clone(), r["started"].clone(), r["result"]["id"].clone()), (json!("t3"), json!(true), json!(THREAD)), "{r}");
    assert_eq!(argv(&fx), ["thread", "start", "--json", "--title", "T", "--provider", "codex", "--model", "gpt",
        "--worktree", "main", "--branch", "b", "--wait", "/work/p", "build it"]);

    let r = tool_json(&call(&fx, &t3, json!({"action":"send","thread":THREAD,"text":"next step"})));
    assert_eq!(r["sent"], true, "{r}");
    assert_eq!(argv(&fx), ["thread", "send", "--json", THREAD, "next step"]);

    let r = tool_json(&call(&fx, &t3, json!({"action":"wake","thread":THREAD})));
    assert_eq!(r["sent"], true, "{r}");
    assert!(argv(&fx)[4].contains("everett_inbox"));

    let r = tool_json(&call(&fx, &t3, json!({"action":"read","thread":THREAD,"messages":3})));
    assert_eq!(r["thread"], THREAD, "{r}");
    assert_eq!(argv(&fx), ["thread", "show", "--json", "--messages", "3", THREAD]);
}

#[test]
fn t3_waiting_for_answer_is_reported_not_failed() {
    let fx = fixture();
    let t3 = fake_t3(&fx, 2);
    let r = tool_json(&call(&fx, &t3, json!({"action":"send","thread":THREAD,"text":"go","wait":true})));
    assert_eq!(r["result"]["waiting_for_answer"], true, "{r}");
}

#[test]
fn t3_failure_and_bad_arguments_are_errors() {
    let fx = fixture();
    let t3 = fake_t3(&fx, 1);
    let r = call(&fx, &t3, json!({"action":"send","thread":THREAD,"text":"go"}));
    assert_eq!(r["isError"], true, "{r}");
    let r = call(&fx, &t3, json!({"action":"start","project":"/p","text":"x","provider":"codex"}));
    assert_eq!(r["isError"], true, "{r}");
    let r = call(&fx, &t3, json!({"action":"start","text":"x"}));
    assert_eq!(r["isError"], true, "{r}");
    let r = call(&fx, &t3, json!({"action":"send","thread":THREAD,"text":"   "}));
    assert_eq!(r["isError"], true, "{r}");
}

#[test]
fn read_returns_any_harness_transcript_tail_and_stop_is_explicit() {
    let fx = fixture();
    let t3 = fake_t3(&fx, 0);
    let cwd = fx.home().to_string_lossy().into_owned();
    let rows = [
        json!({"type":"user","sessionId":CLAUDE,"cwd":cwd,"timestamp":"2026-11-10T00:00:00Z","message":{"role":"user","content":"first ask"}}),
        json!({"type":"assistant","sessionId":CLAUDE,"cwd":cwd,"timestamp":"2026-11-10T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"first answer"}]}}),
        json!({"type":"user","sessionId":CLAUDE,"cwd":cwd,"timestamp":"2026-11-10T00:00:02Z","message":{"role":"user","content":"second ask"}}),
    ];
    fx.write(&format!(".claude/projects/p/{CLAUDE}.jsonl"), &common::jsonl(&rows));

    let r = tool_json(&call(&fx, &t3, json!({"action":"read","session":CLAUDE,"messages":2})));
    assert_eq!(r["via"], "transcript", "{r}");
    assert_eq!(r["harness"], "claude");
    assert_eq!(r["messages"], json!([{"role":"assistant","text":"first answer"},{"role":"user","text":"second ask"}]));
    assert!(!fx.home().join("t3-argv").exists(), "a non-T3 read must not call t3");

    let r = call(&fx, &t3, json!({"action":"stop","session":CLAUDE}));
    assert_eq!(r["isError"], true, "{r}");
    assert!(r.to_string().contains("not supported for claude"), "{r}");
    let r = call(&fx, &t3, json!({"action":"stop","thread":THREAD}));
    assert!(r.to_string().contains("not supported for t3"), "{r}");
}
