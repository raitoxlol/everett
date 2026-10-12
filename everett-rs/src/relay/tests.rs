use super::*;

fn devices() -> (Keys, Keys) {
    let owner = json!(["https://owner.example/", "alice"]).to_string();
    (
        Keys::new(owner.clone(), "a".into(), [1; 32], [2; 32]),
        Keys::new(owner, "b".into(), [3; 32], [4; 32]),
    )
}

fn payload(hops: i64) -> Payload {
    Payload::Inbox {
        session: "recipient".into(),
        harness: "codex".into(),
        text: "private message 日本語".into(),
        from: "sender".into(),
        from_harness: "claude".into(),
        from_card: String::new(),
        reply_to: String::new(),
        hops,
    }
}

#[test]
fn encrypted_round_trip_and_receipts() {
    let (a, b) = devices();
    for p in [
        payload(1),
        Payload::Delivered {
            message_id: "message-1".into(),
        },
    ] {
        let e = a.seal(&b.device, "key", &p, 1000, TTL).unwrap();
        assert_eq!(b.open(&a.device, &e, 1001).unwrap(), p);
        let stored = serde_json::to_string(&e).unwrap();
        assert!(!stored.contains("private message"));
        assert!(!stored.contains("recipient"));
    }
}

#[test]
fn ciphertext_signature_metadata_and_aead_tampering_rejected() {
    let (a, b) = devices();
    let original = a.seal(&b.device, "key", &payload(1), 1000, TTL).unwrap();
    let mut e = original.clone();
    e.ciphertext.replace_range(
        0..2,
        if e.ciphertext.starts_with("00") {
            "01"
        } else {
            "00"
        },
    );
    assert!(b.open(&a.device, &e, 1001).is_err());
    e.signature = a.sign(&e.signed_bytes());
    assert!(b
        .open(&a.device, &e, 1001)
        .unwrap_err()
        .message
        .contains("authentication"));
    let mut e = original;
    e.expires_at += 1;
    assert!(b.open(&a.device, &e, 1001).is_err());
    let mut device = a.device.clone();
    device.encryption_key = "00".repeat(32);
    assert!(device.verify().is_err());
}

#[test]
fn wrong_account_wrong_device_ttl_and_hop_limit_rejected() {
    let (a, b) = devices();
    let stranger = Keys::new("other-owner".into(), "c".into(), [5; 32], [6; 32]);
    assert!(a
        .seal(&stranger.device, "key", &payload(1), 1000, TTL)
        .is_err());
    let e = a.seal(&b.device, "key", &payload(1), 1000, TTL).unwrap();
    assert!(stranger.open(&a.device, &e, 1001).is_err());
    assert!(a.open(&a.device, &e, 1001).is_err());
    assert!(b.open(&a.device, &e, e.expires_at).is_err());
    assert_eq!(
        a.seal(&b.device, "key", &payload(4), 1000, TTL)
            .unwrap_err()
            .code,
        7
    );
    let mut too_big = payload(1);
    if let Payload::Inbox { text, .. } = &mut too_big {
        *text = "x".repeat(peer::BODY_CAP + 1);
    }
    assert!(a.seal(&b.device, "key", &too_big, 1000, TTL).is_err());
}

#[test]
fn javascript_and_rust_sign_the_same_wire_bytes() {
    let (a, b) = devices();
    let wire: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/relay-wire.json")).unwrap();
    let device: Device = serde_json::from_value(wire["device"].clone()).unwrap();
    assert_eq!(device, a.device);
    let envelope: Envelope = serde_json::from_value(wire["envelope"].clone()).unwrap();
    crypto::verify(
        &device.signing_key,
        &envelope.signature,
        &envelope.signed_bytes(),
    )
    .unwrap();
    assert_eq!(a.sign(&envelope.signed_bytes()), envelope.signature);
    let e = a.seal(&b.device, "key", &payload(1), 1000, TTL).unwrap();
    let value = serde_json::to_value(&e).unwrap();
    let args = args(json!({"envelope":value})).unwrap();
    let round_trip: Value = args["envelope"].clone().into();
    let decoded: Envelope = serde_json::from_value(round_trip).unwrap();
    assert_eq!(e, decoded);
    assert_eq!(e.signed_bytes(), decoded.signed_bytes());
}
