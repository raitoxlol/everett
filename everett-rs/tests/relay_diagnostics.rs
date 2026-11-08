mod common;

use common::{fixture, Fixture};
use fs2::FileExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{symlink, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Stdio};

fn command(f: &Fixture, args: &[&str]) -> Command {
    let mut command = Command::new(
        std::env::var_os("EVERETT_VERIFY_BINARY")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_everett").into()),
    );
    command
        .args(args)
        .env_clear()
        .env("HOME", f.home())
        .env("EVERETT_HOME", f.home())
        .env("EVERETT_NOTIFY", "none")
        .env("PATH", common::HERMETIC_PATH)
        .current_dir(f.home());
    command
}

fn private(f: &Fixture, path: &str, value: Value) {
    let path = f.write(path, &value.to_string());
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn diagnose(f: &Fixture) -> Value {
    let out = command(f, &["relay", "diagnose"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

fn contents(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_string_lossy().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[test]
fn fresh_home_diagnostics_do_not_create_state_or_require_sign_in() {
    let f = fixture();
    let before = contents(&f.home());
    let value = diagnose(&f);
    assert_eq!(value["offline"], true);
    assert_eq!(value["receiver"]["state"], "stopped");
    assert_eq!(value["registration"]["credential"], "missing");
    assert_eq!(value["outgoing"]["scanned"], 0);
    assert_eq!(value["webhook"]["state"], "not_configured");
    assert_eq!(contents(&f.home()), before);
    assert!(!f.home().join(".everett").exists());
}

#[test]
fn cli_and_mcp_report_cached_state_without_network_secrets_or_state_changes() {
    let f = fixture();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    f.write(".everett/config.toml", "[auth]\nprovider='cloudflare'\n");
    private(
        &f,
        ".everett/cloudflare-auth.json",
        json!({
            "issuer":endpoint,"owner":"private-owner","agent_id":"agent-test","device_name":"private-device",
            "token":"private-native-token","expires_at":u64::MAX
        }),
    );
    let root = f.home().join(".everett/relay-cloudflare");
    fs::create_dir_all(&root).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("daemon.lock"))
        .unwrap();
    lock.try_lock_exclusive().unwrap();
    let lease = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("receiver-capabilities.lock"))
        .unwrap();
    lease.try_lock_exclusive().unwrap();
    private(
        &f,
        ".everett/relay-cloudflare/receiver-status.json",
        json!({
            "schema":1,"owner":"private-owner","device":"agent-test","capabilities":["same-owner-webhook-v1","private-capability-token"]
        }),
    );
    private(
        &f,
        ".everett/external/ext-test.json",
        json!({
            "id":"ext-test","harness":"grok-bot","title":"private-title","cwd":"/private-location","updated":1
        }),
    );
    private(
        &f,
        ".everett/relay-cloudflare/receiver-webhook.json",
        json!({
            "version":1,"enabled":true,"activated_at":1,"rules":[{
                "id":"test","owner":"private-owner","recipient_device":"agent-test","session":"ext-test",
                "harness":"grok-bot","cooldown_seconds":30,"url":"https://example.com/private-webhook-path",
                "bearer_file":"/never-open-private-bearer"
            }]
        }),
    );
    private(
        &f,
        ".everett/relay-cloudflare/receiver-webhook/ledger.json",
        json!({
            "version":1,"rules":{},"messages":{"private-message-id":{
                "rule_id":"test","session":"ext-test","harness":"grok-bot","state":"pending","attempts":1,"at":1
            }}
        }),
    );
    let before = contents(&f.home());
    let cli = diagnose(&f);
    assert_eq!(contents(&f.home()), before);
    assert_eq!(cli["receiver"]["state"], "running");
    assert_eq!(cli["registration"]["credential"], "cached");
    assert_eq!(cli["external"]["readable"], 1);
    assert_eq!(cli["webhook"]["rules"], 1);
    assert_eq!(cli["webhook"]["ledger"]["pending"], 1);
    let input = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"diagnostics-test","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"everett_agent_relay","arguments":{"action":"diagnose"}}})
    ].iter().map(|row| format!("{row}\n")).collect::<String>();
    let mut child = command(&f, &["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let responses: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let result = &responses.iter().find(|row| row["id"] == 2).unwrap()["result"];
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result["structuredContent"], cli);
    for (path, bytes) in before {
        assert_eq!(fs::read(f.home().join(path)).unwrap(), bytes);
    }
    let encoded = format!("{cli}{responses:?}{}", String::from_utf8_lossy(&out.stderr));
    for secret in [
        "private-owner",
        "private-device",
        "private-native-token",
        "private-title",
        "private-location",
        "private-webhook-path",
        "never-open-private-bearer",
        "private-message-id",
        "private-capability-token",
    ] {
        assert!(!encoded.contains(secret), "diagnostics exposed {secret}");
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    private(
        &f,
        ".everett/relay-cloudflare/receiver-status.json",
        json!({
            "schema":1,"owner":"another-owner","device":"agent-test","capabilities":[]
        }),
    );
    assert_eq!(diagnose(&f)["receiver"]["state"], "identity-mismatch");
    drop(lock);
    assert_eq!(diagnose(&f)["receiver"]["state"], "stopped");
}

#[test]
fn cached_message_counts_preserve_replies_after_request_expiry() {
    use everett::relay::{crypto::Keys, Payload};
    let f = fixture();
    let sender = Keys::new("owner".into(), "sender".into(), [1; 32], [2; 32]);
    let recipient = Keys::new("owner".into(), "recipient".into(), [3; 32], [4; 32]);
    let now = (everett::session::now() * 1000.0) as i64;
    let body = Payload::Inbox {
        session: "test".into(),
        harness: "codex".into(),
        text: "private-message-body".into(),
        from: "human".into(),
        from_harness: String::new(),
        from_card: String::new(),
        reply_to: String::new(),
        hops: 1,
        external_untrusted: false,
    };
    for (index, (state, posted, age)) in [
        ("queued", false, 0),
        ("queued", true, 0),
        ("queued", false, 120_000),
        ("delivered", true, 120_000),
        ("replied", true, 120_000),
    ]
    .iter()
    .enumerate()
    {
        let envelope = sender
            .seal(
                &recipient.device,
                &format!("message-{index}"),
                &body,
                now - age,
                60_000,
                None,
                None,
            )
            .unwrap();
        private(
            &f,
            &format!(".everett/relay-cloudflare/outgoing/{index}.json"),
            json!({
                "peer":"private-peer","envelope":envelope,"digest":"private-digest","session":"test","harness":"codex","posted":posted,"state":state
            }),
        );
    }
    let before = contents(&f.home());
    let value = diagnose(&f);
    assert_eq!(
        value["outgoing"]["states"],
        json!({"unconfirmed":1,"queued":1,"expired":1,"delivered":1,"replied":1})
    );
    assert_eq!(value["outgoing"]["readable"], 5);
    assert_eq!(contents(&f.home()), before);
    assert!(!value.to_string().contains("private-"));
}

#[test]
fn diagnostics_bound_cache_scans_and_reject_unsafe_records_without_repairing_them() {
    let f = fixture();
    private(
        &f,
        ".everett/relay-cloudflare/outgoing/bad.json",
        json!({"state":"private-parser-error"}),
    );
    let outside = f.write("outside.json", "{\"private\":true}");
    symlink(
        outside,
        f.home()
            .join(".everett/relay-cloudflare/outgoing/link.json"),
    )
    .unwrap();
    let large = f.write(
        ".everett/relay-cloudflare/ingress/large.json",
        &"x".repeat(256 * 1024 + 1),
    );
    fs::set_permissions(large, fs::Permissions::from_mode(0o600)).unwrap();
    let value = diagnose(&f);
    assert_eq!(value["outgoing"]["unreadable"], 2);
    assert_eq!(value["ingress"]["unreadable"], 1);
    assert!(f
        .home()
        .join(".everett/relay-cloudflare/outgoing/link.json")
        .is_symlink());
    for n in 0..300 {
        private(
            &f,
            &format!(".everett/relay-cloudflare/outgoing/{n}.json"),
            json!({}),
        );
    }
    let value = diagnose(&f);
    assert_eq!(value["outgoing"]["scanned"], 256);
    assert_eq!(value["outgoing"]["truncated"], true);
    assert!(!value.to_string().contains("private-parser-error"));
}

fn diagnose_within(f: &Fixture, limit: std::time::Duration) -> Value {
    let mut child = command(f, &["relay", "diagnose"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > limit {
            child.kill().ok();
            panic!("relay diagnose blocked on configuration");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn diagnostics_bound_configuration_reads() {
    let f = fixture();
    let config = f.home().join(".everett/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();

    // A FIFO must not block diagnostics.
    let status = Command::new("mkfifo").arg(&config).status().unwrap();
    assert!(status.success());
    let value = diagnose_within(&f, std::time::Duration::from_secs(10));
    assert_eq!(value["registration"]["credential"], "config_unreadable");
    fs::remove_file(&config).unwrap();

    // A directive beyond the read bound must not influence the result.
    let mut text = "#".repeat(300 * 1024);
    text.push_str("\n[auth]\nprovider = \"other\"\n");
    fs::write(&config, text).unwrap();
    let value = diagnose_within(&f, std::time::Duration::from_secs(10));
    assert_eq!(value["registration"]["credential"], "config_unreadable");
}
