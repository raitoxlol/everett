mod common;

use common::{fixture, Fixture};
use fs2::FileExt;
use serde_json::{json, Value};
use std::os::unix::fs::OpenOptionsExt;
use std::{fs, os::unix::fs::PermissionsExt};

const OWNER: &str = "[\"https://issuer.example\",\"account\",\"github:1\"]";
const POLICY: &str = ".everett/relay-cloudflare/receiver-webhook.json";

fn setup() -> Fixture {
    let f = fixture();
    f.write(".everett/config.toml", "[auth]\nprovider='cloudflare'\n");
    f.write(
        ".everett/cloudflare-auth.json",
        &json!({
            "issuer": "https://issuer.example",
            "owner": OWNER,
            "agent_id": "agent-hugo",
            "device_name": "Hugo",
            "token": format!("evt_{}", "a".repeat(64)),
            "expires_at": (everett::session::now() * 1000.0) as u64 + 120000,
        })
        .to_string(),
    );
    let out = f.run(&[
        "external",
        "add",
        "--id",
        "ext-hugo",
        "--harness",
        "grok-bot",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bearer = f.write("webhook.key", "fake-private-key");
    fs::set_permissions(&bearer, fs::Permissions::from_mode(0o600)).unwrap();
    let policy = json!({
        "version": 1,
        "enabled": true,
        "activated_at": 1,
        "rules": [{
            "id": "hugo-inbox",
            "owner": OWNER,
            "recipient_device": "agent-hugo",
            "session": "ext-hugo",
            "harness": "grok-bot",
            "sender_device": "agent-max",
            "sender_session": "human",
            "sender_harness": "",
            "cooldown_seconds": 30,
            "url": "https://example.com/private-webhook-path",
            "bearer_file": bearer,
        }],
    });
    save_policy(&f, &policy);
    f
}

fn save_policy(f: &Fixture, policy: &Value) {
    let path = f.write(POLICY, &policy.to_string());
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn legacy_receiver_guard_also_protects_new_webhook_setup() {
    let f = setup();
    let root = f.home().join(".everett/relay-cloudflare");
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("daemon.lock"))
        .unwrap();
    lock.try_lock_exclusive().unwrap();
    let url = f.write("webhook.url", "https://example.com/private-webhook-path");
    fs::set_permissions(&url, fs::Permissions::from_mode(0o600)).unwrap();
    let bearer = f.home().join("webhook.key");
    let before = fs::read(root.join("receiver-webhook.json")).unwrap();
    let out = f.run(&[
        "relay",
        "webhook",
        "ext-hugo",
        "--harness",
        "grok-bot",
        "--url-file",
        url.to_str().unwrap(),
        "--bearer-file",
        bearer.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("upgrade and restart"));
    assert_eq!(
        fs::read(root.join("receiver-webhook.json")).unwrap(),
        before
    );
}
