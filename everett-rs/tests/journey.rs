//! End-to-end journey over a temp HOME: send/route/inbox/core coverage ported
//! from the retired Python suite (release_journey.rs is the packaged-binary twin).

mod common;

use common::{fixture, plant};
use serde_json::{json, Value};

fn parse(out: &std::process::Output) -> Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not json: {}\n{}", e, stdout))
}

fn fixture_with_session() -> common::Fixture {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    fx
}

#[test]
fn ls_empty_home() {
    let fx = fixture();
    let out = fx.run(&["ls"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("no sessions"));
}

#[test]
fn route_local_picks_matching_session() {
    let fx = fixture_with_session();
    let out = fx.run(&["route", "add tests to the login bug fix", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let r = parse(&out);
    assert_eq!(r["decision"], "SESSION", "{r}");
    assert_eq!(r["session"]["id"], "c-1");
    assert!(r["command"].as_str().unwrap().contains("claude --resume c-1"));
    assert_eq!(r["router"], "local");
}

#[test]
fn route_jev_without_key_is_code_3() {
    let fx = fixture_with_session();
    let out = fx.run(&["route", "x", "--router", "jev"]);
    assert_eq!(out.status.code(), Some(3), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn send_to_dry_run_prints_argv_not_shell() {
    let fx = fixture_with_session();
    let out = fx.run(&["send", "a request", "--to", "c-1", "--dry-run", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let r = parse(&out);
    assert_eq!(r["delivered"], false);
    assert_eq!(
        r["command"],
        json!(["claude", "--resume", "c-1", "--print", "a request"])
    );
}

#[test]
fn send_new_route_delivers_nothing() {
    let fx = fixture();
    let out = fx.run(&["send", "a brand new request", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let r = parse(&out);
    assert_eq!(r["delivered"], false);
    assert_eq!(r["decision"], "NEW");
}

#[test]
fn learn_then_core_shows_fact() {
    let fx = fixture();
    let out = fx.run(&["learn", "the ports are merged in tree mode"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("learned (global)"), "{stdout}");
    // Unmerged learnings sit in the inbox; `core` reports them pending.
    let core = fx.run(&["core"]);
    let core_out = String::from_utf8_lossy(&core.stdout);
    assert!(core_out.contains("1 learning(s) waiting"), "{core_out}");
    let inbox = std::fs::read_to_string(fx.home().join(".everett/core/inbox.jsonl")).unwrap();
    assert!(inbox.contains("ports are merged"));
}

#[test]
fn learn_rejects_secret() {
    let fx = fixture();
    let out = fx.run(&["learn", "api_key sk-1234567890abcdefghijklmnop"]);
    assert_eq!(out.status.code(), Some(2));
    let core = fx.run(&["core"]);
    assert!(!String::from_utf8_lossy(&core.stdout).contains("sk-1234567890"));
}

#[test]
fn event_reaches_subscriber_inbox() {
    let fx = fixture();
    let out = fx.run(&["subscribe", "*", "--as", "watcher1"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let out = fx.run(&["event", "done", "finished the rewrite", "--session", "sess-a"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let out = fx.run(&["inbox", "--session", "watcher1", "--json"]);
    let r = parse(&out);
    let messages = r["messages"].as_array().unwrap();
    assert!(messages.iter().any(|m| m["text"].as_str().unwrap_or("").contains("finished the rewrite")
        && m["kind"] == "event"), "{r}");
}

#[test]
fn inbox_empty_for_human() {
    let fx = fixture();
    let out = fx.run(&["inbox"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("empty"));
}

#[test]
fn cards_reports_coverage() {
    let fx = fixture_with_session();
    let out = fx.run(&["cards"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Sessions (last 72 hours): 1"), "{stdout}");
    assert!(stdout.contains("claude: 1 sessions"), "{stdout}");
}

#[test]
fn trunk_view_dry_run_lists_sessions() {
    let fx = fixture_with_session();
    let out = fx.run(&["trunk", "--dry-run"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("c-1") || stdout.contains("Login bug fix"), "{stdout}");
}

#[test]
fn hook_claude_session_start_writes_card_context() {
    let fx = fixture();
    let out = fx.run_stdin(&["hook", "claude_session_start"], "{\"session_id\": \"abc\"}", &[]);
    assert_eq!(out.status.code(), Some(0));
    let r = parse(&out);
    let ctx = r["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert!(ctx.contains(&format!("{}/.everett/cards/abc.md", fx.home().display())), "{ctx}");
}

#[test]
fn hook_is_silent_for_headless_send() {
    let fx = fixture();
    let out = fx.run_stdin(
        &["hook", "claude_session_start"],
        "{\"session_id\": \"abc\"}",
        &[("EVERETT_SEND", "1")],
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

#[test]
fn hook_malformed_input_is_silent() {
    let fx = fixture();
    let out = fx.run_stdin(&["hook", "claude_session_start"], "not json", &[]);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

#[test]
fn mcp_stdio_roundtrip() {
    let fx = fixture();
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"clientInfo\":{\"name\":\"t\",\"version\":\"1\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"everett_ls\",\"arguments\":{\"hours\":72}}}\n",
    );
    let out = fx.run_stdin(&["mcp"], input, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let replies: Vec<Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let init = replies.iter().find(|r| r["id"] == 1).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "everett");
    assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], false);
    let list = replies.iter().find(|r| r["id"] == 2).unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 10, "{names:?}");
    for want in ["everett_ls", "everett_send", "everett_route", "everett_inbox", "everett_learn"] {
        assert!(names.contains(&want), "{names:?}");
    }
    let call = replies.iter().find(|r| r["id"] == 3).unwrap();
    assert_eq!(call["result"]["isError"], false);
    assert_eq!(call["result"]["structuredContent"]["sessions"], json!([]));
}

#[test]
fn install_hooks_apply_is_idempotent() {
    let fx = fixture();
    let out = fx.run(&["install-hooks", "--claude", "--apply"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let settings = fx.home().join(".claude/settings.json");
    assert!(settings.is_file());
    let first = std::fs::read_to_string(&settings).unwrap();
    let out = fx.run(&["install-hooks", "--claude", "--apply"]);
    assert_eq!(out.status.code(), Some(0));
    let second = std::fs::read_to_string(&settings).unwrap();
    let data: Value = serde_json::from_str(&second).unwrap();
    for event in ["SessionStart", "Stop", "UserPromptSubmit", "PostToolUse"] {
        let count = data["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|e| e["hooks"].as_array().unwrap())
            .filter(|h| h["command"].as_str().unwrap_or("").contains("everett hook"))
            .count();
        assert_eq!(count, 1, "{event} duplicated:\n{first}\n{second}");
    }
    // The untouched parts of the file survive a re-apply.
    assert!(second.contains("SessionStart"));
}

#[test]
fn install_mcp_apply_writes_registration() {
    let fx = fixture();
    let out = fx.run(&["install-mcp", "--claude", "--apply"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let data: Value = serde_json::from_str(
        &std::fs::read_to_string(fx.home().join(".claude.json")).unwrap(),
    )
    .unwrap();
    let entry = &data["mcpServers"]["everett"];
    assert_eq!(entry["args"], json!(["mcp"]));
    assert!(entry["command"].as_str().unwrap().ends_with("everett"));
}

#[test]
fn doctor_runs_on_empty_home() {
    let fx = fixture();
    let out = fx.run(&["doctor"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[ok  ] everett"), "{stdout}");
}

#[test]
fn events_json_lists_recorded_event() {
    let fx = fixture();
    fx.run(&["event", "info", "checkpoint", "--session", "sess-a"]);
    let out = fx.run(&["events", "--json"]);
    let events = parse(&out);
    assert!(events.as_array().unwrap().iter().any(|e| e["text"] == "checkpoint"));
}

#[cfg(unix)]
struct TuiProc {
    session: expectrl::Session,
    vt: vt100::Parser,
}

#[cfg(unix)]
impl TuiProc {
    fn drain(&mut self) {
        let mut buf = [0u8; 8192];
        while let Ok(n) = self.session.try_read(&mut buf) {
            if n == 0 {
                break;
            }
            self.vt.process(&buf[..n]);
        }
    }

    fn screen(&mut self) -> String {
        self.drain();
        self.vt.screen().contents()
    }

    fn see(&mut self, needle: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let screen = self.screen();
            if screen.contains(needle) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "never saw {needle:?} on screen:\n{screen}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    fn send(&mut self, keys: &str) {
        self.session.send(keys).unwrap();
    }
}

#[cfg(unix)]
fn spawn_tui(fx: &common::Fixture) -> TuiProc {
    let home = fx.home().to_path_buf();
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_everett"));
    cmd.arg("onboard")
        .env("HOME", &home)
        .env("EVERETT_HOME", &home)
        .env("EVERETT_NO_ANIM", "1")
        .env("TERM", "xterm");
    let mut session = expectrl::Session::spawn(cmd).unwrap();
    session.get_process_mut().set_window_size(100, 30).unwrap();
    TuiProc {
        session,
        vt: vt100::Parser::new(30, 100, 0),
    }
}

#[cfg(unix)]
#[test]
fn onboard_tui_quit_makes_no_changes() {
    let fx = fixture();
    let mut p = spawn_tui(&fx);
    p.see("Welcome to Everett");
    p.send("q");
    p.see("Cancelled -- no changes were made");
    assert!(!fx.home().join(".claude.json").exists());
}

#[cfg(unix)]
#[test]
fn onboard_tui_full_wizard_applies() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    let mut p = spawn_tui(&fx);
    p.see("Welcome to Everett");
    for screen in [
        "What Everett found",
        "Hooks",
        "MCP server",
        "Smarter routing",
        "Backfill",
        "Apply these changes",
    ] {
        p.send("\r");
        p.see(screen);
    }
    p.send("a");
    p.see("onboarding complete");
    p.see("Try this");
    p.send("\r");
    for _ in 0..100 {
        p.drain();
        if !p.session.is_alive().unwrap() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(!p.session.is_alive().unwrap(), "wizard did not exit");
    let settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fx.home().join(".claude/settings.json")).unwrap())
            .unwrap();
    assert!(settings["hooks"]["SessionStart"].is_array());
    let claude_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fx.home().join(".claude.json")).unwrap())
            .unwrap();
    assert!(claude_json["mcpServers"]["everett"].is_object());
}
