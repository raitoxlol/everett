mod common;

use common::{fixture, lib_home, mcp_calls, tool_json, Fixture};
use everett::{inbox, send};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn unbound_cli(fx: &Fixture) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_everett"));
    cmd.env_clear()
        .env("HOME", fx.home())
        .env("EVERETT_HOME", fx.home())
        .env("EVERETT_NOTIFY", "none")
        .env("PATH", common::HERMETIC_PATH)
        .current_dir(fx.home());
    cmd
}

fn queued(fx: &Fixture, to: &str, id: &str, from: &str) {
    fx.write(&format!(".everett/inbox/{to}.jsonl"), &format!("{}\n", json!({
        "id":id,"to":to,"from":from,"text":"request","kind":"message",
        "ts":everett::session::now(),"hops":1,"reply_to":""
    })));
}

fn pending(fx: &Fixture, sid: &str) -> Value {
    let out = fx.run(&["inbox", "--session", sid, "--peek", "--json"]);
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice::<Value>(&out.stdout).unwrap()["messages"].clone()
}

fn reply(fx: &Fixture, caller: &str, id: &str, text: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(["reply", id, text])
        .env_clear()
        .env("HOME", fx.home())
        .env("EVERETT_HOME", fx.home())
        .env("EVERETT_SESSION_ID", caller)
        .env("EVERETT_NOTIFY", "none")
        .env("PATH", common::HERMETIC_PATH)
        .current_dir(fx.home())
        .output().unwrap()
}

#[test]
fn regression_unbound_terminal_reply_uses_global_lookup_without_acknowledgement() {
    let fx = fixture();
    queued(&fx, "recipient", "mTerminal", "sender");
    let read = unbound_cli(&fx)
        .args(["inbox", "--session", "recipient", "--peek", "--json"])
        .output().unwrap();
    assert!(read.status.success(), "{read:?}");
    assert_eq!(serde_json::from_slice::<Value>(&read.stdout).unwrap()["messages"][0]["id"], "mTerminal");
    let out = unbound_cli(&fx).args(["reply", "mTerminal", "terminal answer"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let answers = pending(&fx, "sender");
    assert_eq!(answers.as_array().unwrap().len(), 1);
    assert_eq!(answers[0]["reply_to"], "mTerminal");
    assert_eq!(answers[0]["from"], "human");
    assert_eq!(answers[0]["text"], "terminal answer");
    assert_eq!(pending(&fx, "recipient")[0]["id"], "mTerminal");
}

#[test]
fn regression_unbound_hook_reply_instruction_binds_recipient() {
    let fx = fixture();
    queued(&fx, "recipient", "mHook", "sender");
    let mut hook = unbound_cli(&fx)
        .args(["hook", "claude_inbox"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    hook.stdin.take().unwrap().write_all(
        br#"{"session_id":"recipient","hook_event_name":"PostToolUse"}"#,
    ).unwrap();
    let out = hook.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let context = serde_json::from_slice::<Value>(&out.stdout).unwrap();
    let text = context["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert!(text.contains("mHook"), "{text}");
    let instruction = text.split('`').skip(1).step_by(2)
        .find(|command| command.contains("everett reply ")).unwrap()
        .replace("<text>", "hook answer");
    fs::hard_link(env!("CARGO_BIN_EXE_everett"), fx.home().join("everett")).unwrap();
    let answer = Command::new("/bin/sh")
        .args(["-c", &instruction])
        .env_clear()
        .env("HOME", fx.home()).env("EVERETT_HOME", fx.home())
        .env("EVERETT_NOTIFY", "none")
        .env("PATH", format!("{}:{}", fx.home().display(), common::HERMETIC_PATH))
        .current_dir(fx.home()).output().unwrap();
    assert!(answer.status.success(), "instruction={instruction}\n{answer:?}");
    let answers = pending(&fx, "sender");
    assert_eq!(answers.as_array().unwrap().len(), 1);
    assert_eq!(answers[0]["reply_to"], "mHook");
    assert_eq!(answers[0]["from"], "recipient");
    assert_eq!(answers[0]["text"], "hook answer");
    assert_eq!(pending(&fx, "recipient"), json!([]));
}

#[test]
fn regression_mcp_pickup_consumes_only_the_named_message() {
    let fx = fixture();
    queued(&fx, "recipient", "mFirst", "sender");
    let first = fs::read_to_string(fx.home().join(".everett/inbox/recipient.jsonl")).unwrap();
    let second = first.replace("mFirst", "mSecond");
    fx.write(".everett/inbox/recipient.jsonl", &(first + &second));
    let calls = mcp_calls(&fx, &[], &[("EVERETT_SESSION_ID", "recipient")], &[
        ("everett_inbox", json!({"message_id":"mFirst"})),
        ("everett_inbox", json!({"peek":true})),
        ("everett_inbox", json!({"message_id":"mSecond"})),
        ("everett_inbox", json!({})),
    ]);
    assert_eq!(tool_json(&calls[0])["messages"][0]["id"], "mFirst", "{:?}", calls[0]);
    assert_eq!(tool_json(&calls[0])["messages"].as_array().unwrap().len(), 1);
    assert_eq!(tool_json(&calls[1])["messages"][0]["id"], "mSecond");
    assert_eq!(tool_json(&calls[2])["messages"][0]["id"], "mSecond");
    assert_eq!(tool_json(&calls[3])["messages"], json!([]));
}

#[test]
fn regression_cli_reply_after_peek_acknowledges_original() {
    let fx = fixture();
    queued(&fx, "recipient", "mOriginal", "sender");
    assert_eq!(pending(&fx, "recipient")[0]["id"], "mOriginal");
    assert_eq!(pending(&fx, "recipient")[0]["id"], "mOriginal");
    let out = reply(&fx, "recipient", "mOriginal", "answer");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(pending(&fx, "recipient"), json!([]));
    let answers = pending(&fx, "sender");
    assert_eq!(answers[0]["reply_to"], "mOriginal");
    assert_eq!(answers[0]["from"], "recipient");
    assert_eq!(answers[0]["text"], "answer");
    assert!(fs::read_to_string(fx.home().join(".everett/inbox/recipient.jsonl")).unwrap().contains("mOriginal"));
}

#[test]
fn regression_mcp_reply_after_peek_consumes_only_original() {
    let fx = fixture();
    queued(&fx, "recipient", "mOriginal", "sender");
    let original = fs::read_to_string(fx.home().join(".everett/inbox/recipient.jsonl")).unwrap();
    fx.write(".everett/inbox/recipient.jsonl", &(original.clone() + &original.replace("mOriginal", "mOther")));
    let calls = mcp_calls(&fx, &[], &[("EVERETT_SESSION_ID", "recipient")], &[
        ("everett_inbox", json!({"peek":true})),
        ("everett_send", json!({"reply_to":"mOriginal","text":"answer"})),
        ("everett_inbox", json!({"peek":true})),
    ]);
    assert_eq!(tool_json(&calls[0])["messages"].as_array().unwrap().len(), 2);
    assert_eq!(calls[1]["isError"], false, "{:?}", calls[1]);
    let remaining = tool_json(&calls[2]);
    assert_eq!(remaining["messages"].as_array().unwrap().len(), 1, "{remaining}");
    assert_eq!(remaining["messages"][0]["id"], "mOther");
    assert_eq!(pending(&fx, "sender")[0]["reply_to"], "mOriginal");
}

#[test]
fn regression_cli_other_session_cannot_reply_or_acknowledge() {
    let fx = fixture();
    queued(&fx, "recipient", "mOriginal", "sender");
    let out = reply(&fx, "other", "mOriginal", "wrong session");
    assert!(!out.status.success(), "{out:?}");
    assert_eq!(pending(&fx, "recipient")[0]["id"], "mOriginal");
    assert_eq!(pending(&fx, "sender"), json!([]));
    let calls = mcp_calls(&fx, &[], &[("EVERETT_SESSION_ID", "other")], &[
        ("everett_send", json!({"reply_to":"mOriginal","text":"wrong session"})),
    ]);
    assert_eq!(calls[0]["isError"], true);
    assert_eq!(pending(&fx, "recipient")[0]["id"], "mOriginal");
}

#[test]
fn failed_reply_enqueue_keeps_original_pending() {
    let home = lib_home();
    let original = inbox::post("recipient", "request", "sender", "message", "", 1, "", "", None).unwrap();
    let mid = original["id"].as_str().unwrap();
    fs::create_dir(home.path().join(".everett/inbox/sender.jsonl")).unwrap();
    assert!(send::reply_as(mid, "answer", Some("recipient"), Some("codex")).is_err());
    assert_eq!(inbox::pending("recipient", None)[0]["id"], mid);
    assert!(send::reply_as(mid, "", Some("recipient"), None).is_err());
    assert_eq!(inbox::pending("recipient", None)[0]["id"], mid);
}

#[test]
fn regression_reply_resolves_and_acknowledges_within_caller_inbox() {
    let fx = fixture();
    queued(&fx, "aaa-other", "mCollision", "wrong-sender");
    queued(&fx, "recipient", "mCollision", "sender");
    let out = reply(&fx, "recipient", "mCollision", "answer");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(pending(&fx, "sender")[0]["reply_to"], "mCollision");
    assert_eq!(pending(&fx, "wrong-sender"), json!([]));
    assert_eq!(pending(&fx, "recipient"), json!([]));
    assert_eq!(pending(&fx, "aaa-other")[0]["id"], "mCollision");
}
