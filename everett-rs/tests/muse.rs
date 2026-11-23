mod common;

use common::fixture;
use serde_json::{json, Value};

fn read(path: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn commands(settings: &Value, event: &str) -> Vec<String> {
    settings["hooks"][event]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|e| e["hooks"].as_array().unwrap().iter())
        .map(|h| h["command"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn muse_hooks_and_mcp_install_into_settings_json() {
    let fx = fixture();
    let path = fx.home().join(".config/muse/settings.json");

    let out = fx.run(&["install-hooks", "--muse", "--apply"]);
    assert!(out.status.success(), "{}", fx.stdout(&out));
    let settings = read(&path);
    assert_eq!(settings["schema_version"], json!(1));
    for (event, stem) in [
        ("SessionStart", "muse_session_start"),
        ("Stop", "muse_stop"),
        ("UserPromptSubmit", "muse_inbox"),
        ("PostToolUse", "muse_inbox"),
    ] {
        let cmds = commands(&settings, event);
        assert!(cmds.iter().any(|c| c.contains(&format!("hook {stem}"))), "{event}: {cmds:?}");
    }

    // MCP-only install on a fresh settings file must still produce schema_version.
    let fx2 = fixture();
    let out = fx2.run(&["install-mcp", "--muse", "--apply"]);
    assert!(out.status.success(), "{}", fx2.stdout(&out));
    let settings = read(&fx2.home().join(".config/muse/settings.json"));
    assert_eq!(settings["schema_version"], json!(1));
    let entry = &settings["mcp_servers"]["everett"];
    assert_eq!(entry["transport"], json!("stdio"));
    assert_eq!(entry["args"], json!(["mcp"]));
    assert!(entry["command"].as_str().unwrap().ends_with("everett"));

    // A settings file written before schema_version existed is repaired by
    // both the plain and --repair apply paths even though the entry is current.
    for extra in [vec![], vec!["--repair"]] {
        let mut args = vec!["install-mcp", "--muse"];
        args.extend(extra);
        args.push("--apply");
        let out = fx2.run(&args);
        assert!(out.status.success(), "{args:?}: {}", fx2.stdout(&out));
        let mut settings = read(&fx2.home().join(".config/muse/settings.json"));
        settings.as_object_mut().unwrap().remove("schema_version");
        std::fs::write(fx2.home().join(".config/muse/settings.json"), settings.to_string()).unwrap();
        let out = fx2.run(&args);
        assert!(out.status.success(), "{args:?}: {}", fx2.stdout(&out));
        assert_eq!(read(&fx2.home().join(".config/muse/settings.json"))["schema_version"], json!(1));
    }

    // Idempotent: a second apply reports already installed and keeps one backup.
    let again = fx.stdout(&fx.run(&["install-hooks", "--muse", "--apply"]));
    assert!(again.contains("already installed"), "{again}");
    let backups = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("settings.json.everett-bak-"))
        .count();
    // Fresh-file installs need no backup; at most one may exist.
    assert!(backups <= 1, "{backups} backups");
}

#[test]
fn muse_route_resume_uses_muse_resume_not_omp() {
    let s = everett::session::Session::new("muse", "m-1", "/work/muse", "", "", 0.0);
    let cmd = everett::route::resume_command(&s, "go");
    assert!(cmd.contains("muse resume m-1"), "{cmd}");
    assert!(!cmd.contains("omp -r"), "{cmd}");
}
