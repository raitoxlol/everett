mod common;

use common::{fixture, lib_home};
use everett::{events, inbox};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::HashMap;

fn record(cwd: &str, kind: &str, text: &str, source: &str, at: Option<f64>) -> Option<Map<String, Value>> {
    events::record(kind, text, Some("s1"), None, "claude", Some(cwd), source, at).unwrap()
}

fn project(h: &common::LibHome) -> String {
    let dir = h.path().join("atlas");
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into()
}

#[test]
fn classify_rules() {
    let cases = [
        ("All tests pass. Committed as abc123.", "done"),
        ("Done. Next: ship it.", "done"),
        ("I added the flag.\n\nShould I also update the README?", "needs-input"),
        ("Which option do you prefer: A or B?", "needs-input"),
        ("The migration is ready. I need you to run it against prod.", "needs-input"),
        ("Let me know if you want the retry cap raised.", "needs-input"),
        ("I'm blocked: the staging key is missing.", "blocked"),
        ("Waiting on the staging credentials from ops.", "blocked"),
        ("I can't proceed until the API key is rotated.", "blocked"),
        ("The deploy is no longer blocked; it finished.", "done"),
        ("The job is not blocked anymore. Shipped.", "done"),
        ("```\nwhat is this?\nblocked\n```\nFixed the parser.", "done"),
        ("Is it blocked? No: it shipped fine.", "done"),
    ];
    for (text, kind) in cases {
        assert_eq!(events::classify(text).unwrap().0, kind, "{text}");
    }
    assert!(events::classify("").is_none());
    assert!(events::classify("```\ncode only\n```").is_none());
    assert_eq!(events::classify("Everything else works. I'm blocked on the staging key.").unwrap().1,
        "I'm blocked on the staging key.");
}

#[test]
fn event_log_state_and_describe() {
    let h = lib_home();
    let cwd = project(&h);
    let e = record(&cwd, "blocked", "waiting on staging key", "", None).unwrap();
    assert_eq!((e["kind"].clone(), e["project"].clone(), e["session"].clone()), (json!("blocked"), json!("atlas"), json!("s1")));
    assert_eq!(events::read(0.0)[0]["id"], e["id"]);
    assert_eq!(events::state("s1").unwrap()["since"], e["ts"]);
    assert!(record(&cwd, "blocked", "still waiting on staging key", "", None).is_some());
    assert_eq!(events::state("s1").unwrap()["since"], e["ts"], "the same state keeps its start");
    let ts = e["ts"].as_f64().unwrap();
    assert!(events::describe(&events::state("s1").unwrap(), Some(ts + 32.0 * 3600.0)).starts_with("blocked 32h: "));
    record(&cwd, "done", "shipped", "", None);
    assert_eq!(events::state("s1").unwrap()["kind"], "done");
}

#[test]
fn validation() {
    let h = lib_home();
    let cwd = project(&h);
    for (kind, text) in [("nope", "x"), ("info", "  "), ("info", "token = sk-ant-abcdefghijklmnopqrstu")] {
        assert!(events::record(kind, text, Some("s1"), None, "claude", Some(&cwd), "", None).is_err(), "{kind} {text}");
    }
}

#[test]
fn auto_debounce() {
    let h = lib_home();
    let cwd = project(&h);
    let later = Some(everett::session::now() + 3600.0);
    assert!(record(&cwd, "done", "finished A", "auto", None).is_some());
    assert!(record(&cwd, "done", "finished B", "auto", None).is_none(), "same state within 10 min");
    assert!(record(&cwd, "done", "finished A", "auto", later).is_none(), "same text");
    assert!(record(&cwd, "done", "finished C", "auto", later).is_some());
    assert!(record(&cwd, "needs-input", "Ship it?", "auto", None).is_some(), "a change of state always counts");
    assert_eq!(events::read(0.0).len(), 3);
}

type Calls = RefCell<Vec<(Vec<String>, Option<HashMap<String, String>>)>>;

#[test]
fn notify_modes_and_command_environment() {
    let h = lib_home();
    let calls: Calls = RefCell::new(Vec::new());
    let runner = |argv: &[String], env: Option<&HashMap<String, String>>| calls.borrow_mut().push((argv.to_vec(), env.cloned()));
    let mut e = Map::new();
    for (k, v) in [("kind", "needs-input"), ("text", "Should I \"deploy\"?"), ("session", "s"), ("project", "atlas")] {
        e.insert(k.into(), json!(v));
    }
    assert!(events::notify(&e, "", Some(&runner)).is_empty(), "EVERETT_NOTIFY=none");
    std::env::set_var("EVERETT_NOTIFY", "command");
    std::env::set_var("EVERETT_NOTIFY_COMMAND", "max-notify \"$1\"");
    let runs = events::notify(&e, "", Some(&runner));
    assert_eq!(runs.len(), 1);
    let (argv, env) = calls.borrow()[0].clone();
    assert_eq!(&argv[..3], ["/bin/sh", "-c", "max-notify \"$1\""]);
    assert!(argv[4].contains("needs-input [atlas] Should I \"deploy\"?"), "{argv:?}");
    assert_eq!(env.unwrap()["EVERETT_EVENT_KIND"], "needs-input");
    std::env::remove_var("EVERETT_NOTIFY");
    std::fs::create_dir_all(h.path().join(".everett")).unwrap();
    std::fs::write(h.path().join(".everett/config.toml"), "notify = \"none\"\n").unwrap();
    assert!(events::notify(&e, "", Some(&runner)).is_empty(), "config notify = none");
    std::env::remove_var("EVERETT_NOTIFY_COMMAND");
}

#[test]
fn real_notify_command_runs_detached_only_for_human_kinds() {
    let h = lib_home();
    let cwd = project(&h);
    let out = h.path().join("notified.txt");
    std::env::set_var("EVERETT_NOTIFY", "command");
    std::env::set_var("EVERETT_NOTIFY_COMMAND", format!("printf \"%s|$EVERETT_EVENT_KIND\" \"$1\" >> {}", out.display()));
    record(&cwd, "done", "ok", "", None);
    record(&cwd, "info", "fyi", "", None);
    events::record("blocked", "need creds", Some("s9"), None, "", Some(&cwd), "", None).unwrap();
    for _ in 0..100 {
        if std::fs::read_to_string(&out).map(|t| !t.is_empty()).unwrap_or(false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    std::env::remove_var("EVERETT_NOTIFY_COMMAND");
    std::env::set_var("EVERETT_NOTIFY", "none");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "blocked [atlas] need creds|blocked");
}

#[test]
fn escalation_once_after_threshold_with_reason() {
    let h = lib_home();
    let cwd = project(&h);
    let start = everett::session::now();
    record(&cwd, "blocked", "waiting on staging key", "", Some(start));
    let calls: Calls = RefCell::new(Vec::new());
    let runner = |argv: &[String], env: Option<&HashMap<String, String>>| calls.borrow_mut().push((argv.to_vec(), env.cloned()));
    std::env::set_var("EVERETT_ESCALATE_MINUTES", "30");
    std::env::set_var("EVERETT_NOTIFY", "command");
    std::env::set_var("EVERETT_NOTIFY_COMMAND", "x");
    assert!(events::check_escalations(Some(start + 600.0), Some(&runner)).is_empty());
    assert_eq!(events::check_escalations(Some(start + 31.0 * 60.0), Some(&runner)).len(), 1);
    assert_eq!(calls.borrow().len(), 1);
    let (argv, env) = calls.borrow()[0].clone();
    assert!(argv[4].starts_with("blocked for 31 min: blocked [atlas]"), "{argv:?}");
    assert_eq!(env.unwrap()["EVERETT_EVENT_REASON"], "blocked for 31 min");
    assert!(events::check_escalations(Some(start + 90.0 * 60.0), Some(&runner)).is_empty());
    for name in ["EVERETT_ESCALATE_MINUTES", "EVERETT_NOTIFY_COMMAND"] {
        std::env::remove_var(name);
    }
    std::env::set_var("EVERETT_NOTIFY", "none");
    assert_eq!(events::state("s1").unwrap()["escalated"], true);
    record(&cwd, "done", "unblocked, shipped", "", None);
    assert_eq!(events::state("s1").unwrap()["escalated"], false);
}

#[test]
fn session_and_project_subscribers_get_inbox_events() {
    let h = lib_home();
    let cwd = project(&h);
    events::subscribe("watcher", "s1", false).unwrap();
    events::subscribe("pm", "project:atlas", false).unwrap();
    events::subscribe("other", "project:web", false).unwrap();
    events::subscribe("s1", "*", false).unwrap();
    record(&cwd, "done", "shipped retries", "", None).unwrap();
    let watched = inbox::pending("watcher", None);
    assert_eq!(watched[0]["kind"], "event");
    assert!(watched[0]["text"].as_str().unwrap().contains("DONE from claude s1 in atlas: shipped retries"));
    assert_eq!(inbox::pending("pm", None).len(), 1);
    assert!(inbox::pending("other", None).is_empty());
    assert!(inbox::pending("s1", None).is_empty(), "never gets its own events");
    assert!(inbox::take("pm").contains("EVENT"));
    assert!(events::subscribe("watcher", "s1", true).unwrap().is_empty());
    assert!(!events::subscriptions().contains_key("watcher"));
    assert!(events::subscribe("", "s1", false).is_err());
    assert_eq!(events::resolve_target("project:Atlas"), "project:atlas");
    assert_eq!(events::resolve_target("*"), "*");
    assert_eq!(events::resolve_target("Web App"), "project:web-app");
}

#[test]
fn stop_hooks_auto_detect_state() {
    let h = lib_home();
    let cwd = project(&h);
    let hooks = everett::hooks_common::stop_event;
    hooks(&json!({"session_id": "c1", "cwd": cwd, "last_assistant_message": "Tests pass. Should I push the branch?"}).to_string(), "claude");
    let st = events::state("c1").unwrap();
    assert_eq!((st["kind"].clone(), st["source"].clone()), (json!("needs-input"), json!("auto")));
    let transcript = h.path().join("t.jsonl");
    std::fs::write(&transcript, json!({"type": "assistant", "message": {"role": "assistant", "content": [
        {"type": "text", "text": "I'm blocked: waiting on the staging key."}]}}).to_string() + "\n").unwrap();
    hooks(&json!({"session_id": "c1", "cwd": cwd, "transcript_path": transcript}).to_string(), "claude");
    assert_eq!(events::state("c1").unwrap()["kind"], "blocked");
    everett::hooks_common::grok_stop_hook(&json!({"sessionId": "g1", "cwd": cwd, "lastAssistantMessage": "Need you to approve the plan."}).to_string());
    assert_eq!(events::state("g1").unwrap()["kind"], "needs-input");
}

#[test]
fn stop_hook_silent_cases() {
    let _h = lib_home();
    for raw in ["junk".to_string(), "[]".to_string(),
        json!({"session_id": "../x", "last_assistant_message": "done"}).to_string(),
        json!({"session_id": "c2", "stop_hook_active": true, "last_assistant_message": "Ok?"}).to_string()] {
        everett::hooks_common::stop_event(&raw, "claude");
    }
    std::env::set_var("EVERETT_SEND", "1");
    everett::hooks_common::stop_event(&json!({"session_id": "c2", "last_assistant_message": "Ok?"}).to_string(), "claude");
    std::env::remove_var("EVERETT_SEND");
    assert!(events::read(0.0).is_empty());
}

#[test]
fn codex_stop_command_records_event_silently() {
    let fx = fixture();
    let payload = json!({"session_id": "x1", "cwd": fx.home(), "hook_event_name": "Stop", "stop_hook_active": false,
        "last_assistant_message": "Deployed to staging.", "transcript_path": null, "turn_id": "t", "model": "m",
        "permission_mode": "default"}).to_string();
    let out = fx.run_stdin(&["hook", "codex_stop"], &payload, &[("PATH", "/nonexistent")]);
    assert_eq!((out.status.code(), out.stdout.len(), out.stderr.len()), (Some(0), 0, 0));
    let state: Value = serde_json::from_str(&std::fs::read_to_string(fx.home().join(".everett/state/x1.json")).unwrap()).unwrap();
    assert_eq!(state["kind"], "done");
}

#[test]
fn cli_event_events_subscribe_and_mcp_tools() {
    let fx = fixture();
    let sid = [("EVERETT_SESSION_ID", "sess-1")];
    let out = fx.run_stdin(&["event", "blocked", "waiting on staging key", "--project", "atlas"], "", &sid);
    assert!(fx.stdout(&out).contains("blocked [atlas] waiting on staging key"), "{}", fx.stdout(&out));
    assert!(fx.stdout(&fx.run(&["events", "--since", "1h"])).contains("waiting on staging key"));
    let listed: Value = serde_json::from_str(&fx.stdout(&fx.run(&["events", "--json"]))).unwrap();
    assert_eq!(listed[0]["kind"], "blocked");
    assert_eq!(fx.run(&["events", "--since", "soon"]).status.code(), Some(2));
    assert_eq!(fx.run(&["subscribe", "atlas"]).status.code(), Some(2), "no session to subscribe");
    assert_eq!(fx.run(&["subscribe", "atlas", "--as", "me-1"]).status.code(), Some(0));
    let subs: Value = serde_json::from_str(&std::fs::read_to_string(fx.home().join(".everett/subscriptions.json")).unwrap()).unwrap();
    assert_eq!(subs, json!({"me-1": ["project:atlas"]}));

    let input = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "everett_event", "arguments": {"kind": "needs-input", "message": "Ship?"}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "everett_subscribe", "arguments": {"target": "project:web"}}}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "everett_event", "arguments": {"kind": "bogus", "message": "x"}}}),
    ];
    let out = fx.run_stdin(&["mcp"], &common::jsonl(&input), &[("EVERETT_SESSION_ID", "mcp-1")]);
    let replies: Vec<Value> = fx.stdout(&out).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies[0]["result"]["structuredContent"]["event"]["session"], "mcp-1");
    assert_eq!(replies[1]["result"]["structuredContent"]["following"], json!(["project:web"]));
    assert_eq!(replies[2]["error"]["code"], -32602);
}
