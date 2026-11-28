mod common;

use common::{fixture, lib_home};
use everett::teresis::store::MemoryStore;
use everett::core;
use everett::proc::RunOutput;
use everett::registry;
use everett::session::{now, Session};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
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
    let release_rx: &'static std::sync::Mutex<_> = Box::leak(Box::new(std::sync::Mutex::new(release_rx)));
    let started_tx: &'static std::sync::Mutex<_> = Box::leak(Box::new(std::sync::Mutex::new(started_tx)));
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
                    common::fixture_command(&home)
                        .args(["learn", &format!("Convention number {}", n)])
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
    common::native_mcp_account(&fx.home());
    let out = fx.run_stdin(&["mcp"], &input, &[("PATH", &path), ("EVERETT_SESSION_ID", "caller-1")]);
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
    common::native_mcp_account(&fx.home());
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

// ---------------------------------------------------------------------------
// Teresis memory.db wiring: `everett learn` also lands in the shared store and
// `everett recall` searches it.
// ---------------------------------------------------------------------------

fn db_path(home: &Path) -> std::path::PathBuf {
    home.join(".local/share/teresis/memory.db")
}

fn recall_json(fx: &common::Fixture, args: &[&str]) -> serde_json::Value {
    let mut argv = vec!["recall"];
    argv.extend_from_slice(args);
    argv.push("--json");
    let out = fx.run(&argv);
    assert_eq!(out.status.code(), Some(0), "recall failed: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_str(&fx.stdout(&out)).unwrap()
}

fn memory_contents(out: &serde_json::Value) -> Vec<String> {
    out["memories"].as_array().unwrap().iter()
        .map(|m| m["content"].as_str().unwrap().to_string()).collect()
}

#[test]
fn learn_is_saved_and_recalled() {
    let fx = fixture();
    let out = fx.run(&["learn", "Deploys go through the blue pipeline", "--project", "orbit"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(fx.stdout(&out).starts_with("learned (project orbit)"), "{}", fx.stdout(&out));
    let db = db_path(&fx.home());
    assert!(db.exists(), "memory.db not created");
    #[cfg(unix)]
    assert_eq!(std::fs::metadata(&db).unwrap().permissions().mode() & 0o777, 0o600);

    let got = recall_json(&fx, &["blue pipeline", "--project", "orbit"]);
    let memories = got["memories"].as_array().unwrap();
    assert_eq!(memories.len(), 1, "{got}");
    assert_eq!(memories[0]["content"], "Deploys go through the blue pipeline");
    assert_eq!(memories[0]["scope"], "project/orbit");

    // Global learnings are found from a project scope (inheritance)...
    assert!(fx.run(&["learn", "All hands meet on Fridays"]).status.success());
    let got = recall_json(&fx, &["hands meet", "--project", "orbit"]);
    assert!(memory_contents(&got).contains(&"All hands meet on Fridays".to_string()), "{got}");
    // ...but a project fact is not found in a global-only search.
    let got = recall_json(&fx, &["blue pipeline", "--scope", "global"]);
    assert!(!memory_contents(&got).iter().any(|c| c.contains("blue pipeline")), "{got}");
}

#[test]
fn recall_tolerates_typos_and_stems() {
    let fx = fixture();
    assert!(fx.run(&["learn", "Run cargo clippy with warnings denied before each push"]).status.success());
    for query in ["clipy warnigns", "pushing"] {
        let got = recall_json(&fx, &[query]);
        assert!(memory_contents(&got).iter().any(|c| c.contains("cargo clippy")), "{query}: {got}");
    }
}

#[test]
fn keyed_learn_replaces_old_value() {
    let fx = fixture();
    assert!(fx.run(&["learn", "Editor is Neovim", "--key", "user.editor"]).status.success());
    assert!(fx.run(&["learn", "Editor is Zed", "--key", "user.editor"]).status.success());
    let got = recall_json(&fx, &["editor"]);
    let contents = memory_contents(&got);
    assert!(contents.iter().any(|c| c.contains("Zed")), "{got}");
    assert!(!contents.iter().any(|c| c.contains("Neovim")), "{got}");
    let store = MemoryStore::open(&db_path(&fx.home())).unwrap();
    let history = store.history_key("global", "user.editor").unwrap();
    assert_eq!(history.len(), 2, "{history:?}");
}

#[test]
fn ttl_sets_expiry() {
    let fx = fixture();
    let out = fx.run(&["learn", "Preview window closes tonight", "--ttl-seconds", "3600"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let con = rusqlite::Connection::open(db_path(&fx.home())).unwrap();
    let (created, expires): (f64, f64) = con
        .query_row("SELECT created_at, expires_at FROM memories", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert!((expires - created - 3600.0).abs() <= 1.0, "created {created} expires {expires}");
}

#[test]
fn core_forget_and_correct_reach_memory() {
    let fx = fixture();
    assert!(fx.run(&["learn", "Quartz builds use ccache"]).status.success());
    let res = common::mcp_calls(&fx, &[], &[], &[("everett_core",
        json!({"action": "forget", "fact": "Quartz builds use ccache"}))]);
    assert_eq!(res[0]["isError"], false, "{}", res[0]);
    let got = recall_json(&fx, &["ccache"]);
    assert!(!memory_contents(&got).iter().any(|c| c.contains("ccache")), "{got}");

    assert!(fx.run(&["learn", "Atlas uses npm for builds"]).status.success());
    let res = common::mcp_calls(&fx, &[], &[], &[("everett_core",
        json!({"action": "correct", "fact": "Atlas uses npm for builds", "replacement": "Atlas uses pnpm for builds"}))]);
    assert_eq!(res[0]["isError"], false, "{}", res[0]);
    let got = recall_json(&fx, &["Atlas"]);
    let contents = memory_contents(&got);
    assert!(contents.iter().any(|c| c == "Atlas uses pnpm for builds"), "{got}");
    assert!(!contents.iter().any(|c| c == "Atlas uses npm for builds"), "{got}");
    let con = rusqlite::Connection::open(db_path(&fx.home())).unwrap();
    let superseded: i64 = con.query_row(
        "SELECT COUNT(*) FROM memories WHERE content='Atlas uses npm for builds' AND superseded_by IS NOT NULL",
        [], |r| r.get(0)).unwrap();
    assert_eq!(superseded, 1);
}

#[test]
fn forget_and_correct_find_redacted_memories() {
    let fx = fixture();
    // Teresis redacts this token, but Everett's own filter accepts the fact.
    let fact = format!("webhook signing secret {}{}", "whsec", "_AbCdEfGh12345678");
    let out = fx.run(&["learn", &fact]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let con = rusqlite::Connection::open(db_path(&fx.home())).unwrap();
    let stored: String = con.query_row("SELECT content FROM memories", [], |r| r.get(0)).unwrap();
    assert!(stored.contains("[REDACTED:"), "{stored}");
    drop(con);

    let res = common::mcp_calls(&fx, &[], &[], &[("everett_core",
        json!({"action": "forget", "fact": fact}))]);
    assert_eq!(res[0]["isError"], false, "{}", res[0]);
    assert_eq!(common::tool_json(&res[0])["memory_removed"], json!(1), "{}", res[0]);
    let got = recall_json(&fx, &["webhook signing"]);
    assert!(!memory_contents(&got).iter().any(|c| c.contains("webhook signing")), "{got}");

    assert!(fx.run(&["learn", &fact]).status.success());
    let res = common::mcp_calls(&fx, &[], &[], &[("everett_core",
        json!({"action": "correct", "fact": fact, "replacement": "webhook signing secret rotated"}))]);
    assert_eq!(res[0]["isError"], false, "{}", res[0]);
    let got = recall_json(&fx, &["webhook signing"]);
    assert!(memory_contents(&got).iter().any(|c| c == "webhook signing secret rotated"), "{got}");
    let con = rusqlite::Connection::open(db_path(&fx.home())).unwrap();
    let superseded: i64 = con.query_row(
        "SELECT COUNT(*) FROM memories WHERE content LIKE '%[REDACTED:%' AND superseded_by IS NOT NULL",
        [], |r| r.get(0)).unwrap();
    assert_eq!(superseded, 1);
}

#[test]
fn forget_matches_like_core() {
    let fx = fixture();
    assert!(fx.run(&["learn", "Use cargo_test for builds"]).status.success());
    let res = common::mcp_calls(&fx, &[], &[], &[("everett_core",
        json!({"action": "forget", "fact": "Use cargo test for builds"}))]);
    assert_eq!(res[0]["isError"], false, "{}", res[0]);
    assert_eq!(common::tool_json(&res[0])["memory_removed"], json!(1), "{}", res[0]);
    let got = recall_json(&fx, &["cargo_test builds"]);
    assert!(got["memories"].as_array().unwrap().is_empty(), "{got}");

    assert!(fx.run(&["learn", "Use cargo_test for builds"]).status.success());
    let res = common::mcp_calls(&fx, &[], &[], &[("everett_core",
        json!({"action": "correct", "fact": "Use cargo test for builds", "replacement": "Use cargo_nextest for builds"}))]);
    assert_eq!(res[0]["isError"], false, "{}", res[0]);
    let got = recall_json(&fx, &["cargo builds"]);
    assert!(memory_contents(&got).iter().any(|c| c == "Use cargo_nextest for builds"), "{got}");
    let con = rusqlite::Connection::open(db_path(&fx.home())).unwrap();
    let active_old: i64 = con.query_row(
        "SELECT COUNT(*) FROM memories WHERE content='Use cargo_test for builds' AND superseded_by IS NULL AND deleted_at IS NULL",
        [], |r| r.get(0)).unwrap();
    assert_eq!(active_old, 0, "old row must not stay active");
}

#[test]
fn correct_to_an_existing_fact_retires_the_old_one() {
    let fx = fixture();
    assert!(fx.run(&["learn", "Atlas uses npm for builds", "--project", "orbit"]).status.success());
    assert!(fx.run(&["learn", "Atlas uses pnpm for builds", "--project", "orbit"]).status.success());
    let res = common::mcp_calls(&fx, &[], &[], &[("everett_core",
        json!({"action": "correct", "project": "orbit", "fact": "Atlas uses npm for builds", "replacement": "Atlas uses pnpm for builds"}))]);
    assert_eq!(res[0]["isError"], false, "{}", res[0]);
    let got = recall_json(&fx, &["Atlas builds", "--project", "orbit"]);
    let contents = memory_contents(&got);
    assert_eq!(contents, vec!["Atlas uses pnpm for builds".to_string()], "{got}");
}

#[test]
fn correcting_a_temporary_fact_keeps_its_expiry() {
    let fx = fixture();
    let out = fx.run(&["learn", "Preview closes tonight", "--ttl-seconds", "3600"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let res = common::mcp_calls(&fx, &[], &[], &[("everett_core",
        json!({"action": "correct", "fact": "Preview closes tonight", "replacement": "Preview closes tomorrow"}))]);
    assert_eq!(res[0]["isError"], false, "{}", res[0]);
    let con = rusqlite::Connection::open(db_path(&fx.home())).unwrap();
    let old_expiry: f64 = con.query_row(
        "SELECT expires_at FROM memories WHERE content='Preview closes tonight'", [], |r| r.get(0)).unwrap();
    let (new_content, new_expiry): (String, Option<f64>) = con.query_row(
        "SELECT content, expires_at FROM memories WHERE superseded_by IS NULL AND deleted_at IS NULL",
        [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(new_content, "Preview closes tomorrow");
    let new_expiry = new_expiry.expect("the corrected copy must keep an expiry");
    assert!((new_expiry - old_expiry).abs() <= 2.0, "old {old_expiry} new {new_expiry}");
}

#[test]
fn memory_off_and_broken_db_never_block_learning() {
    let fx = fixture();
    let out = fx.run_stdin(&["learn", "Fact while memory off"], "", &[("EVERETT_MEMORY_DB", "off")]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!db_path(&fx.home()).exists());
    let out = fx.run_stdin(&["recall", "anything"], "", &[("EVERETT_MEMORY_DB", "off")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Memory is off"), "{}", String::from_utf8_lossy(&out.stderr));

    let broken = fx.write("broken.db", "not a sqlite file at all");
    let out = fx.run_stdin(&["learn", "Fact despite broken memory"], "",
        &[("EVERETT_MEMORY_DB", broken.to_str().unwrap())]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(fx.stdout(&out).starts_with("learned ("), "{}", fx.stdout(&out));
    assert!(String::from_utf8_lossy(&out.stderr).contains("memory.db was not updated"),
        "{}", String::from_utf8_lossy(&out.stderr));
    let inbox = std::fs::read_to_string(fx.home().join(".everett/core/inbox.jsonl")).unwrap();
    assert!(inbox.contains("Fact despite broken memory"), "{inbox}");
}

#[test]
fn sandboxed_home_ignores_teresis_db() {
    let fx = fixture();
    let other = fx.home().join("other/memory.db");
    let out = fx.run_stdin(&["learn", "Sandboxed fact"], "",
        &[("TERESIS_DB", other.to_str().unwrap())]);
    assert_eq!(out.status.code(), Some(0));
    assert!(!other.exists(), "TERESIS_DB must be ignored when EVERETT_HOME is set");
    assert!(db_path(&fx.home()).exists());

    let custom = fx.home().join("custom/mem.db");
    let out = fx.run_stdin(&["learn", "Custom path fact"], "",
        &[("EVERETT_MEMORY_DB", custom.to_str().unwrap())]);
    assert_eq!(out.status.code(), Some(0));
    assert!(custom.exists());
    let got = recall_json(&fx, &["Sandboxed"]);
    assert!(memory_contents(&got).iter().any(|c| c.contains("Sandboxed fact")), "{got}");
}

#[test]
fn mcp_recall_round_trip() {
    let fx = fixture();
    let list = common::mcp_exchange(&fx, &[], &[],
        &[("tools/list".to_string(), json!({}))]);
    let tools = list[0]["result"]["tools"].as_array().unwrap();
    let recall = tools.iter().find(|t| t["name"] == "everett_recall").expect("everett_recall advertised");
    assert!(recall["description"].as_str().unwrap().contains("memory.db"), "{recall}");
    let schema = &recall["inputSchema"];
    assert_eq!(schema["required"], json!(["query"]));
    assert_eq!(schema["properties"]["query"]["maxLength"], 500);
    assert_eq!(schema["properties"]["limit"]["minimum"], 1);
    assert_eq!(schema["properties"]["limit"]["maximum"], 50);
    assert_eq!(schema["properties"]["scope"]["enum"], json!(["global", "project"]));

    let res = common::mcp_calls(&fx, &[], &[], &[
        ("everett_learn", json!({"fact": "MCP memories persist across calls", "key": "mcp.persist"})),
        ("everett_recall", json!({"query": "persist across calls"})),
        ("everett_recall", json!({"query": "x", "scope": "project"})),
    ]);
    let learned = common::tool_json(&res[0]);
    assert_eq!(res[0]["isError"], false, "{}", res[0]);
    assert_eq!(learned["memory"]["status"], "created", "{learned}");
    let recalled = common::tool_json(&res[1]);
    assert!(recalled["memories"].as_array().unwrap().iter()
        .any(|m| m["content"] == "MCP memories persist across calls"), "{recalled}");
    assert_eq!(res[2]["isError"], true, "scope=project with no project must fail: {}", res[2]);

    let call = |limit: serde_json::Value| ("tools/call".to_string(),
        json!({"name": "everett_recall", "arguments": {"query": "q", "limit": limit}}));
    let res = common::mcp_exchange(&fx, &[], &[],
        &[call(json!(0)), call(json!(51)), call(json!("x"))]);
    for r in &res {
        assert_eq!(r["error"]["code"], -32602, "{r}");
    }
}

#[test]
fn python_teresis_reads_everett_and_back() {
    let Some(src) = common::teresis_src() else {
        return;
    };
    let fx = fixture();
    let out = fx.run(&["learn", "The relay bill is capped at five dollars", "--project", "orbit"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let db = db_path(&fx.home());
    let script = r#"
import json, sys
sys.path.insert(0, sys.argv[1])
from teresis.store import MemoryStore
s = MemoryStore(sys.argv[2], embedder=None, reranker=None)
hits = s.recall("relay bill", scope="project/orbit", explain=False)
print(json.dumps([{"content": m.content, "source": m.source} for m in hits]))
r = s.remember("Wright wants weekly cost reports", scope="project/orbit", key="reports.cadence")
print(json.dumps({"id": r.id, "status": r.status}))
"#;
    let out = Command::new("python3").args(["-B", "-c"]).arg(script)
        .arg(&src).arg(&db).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    let hits: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert!(hits.as_array().unwrap().iter().any(|m|
        m["content"] == "The relay bill is capped at five dollars"
        && m["source"].as_str().unwrap_or("").starts_with("everett:")), "{hits}");

    let got = recall_json(&fx, &["cost reports", "--project", "orbit"]);
    assert!(memory_contents(&got).iter().any(|c| c.contains("weekly cost reports")), "{got}");

    let out = fx.run(&["learn", "Wright wants monthly cost reports", "--project", "orbit", "--key", "reports.cadence"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let script = r#"
import json, sys
sys.path.insert(0, sys.argv[1])
from teresis.store import MemoryStore
s = MemoryStore(sys.argv[2], embedder=None, reranker=None)
print(json.dumps([(m.content, bool(m.superseded_by)) for m in s.history(scope="project/orbit", key="reports.cadence")]))
"#;
    let out = Command::new("python3").args(["-B", "-c"]).arg(script)
        .arg(&src).arg(&db).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let history: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert_eq!(history, json!([
        ["Wright wants weekly cost reports", true],
        ["Wright wants monthly cost reports", false],
    ]), "{history}");
}

#[test]
fn relearned_duplicate_takes_the_new_expiry() {
    let fx = fixture();
    assert!(fx.run(&["learn", "Preview uses port 4200", "--ttl-seconds", "3600"]).status.success());
    let con = rusqlite::Connection::open(db_path(&fx.home())).unwrap();
    let expiry: Option<f64> = con.query_row("SELECT expires_at FROM memories", [], |r| r.get(0)).unwrap();
    assert!(expiry.is_some(), "temporary fact must carry an expiry");
    drop(con);
    // Relearning the same fact without a TTL promotes it, like the core store does.
    assert!(fx.run(&["learn", "Preview uses port 4200"]).status.success());
    let con = rusqlite::Connection::open(db_path(&fx.home())).unwrap();
    let expiry: Option<f64> = con.query_row("SELECT expires_at FROM memories", [], |r| r.get(0)).unwrap();
    assert_eq!(expiry, None, "relearned fact must lose its expiry");
}

#[test]
fn correction_to_an_existing_match_keeps_the_replacement() {
    let fx = fixture();
    assert!(fx.run(&["learn", "Use cargo test for builds"]).status.success());
    assert!(fx.run(&["learn", "Use cargo_test for builds"]).status.success());
    let res = common::mcp_calls(&fx, &[], &[], &[("everett_core",
        json!({"action": "correct", "fact": "Use cargo_test for builds", "replacement": "Use cargo test for builds"}))]);
    assert_eq!(res[0]["isError"], false, "{}", res[0]);
    let got = recall_json(&fx, &["cargo builds"]);
    let contents = memory_contents(&got);
    assert_eq!(contents, vec!["Use cargo test for builds".to_string()], "{got}");
}

#[test]
fn temporary_refinement_does_not_hide_the_permanent_fact() {
    let fx = fixture();
    assert!(fx.run(&["learn", "Use blue pipeline"]).status.success());
    assert!(fx.run(&["learn", "Use blue pipeline for hotfixes", "--ttl-seconds", "1"]).status.success());
    let got = recall_json(&fx, &["blue pipeline"]);
    assert!(memory_contents(&got).iter().any(|c| c == "Use blue pipeline"), "{got}");
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let got = recall_json(&fx, &["blue pipeline"]);
    let contents = memory_contents(&got);
    assert_eq!(contents, vec!["Use blue pipeline".to_string()], "{got}");
}

#[test]
fn negated_facts_stay_distinct() {
    let fx = fixture();
    assert!(fx.run(&["learn", "Staging is ready"]).status.success());
    assert!(fx.run(&["learn", "Staging is not ready"]).status.success());
    let got = recall_json(&fx, &["staging ready"]);
    let contents = memory_contents(&got);
    assert!(contents.iter().any(|c| c == "Staging is ready"), "{got}");
    assert!(contents.iter().any(|c| c == "Staging is not ready"), "{got}");
}

#[test]
fn non_ascii_digits_do_not_crash_recall() {
    let fx = fixture();
    assert!(fx.run(&["learn", "Deploy notes live in the runbook"]).status.success());
    for query in ["deploy notes from 2026-١١-٢٣", "deploy notes ٣ days ago", "deploy in march ٢٠٢٦"] {
        let got = recall_json(&fx, &[query]);
        assert!(memory_contents(&got).iter().any(|c| c.contains("runbook")), "{query}: {got}");
    }
}

#[test]
fn failed_forget_is_retried() {
    let fx = fixture();
    assert!(fx.run(&["learn", "Quartz builds use ccache"]).status.success());
    let db = db_path(&fx.home());
    // WAL mode keeps memory.db-shm/-wal beside the db; all three share the
    // read-only state, so flip and restore them together.
    let db_files = |suffix: &str| std::path::PathBuf::from(format!("{}{}", db.display(), suffix));
    for suffix in ["", "-shm", "-wal"] {
        let p = db_files(suffix);
        if p.exists() {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o444)).unwrap();
        }
    }
    let res = common::mcp_calls(&fx, &[], &[], &[("everett_core",
        json!({"action": "forget", "fact": "Quartz builds use ccache"}))]);
    assert_eq!(res[0]["isError"], false, "{}", res[0]);
    let body = common::tool_json(&res[0]);
    assert!(body.get("memory_error").is_some() || body["memory_removed"] == json!(0), "{body}");
    for suffix in ["", "-shm", "-wal"] {
        let p = db_files(suffix);
        if p.exists() {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let res = common::mcp_calls(&fx, &[], &[], &[("everett_core",
        json!({"action": "forget", "fact": "Quartz builds use ccache"}))]);
    assert_eq!(res[0]["isError"], false, "{}", res[0]);
    assert_eq!(common::tool_json(&res[0])["memory_removed"], json!(1), "{}", res[0]);
    let got = recall_json(&fx, &["ccache"]);
    assert!(!memory_contents(&got).iter().any(|c| c.contains("ccache")), "{got}");
}

#[test]
fn explicit_project_scope_does_not_search_global() {
    let fx = fixture();
    assert!(fx.run(&["learn", "All hands meet on Fridays"]).status.success());
    assert!(fx.run(&["learn", "Orbit hands meet on Mondays", "--project", "orbit"]).status.success());
    let got = recall_json(&fx, &["hands meet", "--project", "orbit", "--scope", "project"]);
    let contents = memory_contents(&got);
    assert!(contents.iter().any(|c| c.contains("Orbit hands")), "{got}");
    assert!(!contents.iter().any(|c| c.contains("All hands")), "{got}");
    let scopes: Vec<String> = got["scopes"].as_array().unwrap().iter()
        .map(|s| s.as_str().unwrap().to_string()).collect();
    assert_eq!(scopes, vec!["project/orbit".to_string()], "{got}");
    // The default search still inherits global.
    let got = recall_json(&fx, &["hands meet", "--project", "orbit"]);
    assert!(memory_contents(&got).iter().any(|c| c.contains("All hands")), "{got}");
}
