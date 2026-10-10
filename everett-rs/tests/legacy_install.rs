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
fn install_hooks_replaces_python_package_hooks() {
    let fx = fixture();
    let legacy = |script: &str| format!("/usr/bin/python3 '/opt/pipx/venvs/everett-sessions/lib/python3.12/site-packages/everett/hooks/{}.py'", script);
    let settings = json!({"hooks": {
        "Stop": [
            {"hooks": [{"type": "command", "command": legacy("claude_stop"), "timeout": 3}]},
            {"hooks": [{"type": "command", "command": "notify-send done"}]}
        ],
        "UserPromptSubmit": [{"hooks": [
            {"type": "command", "command": legacy("claude_inbox"), "timeout": 3},
            {"type": "command", "command": "my-own-prompt-hook"}
        ]}]
    }});
    let path = fx.write(".claude/settings.json", &settings.to_string());

    let out = fx.run(&["install-hooks", "--claude", "--apply"]);
    let stdout = fx.stdout(&out);
    assert!(out.status.success(), "{}", stdout);
    assert!(stdout.contains("replaced legacy Python hooks for"), "{}", stdout);

    let merged = read(&path);
    for event in ["SessionStart", "Stop", "UserPromptSubmit", "PostToolUse"] {
        let cmds = commands(&merged, event);
        assert!(cmds.iter().all(|c| !c.ends_with(".py'")), "{} kept legacy hook: {:?}", event, cmds);
        assert_eq!(cmds.iter().filter(|c| c.contains(" hook claude_")).count(), 1, "{}: {:?}", event, cmds);
    }
    assert!(commands(&merged, "Stop").contains(&"notify-send done".to_string()));
    assert!(commands(&merged, "UserPromptSubmit").contains(&"my-own-prompt-hook".to_string()));
    let backups = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("settings.json.everett-bak-"))
        .count();
    assert_eq!(backups, 1);

    let again = fx.stdout(&fx.run(&["install-hooks", "--claude", "--apply"]));
    assert!(again.contains("already installed"), "{}", again);
}

#[test]
fn install_mcp_replaces_python_package_launcher_without_repair_flag() {
    let fx = fixture();
    let path = fx.write(
        ".claude.json",
        &json!({"mcpServers": {"everett": {
            "type": "stdio",
            "command": "/usr/bin/python3",
            "args": ["-m", "everett", "mcp"],
            "env": {"PYTHONPATH": "/old/checkout", "EVERETT_ROUTER": "local"}
        }}})
        .to_string(),
    );

    let doctor = fx.stdout(&fx.run(&["doctor"]));
    assert!(doctor.contains("retired Python package"), "{}", doctor);

    let out = fx.run(&["install-mcp", "--claude", "--apply"]);
    assert!(out.status.success(), "{}", fx.stdout(&out));
    let entry = read(&path)["mcpServers"]["everett"].clone();
    assert_eq!(entry["args"], json!(["mcp"]));
    assert!(entry["command"].as_str().unwrap().ends_with("everett"), "{}", entry);
    assert_eq!(entry["env"], json!({"EVERETT_ROUTER": "local"}));
}

#[test]
fn omp_extension_that_spawns_python_is_reinstalled() {
    let fx = fixture();
    let path = fx.write(
        ".omp/agent/extensions/everett.ts",
        "spawnSync(\"python3\", [inboxScript, sessionId]);\n",
    );
    let out = fx.run(&["install-hooks", "--omp", "--apply"]);
    assert!(out.status.success(), "{}", fx.stdout(&out));
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(!body.contains("python3") && body.contains("\"hook\", \"omp_inbox\""), "{}", body);
}
