mod common;

use common::{fixture, lib_home};
use everett::core;
use everett::proc::RunOutput;
use everett::registry;
use everett::session::{now, Session};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

type Runner<'a> = dyn Fn(&[String], Option<&str>, &HashMap<String, String>, f64) -> Result<RunOutput, String> + Sync + 'a;

fn ok(stdout: &str) -> Result<RunOutput, String> {
    Ok(RunOutput { code: 0, stdout: stdout.into(), stderr: String::new(), timed_out: false })
}

fn texts(items: Vec<serde_json::Map<String, Value>>) -> Vec<String> {
    items.iter().map(|i| i["text"].as_str().unwrap().to_string()).collect()
}

#[test]
fn learning_during_merge_stays_pending_and_is_not_archived() {
    let _h = lib_home();
    core::learn("Initial convention", None, None, None).unwrap();
    let runner: &Runner = &|_, _, _, _| {
        core::learn("Late convention", None, None, None).unwrap();
        ok(&json!({"global": "# Core\n\n- Initial convention\n", "projects": {}}).to_string())
    };
    let result = core::merge_with("claude", false, runner).unwrap();
    assert_eq!(result["merged"], 1);
    assert_eq!(texts(core::read_inbox()), ["Late convention"]);
    let archived = std::fs::read_to_string(Path::new(result["history"].as_str().unwrap()).join("inbox.jsonl")).unwrap();
    let archived: Vec<String> = archived.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()["text"].as_str().unwrap().into()).collect();
    assert_eq!(archived, ["Initial convention"]);
    core::merge("none", false).unwrap();
    let text = std::fs::read_to_string(core::global_path()).unwrap();
    assert!(text.contains("Late convention") && text.contains("Initial convention"));
    assert!(core::read_inbox().is_empty());
}

#[test]
fn second_merge_refuses_promptly_while_learning_remains_available() {
    let _h = lib_home();
    core::learn("Initial convention", None, None, None).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = std::sync::Mutex::new(release_rx);
    let started_tx = std::sync::Mutex::new(started_tx);
    std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            let runner: &Runner = &|_, _, _, _| {
                started_tx.lock().unwrap().send(()).unwrap();
                release_rx.lock().unwrap().recv_timeout(std::time::Duration::from_secs(5)).unwrap();
                ok(&json!({"global": "- Initial convention", "projects": {}}).to_string())
            };
            core::merge_with("claude", false, runner).map(|r| r["merged"].clone())
        });
        started_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        core::learn("Concurrent convention", None, None, None).unwrap();
        let started = std::time::Instant::now();
        let err = core::merge("none", false).unwrap_err().to_string();
        assert!(started.elapsed().as_secs_f64() < 2.0);
        assert!(err.contains("merge is already running"), "{err}");
        release_tx.send(()).unwrap();
        assert_eq!(first.join().unwrap().unwrap(), json!(1));
    });
    assert_eq!(texts(core::read_inbox()), ["Concurrent convention"]);
}

#[test]
fn parallel_cli_learnings_produce_complete_unique_json_lines() {
    let fx = fixture();
    let home = fx.home();
    let results: Vec<_> = std::thread::scope(|scope| {
        (0..8)
            .map(|n| {
                let home = home.clone();
                scope.spawn(move || {
                    std::process::Command::new(env!("CARGO_BIN_EXE_everett"))
                        .args(["learn", &format!("Convention number {}", n)])
                        .env("HOME", &home)
                        .env("EVERETT_HOME", &home)
                        .current_dir(&home)
                        .output()
                        .unwrap()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect()
    });
    assert!(results.iter().all(|r| r.status.success()));
    let inbox = std::fs::read_to_string(home.join(".everett/core/inbox.jsonl")).unwrap();
    let mut got: Vec<String> = inbox.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()["text"].as_str().unwrap().into()).collect();
    got.sort();
    let mut want: Vec<String> = (0..8).map(|n| format!("Convention number {}", n)).collect();
    want.sort();
    assert_eq!(got, want);
    assert!(fx.stdout(&fx.run(&["trunk", "merge", "--llm", "none"])).contains('8'));
}

#[test]
fn invalid_or_failed_model_output_preserves_core_and_facts() {
    let _h = lib_home();
    core::learn("Original convention", None, None, None).unwrap();
    core::merge("none", false).unwrap();
    let before = std::fs::read(core::global_path()).unwrap();
    core::learn("Pending convention", None, None, None).unwrap();
    let invalid: &Runner = &|_, _, _, _| ok(r#"{"global":"new","projects":["invalid"]}"#);
    assert!(core::merge_with("claude", false, invalid).is_err());
    assert_eq!(std::fs::read(core::global_path()).unwrap(), before);
    assert_eq!(texts(core::read_inbox()), ["Pending convention"]);
    let failed: &Runner = &|_, _, _, _| {
        core::learn("Late convention", None, None, None).unwrap();
        Ok(RunOutput { code: 0, stdout: String::new(), stderr: String::new(), timed_out: true })
    };
    assert!(core::merge_with("claude", false, failed).is_err());
    assert_eq!(texts(core::read_inbox()), ["Pending convention", "Late convention"]);
    assert_eq!(std::fs::read(core::global_path()).unwrap(), before);
}

#[test]
fn dry_run_creates_no_core_or_archive_files() {
    let _h = lib_home();
    core::learn("Initial convention", None, None, None).unwrap();
    let snapshot = || {
        let mut files: Vec<(String, Vec<u8>)> = walk(&core::core_dir());
        files.sort();
        files
    };
    let before = snapshot();
    assert_eq!(core::merge("none", true).unwrap()["merged"], 1);
    assert_eq!(snapshot(), before);
}

fn walk(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push((path.to_string_lossy().into(), std::fs::read(&path).unwrap()));
        }
    }
    out
}

fn fake_bin(dir: &Path, name: &str, script: &str) {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{}\n", script)).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn resumed_harness_cannot_consume_the_mcp_protocol() {
    let fx = fixture();
    let cwd = fx.home().to_string_lossy().to_string();
    let file = fx.write(".claude/projects/p/peer-1.jsonl", &common::jsonl(&[json!({"type": "user", "sessionId": "peer-1",
        "cwd": cwd, "timestamp": "2026-01-01T00:00:00Z", "message": {"role": "user", "content": "peer task"}})]));
    std::fs::File::options().write(true).open(&file).unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(300)).unwrap();
    let bin = fx.home().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    fake_bin(&bin, "claude", "printf 'stdin=[%s]' \"$(cat)\"");
    let input = common::jsonl(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "everett_send",
            "arguments": {"text": "Check stdin", "to": "peer-1", "mode": "resume"}}}),
        json!({"jsonrpc": "2.0", "id": 99, "method": "ping"}),
    ]);
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let out = fx.run_stdin(&["mcp"], &input, &[("PATH", &path)]);
    let replies: Vec<Value> = fx.stdout(&out).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let sent = replies.iter().find(|r| r["id"] == 1).unwrap();
    assert!(sent.to_string().contains("stdin=[]"), "{sent}");
    assert!(replies.iter().any(|r| r["id"] == 99), "the harness consumed input meant for the MCP server");
}

#[test]
fn interrupted_codex_is_not_retried_and_reports_uncertain_delivery() {
    let h = lib_home();
    let bin = h.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    fake_bin(&bin, "codex", "echo attempt >> \"$PWD/attempts\"; echo 'Error: Interrupted system call (os error 4)' >&2; exit 1");
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("PATH".into(), format!("{}:/usr/bin:/bin", bin.display()));
    let s = Session::new("codex", "interruption-peer", &h.path().to_string_lossy(), "/missing", "", 0.0);
    let err = everett::send::send(&s, "Check delivery", 20.0, 0.0, Some(&env)).err().unwrap().to_string();
    assert!(err.contains("Delivery is unconfirmed") && err.contains("check the target session before retrying"), "{err}");
    assert_eq!(std::fs::read_to_string(h.path().join("attempts")).unwrap(), "attempt\n");
}

#[test]
fn mcp_reaches_older_named_peers_with_explicit_hours() {
    let fx = fixture();
    let file = fx.write(".claude/projects/test/old-peer.jsonl", &common::jsonl(&[json!({"type": "user", "sessionId": "old-peer",
        "cwd": fx.home().join("backups"), "message": {"role": "user", "content": "Restore database backups using sqlite"}})]));
    std::fs::File::options().write(true).open(&file).unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(96 * 3600)).unwrap();
    let args = json!({"text": "Report backup progress", "to": "old-peer", "mode": "inbox"});
    let mut wide = args.clone();
    wide["hours"] = json!(168);
    let call = |id: i64, name: &str, arguments: &Value| json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {"name": name, "arguments": arguments}});
    let input = common::jsonl(&[call(1, "everett_send", &args), call(2, "everett_send", &wide),
        call(3, "everett_route", &json!({"text": "Restore database backups using sqlite", "router": "local", "hours": 168}))]);
    let out = fx.run_stdin(&["mcp"], &input, &[("EVERETT_SESSION_ID", "lookup-caller"), ("EVERETT_ROUTER", "local")]);
    let replies: Vec<Value> = fx.stdout(&out).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies[0]["result"]["isError"], true);
    assert!(replies[0]["result"]["content"][0]["text"].as_str().unwrap().contains("everett_ls"));
    assert_eq!(replies[1]["result"]["isError"], false, "{}", replies[1]);
    let queued = std::fs::read_to_string(fx.home().join(".everett/inbox/old-peer.jsonl")).unwrap();
    assert!(queued.contains("Report backup progress"));
    assert_eq!(replies[2]["result"]["structuredContent"]["session"]["id"], "old-peer", "{}", replies[2]);
    let _ = now();
}

#[test]
fn plain_titles_resolve_and_duplicates_require_an_id() {
    let mut a = Session::new("codex", "peer-a", "/work/one", "", "", 0.0);
    a.title = "Slash resume not showing newest sessions".into();
    let mut b = Session::new("claude", "peer-b", "/work/two", "", "", 0.0);
    b.title = "Other work".into();
    assert_eq!(registry::find("slash resume not showing newest sessions", &[a.clone(), b.clone()]).unwrap().id, "peer-a");
    a.title = "Limen".into();
    b.title = "Limen".into();
    let err = registry::find("Limen", &[a, b]).unwrap_err().to_string();
    assert!(err.contains("matches 2 sessions"), "{err}");
}
