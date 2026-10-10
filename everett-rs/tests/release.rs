mod common;

use common::{fixture, lib_home};
use everett::route::{find_api_key, local_route, route_with};
use everett::session::Session;
use serde_json::{json, Map, Value};

const NOW: f64 = 1_800_000_000.0;

fn session(id: &str, cwd: &str, card: &str, first: &str, last_active: f64) -> Session {
    let mut s = Session::new("claude", id, cwd, &format!("/tmp/{}.jsonl", id), "", last_active);
    s.card = card.into();
    s.first_user = first.into();
    s
}

fn sessions() -> Vec<Session> {
    vec![
        session("a", "/work/atlas", "Atlas: shipping the release notes and changelog", "", NOW),
        session("b", "/work/app", "App: fixing the login redirect bug; next add tests", "", NOW),
        session("c", "/work/site", "", "redesign the marketing site hero", NOW),
    ]
}

#[test]
fn local_router_decisions() {
    let _h = lib_home();
    let r = local_route("keep going on atlas", &sessions(), Some(NOW), None);
    assert_eq!((r["decision"].clone(), r["session"]["id"].clone()), (json!("SESSION"), json!("a")));
    assert!(r["confidence"].as_f64().unwrap() >= 0.6);
    assert!(r["command"].as_str().unwrap().contains("claude --resume a"));
    let r = local_route("login redirect tests", &sessions(), Some(NOW), None);
    assert!(["input", "choice", "confidence", "decision"].iter().all(|k| r.contains_key(*k)));
    assert_eq!(r["choice"], "s1");
    let r = local_route("write a haiku about penguins", &sessions(), Some(NOW), None);
    assert_eq!((r["decision"].clone(), r["command"].clone()), (json!("NEW"), json!("claude 'write a haiku about penguins'")));
    let twins = [session("x", "/w/one", "billing export", "", NOW), session("y", "/w/two", "billing export", "", NOW)];
    let r = local_route("billing export", &twins, Some(NOW), None);
    assert_eq!(r["decision"], "ASK");
    assert!(r.get("suggested").map(|s| !s.is_null() && s != "").unwrap_or(false));
    let twins = [session("old", "/w/one", "billing export", "", NOW - 30.0 * 86400.0), session("new", "/w/two", "billing export", "", NOW)];
    assert_eq!(local_route("billing export", &twins, Some(NOW), None)["session"]["id"], "new");
    assert_eq!(local_route("anything", &[], Some(NOW), None)["decision"], "NEW");
    assert_eq!(local_route("the and of", &sessions(), Some(NOW), None)["decision"], "ASK");
}

#[test]
fn router_flag_forces_local_and_unknown_router_errors() {
    let _h = lib_home();
    let jev = |_: &str, _: &Map<String, Value>, _: &str| -> everett::error::Result<Map<String, Value>> { panic!("jev must not run") };
    let r = route_with("keep going on atlas", &sessions(), Some("local"), Some("k"), &jev, None).unwrap();
    assert_eq!(r["router"], "local");
    assert!(route_with("x", &sessions(), Some("magic"), Some(""), &jev, None).is_err());
}

#[test]
fn config_key_sources_and_default_harness() {
    let h = lib_home();
    let write = |rel: &str, text: &str| {
        let p = h.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    assert_eq!(find_api_key(), "");
    write(".hermes/.env", "OTHER=1\nTYPESAFE_API_KEY=\"from-hermes\"\n");
    assert_eq!(find_api_key(), "from-hermes");
    write(".everett/config.toml", "# c\nvault = \"~/Notes\"  # mine\ndefault_harness = codex\n[jev]\napi_key = 'k-1'\n");
    assert_eq!(find_api_key(), "k-1");
    std::env::set_var("TYPESAFE_API_KEY", "from-env");
    assert_eq!(find_api_key(), "from-env");
    std::env::remove_var("TYPESAFE_API_KEY");
    assert_eq!(local_route("x y", &[], Some(NOW), None)["command"], "codex 'x y'");
}

#[test]
fn trunk_vault_errors_and_paths() {
    let fx = fixture();
    let out = fx.run(&["trunk"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no vault configured"));
    let vault = fx.home().join("v");
    assert!(fx.run_stdin(&["trunk"], "", &[("EVERETT_VAULT", vault.to_str().unwrap())]).status.success());
    assert!(vault.join("Everett/Trunk.md").exists());
    let w = fx.home().join("w");
    fx.write(".everett/config.toml", &format!("vault = \"{}\"\nvault_dir = \"Projects/Everett\"\n", w.display()));
    assert!(fx.run(&["trunk"]).status.success());
    assert!(w.join("Projects/Everett/Trunk.md").exists());
}

const OTHER: &str = r#"{"type": "command", "command": "notify-me", "timeout": 1}"#;

#[test]
fn install_hooks_print_apply_backup_and_idempotence() {
    let fx = fixture();
    let printed = fx.stdout(&fx.run(&["install-hooks"]));
    assert!(printed.contains("hook claude_session_start") && printed.contains("hook codex_stop"), "{printed}");
    assert!(!fx.home().join(".claude").exists());
    let other: Value = serde_json::from_str(OTHER).unwrap();
    let settings = fx.write(".claude/settings.json", &json!({"model": "x", "hooks": {"Stop": [{"hooks": [other]}], "PreToolUse": [{"hooks": [other]}]}}).to_string());
    assert!(fx.run(&["install-hooks", "--claude", "--apply"]).status.success());
    let data: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(data["model"], "x");
    assert_eq!(data["hooks"]["PreToolUse"], json!([{"hooks": [other]}]));
    assert_eq!(data["hooks"]["Stop"][0], json!({"hooks": [other]}));
    assert_eq!(data["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert_eq!(data["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    let backups = || std::fs::read_dir(settings.parent().unwrap()).unwrap().flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("settings.json.everett-bak-")).map(|e| e.path()).collect::<Vec<_>>();
    assert_eq!(backups().len(), 1);
    assert!(!std::fs::read_to_string(&backups()[0]).unwrap().contains("SessionStart"));
    assert!(fx.stdout(&fx.run(&["install-hooks", "--claude", "--apply"])).contains("already installed"));
    assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(&settings).unwrap()).unwrap(), data);
    assert_eq!(backups().len(), 1);
}

#[test]
fn new_files_have_no_backup_and_malformed_settings_are_kept() {
    let fx = fixture();
    assert!(fx.run(&["install-hooks", "--codex", "--omp", "--apply"]).status.success());
    assert!(std::fs::read_dir(fx.home().join(".codex")).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains(".everett-bak-")));
    assert!(fx.home().join(".omp/agent/extensions").exists());
    let path = fx.write(".claude/settings.json", "{not json");
    assert_eq!(fx.run(&["install-hooks", "--claude", "--apply"]).status.code(), Some(1));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "{not json");
}

#[test]
fn doctor_on_empty_home_reports_router_and_vault() {
    let fx = fixture();
    let out = fx.run(&["doctor"]);
    assert_eq!(out.status.code(), Some(0));
    let text = fx.stdout(&out);
    assert!(text.contains("router: local") && text.contains("vault: not configured"), "{text}");
}
