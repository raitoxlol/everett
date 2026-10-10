//! Audit-fix regressions: card path traversal, injected-content allowlist,
//! live-session hop guard, MCP oversized batching, and write-failure
//! propagation.

mod common;

use common::{fixture, jsonl, plant};
use serde_json::{json, Value};

fn fixture_with_session() -> common::Fixture {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    fx
}

fn now_ts() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

#[test]
fn card_path_traversal_is_not_read() {
    let fx = fixture();
    fx.write(
        ".claude/projects/app/evil.jsonl",
        &jsonl(&[json!({
            "type": "user", "sessionId": "../evil", "cwd": "/w",
            "timestamp": "2026-09-23T00:00:00Z",
            "message": {"role": "user", "content": "hello"}
        })]),
    );
    // A card planted where the naive join `cards/<id>.md` would resolve.
    fx.write(".everett/evil.md", "STOLEN CARD BODY");
    let list = fx.ls_json();
    let s = list
        .iter()
        .find(|x| x["id"] == "../evil")
        .expect("session still lists");
    let card = s.get("card").and_then(|v| v.as_str()).unwrap_or("");
    assert!(!card.contains("STOLEN"), "read outside cards dir: {card}");
    assert!(fx.home().join(".everett/evil.md").exists());
}

#[test]
fn markup_first_message_is_listed() {
    let fx = fixture();
    fx.write(
        ".claude/projects/app/h-1.jsonl",
        &jsonl(&[json!({
            "type": "user", "sessionId": "h-1", "cwd": "/w",
            "timestamp": "2026-09-23T00:00:00Z",
            "message": {"role": "user", "content": "<div>broken layout</div>"}
        })]),
    );
    let list = fx.ls_json();
    assert!(
        list.iter().any(|x| x["id"] == "h-1"),
        "user HTML/XML text must not hide the session: {list:?}"
    );
}

#[test]
fn live_session_hop_guard_holds_without_env() {
    let fx = fixture_with_session();
    fx.write(
        ".everett/inbox/b-live.jsonl",
        &(json!({
            "id": "m1", "to": "b-live", "from": "a-live", "text": "forward",
            "ts": now_ts(), "hops": 3, "kind": "message", "reply_to": ""
        })
        .to_string()
            + "\n"),
    );
    // Deliver b-live's pending message as its inbox hook would.
    let out = fx.run_stdin(
        &["hook", "claude_inbox"],
        &json!({"session_id": "b-live", "hook_event_name": "UserPromptSubmit"}).to_string(),
        &[],
    );
    assert_eq!(out.status.code(), Some(0));
    // b-live's next fresh send has no EVERETT_HOPS env; the recorded hops refuse it.
    let out = fx.run_stdin(
        &["send", "onward", "--to", "c-1", "--mode", "inbox", "--json"],
        "",
        &[("EVERETT_SESSION_ID", "b-live")],
    );
    assert_eq!(
        out.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("Hop limit"));
    // The memory expires: after HOP_MEMORY_SECONDS the session sends again.
    let live = fx.home().join(".everett/inbox/live/b-live.json");
    let mut data: Value = serde_json::from_str(&std::fs::read_to_string(&live).unwrap()).unwrap();
    data["hops_ts"] = json!(now_ts() - 901.0);
    std::fs::write(&live, data.to_string()).unwrap();
    let out = fx.run_stdin(
        &["send", "onward", "--to", "c-1", "--mode", "inbox", "--json"],
        "",
        &[("EVERETT_SESSION_ID", "b-live")],
    );
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
}

fn mcp_call_inbox(home_session: &str) -> String {
    [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                          "clientInfo": {"name": "t", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
               "params": {"name": "everett_inbox",
                          "arguments": {"session_id": home_session}}}),
    ]
    .iter()
    .map(|r| r.to_string() + "\n")
    .collect()
}

fn mcp_replies(stdout: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn oversized_first_inbox_message_delivers_truncated() {
    let fx = fixture();
    let big = "x".repeat(300_000);
    fx.write(
        ".everett/inbox/solo.jsonl",
        &(json!({
            "id": "m1", "to": "solo", "from": "a", "text": big,
            "ts": now_ts(), "hops": 1, "kind": "message", "reply_to": ""
        })
        .to_string()
            + "\n"),
    );
    let out = fx.run_stdin(&["mcp"], &mcp_call_inbox("solo"), &[]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let replies = mcp_replies(&out.stdout);
    let call = replies.iter().find(|r| r["id"] == json!(2)).expect("tools/call reply");
    let msgs = call["result"]["structuredContent"]["messages"]
        .as_array()
        .expect("messages array");
    assert_eq!(msgs.len(), 1, "an oversized first message must still deliver");
    assert_eq!(msgs[0]["truncated"], json!(true));
    assert!(msgs[0]["text"].as_str().unwrap().len() < 300_000);
    // Delivered, not left pending forever.
    let done = fx.home().join(".everett/inbox/solo.done");
    assert!(done.exists() && std::fs::read_to_string(&done).unwrap().contains("m1"));
}

fn is_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

#[test]
fn inbox_write_failure_is_an_error() {
    if is_root() {
        return; // root ignores mode bits
    }
    use std::os::unix::fs::PermissionsExt;
    let fx = fixture_with_session();
    let dir = fx.home().join(".everett/inbox");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let out = fx.run(&["send", "hi", "--to", "c-1", "--mode", "inbox", "--json"]);
    assert_ne!(out.status.code(), Some(0), "must not report success");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.starts_with("everett:"), "{err}");
    assert!(
        err.contains("cannot write") || err.contains("cannot create") || err.contains("cannot lock"),
        "{err}"
    );
}

#[test]
fn regenerate_auto_failure_is_an_error() {
    if is_root() {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let fx = fixture_with_session();
    // Existing auto card so regeneration is attempted; a read-only card file
    // makes the write fail.
    let card = fx.write(".everett/cards/c-1.md", "<!-- everett:auto -->\nold\n");
    std::fs::set_permissions(&card, std::fs::Permissions::from_mode(0o400)).unwrap();
    let out = fx.run(&["cards", "--regenerate-auto"]);
    assert_ne!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("everett:"));
}

fn state(fx: &common::Fixture, name: &str) -> std::path::PathBuf {
    fx.write(
        &format!(".everett/state/{name}.json"),
        &json!({"kind": "needs-input", "session_id": name, "ts": now_ts() - 7200.0,
                "since": now_ts() - 7200.0})
        .to_string(),
    )
}

#[test]
fn escalation_scan_throttled_by_stamp() {
    let fx = fixture();
    let s1 = state(&fx, "s1");
    let prompt = json!({"session_id": "x-1", "hook_event_name": "UserPromptSubmit"}).to_string();
    let env = [("EVERETT_ESCALATE_MINUTES", "0.0001")];
    let out = fx.run_stdin(&["hook", "claude_inbox"], &prompt, &env);
    assert_eq!(out.status.code(), Some(0));
    let first: Value = serde_json::from_str(&std::fs::read_to_string(&s1).unwrap()).unwrap();
    assert_eq!(first["escalated"], json!(true));
    // Stamp advanced; a second hook call inside the throttle window must not
    // scan again — a newly aging session stays un-escalated.
    let stamp = fx.home().join(".everett/state/.escalate-check");
    assert!(stamp.exists());
    let s2 = state(&fx, "s2");
    let out = fx.run_stdin(&["hook", "claude_inbox"], &prompt, &env);
    assert_eq!(out.status.code(), Some(0));
    let second: Value = serde_json::from_str(&std::fs::read_to_string(&s2).unwrap()).unwrap();
    assert!(second.get("escalated").is_none(), "throttled scan still ran: {second}");
}

#[test]
fn install_hooks_replaces_stale_everett_entries() {
    let fx = fixture();
    let settings = fx.write(
        ".claude/settings.json",
        &(json!({
            "hooks": {
                "UserPromptSubmit": [
                    {"hooks": [{"type": "command", "command": "/gone/everett hook claude_inbox"}]},
                    {"hooks": [{"type": "command", "command": "cat >> log"}]}
                ]
            }
        })
        .to_string()
            + "\n"),
    );
    let out = fx.run(&["install-hooks", "--claude", "--apply"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let data: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let cmds: Vec<&str> = data["hooks"]["UserPromptSubmit"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|e| e["hooks"].as_array().unwrap().iter())
        .filter_map(|h| h["command"].as_str())
        .collect();
    assert!(cmds.iter().any(|c| c.contains("everett hook") && !c.contains("/gone/")), "{cmds:?}");
    assert!(!cmds.iter().any(|c| c.contains("/gone/")), "{cmds:?}");
    assert!(cmds.iter().any(|c| c.contains("cat >> log")), "{cmds:?}");
    assert!(
        std::fs::read_dir(settings.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("settings.json.everett-bak-"))
    );
}

#[test]
fn doctor_flags_stale_hook() {
    let fx = fixture();
    fx.write(".claude/projects/app/c-1.jsonl", "{}\n"); // claude is seen
    fx.write(
        ".claude/settings.json",
        &(json!({
            "hooks": {
                "UserPromptSubmit": [
                    {"hooks": [{"type": "command", "command": "/gone/everett hook claude_inbox"}]}
                ]
            }
        })
        .to_string()
            + "\n"),
    );
    let out = fx.run(&["doctor"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("stale"), "{text}");
    assert!(text.contains("install-hooks --claude --apply"), "{text}");
}

#[test]
fn cards_hours_subcommand_flag_is_used() {
    let fx = fixture();
    let out = fx.run(&["cards", "--hours", "5"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("last 5 hours"));
}

// ---- audit batch 2 ------------------------------------------------------------

fn post_msg(fx: &common::Fixture, sid: &str, id: &str, text: &str, ts: f64) {
    let line = json!({"id": id, "from": "human", "to": sid, "text": text, "ts": ts,
                      "reply_to": "", "hops": 1, "kind": "message"})
        .to_string()
        + "\n";
    let path = fx.home().join(format!(".everett/inbox/{sid}.jsonl"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut body = std::fs::read_to_string(&path).unwrap_or_default();
    body.push_str(&line);
    std::fs::write(&path, body).unwrap();
}

#[test]
fn inbox_take_is_atomic_and_compacts() {
    let fx = fixture();
    post_msg(&fx, "x-1", "m-live", "hello live", now_ts());
    post_msg(&fx, "x-1", "m-old", "hello old", now_ts() - 8.0 * 24.0 * 3600.0);
    fx.write(".everett/inbox/x-1.done", "m-gone\n");
    // A stale live record older than LIVE_TTL should be reaped in the same pass.
    let stale = fx.home().join(".everett/inbox/live/z-1.json");
    fx.write(".everett/inbox/live/z-1.json", r#"{"pid":1,"ts":1}"#);
    // Age the file past LIVE_TTL via utimensat (no extra dev-deps).
    let c = std::ffi::CString::new(stale.to_string_lossy().as_bytes()).unwrap();
    let old = (now_ts() - 2.0 * 24.0 * 3600.0) as i64;
    let times = [
        libc::timespec { tv_sec: old, tv_nsec: 0 },
        libc::timespec { tv_sec: old, tv_nsec: 0 },
    ];
    assert_eq!(unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) }, 0);

    let prompt = json!({"session_id": "x-1", "hook_event_name": "PostToolUse"}).to_string();
    let out = fx.run_stdin(&["hook", "claude_inbox"], &prompt, &[]);
    let text = fx.stdout(&out);
    assert!(text.contains("hello live"), "{text}");
    assert!(fx.home().join(".everett/inbox/x-1.lock").exists());

    // The same hook again must not re-inject the same message.
    let out = fx.run_stdin(&["hook", "claude_inbox"], &prompt, &[]);
    assert_eq!(fx.stdout(&out), "", "message delivered twice");

    // Compaction: expired records are dropped; delivered records stay so
    // reply_to lookups still resolve; .done is pruned to ids still on disk;
    // the stale live record is reaped.
    let inbox = std::fs::read_to_string(fx.home().join(".everett/inbox/x-1.jsonl")).unwrap();
    assert!(inbox.contains("m-live") && !inbox.contains("m-old"), "{inbox}");
    let done = std::fs::read_to_string(fx.home().join(".everett/inbox/x-1.done")).unwrap();
    assert!(!done.contains("m-gone"), "{done}");
    assert!(!stale.exists(), "stale live record survived");

    // A delivered message is still replyable after compaction.
    let out = fx.run(&["reply", "m-live", "still here"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn subscribe_writes_under_lock() {
    let fx = fixture();
    let out = fx.run(&["subscribe", "atlas", "--as", "s-1"]);
    assert_eq!(out.status.code(), Some(0), "{}", fx.stdout(&out));
    assert!(fx.home().join(".everett/subscriptions.json.lock").exists());
    let subs: Value =
        serde_json::from_str(&std::fs::read_to_string(fx.home().join(".everett/subscriptions.json")).unwrap())
            .unwrap();
    assert_eq!(subs["s-1"], json!(["project:atlas"]));
}

#[test]
fn omp_extension_bakes_absolute_binary() {
    let fx = fixture();
    let out = fx.run(&["install-hooks", "--omp", "--apply"]);
    assert_eq!(out.status.code(), Some(0), "{}", fx.stdout(&out));
    let ext = std::fs::read_to_string(fx.home().join(".omp/agent/extensions/everett.ts")).unwrap();
    assert!(ext.contains("EVERETT_BIN"), "{ext}");
    assert!(ext.contains('/'), "{ext}");
    assert!(ext.contains("timeout: 3000"), "{ext}"); // cardContext got a timeout too
    assert!(!ext.contains("__EVERETT_BIN__"), "{ext}");
}

#[test]
fn mcp_tool_panic_returns_internal_error_and_server_lives() {
    let fx = fixture();
    let calls = [
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
               "params":{"name":"everett_ls","arguments":{}}}).to_string(),
        json!({"jsonrpc":"2.0","id":2,"method":"ping"}).to_string(),
    ]
    .join("\n");
    let out = fx.run_stdin(&["mcp"], &calls, &[("EVERETT_MCP_PANIC_TEST", "1")]);
    let text = fx.stdout(&out);
    assert!(text.contains("-32603"), "{text}");
    assert!(text.contains(r#""id":2"#), "server died after the panic: {text}");
}

#[test]
fn empty_everett_send_value_does_not_skip_delivery() {
    let fx = fixture();
    post_msg(&fx, "x-1", "m-1", "queued hello", now_ts());
    let prompt = json!({"session_id": "x-1", "hook_event_name": "PostToolUse"}).to_string();
    let out = fx.run_stdin(&["hook", "claude_inbox"], &prompt, &[("EVERETT_SEND", "")]);
    assert!(fx.stdout(&out).contains("queued hello"), "empty EVERETT_SEND skipped delivery");
}

#[test]
fn hermes_db_without_messages_table_still_lists() {
    let fx = fixture();
    let db = fx.home().join(".hermes/state.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let con = rusqlite::Connection::open(&db).unwrap();
    con.execute_batch(
        "CREATE TABLE sessions (id TEXT, source TEXT, started_at REAL, cwd TEXT, title TEXT,
            last_activity_at REAL);",
    )
    .unwrap();
    con.execute(
        "INSERT INTO sessions VALUES ('h-9','cli',?1,'/work/nomsg','No messages',?1)",
        [now_ts() - 60.0],
    )
    .unwrap();
    drop(con);
    let list = fx.ls_json();
    let s = list.iter().find(|x| x["id"] == "h-9").expect("hermes session missing");
    assert_eq!(s["harness"], "hermes");
    assert_eq!(s["cwd"], "/work/nomsg");
}

#[test]
fn codex_missing_id_uses_stem_uuid_or_skips() {
    let fx = fixture();
    let uuid = "123e4567-e89b-42d3-a456-426614174000";
    // No id/session_id in session_meta; the uuid is only in the filename.
    fx.write(
        ".codex/sessions/2026/09/20/rollout-2026-09-20T10-00-00-123e4567-e89b-42d3-a456-426614174000.jsonl",
        &jsonl(&[json!({
            "type": "session_meta",
            "payload": {"cwd": "/work/stem", "timestamp": "2026-09-20T10:00:00Z"}
        }), json!({
            "type": "response_item",
            "payload": {"type": "message", "role": "user",
                        "content": [{"text": "stem uuid session"}]}
        })]),
    );
    fx.write(
        ".codex/sessions/2026/09/20/rollout-broken.jsonl",
        &jsonl(&[json!({
            "type": "session_meta",
            "payload": {"cwd": "/work/nouuid", "timestamp": "2026-09-20T10:00:00Z"}
        })]),
    );
    let list = fx.ls_json();
    assert!(list.iter().any(|x| x["id"] == uuid), "{list:?}");
    assert!(!list.iter().any(|x| x["path"].as_str().unwrap_or("").contains("rollout-broken")), "{list:?}");
}

#[test]
fn pi_resume_refuses_without_session_path() {
    // command_for for a pi session with no file errors instead of emitting `pi --session ""`.
    let s = everett::session::Session::new("pi", "p-1", "/w", "", "", now_ts());
    let err = everett::send::command_for(&s, "hello").unwrap_err();
    assert!(format!("{err}").contains("session file"), "{err}");
}
