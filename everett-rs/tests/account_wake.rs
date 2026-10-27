mod common;

use common::{fixture, Fixture};
use fs2::FileExt;
use serde_json::{json, Value};
use std::os::unix::fs::OpenOptionsExt;
use std::{fs, os::unix::fs::PermissionsExt};

const OWNER: &str = "[\"https://issuer.example\",\"account\",\"github:1\"]";
const POLICY: &str = ".everett/relay-cloudflare/receiver-webhook.json";
const ARGS: &[&str] = &[
    "relay",
    "trust-account",
    "ext-hugo",
    "--harness",
    "grok-bot",
];

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

#[test]
fn cli_trusts_account_without_exposing_or_changing_credentials_or_other_rules() {
    let f = setup();
    let path = f.home().join(POLICY);
    let mut expected: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut unrelated = expected["rules"][0].clone();
    unrelated["id"] = json!("other-inbox");
    unrelated["session"] = json!("ext-other");
    expected["rules"].as_array_mut().unwrap().push(unrelated);
    save_policy(&f, &expected);
    let credentials = fs::read(f.home().join(".everett/cloudflare-auth.json")).unwrap();
    let key = fs::read(f.home().join("webhook.key")).unwrap();

    for field in ["sender_device", "sender_session", "sender_harness"] {
        expected["rules"][0].as_object_mut().unwrap().remove(field);
    }
    for _ in 0..2 {
        let out = f.run(ARGS);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = f.stdout(&out);
        let result: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(result["senders"], "same-owner");
        assert!(result["receiver_requirement"]
            .as_str()
            .unwrap()
            .contains("does not upgrade or restart"));
        assert!(!text.contains("fake-private-key"));
        assert!(!text.contains("private-webhook-path"));
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
            expected
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        fs::read(f.home().join(".everett/cloudflare-auth.json")).unwrap(),
        credentials
    );
    assert_eq!(fs::read(f.home().join("webhook.key")).unwrap(), key);
    assert!(!f
        .home()
        .join(".everett/relay-cloudflare/receiver-webhook/ledger.json")
        .exists());
}

#[test]
fn cli_rejects_missing_registration_disabled_policy_and_wrong_account_without_rewriting() {
    for case in [
        "registration",
        "disabled",
        "account",
        "device",
        "unsigned-in",
    ] {
        let f = setup();
        let path = f.home().join(POLICY);
        let mut policy: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match case {
            "registration" => assert!(f
                .run(&["external", "remove", "--id", "ext-hugo"])
                .status
                .success()),
            "disabled" => policy["enabled"] = json!(false),
            "account" => {
                policy["rules"][0]["owner"] =
                    json!("[\"https://other.example\",\"account\",\"github:2\"]")
            }
            "device" => policy["rules"][0]["recipient_device"] = json!("agent-other"),
            "unsigned-in" => {
                fs::remove_file(f.home().join(".everett/cloudflare-auth.json")).unwrap();
            }
            _ => unreachable!(),
        }
        save_policy(&f, &policy);
        let before = fs::read(&path).unwrap();
        let out = f.run(ARGS);
        assert!(!out.status.success(), "{case}");
        assert_eq!(fs::read(&path).unwrap(), before, "{case}");
    }
}

#[test]
fn an_unverified_running_receiver_cannot_be_given_an_incompatible_policy() {
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
    let before = fs::read(root.join("receiver-webhook.json")).unwrap();
    let out = f.run(ARGS);
    assert!(
        !out.status.success(),
        "legacy receiver must block migration"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("upgrade and restart"));
    assert_eq!(
        fs::read(root.join("receiver-webhook.json")).unwrap(),
        before
    );
}

#[test]
fn dry_run_changes_no_files_and_actual_change_retains_exact_private_backup() {
    let f = setup();
    let root = f.home().join(".everett/relay-cloudflare");
    let original = fs::read(root.join("receiver-webhook.json")).unwrap();
    let mut args = ARGS.to_vec();
    args.push("--dry-run");
    let out = f.run(&args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value = serde_json::from_str(&f.stdout(&out)).unwrap();
    assert_eq!(value["state"], "planned");
    assert_eq!(value["changed"], true);
    assert_eq!(value["backup_created"], false);
    assert_eq!(
        fs::read(root.join("receiver-webhook.json")).unwrap(),
        original
    );
    assert_eq!(
        fs::read_dir(&root).unwrap().count(),
        1,
        "preview created files"
    );

    let out = f.run(ARGS);
    assert!(out.status.success());
    let value: Value = serde_json::from_str(&f.stdout(&out)).unwrap();
    assert_eq!(value["backup_created"], true);
    let backup = fs::read_dir(&root)
        .unwrap()
        .flatten()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("receiver-webhook.before-account-")
        })
        .unwrap()
        .path();
    assert_eq!(fs::read(&backup).unwrap(), original);
    assert_eq!(
        fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let out = f.run(ARGS);
    let value: Value = serde_json::from_str(&f.stdout(&out)).unwrap();
    assert_eq!(value["changed"], false);
    assert_eq!(value["backup_created"], false);
    assert_eq!(fs::read(&backup).unwrap(), original);
}

#[test]
fn unsafe_or_corrupted_existing_backups_prevent_changes_instead_of_being_overwritten() {
    let f = setup();
    let root = f.home().join(".everett/relay-cloudflare");
    let original = fs::read(root.join("receiver-webhook.json")).unwrap();
    assert!(f.run(ARGS).status.success());
    let backup = fs::read_dir(&root)
        .unwrap()
        .flatten()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("receiver-webhook.before-account-")
        })
        .unwrap()
        .path();
    fs::write(root.join("receiver-webhook.json"), &original).unwrap();
    for invalid in ["malformed", "public", "symlink"] {
        match invalid {
            "malformed" => fs::write(&backup, b"{bad-private-backup").unwrap(),
            "public" => {
                fs::write(&backup, &original).unwrap();
                fs::set_permissions(&backup, fs::Permissions::from_mode(0o644)).unwrap();
            }
            "symlink" => {
                fs::remove_file(&backup).unwrap();
                let outside = f.write("outside-sentinel", "do-not-overwrite");
                std::os::unix::fs::symlink(outside, &backup).unwrap();
            }
            _ => unreachable!(),
        }
        let out = f.run(ARGS);
        assert!(!out.status.success(), "{invalid}");
        assert_eq!(
            fs::read(root.join("receiver-webhook.json")).unwrap(),
            original
        );
        assert!(!String::from_utf8_lossy(&out.stderr).contains("bad-private-backup"));
    }
    assert_eq!(
        fs::read_to_string(f.home().join("outside-sentinel")).unwrap(),
        "do-not-overwrite"
    );
}
