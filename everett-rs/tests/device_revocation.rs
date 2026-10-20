mod common;

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use ed25519_dalek::{Signer, SigningKey};
use everett::{doctor, relay};
use serde_json::{json, Value};
use std::fs;

fn sign_in(home: &std::path::Path, agent: &str) -> String {
    let issuer = "https://issuer.example/";
    let owner = json!([issuer, "org", "org-a"]).to_string();
    let claims = json!({"iss":issuer,"sub":"alice","aud":"client","exp":everett::session::now()+3600.0,"everett_org":"org-a","everett_agent":agent});
    everett::auth::save(&everett::auth::Stored {
        issuer: issuer.into(),
        client_id: "client".into(),
        sub: "alice".into(),
        email: String::new(),
        name: String::new(),
        device_name: "fixture".into(),
        access_token: "synthetic".into(),
        id_token: format!(
            "e30.{}.synthetic",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        ),
        refresh_token: String::new(),
        expires_at: everett::session::now() + 3600.0,
        scope: String::new(),
    })
    .unwrap();
    fs::create_dir_all(home.join(".everett/peer")).unwrap();
    fs::write(home.join(".everett/peer/device_id"), "local-ssh-id").unwrap();
    std::env::set_var("EVERETT_CONVEX_URL", "https://test.convex.cloud");
    owner
}

fn rows(value: Value) -> everett::error::Result<Vec<relay::RevokedDevice>> {
    relay::parse_revoked_devices(value)
}

#[test]
fn doctor_prunes_exact_revoked_tags_and_preserves_bytes_and_backups() {
    let home = common::lib_home();
    sign_in(&home.path(), "local-ssh-id");
    fs::create_dir_all(home.path().join(".ssh")).unwrap();
    let before = "# unrelated\r\nssh-ed25519 foreign user\r\ncommand=\"everett\" ssh-ed25519 a # everett-peer:revoked-a\r\nssh-ed25519 b # everett-peer:revoked-b\nssh-ed25519 c # everett-peer:revoked-a-extra\nssh-ed25519 d # everett-peer:active\nlast line without newline";
    let file = home.path().join(".ssh/authorized_keys");
    fs::write(&file, before).unwrap();
    let check = || {
        doctor::device_revocation_checks(|| {
            rows(json!([
                {"deviceId":"revoked-a","revokedAt":1000.0},
                {"deviceId":"revoked-b","revokedAt":2000.0},
                {"deviceId":"absent","revokedAt":3000.0}
            ]))
        })
    };
    let checks = check();
    assert!(checks
        .iter()
        .any(|(ok, text)| ok.is_none() && text.contains("IDs match")));
    assert_eq!(
        checks
            .iter()
            .filter(|(ok, text)| *ok == Some(true) && text.starts_with("removed SSH"))
            .count(),
        2
    );
    assert_eq!(fs::read(&file).unwrap(), b"# unrelated\r\nssh-ed25519 foreign user\r\nssh-ed25519 c # everett-peer:revoked-a-extra\nssh-ed25519 d # everett-peer:active\nlast line without newline");
    let backups: Vec<_> = fs::read_dir(home.path().join(".ssh"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".bak"))
        .collect();
    assert_eq!(backups.len(), 2);
    assert!(backups
        .iter()
        .any(|e| fs::read(e.path()).unwrap() == before.as_bytes()));
    assert!(!check()
        .iter()
        .any(|(_, text)| text.starts_with("removed SSH")));
    assert_eq!(fs::read_dir(home.path().join(".ssh")).unwrap().count(), 4);
}

#[test]
fn doctor_offline_missing_config_mismatched_ids_and_rewrite_failure() {
    let home = common::lib_home();
    sign_in(&home.path(), "different-relay-id");
    let checks =
        doctor::device_revocation_checks(|| Err(everett::error::EverettError::new(5, "offline")));
    assert!(checks.iter().any(|(ok, text)| ok.is_none()
        && text.contains("IDs differ")
        && text.contains("local-ssh-id")
        && text.contains("different-relay-id")));
    assert!(checks
        .iter()
        .any(|(ok, text)| ok.is_none() && text.contains("offline")));
    assert!(!home.path().join(".ssh/authorized_keys").exists());
    std::env::remove_var("EVERETT_CONVEX_URL");
    let checks = doctor::device_revocation_checks(|| panic!("unconfigured relay must not fetch"));
    assert!(checks
        .iter()
        .any(|(ok, text)| ok.is_none() && text.contains("relay not configured")));
    std::env::set_var("EVERETT_CONVEX_URL", "https://test.convex.cloud");
    fs::create_dir_all(home.path().join(".ssh")).unwrap();
    let file = home.path().join(".ssh/authorized_keys");
    fs::write(&file, b"\xff\nssh-ed25519 a # everett-peer:revoked\n").unwrap();
    let checks =
        doctor::device_revocation_checks(|| rows(json!([{"deviceId":"revoked","revokedAt":1000}])));
    assert!(checks.iter().any(|(ok, text)| *ok == Some(false)
        && text.contains("revoked")
        && text.contains("UTF-8")));
    assert_eq!(
        fs::read(file).unwrap(),
        b"\xff\nssh-ed25519 a # everett-peer:revoked\n"
    );
}

#[test]
fn doctor_without_sign_in_is_informational_and_does_not_touch_ssh() {
    let fixture = common::fixture();
    let file = fixture.write(
        ".ssh/authorized_keys",
        "ssh-ed25519 a # everett-peer:revoked\n",
    );
    let out = fixture.run(&["doctor"]);
    let text = fixture.stdout(&out);
    assert!(out.status.success(), "{text}");
    assert!(
        text.contains("[--  ] device revocations: not signed in"),
        "{text}"
    );
    assert_eq!(
        fs::read(file).unwrap(),
        b"ssh-ed25519 a # everett-peer:revoked\n"
    );
    assert_eq!(
        fs::read_dir(fixture.home().join(".ssh")).unwrap().count(),
        1
    );
}

#[test]
fn devices_revoke_serializes_sdk_mutation_and_removes_only_matching_local_pins() {
    use convex_sync_types::ClientMessage;
    let home = common::lib_home();
    let owner = sign_in(&home.path(), "local");
    let ssh = home.path().join(".everett/peer");
    for (name, id) in [("lost", "target"), ("keep", "other")] {
        fs::write(ssh.join(format!("{name}.json")), json!({"address":"offline.example","port":22,"device_id":id,"host_fingerprint":"SHA256:test"}).to_string()).unwrap();
        fs::write(ssh.join(format!("{name}.known_hosts")), "pin\n").unwrap();
    }
    let relay_dir = home.path().join(".everett/relay/peers");
    fs::create_dir_all(&relay_dir).unwrap();
    let device = relay::crypto::Keys::new(owner.clone(), "target".into(), [1; 32], [2; 32]).device;
    for (name, deployment) in [
        ("relay-lost", "https://test.convex.cloud"),
        ("other-deployment", "https://other.convex.cloud"),
    ] {
        fs::write(
            relay_dir.join(format!("{name}.json")),
            json!({"device":device,"deployment":deployment,"grant_id":null}).to_string(),
        )
        .unwrap();
    }
    let result = relay::revoke_device_with(
        "target",
        &owner,
        "https://test.convex.cloud",
        |name, arguments, is_query| {
            assert!(!is_query);
            let mut client = convex::base_client::BaseConvexClient::new();
            let args = arguments
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), convex::Value::try_from(v.clone()).unwrap()))
                .collect();
            let _pending = client.mutation(name.parse().unwrap(), args);
            let Some(ClientMessage::Mutation { udf_path, args, .. }) = client.pop_next_message()
            else {
                panic!("expected SDK mutation")
            };
            assert_eq!(udf_path.to_string(), "relay:revokeDevice");
            assert_eq!(
                serde_json::from_str::<Value>(args.get()).unwrap(),
                json!([{"deviceId":"target"}])
            );
            Ok(Value::Null)
        },
    )
    .unwrap();
    assert_eq!(result["revoked"], "target");
    assert_eq!(result["removed_peers"], json!(["lost", "relay-lost"]));
    assert!(!ssh.join("lost.json").exists());
    assert!(!ssh.join("lost.known_hosts").exists());
    assert_eq!(fs::read(ssh.join("keep.known_hosts")).unwrap(), b"pin\n");
    assert!(relay_dir.join("other-deployment.json").exists());
    assert!(!relay_dir.join("relay-lost.json").exists());
}

#[test]
fn denied_revoke_preserves_local_peer_and_invalid_response_never_prunes() {
    let home = common::lib_home();
    let owner = sign_in(&home.path(), "local");
    let path = home.path().join(".everett/peer/lost.json");
    fs::write(&path, "untouched").unwrap();
    assert!(
        relay::revoke_device_with(
            "target",
            &owner,
            "https://test.convex.cloud",
            |_, _, _| Err(everett::error::EverettError::new(2, "denied"))
        )
        .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), b"untouched");
    for value in [
        json!([{"deviceId":"../bad","revokedAt":1}]),
        json!([{"deviceId":"valid","revokedAt":null}]),
        json!([{"deviceId":"valid","revokedAt":1.5}]),
        json!([{"deviceId":"valid","revokedAt":1,"key":"not allowed"}]),
    ] {
        assert!(rows(value).is_err());
    }
}

#[test]
fn cli_exposes_device_revoke_and_best_effort_logout() {
    let fixture = common::fixture();
    let out = fixture.run(&["devices", "revoke", "--help"]);
    assert!(out.status.success());
    assert!(fixture.stdout(&out).contains("<DEVICE_ID>"));
    let out = fixture.run(&["logout", "--revoke-device", "--json"]);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"signed_out": true, "revoked": false, "device_revoked": false})
    );
    fixture.write(".everett/auth.json", &json!({"issuer":"http://127.0.0.1:1","client_id":"fixture","sub":"alice","email":"fixture","access_token":"synthetic","expires_at":0}).to_string());
    let out = fixture.run(&["logout", "--revoke-device", "--json"]);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["device_revoked"],
        false
    );
    assert!(!fixture.home().join(".everett/auth.json").exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("registry device revocation"));
}

fn ssh_key(seed: u8) -> (SigningKey, String) {
    let signing = SigningKey::from_bytes(&[seed; 32]);
    let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
    blob.extend(signing.verifying_key().to_bytes());
    (signing, STANDARD.encode(blob))
}

fn revoked_row(owner: &str, device: &str, ssh: &str, key: &str, signer: &SigningKey) -> Value {
    let signature = signer.sign(&relay::ssh_binding_bytes(owner, device, ssh, key));
    json!({"deviceId":device,"revokedAt":1000,"sshDeviceId":ssh,"sshPublicKey":key,"sshSignature":hex::encode(signature.to_bytes())})
}

#[test]
fn ssh_binding_bytes_match_the_convex_backend() {
    let bytes = relay::ssh_binding_bytes(r#"["i","org","o"]"#, "agent", "ssh-id", "AAAA");
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        r#"["everett-ssh-bind-v1","[\"i\",\"org\",\"o\"]","agent","ssh-id","AAAA"]"#
    );
}

#[test]
fn doctor_prunes_the_proven_ssh_binding_of_a_revoked_relay_agent() {
    let home = common::lib_home();
    let owner = sign_in(&home.path(), "relay-agent-local");
    let (laptop, laptop_key) = ssh_key(1);
    fs::create_dir_all(home.path().join(".ssh")).unwrap();
    let file = home.path().join(".ssh/authorized_keys");
    fs::write(&file, format!("ssh-ed25519 {laptop_key} # everett-peer:ssh-of-laptop\nssh-ed25519 b # everett-peer:keep\n")).unwrap();
    let row = revoked_row(
        &owner,
        "relay-agent-laptop",
        "ssh-of-laptop",
        &laptop_key,
        &laptop,
    );
    let checks = doctor::device_revocation_checks(|| rows(json!([row])));
    assert!(
        checks
            .iter()
            .any(|(ok, t)| *ok == Some(true) && t.contains("relay-agent-laptop")),
        "{checks:?}"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "ssh-ed25519 b # everett-peer:keep\n"
    );
    assert!(rows(json!([{"deviceId":"x","sshDeviceId":"bad id!","revokedAt":1}])).is_err());
    assert!(rows(json!([{"deviceId":"x","sshDeviceId":"ssh-of-laptop","revokedAt":1}])).is_err());
}

#[test]
fn self_revoked_device_claiming_a_victims_ssh_id_cannot_prune_the_victim() {
    let home = common::lib_home();
    let owner = sign_in(&home.path(), "relay-agent-local");
    let (victim, victim_key) = ssh_key(2);
    let (attacker, attacker_key) = ssh_key(3);
    fs::create_dir_all(home.path().join(".ssh")).unwrap();
    let file = home.path().join(".ssh/authorized_keys");
    let before = format!("ssh-ed25519 {victim_key} # everett-peer:ssh-victim\n");
    fs::write(&file, &before).unwrap();
    let claims = [
        revoked_row(
            &owner,
            "relay-agent-attacker",
            "ssh-victim",
            &attacker_key,
            &attacker,
        ),
        revoked_row(
            &owner,
            "relay-agent-attacker",
            "ssh-victim",
            &victim_key,
            &attacker,
        ),
        revoked_row(
            r#"["https://issuer.example/","org","org-b"]"#,
            "relay-agent-attacker",
            "ssh-victim",
            &victim_key,
            &victim,
        ),
    ];
    for claim in claims {
        let checks = doctor::device_revocation_checks(|| rows(json!([claim])));
        assert!(
            !checks.iter().any(|(_, t)| t.starts_with("removed")),
            "{checks:?}"
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), before);
    }
}

#[test]
fn local_ssh_identity_signs_a_binding_doctor_accepts() {
    let home = common::lib_home();
    let owner = sign_in(&home.path(), "relay-agent-local");
    fs::remove_file(home.path().join(".everett/peer/device_id")).unwrap();
    assert!(everett::peer::local_ssh_key().unwrap().is_none());
    let identity = everett::peer::identity().unwrap();
    let (ssh, key, signing) = everett::peer::local_ssh_key().unwrap().unwrap();
    assert_eq!(identity["device_id"], ssh.as_str());
    assert!(identity["public_key"].as_str().unwrap().contains(&key));
    let row = rows(json!([revoked_row(
        &owner,
        "relay-agent-local",
        &ssh,
        &key,
        &signing
    )]))
    .unwrap();
    assert_eq!(
        relay::verified_ssh_binding(&owner, &row[0]).unwrap(),
        Some((ssh.as_str(), key.as_str()))
    );
}

#[test]
fn revoked_devices_follow_the_cursor_to_the_last_page() {
    let mut seen = Vec::new();
    let all = relay::revoked_devices_with(|cursor| {
        seen.push(cursor.clone());
        Ok(match cursor.as_deref() {
            None => json!({"devices":[{"deviceId":"a","revokedAt":1}],"cursor":"p2"}),
            Some("p2") => json!({"devices":[{"deviceId":"b","revokedAt":2}],"cursor":null}),
            other => panic!("unexpected cursor {other:?}"),
        })
    })
    .unwrap();
    assert_eq!(
        all.iter().map(|d| d.device_id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(seen, [None, Some("p2".to_string())]);
    let endless = relay::revoked_devices_with(|_| Ok(json!({"devices":[],"cursor":"again"})));
    assert!(endless.is_err());
}
