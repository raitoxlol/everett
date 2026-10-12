mod common;

use everett::{
    inbox,
    relay::{self, crypto::Keys, Payload},
};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn put(home: &Path, relative: &str, value: &impl serde::Serialize) {
    let path = home.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn select(home: &Path) {
    std::env::set_var("EVERETT_HOME", home);
    std::env::set_var("HOME", home);
}

#[test]
fn two_device_encrypted_inbox_pickup_receipt_and_reply_without_network_or_wake() {
    let guard = common::lib_home();
    let a_home = guard.path().join("a");
    let b_home = guard.path().join("b");
    let owner = json!(["https://owner.example/", "alice"]).to_string();
    let a = Keys::new(owner.clone(), "device-a".into(), [1; 32], [2; 32]);
    let b = Keys::new(owner.clone(), "device-b".into(), [3; 32], [4; 32]);
    std::env::set_var("EVERETT_CONVEX_URL", "https://test.convex.cloud");
    for (home, remote) in [(&a_home, &b), (&b_home, &a)] {
        put(
            home,
            ".everett/relay/peers/remote.json",
            &json!({"device":remote.device,"deployment":"https://test.convex.cloud"}),
        );
    }
    let transcript = include_str!("fixtures/codex.jsonl");
    let transcript_path = b_home.join(".codex/sessions/2026/10/12/x-1.jsonl");
    fs::create_dir_all(transcript_path.parent().unwrap()).unwrap();
    fs::write(transcript_path, transcript).unwrap();
    let now = (everett::session::now() * 1000.0) as i64;
    let request = a
        .seal(
            &b.device,
            "request",
            &Payload::Inbox {
                session: "x-1".into(),
                harness: "codex".into(),
                text: "private request".into(),
                from: "c-1".into(),
                from_harness: "claude".into(),
                from_card: String::new(),
                reply_to: String::new(),
                hops: 1,
            },
            now,
            3600000,
        )
        .unwrap();
    put(
        &a_home,
        ".everett/relay/outgoing/request.json",
        &json!({
            "peer":"remote","envelope":request,"digest":"test","session":"c-1","harness":"claude","posted":true,"state":"queued"
        }),
    );

    select(&b_home);
    relay::import(&b, &request, now).unwrap();
    relay::import(&b, &request, now).unwrap();
    assert_eq!(inbox::pending("x-1", None).len(), 1);
    assert!(!inbox::done_ids("x-1").contains(&request.message_id));
    let message = inbox::find_in("x-1", &request.message_id).unwrap();
    assert_eq!(message["relay_sender"], "c-1");
    assert_eq!(message["from_harness"], "claude");
    assert!(inbox::take("x-1").contains("private request"));
    assert!(inbox::take("x-1").is_empty());
    assert!(inbox::done_ids("x-1").contains(&request.message_id));

    let receipt = b
        .seal(
            &a.device,
            "receipt",
            &Payload::Delivered {
                message_id: request.message_id.clone(),
            },
            now,
            3600000,
        )
        .unwrap();
    select(&a_home);
    relay::import(&a, &receipt, now).unwrap();
    let outgoing = a_home.join(".everett/relay/outgoing/request.json");
    let state =
        || serde_json::from_slice::<Value>(&fs::read(&outgoing).unwrap()).unwrap()["state"].clone();
    assert_eq!(state(), "delivered");
    assert!(inbox::read("c-1").is_empty());

    let reply = b
        .seal(
            &a.device,
            "reply",
            &Payload::Inbox {
                session: "c-1".into(),
                harness: "claude".into(),
                text: "private reply".into(),
                from: "x-1".into(),
                from_harness: "codex".into(),
                from_card: String::new(),
                reply_to: request.message_id.clone(),
                hops: 2,
            },
            now,
            3600000,
        )
        .unwrap();
    relay::import(&a, &reply, now).unwrap();
    relay::import(&a, &reply, now).unwrap();
    assert_eq!(state(), "replied");
    assert_eq!(inbox::pending("c-1", None).len(), 1);
    assert_eq!(
        inbox::find_in("c-1", &reply.message_id).unwrap()["reply_to"],
        request.message_id
    );
    assert!(inbox::take("c-1").contains("private reply"));
    relay::import(&a, &receipt, now).unwrap();
    assert_eq!(state(), "replied", "late receipt cannot downgrade a reply");

    let mut tampered = reply;
    tampered.ciphertext.replace_range(0..2, "ff");
    assert!(relay::import(&a, &tampered, now).is_err());
    assert_eq!(inbox::read("c-1").len(), 1);
    fs::remove_file(a_home.join(".everett/relay/peers/remote.json")).unwrap();
    assert!(
        relay::import(&a, &receipt, now).is_err(),
        "unpaired sender cannot inject"
    );
    std::env::remove_var("EVERETT_CONVEX_URL");
}

#[test]
fn relay_cli_is_explicit_and_local_peer_listing_needs_no_login() {
    let f = common::fixture();
    for args in [
        vec!["relay", "--help"],
        vec!["peer", "add-relay", "--help"],
        vec!["peer", "list"],
    ] {
        let out = f.run(&args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let out = f.run(&["relay", "register"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("login"));
}
