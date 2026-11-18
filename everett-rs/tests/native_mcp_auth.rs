mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

#[test]
fn running_stdio_server_rechecks_saved_credential() {
    let fixture = common::fixture();
    let home = fixture.home();
    common::native_mcp_account(&home);
    let credential_path = home.join(".everett/cloudflare-auth.json");
    let saved = std::fs::read(&credential_path).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_everett"))
        .arg("mcp")
        .env_clear()
        .env("HOME", &home)
        .env("CODEX_THREAD_ID", "cached-session")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut request = |id, method, params| {
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
        let mut line = String::new();
        assert_ne!(output.read_line(&mut line).unwrap(), 0);
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], id);
        response["result"].clone()
    };
    assert_eq!(
        request(1, "initialize", json!({}))["serverInfo"]["name"],
        "everett"
    );
    let args = json!({"name":"everett_whoami"});
    let first = request(2, "tools/call", args.clone());
    assert_eq!(first["structuredContent"]["session_id"], "cached-session");
    assert_eq!(
        first["structuredContent"]["identity"]["account"]["status"],
        "verified"
    );

    let unavailable = common::jev_stub(503, "{}");
    let config = fixture.write(
        ".everett/config.toml",
        &format!(
            "[auth]\nbase_url='{}'\n",
            unavailable.trim_end_matches("/v1/systemone")
        ),
    );
    let denied = request(3, "tools/call", args.clone());
    assert_eq!(denied["isError"], true);
    assert!(denied.to_string().contains("Cannot verify"));
    assert_eq!(std::fs::read(&credential_path).unwrap(), saved);
    std::fs::remove_file(config).unwrap();

    let mut expired: Value = serde_json::from_slice(&saved).unwrap();
    expired["expires_at"] = json!(1);
    std::fs::write(&credential_path, expired.to_string()).unwrap();
    let denied = request(4, "tools/call", args.clone());
    assert_eq!(denied["isError"], true);
    assert!(denied.to_string().contains("expired"));

    std::fs::write(&credential_path, saved).unwrap();
    assert_eq!(request(5, "tools/call", args.clone())["isError"], false);
    let logout = fixture.run(&["logout", "--json"]);
    assert!(
        logout.status.success(),
        "{}",
        String::from_utf8_lossy(&logout.stderr)
    );
    let denied = request(6, "tools/call", args);
    assert_eq!(denied["isError"], true);
    assert!(denied.to_string().contains("everett login"));
    drop(input);
    assert!(child.wait().unwrap().success());
}

#[test]
fn gateway_worker_rejects_public_mcp_and_unscoped_operations() {
    let fixture = common::fixture();
    let binding = [
        ("EVERETT_SESSION_ID", "dot-test"),
        ("EVERETT_HARNESS_NAME", "openai-dot"),
        ("EVERETT_GATEWAY_EXACT_IDS", "1"),
    ];
    for request in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}),
        json!({"Spawn":{"text":"fixture"}}),
        json!({"Core":{"project":"test","action":"search"}}),
    ] {
        let output = fixture.run_stdin(&["gateway-worker"], &request.to_string(), &binding);
        assert!(!output.status.success(), "accepted {request}");
        assert!(output.stdout.is_empty(), "answered {request}");
    }

    let call =
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"everett_whoami"}});
    let output = fixture.run_stdin(&["mcp"], &format!("{call}\n"), &binding);
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert!(response.to_string().contains("everett login"));
}
