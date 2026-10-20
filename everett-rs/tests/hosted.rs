mod common;

use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::process::Output;

const URL: &str = "https://hosted.convex.cloud";
const ISSUER: &str = "https://hosted.convex.site";
fn secret() -> String {
    format!(
        "evt_{}.{}{}",
        "a".repeat(64),
        "b".repeat(64),
        "c".repeat(64)
    )
}
fn seed(home: &common::Fixture) {
    let secret = secret();
    let claims = json!({"iss":ISSUER,"aud":"convex","sub":"agent-user|auth-session","exp":everett::session::now()+3600.0,"everett_agent":"agent-123","everett_org":"owner-1"});
    let jwt = format!(
        "header.{}.signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let stored = everett::auth::Stored {
        issuer: ISSUER.into(),
        client_id: "convex".into(),
        sub: "agent-user|auth-session".into(),
        email: String::new(),
        name: "agent-123".into(),
        device_name: "vm".into(),
        access_token: jwt.clone(),
        id_token: jwt,
        refresh_token: String::new(),
        expires_at: everett::session::now() + 3600.0,
        scope: format!(
            "{} {} {URL}",
            everett::hosted::SCOPE,
            hex::encode(Sha256::digest(secret.as_bytes()))
        ),
    };
    home.write(
        ".everett/auth.json",
        &serde_json::to_string(&stored).unwrap(),
    );
    let path = home.write(
        ".everett/agent-token.json",
        &json!({"deployment":URL,"token":secret}).to_string(),
    );
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn run(home: &common::Fixture, args: &[&str], extra: &[(&str, &str)]) -> Output {
    let mut env = vec![("EVERETT_HOSTED_URL", URL)];
    env.extend_from_slice(extra);
    home.run_stdin(args, "", &env)
}
fn safe(output: &Output) {
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!combined.contains(&"a".repeat(64)));
    assert!(!combined.contains(&"b".repeat(64)));
    assert!(!combined.contains(&"c".repeat(64)));
    assert!(!combined.contains("header."));
}
#[test]
fn cached_agent_login_and_export_are_private_and_never_print_secrets() {
    let home = common::fixture();
    seed(&home);
    let login = run(&home, &["login", "--json"], &[]);
    safe(&login);
    assert!(login.status.success(), "{}", home.stdout(&login));
    let value: Value = serde_json::from_slice(&login.stdout).unwrap();
    assert_eq!(value["name"], "agent-123");
    let out = home.home().join("token");
    let exported = run(&home, &["agent-token", "--out", out.to_str().unwrap()], &[]);
    safe(&exported);
    assert!(exported.status.success());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), secret());
    assert_eq!(
        std::fs::metadata(&out).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let again = run(&home, &["agent-token", "--out", out.to_str().unwrap()], &[]);
    safe(&again);
    assert!(!again.status.success());
    assert_eq!(std::fs::read_to_string(out).unwrap(), secret());
}
#[test]
fn unsafe_token_files_symlinks_and_mismatched_deployments_are_rejected_without_leaks() {
    let home = common::fixture();
    seed(&home);
    let credential = home.home().join(".everett/agent-token.json");
    std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o644)).unwrap();
    let output = run(&home, &["whoami", "--json"], &[]);
    safe(&output);
    assert!(!output.status.success());
    std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o600)).unwrap();
    let other = run(
        &home,
        &["whoami"],
        &[("EVERETT_HOSTED_URL", "https://other.convex.cloud")],
    );
    safe(&other);
    assert!(!other.status.success());
    let target = home.home().join("export");
    std::os::unix::fs::symlink(&credential, &target).unwrap();
    let output = run(
        &home,
        &["agent-token", "--out", target.to_str().unwrap()],
        &[],
    );
    safe(&output);
    assert!(!output.status.success());
    let output = run(
        &home,
        &["whoami"],
        &[("EVERETT_AGENT_TOKEN_FILE", target.to_str().unwrap())],
    );
    safe(&output);
    assert!(!output.status.success());
}
#[test]
fn config_preset_and_injected_secret_work_without_a_local_long_lived_token_file() {
    let home = common::fixture();
    seed(&home);
    std::fs::remove_file(home.home().join(".everett/agent-token.json")).unwrap();
    let token = secret();
    let output = run(
        &home,
        &["whoami", "--json"],
        &[("EVERETT_AGENT_TOKEN", &token)],
    );
    safe(&output);
    assert!(output.status.success());
    assert!(!home.home().join(".everett/agent-token.json").exists());
    home.write(
        ".everett/config.toml",
        &format!("[hosted]\nurl = '{URL}'\n"),
    );
    let file = home.write("token", &token);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let output = home.run_stdin(
        &["whoami", "--json"],
        "",
        &[("EVERETT_AGENT_TOKEN_FILE", file.to_str().unwrap())],
    );
    safe(&output);
    assert!(output.status.success());
}
#[test]
fn malformed_secret_and_insecure_deployment_fail_before_network_or_token_output() {
    let home = common::fixture();
    for url in [
        "http://localhost:8000",
        "https://hosted.convex.cloud/path",
        "https://user:password@hosted.convex.cloud",
        "https://hosted.convex.cloud?query=1",
    ] {
        let output = run(
            &home,
            &["login"],
            &[
                ("EVERETT_HOSTED_URL", url),
                ("EVERETT_AGENT_TOKEN", &secret()),
            ],
        );
        safe(&output);
        assert!(!output.status.success());
    }
    let output = run(&home, &["login"], &[("EVERETT_AGENT_TOKEN", "bad-secret")]);
    safe(&output);
    assert!(!output.status.success());
    assert!(!home.home().join(".everett/auth.json").exists());
}

#[test]
fn export_uses_current_relay_keys_after_rotation_not_initial_bootstrap_seeds() {
    let home = common::fixture();
    seed(&home);
    let owner = json!([ISSUER, "org", "owner-1"]).to_string();
    let digest = hex::encode(Sha256::digest(json!([owner, "agent-123"]).to_string()));
    let signing = "d".repeat(64);
    let encryption = "e".repeat(64);
    home.write(
        &format!(".everett/relay/keys-{digest}.json"),
        &json!({"signing":signing,"encryption":encryption}).to_string(),
    );
    let out = home.home().join("after-rotation");
    let output = run(&home, &["agent-token", "--out", out.to_str().unwrap()], &[]);
    safe(&output);
    assert!(output.status.success(), "{}", home.stdout(&output));
    let restored = std::fs::read_to_string(out).unwrap();
    assert_eq!(
        restored,
        format!("evt_{}.{signing}{encryption}", "a".repeat(64))
    );
}

#[test]
fn configured_relay_ca_must_be_readable_pem_before_any_hosted_request() {
    let home = common::fixture();
    let empty = home.write("empty-ca.pem", "");
    let empty = empty.to_str().unwrap();
    for ca in ["/nonexistent/everett-ca.pem", empty] {
        let output = run(
            &home,
            &["login", "--device", "vm"],
            &[
                ("EVERETT_HOSTED_URL", "https://127.0.0.1:9"),
                ("EVERETT_RELAY_CA_FILE", ca),
            ],
        );
        safe(&output);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
        assert!(stderr.contains("ca_file"), "{stderr}");
        assert!(!home.home().join(".everett/enrollment.json").exists());
    }
}

#[test]
fn mcp_callers_cannot_export_the_agent_secret_to_any_path() {
    let home = common::fixture();
    seed(&home);
    let out = home.home().join("stolen");
    let results = common::mcp_calls(
        &home,
        &[],
        &[("EVERETT_HOSTED_URL", URL)],
        &[(
            "everett_agent_relay",
            json!({"action":"token-export","out":out.to_str().unwrap()}),
        )],
    );
    safe_text(&results[0].to_string());
    assert!(
        results[0]["isError"] == true || results[0].get("error").is_some(),
        "{}",
        results[0]
    );
    assert!(!out.exists(), "MCP must never write the agent secret");
    let tools = everett::mcp::tools();
    let relay = tools
        .iter()
        .find(|t| t["name"] == "everett_agent_relay")
        .unwrap();
    let actions = relay["inputSchema"]["properties"]["action"]["enum"].to_string();
    assert!(!actions.contains("export"), "{actions}");
    assert!(relay["inputSchema"]["properties"].get("out").is_none());
}
fn safe_text(text: &str) {
    for secret in ["a".repeat(64), "b".repeat(64), "c".repeat(64)] {
        assert!(!text.contains(&secret));
    }
}
fn seed_pending(home: &common::Fixture, deployment: &str, ttl: f64) -> std::path::PathBuf {
    let path = home.write(
        ".everett/enrollment.json",
        &json!({"deployment":deployment,"token":secret(),"device_code":"d".repeat(64),"agent_id":"agent-approved","name":"vm","expires_at":everett::session::now()+ttl,"user_code":"C0DE42","verification_uri":format!("{deployment}/enroll")}).to_string(),
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    path
}
#[test]
fn rerunning_enroll_resumes_the_pending_enrollment_instead_of_overwriting_it() {
    let home = common::fixture();
    let unreachable = "https://127.0.0.1:9";
    let path = seed_pending(&home, unreachable, 300.0);
    let before = std::fs::read_to_string(&path).unwrap();
    let results = common::mcp_calls(
        &home,
        &[],
        &[("EVERETT_HOSTED_URL", unreachable)],
        &[("everett_agent_relay", json!({"action":"enroll","name":"vm"}))],
    );
    safe_text(&results[0].to_string());
    let value = common::tool_json(&results[0]);
    assert_eq!(value["user_code"], "C0DE42", "{}", results[0]);
    assert_eq!(value["agent_id"], "agent-approved");
    assert_eq!(value["resumed"], true);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}
#[test]
fn login_resumes_pending_enrollment_and_keeps_polling_through_failed_polls() {
    let home = common::fixture();
    let unreachable = "https://127.0.0.1:9";
    seed_pending(&home, unreachable, 3.0);
    let output = run(
        &home,
        &["login", "--device", "vm"],
        &[("EVERETT_HOSTED_URL", unreachable)],
    );
    safe(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("approve code C0DE42"), "{stderr}");
    assert!(stderr.contains("resumed"), "{stderr}");
    assert!(stderr.contains("retrying"), "{stderr}");
    assert!(stderr.contains("expired"), "{stderr}");
    assert!(!output.status.success());
}
fn assert_stale_enrollment_is_replaced(seed_file: fn(&common::Fixture) -> std::path::PathBuf) {
    let unreachable = "https://127.0.0.1:9";
    let home = common::fixture();
    let path = seed_file(&home);
    let output = run(
        &home,
        &["login", "--device", "vm"],
        &[("EVERETT_HOSTED_URL", unreachable)],
    );
    safe(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("another deployment"), "{stderr}");
    assert!(!stderr.contains("No valid pending"), "{stderr}");
    assert!(stderr.contains("request failed"), "{stderr}");
    assert!(!path.exists(), "login must discard the stale enrollment");

    let path = seed_file(&home);
    let results = common::mcp_calls(
        &home,
        &[],
        &[("EVERETT_HOSTED_URL", unreachable)],
        &[("everett_agent_relay", json!({"action":"enroll","name":"vm"}))],
    );
    let text = results[0].to_string();
    safe_text(&text);
    assert!(!text.contains("another deployment"), "{text}");
    assert!(!text.contains("No valid pending"), "{text}");
    assert!(!path.exists(), "MCP enroll must discard the stale enrollment");
}
#[test]
fn enrollment_from_another_deployment_is_replaced_not_fatal() {
    assert_stale_enrollment_is_replaced(|home| seed_pending(home, "https://other.convex.cloud", 300.0));
}
#[test]
fn corrupt_enrollment_file_is_replaced_not_fatal() {
    assert_stale_enrollment_is_replaced(|home| {
        let path = home.write(".everett/enrollment.json", "{not json");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    });
}
#[test]
fn expired_enrollment_from_another_deployment_is_replaced_not_fatal() {
    assert_stale_enrollment_is_replaced(|home| seed_pending(home, "https://other.convex.cloud", -1.0));
}
