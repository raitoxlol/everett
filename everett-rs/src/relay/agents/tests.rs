use super::*;

fn identity(subject: &str, org: &str, agent: &str) -> Identity {
    Identity {
        owner: json!(["https://owner.example/", subject]).to_string(),
        org: org.repeat(64),
        agent_id: Some(agent.repeat(64)),
        credential_id: Some("c".repeat(64)),
        admin: false,
        deployment: "https://test.convex.cloud".into(),
    }
}

fn devices() -> (Identity, Keys, Identity, Keys) {
    let a = identity("alice", "a", "1");
    let b = identity("bob", "b", "2");
    let ka = Keys::new(a.owner.clone(), "alice-device".into(), [1; 32], [2; 32]);
    let kb = Keys::new(b.owner.clone(), "bob-device".into(), [3; 32], [4; 32]);
    (a, ka, b, kb)
}

fn payload() -> Payload {
    Payload::Inbox {
        session: "recipient".into(),
        harness: "pi".into(),
        text: "private agent 日本語".into(),
        from: "sender".into(),
        from_harness: "omp".into(),
        from_card: "private card".into(),
        reply_to: String::new(),
        hops: 1,
    }
}

#[test]
fn agent_v2_matches_javascript_signature_vector() {
    let wire: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/relay-wire.json")).unwrap();
    let mut e: Envelope = serde_json::from_value(wire["envelope"].clone()).unwrap();
    e.scope = Some(Scope {
        from_owner: e.owner.clone(),
        to_owner: json!(["https://owner.example/", "bob"]).to_string(),
        from_org: "a".repeat(64),
        to_org: "b".repeat(64),
        from_agent: "1".repeat(64),
        to_agent: "2".repeat(64),
    });
    let key = Keys::new(e.owner.clone(), "a".into(), [1; 32], [2; 32]);
    assert_eq!(key.sign(&e.signed_bytes()),"e847093718c3d7590679ca9186e3b2cd7bba2be00a0969ba7703aa43ddb115d8f7ae326c174b289b6b2e838598af81bca5bc524de259fa664ab16e9e1373bb05");
}

#[test]
fn cross_org_scoped_ciphertext_roundtrip_and_receipts() {
    let (a, ka, b, kb) = devices();
    let (sender, recipient) = (a.agent(&ka).unwrap(), b.agent(&kb).unwrap());
    for p in [
        payload(),
        Payload::Delivered {
            message_id: "original".into(),
        },
    ] {
        let e = ka
            .seal_agent(&kb.device, "key", &p, 1000, scope(&sender, &recipient))
            .unwrap();
        assert_eq!(kb.open(&ka.device, &e, 1001).unwrap(), p);
        let wire = serde_json::to_string(&e).unwrap();
        for hidden in [
            "private agent",
            "recipient",
            "sender",
            "\"pi\"",
            "\"omp\"",
            "private card",
        ] {
            assert!(!wire.contains(hidden), "plaintext {hidden} in envelope");
        }
    }
    assert!(ka
        .seal(&kb.device, "legacy", &payload(), 1000, TTL)
        .is_err());
}

#[test]
fn org_agent_account_scope_is_signed_and_aead_bound() {
    let (a, ka, b, kb) = devices();
    let original = ka
        .seal_agent(
            &kb.device,
            "key",
            &payload(),
            1000,
            scope(&a.agent(&ka).unwrap(), &b.agent(&kb).unwrap()),
        )
        .unwrap();
    for field in 0..6 {
        let mut e = original.clone();
        let s = e.scope.as_mut().unwrap();
        let value = match field {
            0 => &mut s.from_owner,
            1 => &mut s.to_owner,
            2 => &mut s.from_org,
            3 => &mut s.to_org,
            4 => &mut s.from_agent,
            _ => &mut s.to_agent,
        };
        *value = "f".repeat(64);
        assert!(kb.open(&ka.device, &e, 1001).is_err());
        e.signature = ka.sign(&e.signed_bytes());
        assert!(kb.open(&ka.device, &e, 1001).is_err());
    }
    let mut e = original.clone();
    e.scope = None;
    e.signature = ka.sign(&e.signed_bytes());
    assert!(kb.open(&ka.device, &e, 1001).is_err());
    let stranger = Keys::new(b.owner.clone(), "stranger-device".into(), [5; 32], [6; 32]);
    assert!(stranger.open(&ka.device, &original, 1001).is_err());
    assert!(kb.open(&ka.device, &original, original.expires_at).is_err());
    let mut s = original.scope.unwrap();
    s.to_owner = a.owner;
    assert!(ka
        .seal_agent(&kb.device, "key", &payload(), 1000, s)
        .is_err());
}

#[test]
fn agent_pins_include_org_and_device_keys_and_storage_partitions_named_agents() {
    let (a, ka, b, kb) = devices();
    let agent = b.agent(&kb).unwrap();
    let mut forged = agent.clone();
    forged.org = "a".repeat(64);
    assert_ne!(agent.fingerprint(), forged.fingerprint());
    forged.device.encryption_key = "00".repeat(32);
    assert!(forged.verify().is_err());
    let mut other = a.clone();
    other.agent_id = Some("2".repeat(64));
    assert_ne!(a.root(), other.root());
    other = a.clone();
    other.deployment = "https://other.convex.cloud".into();
    assert_ne!(a.root(), other.root());
    assert_ne!(a.agent(&ka).unwrap().fingerprint(), agent.fingerprint());
}

#[test]
fn invalid_scope_and_hops_fail_before_encryption() {
    let (a, ka, b, kb) = devices();
    let mut s = scope(&a.agent(&ka).unwrap(), &b.agent(&kb).unwrap());
    s.to_agent = "../escape".into();
    assert!(ka
        .seal_agent(&kb.device, "key", &payload(), 1000, s)
        .is_err());
    let mut p = payload();
    if let Payload::Inbox { hops, .. } = &mut p {
        *hops = 4;
    }
    assert!(ka
        .seal_agent(
            &kb.device,
            "key",
            &p,
            1000,
            scope(&a.agent(&ka).unwrap(), &b.agent(&kb).unwrap())
        )
        .is_err());
}

#[test]
fn local_agent_inbox_journey() {
    if std::env::var_os("EVERETT_AGENT_TRIAL").is_none() {
        let home = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "relay::agents::tests::local_agent_inbox_journey",
                "--nocapture",
            ])
            .env("EVERETT_AGENT_TRIAL", "1")
            .env("HOME", home.path())
            .env("EVERETT_HOME", home.path())
            .env("EVERETT_NOTIFY", "none")
            .env("EVERETT_CONVEX_URL", "https://test.convex.cloud")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let a = identity("alice", "a", "1");
    let b = identity("bob", "b", "2");
    let ka = agent_keys(&a).unwrap();
    let kb = agent_keys(&b).unwrap();
    assert_eq!(agent_keys(&a).unwrap().device, ka.device);
    let mut sibling = a.clone();
    sibling.agent_id = Some("3".repeat(64));
    assert_ne!(agent_keys(&sibling).unwrap().device, ka.device);
    let sender = a.agent(&ka).unwrap();
    let recipient = b.agent(&kb).unwrap();
    write(
        &a.root()
            .join("pins")
            .join(format!("{}.json", recipient.agent_id)),
        &Pin {
            agent: recipient.clone(),
            deployment: a.deployment.clone(),
        },
    )
    .unwrap();
    write(
        &b.root()
            .join("pins")
            .join(format!("{}.json", sender.agent_id)),
        &Pin {
            agent: sender.clone(),
            deployment: b.deployment.clone(),
        },
    )
    .unwrap();
    let now = milliseconds();
    let p = payload();
    let (_, attempt) = prepare(&a, &ka, &recipient, "key", &p).unwrap();
    let (_, retry) = prepare(&a, &ka, &recipient, "key", &p).unwrap();
    assert_eq!(attempt.envelope, retry.envelope);
    assert!(prepare(
        &a,
        &ka,
        &recipient,
        "key",
        &Payload::Delivered {
            message_id: "changed".into()
        }
    )
    .is_err());
    let mut incoming = Incoming {
        envelope: attempt.envelope.clone(),
        external: false,
        trust: "same-org".into(),
    };
    assert!(import(&b, &kb, &incoming, "recipient", "pi").is_err());
    incoming.external = true;
    incoming.trust = "external-untrusted".into();
    assert!(import(&b, &kb, &incoming, "other-session", "pi").is_err());
    assert!(import(&b, &kb, &incoming, "recipient", "omp").is_err());
    import(&b, &kb, &incoming, "recipient", "pi").unwrap();
    import(&b, &kb, &incoming, "recipient", "pi").unwrap();
    let messages = inbox::read("recipient");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["external"], true);
    assert_eq!(messages[0]["trust"], "external-untrusted");
    assert!(inbox::render(&messages, None)
        .0
        .contains("EXTERNAL / UNTRUSTED"));
    inbox::mark_done("recipient", &[&attempt.envelope.message_id]);
    let receipt = kb
        .seal_agent(
            &ka.device,
            "receipt",
            &Payload::Delivered {
                message_id: attempt.envelope.message_id.clone(),
            },
            now,
            scope(&recipient, &sender),
        )
        .unwrap();
    import(
        &a,
        &ka,
        &Incoming {
            envelope: receipt,
            external: true,
            trust: "external-untrusted".into(),
        },
        "sender",
        "omp",
    )
    .unwrap();
    assert_eq!(
        find(&a, &attempt.envelope.message_id).unwrap().1.state,
        State::Delivered
    );
    let reply = Payload::Inbox {
        session: "sender".into(),
        harness: "omp".into(),
        text: "fixed".into(),
        from: "recipient".into(),
        from_harness: "pi".into(),
        from_card: String::new(),
        reply_to: attempt.envelope.message_id.clone(),
        hops: 2,
    };
    let e = kb
        .seal_agent(&ka.device, "reply", &reply, now, scope(&recipient, &sender))
        .unwrap();
    let reply = Incoming {
        envelope: e,
        external: true,
        trust: "external-untrusted".into(),
    };
    import(&a, &ka, &reply, "sender", "omp").unwrap();
    import(&a, &ka, &reply, "sender", "omp").unwrap();
    assert_eq!(inbox::read("sender").len(), 1);
    assert_eq!(
        find(&a, &attempt.envelope.message_id).unwrap().1.state,
        State::Replied
    );
    observe(
        &a,
        &recipient,
        &attempt.envelope.message_id,
        State::Delivered,
        None,
    )
    .unwrap();
    assert_eq!(
        find(&a, &attempt.envelope.message_id).unwrap().1.state,
        State::Replied
    );
}
