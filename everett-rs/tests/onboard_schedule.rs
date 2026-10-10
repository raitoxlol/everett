mod common;

use common::{fixture, lib_home, LibHome};
use everett::onboard::{apply_jev, default_config, find_jev_key_with_source, generate_backfill_cards, missing_card_sessions, run_apply};
use everett::{cards, config, install, trunk_schedule};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

const BIN: &str = env!("CARGO_BIN_EXE_everett");

fn claude_session(home: &Path, sid: &str, cwd: &str, text: &str, days_old: f64) {
    let root = home.join(".claude/projects").join(cwd.trim_matches('/').replace('/', "-"));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join(format!("{sid}.jsonl"));
    let rows = [json!({"type": "user", "sessionId": sid, "cwd": cwd, "timestamp": "2026-09-20T00:00:00Z", "message": {"role": "user", "content": text}}),
        json!({"type": "assistant", "sessionId": sid, "message": {"role": "assistant", "content": [{"type": "text", "text": format!("Working on it. Next: ship {sid}.")}]}})];
    std::fs::write(&path, rows.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
    let when = SystemTime::now() - Duration::from_secs_f64(days_old * 86400.0);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// A fake `launchctl` on PATH that records its arguments, so nothing touches the real launchd.
fn fake_launchctl(h: &LibHome) -> PathBuf {
    let bin = h.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = h.path().join("launchctl.log");
    let script = bin.join("launchctl");
    std::fs::write(&script, format!("#!/bin/sh\necho \"$@\" >> '{}'\n", log.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    log
}

fn cli(h: &LibHome, args: &[&str]) -> Output {
    Command::new(BIN).args(args).env("HOME", h.path()).env("EVERETT_HOME", h.path()).env("EVERETT_NOTIFY", "none")
        .env("PATH", "/nonexistent").env_remove("TYPESAFE_API_KEY").output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn onboard_yes_end_to_end_flags_and_idempotence() {
    let h = lib_home();
    claude_session(&h.path(), "c-fresh", "/work/app", "fix the login bug", 0.0);
    assert_eq!(cli(&h, &["onboard", "--yes", "--no-backfill", "--no-mcp"]).status.code(), Some(0));
    assert!(!cards::card_path("c-fresh").exists());
    assert_eq!(install::mcp_status("claude").0, "missing");
    assert!(install::installed("claude", None).unwrap()["SessionStart"]);

    let out = cli(&h, &["onboard", "--yes"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(text.contains("Everett onboarding complete.") && text.contains("1 card(s) written"), "{text}");
    assert!(std::fs::read_to_string(cards::card_path("c-fresh")).unwrap().starts_with(cards::AUTO_MARKER));
    assert_eq!(install::mcp_status("claude").0, "ready");
    assert!(!config::config_path().exists() && !trunk_schedule::plist_path().exists());

    let text = stdout(&cli(&h, &["onboard", "--yes"]));
    assert!(text.contains("already installed") && text.contains("0 card(s) written"), "{text}");
    for days in ["0", "31"] {
        assert_eq!(cli(&h, &["onboard", "--yes", "--span-days", days]).status.code(), Some(2));
    }
}

#[test]
fn backfill_respects_span_agent_cards_and_progress() {
    let h = lib_home();
    claude_session(&h.path(), "c-inside", "/work/a", "inside span", 1.0);
    claude_session(&h.path(), "c-outside", "/work/b", "outside span", 10.0);
    claude_session(&h.path(), "c-agent", "/work/agent", "has a real card", 0.0);
    std::fs::create_dir_all(cards::card_dir()).unwrap();
    std::fs::write(cards::card_path("c-agent"), "Atlas: agent-written, do not touch.").unwrap();
    let mut ids: Vec<String> = missing_card_sessions(3).into_iter().map(|s| s.id).collect();
    ids.sort();
    assert_eq!(ids, ["c-inside"]);
    let seen = std::cell::RefCell::new(Vec::new());
    let progress = |i: usize, t: usize| seen.borrow_mut().push((i, t));
    assert_eq!(generate_backfill_cards(&missing_card_sessions(3), Some(&progress)), (1, 0));
    assert_eq!(*seen.borrow(), [(1, 1)]);
    assert!(missing_card_sessions(3).is_empty() && !cards::card_path("c-outside").exists());
    assert_eq!(std::fs::read_to_string(cards::card_path("c-agent")).unwrap(), "Atlas: agent-written, do not touch.");
    let mut cfg = default_config();
    cfg.hooks.clear();
    cfg.mcp.clear();
    cfg.span_days = 30;
    assert_eq!(run_apply(&cfg, None).backfill_written, 1);
    assert!(cards::card_path("c-outside").exists());
}

#[test]
fn config_set_value_is_private_top_level_and_idempotent() {
    let _h = lib_home();
    let path = config::set_value("typesafe_api_key", "sk-one").unwrap();
    assert_eq!(mode(&path), 0o600);
    config::set_value("typesafe_api_key", "sk-two").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("typesafe_api_key").count(), 1);
    assert_eq!(config::get("typesafe_api_key", None, ""), "sk-two");
    std::fs::write(&path, "vault = \"~/Notes\"\nrouter = \"local\"\n\n[jev]\napi_key = \"old\"\n").unwrap();
    config::set_value("typesafe_api_key", "sk-new").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    for part in ["vault = \"~/Notes\"", "router = \"local\"", "[jev]", "api_key = \"old\"", "typesafe_api_key = \"sk-new\""] {
        assert!(text.contains(part), "{text}");
    }
    assert!(text.find("typesafe_api_key").unwrap() < text.find("[jev]").unwrap());
}

#[test]
fn jev_key_sources_and_apply_report_never_echo_the_key() {
    let h = lib_home();
    assert_eq!(find_jev_key_with_source(), (String::new(), String::new()));
    std::fs::create_dir_all(h.path().join(".hermes")).unwrap();
    std::fs::write(h.path().join(".hermes/.env"), "OTHER=1\nTYPESAFE_API_KEY=from-hermes\n").unwrap();
    assert_eq!(find_jev_key_with_source(), ("from-hermes".into(), "hermes".into()));
    config::set_value("typesafe_api_key", "from-config").unwrap();
    assert_eq!(find_jev_key_with_source(), ("from-config".into(), "config".into()));
    std::env::set_var("TYPESAFE_API_KEY", "from-env");
    assert_eq!(find_jev_key_with_source(), ("from-env".into(), "env".into()));
    std::env::remove_var("TYPESAFE_API_KEY");
    std::fs::remove_file(config::config_path()).unwrap();

    let mut cfg = default_config();
    cfg.jev_choice = "skip".into();
    assert!(apply_jev(&cfg).contains("skipped") && !config::config_path().exists());
    cfg.jev_choice = "paste".into();
    cfg.jev_key = "sk-secret-value".into();
    cfg.jev_validated = Some(true);
    let line = apply_jev(&cfg);
    assert!(line.contains("validated") && !line.contains("sk-secret-value"), "{line}");
    assert_eq!(config::get("typesafe_api_key", None, ""), "sk-secret-value");
    assert_eq!(mode(&config::config_path()), 0o600);
    cfg.jev_validated = Some(false);
    assert!(apply_jev(&cfg).contains("validation failed"));
}

#[test]
fn trunk_schedule_plist_shape_and_time_validation() {
    let _h = lib_home();
    let data = trunk_schedule::plist_data("04:00", "claude").unwrap();
    assert_eq!(data["Label"], trunk_schedule::LABEL);
    assert_eq!(data["StartCalendarInterval"], json!({"Hour": 4, "Minute": 0}));
    let args: Vec<&str> = data["ProgramArguments"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(["trunk", "merge", "--llm", "claude"].iter().all(|a| args.contains(a)), "{args:?}");
    assert_eq!(data["RunAtLoad"], false);
    assert_eq!(trunk_schedule::plist_data("23:59", "codex").unwrap()["StartCalendarInterval"], json!({"Hour": 23, "Minute": 59}));
    assert!(trunk_schedule::plist_data("not-a-time", "claude").is_err() && trunk_schedule::plist_data("25:00", "claude").is_err());
    let xml = String::from_utf8(trunk_schedule::render_plist("04:00", "none").unwrap()).unwrap();
    assert!(xml.starts_with("<?xml") && xml.contains(&format!("<string>{}</string>", trunk_schedule::LABEL)), "{xml}");
}

#[test]
fn trunk_schedule_install_remove_use_sandboxed_paths_and_launchctl() {
    let h = lib_home();
    let log = fake_launchctl(&h);
    assert!(trunk_schedule::plist_path().starts_with(h.path()) && trunk_schedule::log_path().starts_with(h.path()));
    assert!(!trunk_schedule::remove(None)["removed"].as_bool().unwrap());
    assert!(!log.exists());
    let result = trunk_schedule::install("04:00", "claude", None, None).unwrap();
    let path = PathBuf::from(result["path"].as_str().unwrap());
    assert!(path.starts_with(h.path()) && result["launchctl_ok"] == true, "{result:?}");
    assert_eq!(mode(&path), 0o644);
    trunk_schedule::install("04:00", "claude", None, None).unwrap();
    assert!(trunk_schedule::is_scheduled(None));
    assert!(trunk_schedule::remove(None)["removed"].as_bool().unwrap());
    assert!(!trunk_schedule::is_scheduled(None));
    let calls: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().map(|l| l.split(' ').next().unwrap().to_string()).collect();
    assert_eq!(calls, ["bootstrap", "bootstrap", "bootout"]);
}

#[test]
fn trunk_schedule_cli_previews_and_rejects_bad_time() {
    let fx = fixture();
    let out = fx.run(&["trunk", "schedule"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(fx.stdout(&out).contains("would write"));
    assert!(!fx.home().join("Library").exists());
    let out = fx.run(&["trunk", "schedule", "--remove"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(fx.stdout(&out).contains("would remove"));
    assert_eq!(fx.run(&["trunk", "schedule", "--at", "bogus"]).status.code(), Some(2));
    if !cfg!(target_os = "macos") {
        assert_ne!(fx.run(&["trunk", "schedule", "--apply"]).status.code(), Some(0));
        assert_eq!(fx.run(&["onboard", "--yes", "--schedule-merge"]).status.code(), Some(2));
    }
}
