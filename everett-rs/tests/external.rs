mod common;

use std::io::Write;
use std::process::{Command, Output, Stdio};

use common::{fixture, Fixture};
use everett::adapters::external;
use everett::send;
use everett::session::Session;
use serde_json::{json, Value};

fn registration(fx: &Fixture, id: &str, harness: &str, updated: f64) -> std::path::PathBuf {
    fx.write(
        &format!(".everett/external/{id}.json"),
        &json!({"id": id, "harness": harness, "title": "Database backup reports",
                "cwd": "/work/backups", "updated": updated})
        .to_string(),
    )
}

fn run(fx: &Fixture, args: &[&str], input: Option<&str>, caller: &str, harness: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_everett"));
    command
        .args(args)
        .current_dir(fx.home())
        .env_clear()
        .env("HOME", fx.home())
        .env("EVERETT_HOME", fx.home())
        .env("PATH", fx.home().join("bin"))
        .env("EVERETT_NOTIFY", "none")
        .env("EVERETT_ROUTER", "local")
        .env("EVERETT_SESSION_ID", caller)
        .env("EVERETT_HARNESS_NAME", harness)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(input) = input {
        command.stdin(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    } else {
        command.stdin(Stdio::null()).output().unwrap()
    }
}

fn mcp(fx: &Fixture, calls: &[(&str, Value)], caller: &str, harness: &str) -> Vec<Value> {
    let mut requests = vec![json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2024-11-05", "capabilities": {},
        "clientInfo": {"name": "external-fixture", "version": "1"}}})];
    for (i, (name, arguments)) in calls.iter().enumerate() {
        requests.push(json!({"jsonrpc": "2.0", "id": i + 1, "method": "tools/call",
                            "params": {"name": name, "arguments": arguments}}));
    }
    let input: String = requests.iter().map(|r| r.to_string() + "\n").collect();
    let output = run(fx, &["mcp"], Some(&input), caller, harness);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let responses: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (1..=calls.len())
        .map(|id| responses.iter().find(|r| r["id"] == json!(id)).unwrap()["result"].clone())
        .collect()
}

fn data(result: &Value) -> &Value {
    assert_eq!(result["isError"], false, "{result}");
    &result["structuredContent"]
}

#[test]
fn reads_shared_registration_and_rejects_invalid_envelopes() {
    let fx = fixture();
    let path = registration(&fx, "ext-grok", "grok-bot", 0.0);
    let record = external::load(&path).unwrap();
    assert_eq!(record.id, "ext-grok");
    assert_eq!(record.harness, "grok-bot");
    assert_eq!(record.title, "Database backup reports");
    assert_eq!(record.cwd, "/work/backups");
    assert_eq!(record.updated, 0.0);
    let valid: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    #[cfg(unix)]
    {
        let linked = fx.home().join("ext-grok.json");
        std::os::unix::fs::symlink(&path, &linked).unwrap();
        assert!(external::load(&linked).is_none());
    }
    for (field, value) in [
        ("id", json!("../escape")),
        ("id", json!("human")),
        ("id", json!("HUMAN")),
        ("id", json!("live")),
        ("harness", json!("grok")),
        ("title", Value::Null),
        ("cwd", json!([])),
        ("updated", json!(true)),
        ("updated", json!(-1)),
        ("updated", json!("NaN")),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        std::fs::write(&path, invalid.to_string()).unwrap();
        assert!(external::load(&path).is_none(), "accepted {invalid}");
    }
    let wrong = fx.write(".everett/external/wrong.json", &valid.to_string());
    assert!(external::load(&wrong).is_none());
    let malformed = fx.write(".everett/external/malformed.json", "{");
    assert!(external::load(&malformed).is_none());
    for id in ["", "dots.with.periods", "a/b", "é", ".", "..", "グロク"] {
        assert!(!external::valid_id(id), "{id}");
    }
    assert!(!external::valid_id(&"a".repeat(101)));
    assert!(external::valid_id(&"A".repeat(100)));
}

#[test]
fn registered_sessions_bypass_the_default_local_cap() {
    let fx = fixture();
    for index in 0..45 {
        registration(&fx, &format!("ext-grok-{index}"), "grok-bot", 0.0);
    }
    let output = run(&fx, &["ls", "--json"], None, "fixture-owner", "");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let sessions: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(sessions.len(), 45);
}

#[test]
fn persistent_listing_and_routes_do_not_expose_transcript_or_resume_ids() {
    let fx = fixture();
    registration(&fx, "ext-grok", "grok-bot", 0.0);
    registration(&fx, "ext-dot", "openai-dot", 1.0);
    let output = run(&fx, &["ls", "--hours", "0", "--json"], None, "fixture-owner", "");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let sessions: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(sessions.len(), 2);
    for session in sessions {
        assert_eq!(session["source"], "external");
        assert_eq!(session["path"], "");
        assert_eq!(session["started"], "");
        assert_eq!(session["first_user"], "");
        assert_eq!(session["running"], false);
    }
    std::fs::remove_file(fx.home().join(".everett/external/ext-dot.json")).unwrap();
    let output =
        run(&fx, &["route", "Database backup reports", "--hours", "0", "--json"], None, "fixture-owner", "");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["decision"], "SESSION", "{result}");
    assert_eq!(result["session"]["id"], "ext-grok");
    let command = result["command"].as_str().unwrap();
    assert!(command.contains("--mode inbox"));
    assert!(!command.contains("--resume"));
    assert!(!command.contains("omp -r"));
}

#[test]
fn external_delivery_refuses_resume_and_spawn_even_with_live_process_evidence() {
    for harness in external::HARNESSES {
        let session = Session::new(harness, "ext-agent", "/work/backups", "", "", 0.0);
        for mode in ["auto", "inbox"] {
            assert_eq!(send::delivery_mode(&session, mode, Some("")).unwrap(), "inbox");
        }
        assert!(send::delivery_mode(&session, "resume", Some("grok --resume ext-agent"))
            .unwrap_err()
            .message
            .contains("inbox-only"));
        assert!(send::command_for(&session, "Task").unwrap_err().message.contains("inbox-only"));
        assert!(send::spawn_command(harness, "Task", "", "").unwrap_err().message.contains("cannot spawn"));
        assert!(!everett::registry::session_running(&session, Some("grok --resume ext-agent"), Some(0.0)));
    }
}

#[test]
fn stdio_mcp_queues_polls_and_replies_for_both_external_harnesses() {
    let fx = fixture();
    for harness in external::HARNESSES {
        let id = format!("ext-{harness}");
        registration(&fx, &id, harness, 0.0);
        let results = mcp(
            &fx,
            &[
                ("everett_ls", json!({"hours": 0, "harness": harness})),
                ("everett_send", json!({"to": id, "text": "Report backups", "mode": "auto"})),
                ("everett_send", json!({"to": id, "text": "Do not resume", "mode": "resume"})),
            ],
            "fixture-owner",
            "",
        );
        assert_eq!(data(&results[0])["sessions"][0]["id"], id);
        let queued = data(&results[1]);
        assert_eq!(queued["mode"], "inbox");
        assert_eq!(queued["queued"], true);
        assert_eq!(queued["hooked"], false);
        assert!(queued["note"].as_str().unwrap().contains("No provider wake-up or consumption is confirmed"));
        assert_eq!(results[2]["isError"], true);
        assert!(results[2]["content"][0]["text"].as_str().unwrap().contains("inbox-only"));
        let mid = queued["message_id"].as_str().unwrap();
        let results = mcp(
            &fx,
            &[
                ("everett_inbox", json!({"peek": true})),
                ("everett_inbox", json!({})),
                ("everett_send", json!({"text": "Backups complete", "reply_to": mid})),
                ("everett_inbox", json!({})),
            ],
            &id,
            harness,
        );
        assert_eq!(data(&results[0])["messages"][0]["id"], mid);
        assert_eq!(data(&results[1])["messages"][0]["text"], "Report backups");
        assert_eq!(data(&results[2])["to"], "fixture-owner");
        assert_eq!(data(&results[3])["messages"], json!([]));
        let results = mcp(&fx, &[("everett_inbox", json!({}))], "fixture-owner", "");
        let reply = &data(&results[0])["messages"][0];
        assert_eq!(reply["from"], id);
        assert_eq!(reply["from_harness"], *harness);
        assert_eq!(reply["reply_to"], mid);
        assert_eq!(reply["text"], "Backups complete");
    }
}
