mod common;

use common::{fixture, lib_home};
use everett::inbox;
use everett::send::{delivery_mode, reply, send_inbox};
use everett::session::{now, Session};
use serde_json::{json, Value};

fn session(home: &std::path::Path, id: &str) -> Session {
    let cwd = home.to_string_lossy();
    Session::new("claude", id, &cwd, &format!("{}/x.jsonl", cwd), "", now())
}

fn post(to: &str, text: &str, sender: &str) -> serde_json::Map<String, Value> {
    inbox::post(to, text, sender, "message", "", 0, "", "", None).unwrap()
}

fn ids(messages: &[serde_json::Map<String, Value>]) -> Vec<String> {
    messages.iter().map(|m| m["id"].as_str().unwrap().to_string()).collect()
}

#[test]
fn post_pending_take() {
    let _h = lib_home();
    let m = inbox::post("s1", "hello there", "s0", "message", "", 0, "codex", "Atlas: retries", None).unwrap();
    let id = m["id"].as_str().unwrap().to_string();
    assert!(id.starts_with('m'));
    assert_eq!(ids(&inbox::pending("s1", None)), vec![id.clone()]);
    let text = inbox::take("s1");
    for needle in ["[Everett]", "hello there", "codex session s0", "Atlas: retries", &format!("everett_send(reply_to=\"{}\"", id)] {
        assert!(text.contains(needle), "{needle} missing from {text}");
    }
    assert!(inbox::pending("s1", None).is_empty());
    assert_eq!(inbox::take("s1"), "");
}

#[test]
fn bounded_batch_leaves_rest_pending() {
    let _h = lib_home();
    for i in 0..8 {
        post("s1", &format!("msg {} {}", i, "x".repeat(3000)), "");
    }
    let text = inbox::take("s1");
    assert!(text.chars().count() <= inbox::MAX_INJECT + 600, "{}", text.len());
    assert!(text.contains("[…clipped]") && text.contains("more waiting"));
    assert!(!inbox::pending("s1", None).is_empty());
}

#[test]
fn expired_and_invalid_messages() {
    let _h = lib_home();
    post("s1", "old", "");
    assert!(inbox::pending("s1", Some(now() + inbox::TTL + 5.0)).is_empty());
    assert!(inbox::post("../evil", "x", "", "message", "", 0, "", "", None).is_err());
    assert!(inbox::post("s1", "   ", "", "message", "", 0, "", "", None).is_err());
}

#[test]
fn wait_reply_consumes_only_the_reply() {
    let _h = lib_home();
    post("me", "unrelated", "x");
    let sent = post("peer", "question", "me");
    let sent_id = sent["id"].as_str().unwrap();
    inbox::post("me", "the answer", "peer", "reply", sent_id, 0, "", "", None).unwrap();
    let got = inbox::wait_reply("me", sent_id, 0.0, 1.0).unwrap();
    assert_eq!(got["text"], "the answer");
    let left: Vec<_> = inbox::pending("me", None).iter().map(|m| m["text"].clone()).collect();
    assert_eq!(left, vec![json!("unrelated")]);
}

#[test]
fn wait_reply_times_out() {
    let _h = lib_home();
    let tick = std::cell::Cell::new(0.0);
    let got = inbox::wait_reply_with("me", "mnope", 3.0, 1.0, || { let t = tick.get(); tick.set(t + 1.0); t }, |_| {});
    assert!(got.is_none());
}

#[test]
fn live_record_follows_the_harness_process() {
    let _h = lib_home();
    assert!(inbox::live("s1").is_none());
    inbox::touch_live("s1", "claude", "");
    assert_eq!(inbox::live("s1").unwrap()["harness"], "claude");
    inbox::touch_live("s2", "claude", "");
    assert!(inbox::live("s1").is_none(), "same process switched sessions");
    assert!(inbox::live("s2").is_some());
    inbox::touch_live("s1", "claude", "");
    std::fs::write(inbox::live_path("s1"), json!({"pid": 999999, "harness": "claude"}).to_string()).unwrap();
    assert!(inbox::live("s1").is_none());
}

fn hook(harness: &str, payload: Value) -> Option<Value> {
    let out = everett::hooks_common::deliver_output(&payload.to_string(), harness);
    (!out.is_empty()).then(|| serde_json::from_str(&out).unwrap())
}

#[test]
fn claude_prompt_submit_and_post_tool_use_inject_pending() {
    let _h = lib_home();
    post("c1", "first", "");
    let out = hook("claude", json!({"session_id": "c1", "hook_event_name": "UserPromptSubmit", "prompt": "hi"})).unwrap();
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    assert!(out["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("first"));
    assert!(hook("claude", json!({"session_id": "c1", "hook_event_name": "PostToolUse"})).is_none());
    post("c1", "second", "");
    let out = hook("claude", json!({"session_id": "c1", "hook_event_name": "PostToolUse", "tool_name": "Bash"})).unwrap();
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    assert!(out["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("second"));
    assert!(inbox::live("c1").is_some());
}

#[test]
fn codex_hook_output_has_only_the_documented_keys() {
    let _h = lib_home();
    post("x1", "for codex", "");
    let out = hook("codex", json!({"session_id": "x1", "hook_event_name": "PostToolUse", "turn_id": "t",
        "tool_name": "shell", "tool_input": {}, "tool_response": {}})).unwrap();
    let keys: Vec<_> = out["hookSpecificOutput"].as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&"hookEventName".into()) && keys.contains(&"additionalContext".into()));
}

#[test]
fn grok_prompt_submit_is_not_consumed() {
    let _h = lib_home();
    post("g1", "for grok", "");
    assert!(hook("claude", json!({"sessionId": "g1", "hook_event_name": "UserPromptSubmit"})).is_none());
    assert_eq!(inbox::pending("g1", None).len(), 1);
    let out = hook("grok", json!({"sessionId": "g1", "hook_event_name": "PostToolUse", "hookEventName": "post_tool_use"})).unwrap();
    assert!(out["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("for grok"));
}

#[test]
fn deliver_hook_silent_cases() {
    let _h = lib_home();
    post("c1", "queued", "");
    for raw in ["nope".to_string(), "[]".to_string(),
        json!({"session_id": "../x", "hook_event_name": "PostToolUse"}).to_string(),
        json!({"session_id": "c1", "hook_event_name": "Stop"}).to_string()] {
        assert_eq!(everett::hooks_common::deliver_output(&raw, "claude"), "");
    }
    std::env::set_var("EVERETT_SEND", "1");
    let out = everett::hooks_common::deliver_output(&json!({"session_id": "c1", "hook_event_name": "PostToolUse"}).to_string(), "claude");
    std::env::remove_var("EVERETT_SEND");
    assert_eq!(out, "");
    assert_eq!(inbox::pending("c1", None).len(), 1);
}

#[test]
fn inbox_hooks_run_fast_and_silent_as_native_commands() {
    let fx = fixture();
    let payload = json!({"session_id": "fast-1", "hook_event_name": "PostToolUse"}).to_string();
    for stem in ["claude_inbox", "codex_inbox", "grok_inbox"] {
        let started = std::time::Instant::now();
        let out = fx.run_stdin(&["hook", stem], &payload, &[("PATH", "/nonexistent")]);
        assert!(started.elapsed().as_secs_f64() < 2.0);
        assert_eq!((out.status.code(), out.stdout.len(), out.stderr.len()), (Some(0), 0, 0), "{stem}");
    }
    let inbox_file = fx.home().join(".everett/inbox/fast-1.jsonl");
    std::fs::create_dir_all(inbox_file.parent().unwrap()).unwrap();
    std::fs::write(&inbox_file, json!({"id": "m1", "text": "ping", "ts": now(), "hops": 1, "kind": "message"}).to_string() + "\n").unwrap();
    let out = fx.run_stdin(&["hook", "claude_inbox"], &payload, &[("PATH", "/nonexistent")]);
    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(parsed["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("ping"));
    let out = fx.run_stdin(&["hook", "claude_inbox"], "garbage", &[("PATH", "/nonexistent")]);
    assert_eq!((out.status.code(), out.stdout.len(), out.stderr.len()), (Some(0), 0, 0));
}

#[test]
fn delivery_mode_prefers_inbox_for_live_sessions() {
    let h = lib_home();
    let s = session(&h.path(), "target-1");
    assert_eq!(delivery_mode(&s, "auto", Some("")).unwrap(), "resume");
    assert_eq!(delivery_mode(&s, "auto", Some("/usr/bin/claude --resume target-1")).unwrap(), "inbox");
    assert_eq!(delivery_mode(&s, "inbox", Some("")).unwrap(), "inbox");
    inbox::touch_live("target-1", "claude", "");
    assert_eq!(delivery_mode(&s, "auto", Some("")).unwrap(), "inbox");
    assert_eq!(delivery_mode(&s, "resume", Some("")).unwrap(), "resume");
    let mut t3 = session(&h.path(), "t3");
    t3.source = "t3code".into();
    assert_eq!(delivery_mode(&t3, "auto", Some("")).unwrap(), "inbox");
    assert!(delivery_mode(&s, "bogus", None).is_err());
}

#[test]
fn inbox_round_trip_with_reply() {
    let h = lib_home();
    let result = send_inbox(&session(&h.path(), "target-1"), "please review", 0.0, Some("sender-1"), 1.0).unwrap();
    assert_eq!(result["reply_inbox"], "sender-1");
    let msg = inbox::pending("target-1", None).remove(0);
    assert_eq!((msg["from"].clone(), msg["hops"].clone()), (json!("sender-1"), json!(1)));
    let id = msg["id"].as_str().unwrap();
    let out = reply(id, "looks good", Some("target-1")).unwrap();
    assert_eq!(out["to"], "sender-1");
    let got = inbox::pending("sender-1", None).remove(0);
    assert_eq!((got["kind"].clone(), got["reply_to"].clone(), got["text"].clone(), got["hops"].clone()),
        (json!("reply"), json!(id), json!("looks good"), json!(2)));
    assert!(inbox::take("sender-1").contains(&format!("REPLY to your message {}", id)));
}

#[test]
fn hop_limit_on_reply_chain() {
    let h = lib_home();
    let m = inbox::post("a", "x", "b", "message", "", 3, "", "", None).unwrap();
    assert_eq!(reply(m["id"].as_str().unwrap(), "again", None).unwrap_err().code, 7);
    std::env::set_var("EVERETT_HOPS", "3");
    let looped = send_inbox(&session(&h.path(), "target-1"), "loop", 0.0, None, 1.0);
    std::env::remove_var("EVERETT_HOPS");
    assert!(looped.is_err());
    assert!(reply("mmissing", "x", None).is_err());
}

#[test]
fn cli_send_inbox_reply_and_human_inbox() {
    let fx = fixture();
    let cwd = fx.home().to_string_lossy().to_string();
    fx.write(".claude/projects/p/target-1.jsonl", &common::jsonl(&[
        json!({"type": "user", "sessionId": "target-1", "cwd": cwd, "timestamp": "2026-01-01T00:00:00Z",
               "message": {"role": "user", "content": "check the build pipeline"}}),
    ]));
    let out = fx.run(&["send", "check the build", "--to", "target-1", "--mode", "inbox", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", fx.stdout(&out));
    let data: Value = serde_json::from_str(&fx.stdout(&out)).unwrap();
    assert_eq!((data["mode"].clone(), data["reply_inbox"].clone()), (json!("inbox"), json!(inbox::HUMAN)));
    let id = data["message_id"].as_str().unwrap();
    let replied = fx.run_stdin(&["reply", id, "build is green"], "", &[("EVERETT_SESSION_ID", "target-1")]);
    assert!(fx.stdout(&replied).contains("REPLIED"), "{}", fx.stdout(&replied));
    assert!(fx.stdout(&fx.run(&["inbox"])).contains("build is green"));
    assert!(fx.stdout(&fx.run(&["inbox"])).contains("empty"));
}
