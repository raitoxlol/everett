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
    run_gateway(fx, args, input, caller, harness, false)
}

fn run_gateway(
    fx: &Fixture,
    args: &[&str],
    input: Option<&str>,
    caller: &str,
    harness: &str,
    gateway: bool,
) -> Output {
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
        .env("EVERETT_GATEWAY_EXACT_IDS", if gateway { "1" } else { "" })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(input) = input {
        command.stdin(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    } else {
        command.stdin(Stdio::null()).output().unwrap()
    }
}

fn mcp(fx: &Fixture, calls: &[(&str, Value)], caller: &str, harness: &str) -> Vec<Value> {
    mcp_gateway(fx, calls, caller, harness, false)
}

fn mcp_gateway(
    fx: &Fixture,
    calls: &[(&str, Value)],
    caller: &str,
    harness: &str,
    gateway: bool,
) -> Vec<Value> {
    let mut requests = vec![
        json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2024-11-05", "capabilities": {},
        "clientInfo": {"name": "external-fixture", "version": "1"}}}),
    ];
    for (i, (name, arguments)) in calls.iter().enumerate() {
        requests.push(
            json!({"jsonrpc": "2.0", "id": i + 1, "method": "tools/call",
                            "params": {"name": name, "arguments": arguments}}),
        );
    }
    let input: String = requests.iter().map(|r| r.to_string() + "\n").collect();
    let output = run_gateway(fx, &["mcp"], Some(&input), caller, harness, gateway);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
fn native_registration_roundtrip_preserves_the_shared_store_and_inbox() {
    let fx = fixture();
    let path = fx.home().join(".everett/external/ext-dot.json");
    let add = run(
        &fx,
        &["external", "add", "--id", "ext-dot", "--harness", "openai-dot", "--title", "Owner dot", "--cwd", "/work/dot"],
        None,
        "fixture-owner",
        "",
    );
    assert!(add.status.success(), "{}", String::from_utf8_lossy(&add.stderr));
    let record: Value = serde_json::from_slice(&add.stdout).unwrap();
    assert_eq!(record["id"], "ext-dot");
    assert_eq!(record["harness"], "openai-dot");
    assert_eq!(record["title"], "Owner dot");
    assert_eq!(record["cwd"], "/work/dot");
    assert!(record["updated"].as_f64().unwrap().is_finite());
    assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(&path).unwrap()).unwrap(), record);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
        assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o077, 0);
    }

    let listed = run(&fx, &["external", "list"], None, "fixture-owner", "");
    assert!(listed.status.success());
    assert_eq!(serde_json::from_slice::<Value>(&listed.stdout).unwrap(), json!([record]));
    let filtered = run(&fx, &["ls", "--harness", "openai-dot", "--json"], None, "fixture-owner", "");
    assert!(filtered.status.success(), "{}", String::from_utf8_lossy(&filtered.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&filtered.stdout).unwrap()[0]["id"], "ext-dot");
    let empty = run(&fx, &["ls", "--harness", "grok-bot", "--json"], None, "fixture-owner", "");
    assert!(empty.status.success(), "{}", String::from_utf8_lossy(&empty.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&empty.stdout).unwrap(), json!([]));

    let send = run(&fx, &["send", "Pending task", "--to", "ext-dot", "--mode", "inbox", "--json"], None, "fixture-owner", "");
    assert!(send.status.success(), "{}", String::from_utf8_lossy(&send.stderr));
    let queued = mcp(&fx, &[("everett_inbox", json!({"peek":true}))], "ext-dot", "openai-dot");
    assert_eq!(data(&queued[0])["messages"][0]["text"], "Pending task");

    let add_again = run(&fx, &["external", "add", "--id", "ext-dot", "--harness", "openai-dot"], None, "fixture-owner", "");
    assert_eq!(add_again.status.code(), Some(2));
    assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(&path).unwrap()).unwrap(), record);
    let wrong_harness = run(&fx, &["external", "add", "--id", "ext-dot", "--harness", "grok-bot", "--replace"], None, "fixture-owner", "");
    assert_eq!(wrong_harness.status.code(), Some(2));
    let updated = run(&fx, &["external", "add", "--id", "ext-dot", "--harness", "openai-dot", "--replace", "--title", "New scope"], None, "fixture-owner", "");
    assert!(updated.status.success(), "{}", String::from_utf8_lossy(&updated.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&updated.stdout).unwrap()["title"], "New scope");
    assert_eq!(external::load(&path).unwrap().title, "New scope");

    let removed = run(&fx, &["external", "remove", "--id", "ext-dot"], None, "fixture-owner", "");
    assert!(removed.status.success());
    assert!(!path.exists());
    let queued = mcp(&fx, &[("everett_inbox", json!({"peek":true}))], "ext-dot", "openai-dot");
    assert_eq!(data(&queued[0])["messages"][0]["text"], "Pending task");
    assert_eq!(run(&fx, &["external", "remove", "--id", "ext-dot"], None, "fixture-owner", "").status.code(), Some(2));
}

#[test]
fn registration_refuses_unsafe_ids_and_existing_symlinks() {
    let fx = fixture();
    for id in ["human", "HUMAN", "live", "../outside", "x/y", "a.b", "é", ""] {
        let output = run(&fx, &["external", "add", "--id", id, "--harness", "grok-bot"], None, "fixture-owner", "");
        assert_eq!(output.status.code(), Some(2), "{id}");
    }
    assert!(!fx.home().join(".everett/external").exists());
    let outside = fx.write("outside.json", "untouched");
    #[cfg(unix)]
    {
        let folder = fx.home().join(".everett/external");
        std::fs::create_dir_all(&folder).unwrap();
        let link = folder.join("ext-link.json");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        for extra in [vec![], vec!["--replace"]] {
            let mut args = vec!["external", "add", "--id", "ext-link", "--harness", "grok-bot"];
            args.extend(extra);
            assert_eq!(run(&fx, &args, None, "fixture-owner", "").status.code(), Some(2));
        }
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "untouched");
    }
}

#[test]
fn gateway_requires_exact_unambiguous_destinations_and_inbox_only_delivery() {
    let fx = fixture();
    let path = registration(&fx, "ext-peer-long", "grok-bot", 0.0);
    let mut record: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    record["title"] = json!("ext-missing");
    std::fs::write(&path, record.to_string()).unwrap();
    let calls = [
        (
            "everett_send",
            json!({"to":"ext-peer","text":"Blocked","mode":"inbox"}),
        ),
        (
            "everett_send",
            json!({"to":"ext-missing","text":"Blocked","mode":"inbox"}),
        ),
        ("everett_send", json!({"text":"Blocked","mode":"inbox"})),
        (
            "everett_send",
            json!({"to":"ext-peer-long","text":"Blocked","mode":"resume"}),
        ),
        (
            "everett_send",
            json!({"text":"Blocked","spawn":true,"mode":"inbox"}),
        ),
        (
            "everett_send",
            json!({"to":"ext-peer-long","text":"Exact","mode":"inbox"}),
        ),
    ];
    let results = mcp_gateway(&fx, &calls, "ext-owner", "openai-dot", true);
    assert!(results[..5].iter().all(|r| r["isError"] == true));
    assert_eq!(data(&results[5])["queued"], true);
    let polled = mcp(
        &fx,
        &[("everett_inbox", json!({}))],
        "ext-peer-long",
        "grok-bot",
    );
    assert_eq!(data(&polled[0])["messages"].as_array().unwrap().len(), 1);
    assert_eq!(data(&polled[0])["messages"][0]["text"], "Exact");

    fx.write(
        ".claude/projects/p/ext-peer-long.jsonl",
        &json!({"type":"user", "sessionId":"ext-peer-long",
        "cwd":"/work/native", "message":{"role":"user","content":"Native duplicate"}})
        .to_string(),
    );
    let duplicate = mcp_gateway(&fx, &[calls[5].clone()], "ext-owner", "openai-dot", true);
    assert_eq!(duplicate[0]["isError"], true);
    assert!(duplicate[0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("one exact session id"));
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
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let sessions: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(sessions.len(), 45);
}

#[test]
fn persistent_listing_and_routes_do_not_expose_transcript_or_resume_ids() {
    let fx = fixture();
    registration(&fx, "ext-grok", "grok-bot", 0.0);
    registration(&fx, "ext-dot", "openai-dot", 1.0);
    let output = run(
        &fx,
        &["ls", "--hours", "0", "--json"],
        None,
        "fixture-owner",
        "",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
    let output = run(
        &fx,
        &["route", "Database backup reports", "--hours", "0", "--json"],
        None,
        "fixture-owner",
        "",
    );
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
            assert_eq!(
                send::delivery_mode(&session, mode, Some("")).unwrap(),
                "inbox"
            );
        }
        assert!(
            send::delivery_mode(&session, "resume", Some("grok --resume ext-agent"))
                .unwrap_err()
                .message
                .contains("inbox-only")
        );
        assert!(send::command_for(&session, "Task")
            .unwrap_err()
            .message
            .contains("inbox-only"));
        assert!(send::spawn_command(harness, "Task", "", "")
            .unwrap_err()
            .message
            .contains("cannot spawn"));
        assert!(!everett::registry::session_running(
            &session,
            Some("grok --resume ext-agent"),
            Some(0.0)
        ));
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
                (
                    "everett_send",
                    json!({"to": id, "text": "Report backups", "mode": "auto"}),
                ),
                (
                    "everett_send",
                    json!({"to": id, "text": "Do not resume", "mode": "resume"}),
                ),
            ],
            "fixture-owner",
            "",
        );
        assert_eq!(data(&results[0])["sessions"][0]["id"], id);
        let queued = data(&results[1]);
        assert_eq!(queued["mode"], "inbox");
        assert_eq!(queued["queued"], true);
        assert_eq!(queued["hooked"], false);
        assert!(queued["note"]
            .as_str()
            .unwrap()
            .contains("No provider wake-up or consumption is confirmed"));
        assert_eq!(results[2]["isError"], true);
        assert!(results[2]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("inbox-only"));
        let mid = queued["message_id"].as_str().unwrap();
        let results = mcp(
            &fx,
            &[
                ("everett_inbox", json!({"peek": true})),
                ("everett_inbox", json!({})),
                (
                    "everett_send",
                    json!({"text": "Backups complete", "reply_to": mid}),
                ),
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
