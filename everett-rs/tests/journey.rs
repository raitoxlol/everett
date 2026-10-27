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
fn ls_names_the_project_behind_a_worktree() {
    let fx = fixture();
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude.jsonl"),
    )
    .unwrap();
    fx.write(
        ".claude/projects/app/c-1.jsonl",
        &src.replace("/work/app", "/work/.t3/worktrees/everett/t3-1"),
    );
    let out = fx.run(&["ls"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("everett"), "{text}");
    assert!(!text.contains("t3-1"), "{text}");
}

#[test]
fn route_local_picks_matching_session() {
    let fx = fixture_with_session();
    let out = fx.run(&["route", "add tests to the login bug fix", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let r = parse(&out);
    assert_eq!(r["decision"], "SESSION", "{r}");
    assert_eq!(r["session"]["id"], "c-1");
    assert!(r["command"]
        .as_str()
        .unwrap()
        .contains("claude --resume c-1"));
    assert_eq!(r["router"], "local");
}

#[test]
fn route_jev_without_key_is_code_3() {
    let fx = fixture_with_session();
    let out = fx.run(&["route", "x", "--router", "jev"]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = fx.run(&[
        "event",
        "done",
        "finished the rewrite",
        "--session",
        "sess-a",
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = fx.run(&["inbox", "--session", "watcher1", "--json"]);
    let r = parse(&out);
    let messages = r["messages"].as_array().unwrap();
    assert!(
        messages.iter().any(|m| m["text"]
            .as_str()
            .unwrap_or("")
            .contains("finished the rewrite")
            && m["kind"] == "event"),
        "{r}"
    );
}

#[test]
fn inbox_empty_for_human() {
    let fx = fixture();
    let out = fx.run(&["inbox"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("empty"));
}

#[test]
fn inbox_message_acks_only_that_message() {
    let fx = fixture_with_session();
    let mut ids = Vec::new();
    for text in ["first task", "second task", "third task"] {
        let out = fx.run(&["send", text, "--to", "c-1", "--mode", "inbox", "--json"]);
        assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        ids.push(parse(&out)["message_id"].as_str().unwrap().to_string());
    }
    let out = fx.run(&["inbox", "--session", "c-1", "--message", &ids[1]]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("second task"), "{stdout}");
    assert!(!stdout.contains("first task") && !stdout.contains("third task"), "{stdout}");
    let pending = |fx: &common::Fixture| -> Vec<String> {
        parse(&fx.run(&["inbox", "--session", "c-1", "--peek", "--json"]))["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["id"].as_str().map(String::from))
            .collect()
    };
    assert_eq!(pending(&fx), vec![ids[0].clone(), ids[2].clone()]);
    let out = fx.run(&["inbox", "--session", "c-1", "--message", "m-unknown"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(pending(&fx), vec![ids[0].clone(), ids[2].clone()]);
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
    assert!(
        stdout.contains("c-1") || stdout.contains("Login bug fix"),
        "{stdout}"
    );
}

#[test]
fn hook_claude_session_start_writes_card_context() {
    let fx = fixture();
    let out = fx.run_stdin(
        &["hook", "claude_session_start"],
        "{\"session_id\": \"abc\"}",
        &[],
    );
    assert_eq!(out.status.code(), Some(0));
    let r = parse(&out);
    let ctx = r["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        ctx.contains(&format!("{}/.everett/cards/abc.md", fx.home().display())),
        "{ctx}"
    );
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
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let replies: Vec<Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let init = replies.iter().find(|r| r["id"] == 1).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "everett");
    assert_eq!(
        init["result"]["capabilities"]["tools"]["listChanged"],
        false
    );
    let list = replies.iter().find(|r| r["id"] == 2).unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 13, "{names:?}");
    for want in [
        "everett_ls",
        "everett_send",
        "everett_route",
        "everett_inbox",
        "everett_learn",
    ] {
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
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let data: Value =
        serde_json::from_str(&std::fs::read_to_string(fx.home().join(".claude.json")).unwrap())
            .unwrap();
    let entry = &data["mcpServers"]["everett"];
    assert_eq!(entry["args"], json!(["mcp"]));
    assert!(entry["command"].as_str().unwrap().ends_with("everett"));
}

#[test]
fn doctor_runs_on_empty_home() {
    let fx = fixture();
    let out = fx.run(&["doctor"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[ok  ] everett"), "{stdout}");
}

#[test]
fn events_json_lists_recorded_event() {
    let fx = fixture();
    fx.run(&["event", "info", "checkpoint", "--session", "sess-a"]);
    let out = fx.run(&["events", "--json"]);
    let events = parse(&out);
    assert!(events
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["text"] == "checkpoint"));
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

    fn review(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut last = String::new();
        loop {
            let screen = self.screen();
            if screen.contains("Review your setup") {
                return;
            }
            if screen.contains("/ setup guide") {
                let title = screen
                    .lines()
                    .find(|line| line.contains("/ setup guide"))
                    .unwrap()
                    .to_string();
                if title != last {
                    self.send("\r");
                    last = title;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "never reached review:\n{screen}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    fn send(&mut self, keys: &str) {
        self.session.send(keys).unwrap();
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.session
            .get_process_mut()
            .set_window_size(cols, rows)
            .unwrap();
        self.vt.set_size(rows, cols);
    }

    fn exited(&mut self) {
        for _ in 0..100 {
            self.drain();
            if self
                .vt
                .screen()
                .contents()
                .contains("Teach your agents about Everett")
            {
                self.send("\r");
            }
            if !self.session.is_alive().unwrap() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("wizard did not exit");
    }

    fn frame(&mut self) -> Value {
        self.drain();
        let screen = self.vt.screen();
        let (rows, cols) = screen.size();
        let color = |color| match color {
            vt100::Color::Default => Value::Null,
            vt100::Color::Idx(index) => json!(index),
            vt100::Color::Rgb(r, g, b) => json!([r, g, b]),
        };
        let mut cells = Vec::new();
        for row in 0..rows {
            for col in 0..cols {
                let cell = screen.cell(row, col).unwrap();
                if cell.contents().is_empty() || cell.is_wide_continuation() {
                    continue;
                }
                cells.push(json!([
                    col,
                    row,
                    cell.contents(),
                    color(cell.fgcolor()),
                    color(cell.bgcolor()),
                    cell.bold(),
                    cell.inverse()
                ]));
            }
        }
        json!({"rows": rows, "cols": cols, "cells": cells})
    }
}

#[cfg(unix)]
fn spawn_tui(fx: &common::Fixture) -> TuiProc {
    spawn_tui_with(fx, &[], 80, 24)
}

#[cfg(unix)]
fn spawn_tui_with(fx: &common::Fixture, args: &[&str], cols: u16, rows: u16) -> TuiProc {
    let mut cmd = tui_command(fx, args);
    cmd.env("EVERETT_NO_ANIM", "1");
    spawn_tui_command(cmd, cols, rows)
}

#[cfg(unix)]
fn tui_command(fx: &common::Fixture, args: &[&str]) -> std::process::Command {
    let home = fx.home().to_path_buf();
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_everett"));
    cmd.arg("onboard")
        .args(args)
        .env_clear()
        .env("HOME", &home)
        .env("EVERETT_HOME", &home)
        .env("EVERETT_AUTH_PROVIDER", "oidc")
        .env("PATH", "/nonexistent")
        .env("LANG", "en_US.UTF-8")
        .env("TERM", "xterm");
    cmd
}

#[cfg(unix)]
fn spawn_tui_command(cmd: std::process::Command, cols: u16, rows: u16) -> TuiProc {
    let mut session = expectrl::Session::spawn(cmd).unwrap();
    session
        .get_process_mut()
        .set_window_size(cols, rows)
        .unwrap();
    TuiProc {
        session,
        vt: vt100::Parser::new(rows, cols, 0),
    }
}

#[cfg(unix)]
#[test]
fn onboard_tui_quit_makes_no_changes() {
    let fx = fixture();
    let mut p = spawn_tui(&fx);
    p.see("Welcome to Everett");
    p.send("q");
    p.see("Setup stopped. Applied changes, if any, are kept.");
    assert!(!fx.home().join(".claude.json").exists());
}

#[cfg(unix)]
#[test]
fn onboard_tui_full_wizard_applies() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    fx.write("AGENTS.md", "Existing user rules.\n");
    let mut p = spawn_tui(&fx);
    p.see("Welcome to Everett");
    for screen in [
        "Sign in to Everett",
        "Choose your agents",
        "Review your setup",
    ] {
        p.send("\r");
        if screen == "Review your setup" {
            p.review();
        } else {
            p.see(screen);
        }
    }
    p.send("a");
    p.see("Local setup applied");
    p.see("Agent tools are not verified yet.");
    p.see("Restart Claude Code.");
    if let Ok(path) = std::env::var("EVERETT_SUMMARY_FRAME") {
        std::fs::write(path, serde_json::to_vec(&p.frame()).unwrap()).unwrap();
    }
    p.send("\r");
    p.see("Teach your agents about Everett");
    p.see("Prompt / review before applying");
    p.send("\r");
    p.see("Copy into your selected agents:");
    assert_eq!(
        std::fs::read_to_string(fx.home().join("AGENTS.md")).unwrap(),
        "Existing user rules.\n"
    );
    for _ in 0..100 {
        p.drain();
        if !p.session.is_alive().unwrap() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(!p.session.is_alive().unwrap(), "wizard did not exit");
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fx.home().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    assert!(settings["hooks"]["SessionStart"].is_array());
    let claude_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fx.home().join(".claude.json")).unwrap())
            .unwrap();
    assert!(claude_json["mcpServers"]["everett"].is_object());
}

#[cfg(unix)]
#[test]
fn onboard_tui_agents_empty_and_discovery_only() {
    let fx = fixture();
    let mut p = spawn_tui(&fx);
    p.see("Welcome to Everett");
    p.send("\r");
    p.see("Sign in to Everett");
    p.send("\r");
    p.see("No supported agents found");
    p.see("everett onboard again.");
    p.send("q");
    p.see("Setup stopped. Applied changes, if any, are kept.");

    std::fs::create_dir_all(fx.home().join(".hermes")).unwrap();
    let mut p = spawn_tui_with(&fx, &[], 54, 14);
    p.see("Welcome to Everett");
    p.see("Enter ↵   Sign in and get started");
    p.send("\r");
    p.see("Sign in to Everett");
    p.send("\r");
    p.see("discovery only");
    p.see("Enter ↵   Continue without integrations");
    p.send(" d");
    p.see("No automatic changes enabled for this agent.");
    p.send("bq");
    p.see("Setup stopped. Applied changes, if any, are kept.");
    assert!(!fx.home().join(".everett/onboarding-complete").exists());
}

#[cfg(unix)]
#[test]
fn onboard_tui_custom_agent_selection_survives_resize_and_back() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    plant(
        &fx,
        "codex.jsonl",
        ".codex/sessions/2026/09/20/rollout-x-1.jsonl",
    );
    let mut p = spawn_tui(&fx);
    p.see("Welcome to Everett");
    p.send("\r");
    p.see("Sign in to Everett");
    p.send("\r");
    p.see("Choose your agents");
    p.see("[x] Claude Code");
    p.see("[x] Codex");
    println!("80x24 agent selection:\n{}", p.screen());
    p.send("d");
    p.see("Claude Code / setup details");
    p.see(".claude/settings");
    p.send("k");
    p.see(".claude.json");
    p.send(" \r");
    p.see("[-] Claude Code");
    p.see("Custom selection.");
    p.resize(54, 14);
    p.see("Enter ↵   Continue with 2 agents");
    p.send("j");
    p.see("Selected. Changes are reviewed before applying.");
    println!("54x14 agent selection:\n{}", p.screen());
    p.send("d");
    p.see("Codex / setup details");
    p.send("k");
    p.see("config.toml");
    p.resize(80, 24);
    p.see("File changed on apply:");
    p.send("\r\r");
    p.review();
    p.send("b");
    p.see("Codex / setup guide");
    p.send("b");
    p.see("Claude Code / setup guide");
    p.send("b");
    p.see("[-] Claude Code");
    p.send("q");
    p.see("Setup stopped. Applied changes, if any, are kept.");
    assert!(!fx.home().join(".claude/settings.json").exists());
    assert!(!fx.home().join(".claude.json").exists());
    assert!(!fx.home().join(".codex/config.toml").exists());
}

#[cfg(unix)]
#[test]
fn onboard_tui_deselected_agent_preserves_existing_settings() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    plant(
        &fx,
        "codex.jsonl",
        ".codex/sessions/2026/09/20/rollout-x-1.jsonl",
    );
    let settings = "{\"permissions\":{\"allow\":[\"Read\"]}}\n";
    let mcp = "{\"mcpServers\":{\"other\":{\"command\":\"other\"}}}\n";
    fx.write(".claude/settings.json", settings);
    fx.write(".claude.json", mcp);
    let mut p = spawn_tui_with(&fx, &["--no-backfill"], 80, 24);
    p.see("Welcome to Everett");
    p.send("\r");
    p.see("Sign in to Everett");
    p.send("\r");
    p.see("Choose your agents");
    p.send(" ");
    p.see("[ ] Claude Code");
    p.see("Skipped. Existing settings will be left as they are.");
    p.send("\r");
    p.review();
    p.send("a");
    p.see("Local setup applied");
    p.send("\r");
    p.exited();
    assert_eq!(
        std::fs::read_to_string(fx.home().join(".claude/settings.json")).unwrap(),
        settings
    );
    assert_eq!(
        std::fs::read_to_string(fx.home().join(".claude.json")).unwrap(),
        mcp
    );
    assert!(
        std::fs::read_to_string(fx.home().join(".codex/config.toml"))
            .unwrap()
            .contains("mcp_servers.everett")
    );
}

#[cfg(unix)]
#[test]
fn onboard_tui_agent_toggle_respects_no_mcp() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    let mut p = spawn_tui_with(&fx, &["--no-mcp", "--no-backfill"], 80, 24);
    p.see("Welcome to Everett");
    p.send("\r");
    p.see("Sign in to Everett");
    p.send("\r");
    p.see("Choose your agents");
    p.send("  ");
    p.see("[x] Claude Code");
    p.send("\r");
    p.review();
    p.send("a");
    p.see("Local setup applied");
    p.send("\r");
    p.exited();
    assert!(!fx.home().join(".claude.json").exists());
    let settings: Value = serde_json::from_str(
        &std::fs::read_to_string(fx.home().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    assert!(settings["hooks"]["SessionStart"].is_array());
}

#[cfg(unix)]
#[test]
fn onboard_tui_animation_and_multiple_colors_reach_the_terminal() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    plant(
        &fx,
        "codex.jsonl",
        ".codex/sessions/2026/09/20/rollout-x-1.jsonl",
    );
    std::fs::create_dir_all(fx.home().join(".hermes")).unwrap();
    let mut p = spawn_tui_command(tui_command(&fx, &["--no-backfill"]), 80, 24);
    p.see("Welcome to Everett");
    p.see("Enter ↵   Sign in and get started");
    let first = p.screen();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_ne!(first, p.screen(), "welcome animation did not reach the PTY");
    let mut frames = Vec::new();
    let mut record = |p: &mut TuiProc, count| {
        for _ in 0..count {
            frames.push(p.frame());
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    record(&mut p, 45);
    let pressed = std::time::Instant::now();
    p.send("\r");
    p.see("Sign in to Everett");
    assert!(
        pressed.elapsed() < std::time::Duration::from_secs(2),
        "animation delayed input"
    );
    record(&mut p, 14);
    p.send("\r");
    p.see("Choose your agents");
    p.see("Enter ↵   Continue with 2 agents");
    record(&mut p, 12);
    p.send("j");
    record(&mut p, 10);
    p.send(" ");
    p.see("Skipped. Existing settings will be left as they are.");
    p.see("Enter ↵   Continue with 1 agent");
    record(&mut p, 10);
    p.send("j");
    p.see("DISCOVERY ONLY");
    record(&mut p, 10);
    p.send("2");
    p.see("OpenAI Dots / setup needed");
    record(&mut p, 10);
    p.send(" ");
    p.see("[x] OpenAI Dots");
    record(&mut p, 10);
    p.send("3 ");
    p.see("[x] Muse");
    record(&mut p, 10);
    p.send("\r");
    p.see("Claude Code / setup guide");
    record(&mut p, 10);
    p.send("\r");
    p.see("OpenAI Dots / setup needed");
    record(&mut p, 10);
    p.send("\r");
    p.see("Muse / unverified");
    record(&mut p, 10);
    p.send("\r");
    p.review();
    p.see("OpenAI Dots / pending, not added");
    record(&mut p, 10);
    let emitted: std::collections::HashSet<_> = frames
        .iter()
        .flat_map(|frame| {
            frame["cells"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|cell| cell[3].as_u64())
        })
        .collect();
    for index in [81, 114, 141, 209, 221] {
        assert!(
            emitted.contains(&index),
            "terminal never received palette color {index}: {emitted:?}"
        );
    }
    if let Ok(path) = std::env::var("EVERETT_TUI_FRAMES") {
        std::fs::write(
            path,
            serde_json::to_vec(&json!({"frame_ms": 50, "frames": frames})).unwrap(),
        )
        .unwrap();
    }
    p.send("q");
    p.see("Setup stopped. Applied changes, if any, are kept.");
    assert!(!fx.home().join(".everett/onboarding-complete").exists());
}

#[cfg(unix)]
#[test]
fn onboard_tui_motion_and_color_can_be_disabled_independently() {
    let fx = fixture();
    let mut p = spawn_tui(&fx);
    p.see("Welcome to Everett");
    p.see("q quit");
    let first = p.frame();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        first == p.frame(),
        "EVERETT_NO_ANIM changed a complete terminal frame"
    );
    assert!(first["cells"]
        .as_array()
        .unwrap()
        .iter()
        .any(|cell| cell[3].is_number()));
    p.send("q");
    p.see("Setup stopped. Applied changes, if any, are kept.");

    let mut cmd = tui_command(&fx, &[]);
    cmd.env("NO_COLOR", "1");
    let mut p = spawn_tui_command(cmd, 80, 24);
    p.see("Welcome to Everett");
    p.see("q quit");
    let first = p.frame();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let second = p.frame();
    assert_ne!(first, second, "NO_COLOR unexpectedly stopped motion");
    for frame in [first, second] {
        assert!(frame["cells"]
            .as_array()
            .unwrap()
            .iter()
            .all(|cell| cell[3].is_null() && cell[4].is_null()));
    }
    p.send("\x03");
    p.see("Setup stopped. Applied changes, if any, are kept.");
}

#[cfg(unix)]
#[test]
fn onboard_tui_optional_explanation_and_account_back_navigation() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    let mut p = spawn_tui(&fx);
    p.see("Enter ↵   Sign in and get started");
    p.send("d");
    p.see("What can Everett do?");
    p.see("Hand off work");
    p.see("Share context");
    p.see("Get a reply");
    p.send("2");
    p.see("Claude Code / Codex / OMP / Pi / Hermes");
    p.see("No Everett account is needed for local setup.");
    p.send("3");
    p.see("OpenAI Dots / Grok Bot");
    p.see("Hark: connection support needs verification.");
    p.see("Queued work is not a confirmed reply.");
    p.send("4");
    p.see("Muse integration has not been verified.");
    p.resize(54, 14);
    p.see("1 Overview");
    p.see("Enter ↵   Check this computer");
    p.send("jjjjjjjjjjjjjjj");
    p.see("Does the connection survive a VM restart?");
    p.send("\r");
    p.see("Welcome to Everett");
    p.send("\r");
    p.see("Sign in to Everett");
    p.see("Account sign-in is not available");
    p.send("l");
    p.see("Choose your agents");
    p.send("b");
    p.see("Sign in to Everett");
    p.send("b");
    p.see("Welcome to Everett");
    p.send("\r3q");
    p.see("Setup stopped. Applied changes, if any, are kept.");
    assert!(!fx.home().join(".claude/settings.json").exists());
    assert!(!fx.home().join(".claude.json").exists());
    assert!(!fx.home().join(".everett/onboarding-complete").exists());
}

#[cfg(unix)]
#[test]
fn onboard_tui_cloud_vm_choices_remain_pending_and_cancel_without_writes() {
    let fx = fixture();
    let mut p = spawn_tui(&fx);
    p.see("Welcome to Everett");
    p.send("\r\r2");
    p.see("OpenAI Dots / setup needed");
    p.see("Hark / unverified");
    p.send(" ");
    p.see("[x] OpenAI Dots");
    p.send("3 ");
    p.see("[x] Muse / unverified");
    p.resize(54, 14);
    p.see("Review connection setup");
    p.send("\r");
    p.see("OpenAI Dots / setup needed");
    p.send("\x1b[F");
    p.see("It stays pending; no client grant is added.");
    p.send("\r");
    p.see("Muse / unverified");
    p.send("\r");
    p.review();
    p.see("OpenAI Dots / pending, not added");
    p.see("Muse / pending, not added");
    p.send("bb");
    p.see("OpenAI Dots / setup needed");
    p.send("b");
    p.see("Choose your agents");
    p.send("2");
    p.see("[x] OpenAI Dots");
    p.send("3");
    p.see("[x] Muse");
    p.send("q");
    p.see("Setup stopped. Applied changes, if any, are kept.");
    assert!(!fx.home().join(".everett/auth.json").exists());
    assert!(!fx.home().join(".everett/external").exists());
    assert!(!fx.home().join(".everett/onboarding-complete").exists());
}

#[cfg(unix)]
#[test]
fn onboard_tui_review_hides_advanced_and_preserves_opt_in() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    let mut p = spawn_tui_with(&fx, &[], 54, 14);
    p.see("Welcome to Everett");
    p.send("\r\r\r");
    p.review();
    p.send("\x1b[F");
    p.see("Older cards: off / nightly merge: off");
    p.see("Apply local changes");
    p.send("v");
    p.see("Advanced / optional");
    p.send("2");
    p.see("Backfill cards / optional");
    p.see("☐ enabled");
    p.send(" \r\r");
    p.review();
    p.send("\x1b[F");
    p.see("Older cards: on / nightly merge: off");
    p.send("b\r");
    p.review();
    p.send("\x1b[F");
    p.see("Older cards: on / nightly merge: off");
    p.send("q");
    p.see("Setup stopped. Applied changes, if any, are kept.");
    assert!(!fx.home().join(".everett/cards").exists());
}

#[cfg(unix)]
#[test]
fn onboard_tui_apply_failure_has_no_completion_marker() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    let broken = "{invalid mcp config";
    fx.write(".claude.json", broken);
    let mut p = spawn_tui(&fx);
    p.see("Welcome to Everett");
    p.send("\r\r\r");
    p.review();
    p.send("a");
    p.see("Setup needs attention");
    p.see("Run: everett doctor");
    p.send("d");
    p.see("Needs attention:");
    p.see("claude tools:");
    p.send("b\r");
    p.exited();
    assert_eq!(
        std::fs::read_to_string(fx.home().join(".claude.json")).unwrap(),
        broken
    );
    assert!(!fx.home().join(".everett/onboarding-complete").exists());
    let out = fx.run(&["onboard", "--yes", "--no-backfill"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("setup needs attention"));
    assert!(!fx.home().join(".everett/onboarding-complete").exists());
}

#[cfg(unix)]
#[test]
fn onboard_tui_remote_only_apply_never_claims_a_connection() {
    let fx = fixture();
    let mut p = spawn_tui(&fx);
    p.see("Welcome to Everett");
    p.send("\r\r2 \r");
    p.see("OpenAI Dots / setup needed");
    p.send("\r");
    p.review();
    p.send("a");
    p.see("Remote setup pending");
    p.see("No cloud client or VM connection was added.");
    p.see("Press d for OpenAI Dots setup steps.");
    p.send("d\x1b[F");
    p.see("It stays pending; no client grant is added.");
    p.send("b\r");
    p.exited();
    assert!(!fx.home().join(".everett/auth.json").exists());
    assert!(!fx.home().join(".everett/external").exists());
}

#[cfg(unix)]
#[test]
fn onboard_tui_masked_key_accepts_q_and_escape_cancels() {
    let fx = fixture();
    let mut p = spawn_tui(&fx);
    p.see("Welcome to Everett");
    p.send("\r\r\rv1p");
    p.see("Paste the Jev key");
    p.send("qx");
    p.see("> **");
    assert!(!p.screen().contains("qx"));
    p.send("\x1b");
    p.see("Setup stopped. Applied changes, if any, are kept.");
    assert!(!fx.home().join(".everett/config.toml").exists());
    assert!(!fx.home().join(".everett/onboarding-complete").exists());
}

#[cfg(unix)]
#[test]
fn onboard_tui_too_small_terminal_cannot_apply_hidden_review() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    let mut p = spawn_tui(&fx);
    p.see("Welcome to Everett");
    p.send("\r\r\r");
    p.review();
    p.resize(40, 10);
    p.see("terminal too small");
    p.send("a\r");
    std::thread::sleep(std::time::Duration::from_millis(200));
    p.see("terminal too small");
    assert!(!fx.home().join(".claude/settings.json").exists());
    assert!(!fx.home().join(".claude.json").exists());
    p.resize(54, 14);
    p.review();
    p.send("q");
    p.see("Setup stopped. Applied changes, if any, are kept.");
    assert!(!fx.home().join(".everett/onboarding-complete").exists());
}
