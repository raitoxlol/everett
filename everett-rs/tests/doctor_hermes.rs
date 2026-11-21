mod common;

use common::fixture;
use std::process::Command;

fn run_everett(home: &std::path::Path, hermes_home: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(args)
        .env("HOME", home)
        .env("EVERETT_HOME", home)
        .env("HERMES_HOME", hermes_home)
        .env("PATH", common::HERMETIC_PATH)
        .env_remove("TYPESAFE_API_KEY")
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn doctor_reports_hermes_plugin_status_and_duplicate_manual_mcp() {
    let f = fixture();
    let hermes_home = f.home().join(".hermes");
    std::fs::create_dir_all(&hermes_home).unwrap();

    // 1. Hermes detected without plugin -> doctor reports missing native plugin and install step.
    let missing = run_everett(&f.home(), &hermes_home, &["doctor"]);
    assert!(
        missing.contains("hermes") && missing.contains("everett-hermes") && missing.contains("hermes plugins install"),
        "expected Hermes plugin install guidance in doctor output:\n{missing}"
    );

    // 2. Native plugin installed and enabled -> doctor reports ok.
    let plugin_dir = hermes_home.join("plugins/everett-hermes");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("plugin.yaml"), "name: everett-hermes\nversion: 0.2.0\n").unwrap();
    std::fs::write(
        hermes_home.join("config.yaml"),
        "plugins:\n  enabled:\n    - everett-hermes\n",
    )
    .unwrap();
    let ready = run_everett(&f.home(), &hermes_home, &["doctor"]);
    assert!(
        ready.contains("[ok  ]   hermes") && ready.contains("native plugin"),
        "expected ok Hermes native plugin line in doctor output:\n{ready}"
    );

    // 3. Enabled native plugin + manual mcp_servers.everett -> doctor warns about duplicate tools.
    std::fs::write(
        hermes_home.join("config.yaml"),
        "plugins:\n  enabled:\n    - everett-hermes\nmcp_servers:\n  everett:\n    command: everett\n    args: [\"mcp\", \"--harness\", \"hermes\"]\n",
    )
    .unwrap();
    let dup = run_everett(&f.home(), &hermes_home, &["doctor"]);
    assert!(
        dup.contains("[warn]   hermes") && dup.contains("duplicate"),
        "expected duplicate MCP warning in doctor output:\n{dup}"
    );
}

#[test]
fn install_mcp_hermes_recommends_native_plugin_with_manual_fallback() {
    let f = fixture();
    let hermes_home = f.home().join(".hermes");
    std::fs::create_dir_all(&hermes_home).unwrap();
    let snippet = run_everett(&f.home(), &hermes_home, &["install-mcp", "--hermes"]);
    assert!(
        snippet.contains("hermes plugins install raitoxlol/everett#plugins/everett-hermes"),
        "expected native plugin install command:\n{snippet}"
    );
    assert!(
        snippet.contains("mcp_servers:") && snippet.contains("\"--harness\", \"hermes\""),
        "expected manual mcp_servers fallback preserved:\n{snippet}"
    );
}
