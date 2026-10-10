mod common;

use common::{fixture, lib_home};
use everett::install;
use everett::route::route_with;
use everett::session::Session;
use serde_json::{json, Map, Value};
use std::time::{Duration, Instant};

fn claude_jsonl(fx: &common::Fixture, sid: &str, text: &str) {
    let row = json!({"type": "user", "sessionId": sid, "cwd": "/work/app", "timestamp": "2026-09-20T00:00:00Z", "message": {"role": "user", "content": text}});
    fx.write(&format!(".claude/projects/-work-app/{sid}.jsonl"), &format!("{row}\n"));
}

#[test]
fn missing_harness_fails_before_waiting_for_a_session() {
    let fx = fixture();
    claude_jsonl(&fx, "busy", "review the parser");
    let start = Instant::now();
    let out = fx.run_stdin(&["send", "--to", "busy", "review"], "", &[("PATH", "/nonexistent")]);
    assert!(start.elapsed() < Duration::from_secs(5));
    let err = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0));
    assert!(err.contains("not on PATH") && err.contains("everett doctor"), "{err}");
}

#[test]
fn doctor_probes_stdio_and_explains_empty_home() {
    let fx = fixture();
    let out = fx.run_stdin(&["doctor"], "", &[("PATH", "/nonexistent")]);
    assert_eq!(out.status.code(), Some(0));
    let text = fx.stdout(&out);
    assert!(text.contains("10 tools over stdio") && text.contains("No harness CLI") && text.contains("everett onboard --yes"), "{text}");
    assert!(!text.contains("\nhealthy\n"));
}

/// The real binary, so the repaired registration points at an executable named `everett`.
fn install_mcp(h: &common::LibHome, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_everett")).arg("install-mcp").args(args)
        .env("HOME", h.path()).env("EVERETT_HOME", h.path()).env("EVERETT_NOTIFY", "none").output().unwrap()
}

#[test]
fn stale_registration_is_repaired_with_backup_and_other_servers_kept() {
    let h = lib_home();
    for harness in ["claude", "codex", "omp", "grok"] {
        let path = install::mcp_path(harness);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = if path.extension().map(|e| e == "toml").unwrap_or(false) {
            "model = \"chosen\"\n[mcp_servers.other]\ncommand = \"other\"\n[mcp_servers.everett]\ncommand = \"/removed/everett\"\n\
             args = [\"mcp\"]\nenabled = false\n[mcp_servers.everett.env]\nKEEP = \"value\"\n[mcp_servers.later]\ncommand = \"later\"\n".to_string()
        } else {
            json!({"keep": 1, "mcpServers": {"other": {"command": "other"}, "everett": {"command": "/removed/everett", "args": ["mcp"],
                "disabled": true, "env": {"KEEP": "value"}}}}).to_string()
        };
        std::fs::write(&path, &original).unwrap();
        assert_ne!(install::mcp_status(harness).0, "ready", "{harness}");
        let flag = format!("--{harness}");
        let out = install_mcp(&h, &[&flag, "--apply"]);
        assert!(String::from_utf8_lossy(&out.stderr).contains("--repair --apply"), "{harness}: {out:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert_eq!(install_mcp(&h, &[&flag, "--repair", "--apply"]).status.code(), Some(0), "{harness}");
        assert_eq!(install::mcp_status(harness).0, "ready", "{harness}: {:?}", install::mcp_status(harness));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("other") && text.contains("KEEP"), "{harness}: {text}");
        let backup = std::fs::read_dir(path.parent().unwrap()).unwrap().flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with(&format!("{}.everett-bak-", path.file_name().unwrap().to_string_lossy())))
            .expect("backup");
        assert_eq!(std::fs::read_to_string(backup.path()).unwrap(), original);
    }
}

#[test]
fn malformed_configuration_is_not_overwritten_and_repair_requires_apply() {
    let _h = lib_home();
    for (harness, original) in [("claude", "{broken"), ("codex", "[broken")] {
        let path = install::mcp_path(harness);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, original).unwrap();
        assert!(install::apply_mcp(harness, true).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }
    let fx = fixture();
    assert_eq!(fx.run(&["install-mcp", "--claude", "--repair"]).status.code(), Some(2));
    assert!(!fx.home().join(".claude.json").exists());
}

fn sessions() -> Vec<Session> {
    let mut a = Session::new("claude", "c-1", "/work/app", "/tmp/c-1.jsonl", "", 0.0);
    a.card = "API work: implement the upload fix".into();
    let mut b = Session::new("omp", "o-1", "/work/infra", "/tmp/o-1.jsonl", "", 0.0);
    b.card = "Infra work: implement the deploy fix".into();
    vec![a, b]
}

#[test]
fn ask_carries_candidates_for_local_and_jev() {
    let _h = lib_home();
    let jev = |_: &str, _: &Map<String, Value>, _: &str| -> everett::error::Result<Map<String, Value>> {
        Ok(json!({"choice": "s0", "confidence": 0.3}).as_object().unwrap().clone())
    };
    let r = route_with("implement fix", &sessions(), None, Some(""), &jev, None).unwrap();
    assert_eq!((r["decision"].clone(), r["router"].clone()), (json!("ASK"), json!("local")), "{r:?}");
    assert_eq!(r["candidates"].as_array().unwrap().len(), 2);
    let r = route_with("something", &sessions(), None, Some("k"), &jev, None).unwrap();
    assert_eq!((r["decision"].clone(), r["router"].clone()), (json!("ASK"), json!("jev")), "{r:?}");
    let candidates = r["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].as_str().unwrap().contains("claude"));
}

#[test]
fn cli_send_ask_prints_candidates_and_sends_nothing() {
    let fx = fixture();
    claude_jsonl(&fx, "c-1", "implement the upload fix");
    claude_jsonl(&fx, "c-2", "implement the deploy fix");
    let out = fx.run_stdin(&["send", "implement fix"], "", &[("EVERETT_ROUTER", "local"), ("PATH", "/nonexistent")]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let text = fx.stdout(&out);
    assert!(text.contains("candidate:") && !text.contains("SENT"), "{text}");
}
