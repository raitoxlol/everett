mod common;

use common::{fixture, Fixture};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Output;

fn run(fx: &Fixture, args: &[&str]) -> Output {
    let data = fx.home().join("session-data").to_string_lossy().to_string();
    let appdata = fx.home().join("appdata").to_string_lossy().to_string();
    let path = fx.home().to_string_lossy().to_string();
    fx.run_stdin(
        args,
        "",
        &[
            ("DEVIN_HOME", &data),
            ("APPDATA", &appdata),
            ("PATH", &path),
        ],
    )
}

fn config_path(fx: &Fixture) -> PathBuf {
    let root = if cfg!(windows) {
        fx.home().join("appdata")
    } else {
        fx.home().join(".config")
    };
    root.join("devin").join("mcp_config.json")
}

fn write_config(fx: &Fixture, text: &str) {
    let path = config_path(fx);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn config(fx: &Fixture) -> Value {
    serde_json::from_slice(&std::fs::read(config_path(fx)).unwrap()).unwrap()
}

fn backups(fx: &Fixture) -> Vec<PathBuf> {
    std::fs::read_dir(config_path(fx).parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("mcp_config.json.everett-bak-")
        })
        .collect()
}

fn success(out: &Output) {
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn devin_dry_run_uses_dedicated_config_and_has_no_hook_flag() {
    let fx = fixture();
    let out = run(&fx, &["install-mcp", "--devin"]);
    success(&out);
    let text = fx.stdout(&out);
    assert!(text.contains(config_path(&fx).to_str().unwrap()), "{text}");
    assert!(text.contains("v3000.3+"));
    assert!(text.contains("\"mcpServers\""));
    assert!(text.contains("## devin"));
    assert!(!text.contains("## claude"));
    assert!(!config_path(&fx).exists());
    assert!(!fx.home().join("session-data").exists());
    let hooks = run(&fx, &["install-hooks", "--devin"]);
    assert_eq!(hooks.status.code(), Some(2));
    let repair = run(&fx, &["install-mcp", "--devin", "--repair"]);
    assert_eq!(repair.status.code(), Some(2));
    assert!(!config_path(&fx).exists());
}

#[test]
fn devin_default_mcp_selection_detects_config_directory_not_hooks() {
    let fx = fixture();
    std::fs::create_dir_all(config_path(&fx).parent().unwrap()).unwrap();
    let out = run(&fx, &["install-mcp"]);
    success(&out);
    assert!(fx.stdout(&out).contains("## devin"));
    let hooks = run(&fx, &["install-hooks"]);
    success(&hooks);
    assert!(!fx.stdout(&hooks).contains("## devin"));
    assert!(!config_path(&fx).exists());
}

#[test]
fn devin_apply_preserves_siblings_backups_and_idempotency() {
    let fx = fixture();
    let original =
        json!({"custom": 1, "mcpServers": {"other": {"url": "https://example.invalid/mcp"}}});
    write_config(&fx, &original.to_string());
    let before = std::fs::read(config_path(&fx)).unwrap();
    success(&run(&fx, &["install-mcp", "--devin", "--apply"]));
    let registered = std::fs::read(config_path(&fx)).unwrap();
    let data = config(&fx);
    assert_eq!(data["custom"], original["custom"]);
    assert_eq!(data["mcpServers"]["other"], original["mcpServers"]["other"]);
    assert_eq!(
        data["mcpServers"]["everett"]["command"],
        env!("CARGO_BIN_EXE_everett")
    );
    assert_eq!(data["mcpServers"]["everett"]["args"], json!(["mcp"]));
    assert!(data["mcpServers"]["everett"].get("type").is_none());
    let saved = backups(&fx);
    assert_eq!(saved.len(), 1);
    assert_eq!(std::fs::read(&saved[0]).unwrap(), before);
    success(&run(&fx, &["install-mcp", "--devin", "--apply"]));
    success(&run(
        &fx,
        &["install-mcp", "--devin", "--repair", "--apply"],
    ));
    assert_eq!(std::fs::read(config_path(&fx)).unwrap(), registered);
    assert_eq!(backups(&fx), saved);
    assert!(!fx.home().join("session-data").exists());
    assert!(!config_path(&fx)
        .parent()
        .unwrap()
        .join("config.json")
        .exists());
}

#[test]
fn devin_repair_refreshes_disabled_remote_entry_preserving_env() {
    let fx = fixture();
    let old = json!({"mcpServers": {
        "other": {"command": "untouched"},
        "everett": {"url": "https://example.invalid/mcp", "transport": "http",
            "disabled": true, "enabled": false, "env": {"KEEP": "yes", "PYTHONPATH": "/missing/checkout"}}
    }});
    write_config(&fx, &old.to_string());
    let before = std::fs::read(config_path(&fx)).unwrap();
    let refused = run(&fx, &["install-mcp", "--devin", "--apply"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--devin --repair --apply"));
    assert_eq!(std::fs::read(config_path(&fx)).unwrap(), before);
    assert!(backups(&fx).is_empty());
    success(&run(
        &fx,
        &["install-mcp", "--devin", "--repair", "--apply"],
    ));
    let data = config(&fx);
    let entry = &data["mcpServers"]["everett"];
    assert_eq!(data["mcpServers"]["other"], old["mcpServers"]["other"]);
    assert_eq!(entry["env"], json!({"KEEP": "yes"}));
    assert_eq!(entry["disabled"], false);
    assert_eq!(entry["enabled"], true);
    assert!(entry.get("url").is_none());
    assert!(entry.get("transport").is_none());
    assert_eq!(backups(&fx).len(), 1);
    assert_eq!(std::fs::read(&backups(&fx)[0]).unwrap(), before);
    success(&run(&fx, &["install-mcp", "--devin", "--apply"]));
    assert_eq!(backups(&fx).len(), 1);
}

#[test]
fn devin_http_transport_is_stale_even_with_valid_command() {
    let fx = fixture();
    write_config(
        &fx,
        &json!({"mcpServers": {"everett": {
            "command": env!("CARGO_BIN_EXE_everett"), "args": ["mcp"], "transport": "http"
        }}})
        .to_string(),
    );
    let out = run(&fx, &["install-mcp", "--devin", "--apply"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not Everett stdio"));
    success(&run(
        &fx,
        &["install-mcp", "--devin", "--repair", "--apply"],
    ));
    assert!(config(&fx)["mcpServers"]["everett"]
        .get("transport")
        .is_none());
}

#[test]
fn devin_missing_and_invalid_config_are_safe() {
    let fx = fixture();
    success(&run(&fx, &["install-mcp", "--devin", "--apply"]));
    assert!(config(&fx)["mcpServers"]["everett"].is_object());
    assert!(backups(&fx).is_empty());
    for invalid in [
        "{broken",
        "[]",
        "{\"mcpServers\": []}",
        "{\"mcpServers\": {\"everett\": \"bad\"}}",
    ] {
        write_config(&fx, invalid);
        let out = run(&fx, &["install-mcp", "--devin", "--repair", "--apply"]);
        assert_eq!(out.status.code(), Some(1), "{invalid}");
        assert_eq!(std::fs::read_to_string(config_path(&fx)).unwrap(), invalid);
        assert!(backups(&fx).is_empty());
    }
}

#[test]
fn devin_doctor_reports_missing_ready_stale_without_hooks() {
    let fx = fixture();
    std::fs::create_dir_all(fx.home().join("session-data")).unwrap();
    let text = fx.stdout(&run(&fx, &["doctor"]));
    assert!(
        text.contains("everett install-mcp --devin --apply"),
        "{text}"
    );
    assert!(!text.contains("install-hooks --devin"));
    success(&run(&fx, &["install-mcp", "--devin", "--apply"]));
    let text = fx.stdout(&run(&fx, &["doctor"]));
    assert!(
        text.contains("[ok  ]   devin   10 tools over stdio (initialize + tools/list passed)"),
        "{text}"
    );
    assert!(!text.contains("everett install-mcp --devin"), "{text}");
    let mut data = config(&fx);
    data["mcpServers"]["everett"]["disabled"] = json!(true);
    write_config(&fx, &data.to_string());
    let text = fx.stdout(&run(&fx, &["doctor"]));
    assert!(
        text.contains("everett install-mcp --devin --repair --apply"),
        "{text}"
    );
}

#[test]
fn devin_onboarding_discovers_config_only_and_installs_mcp_not_hooks() {
    let fx = fixture();
    write_config(&fx, "{}");
    let out = run(&fx, &["onboard", "--yes", "--no-backfill"]);
    success(&out);
    let text = fx.stdout(&out);
    assert!(text.contains("Hooks:\n  (none)\nMCP:\n  devin:"), "{text}");
    assert!(config(&fx)["mcpServers"]["everett"].is_object());
    assert!(!fx.home().join("session-data").exists());
}
