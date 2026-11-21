use super::*;
use crate::receiver_wake;
use std::sync::Mutex;

static NATIVE_DEVICES: Mutex<Vec<Device>> = Mutex::new(vec![]);
static NATIVE_SCRIPT: Mutex<std::collections::VecDeque<(&'static str, std::result::Result<Value, &'static str>)>> =
    Mutex::new(std::collections::VecDeque::new());
static NATIVE_CALLS: Mutex<Vec<(String, Value)>> = Mutex::new(vec![]);

fn configure(keys: &Keys, devices: Vec<Device>) {
    *NATIVE_DEVICES.lock().unwrap() = devices;
    NATIVE_SCRIPT.lock().unwrap().clear();
    NATIVE_CALLS.lock().unwrap().clear();
    crate::storage::private_dir(crate::cloudflare_auth::credential_path().parent().unwrap()).unwrap();
    write(&crate::cloudflare_auth::credential_path(), &json!({
        "issuer":"https://relay.example", "agent_id":keys.device.device_id,
        "owner":keys.device.owner, "device_name":"fixture",
        "token":format!("evt_{}", "a".repeat(64)), "expires_at":u64::MAX
    })).unwrap();
}

static RECEIVER_ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn idle_poll_schedule_caps_at_sixty_seconds_and_resets_after_activity() {
    let mut now = tokio::time::Instant::now();
    let mut polling = PollSchedule::new(now);
    assert!(polling.is_due(now));
    for seconds in [5, 10, 20, 40, 60, 60] {
        polling.complete(now, false);
        assert!(!polling.is_due(now + Duration::from_secs(seconds - 1)));
        now += Duration::from_secs(seconds);
        assert!(polling.is_due(now));
    }
    polling.complete(now, true);
    assert!(!polling.is_due(now + Duration::from_secs(4)));
    now += Duration::from_secs(5);
    assert!(polling.is_due(now));
    polling.complete(now, false);
    now += Duration::from_secs(5);
    assert!(polling.is_due(now));
    polling.complete(now, false);
    assert!(!polling.is_due(now + Duration::from_secs(9)));
    assert!(polling.is_due(now + Duration::from_secs(10)));
}

#[test]
fn local_activity_brings_idle_poll_forward_without_postponing_a_due_poll() {
    let mut now = tokio::time::Instant::now();
    let mut polling = PollSchedule::new(now);
    for seconds in [5, 10, 20, 40] {
        polling.complete(now, false);
        now += Duration::from_secs(seconds);
    }
    polling.complete(now, false);
    polling.activity(now + Duration::from_secs(5));
    assert!(!polling.is_due(now + Duration::from_secs(9)));
    assert!(polling.is_due(now + Duration::from_secs(10)));
    polling.activity(now + Duration::from_secs(10));
    assert!(polling.is_due(now + Duration::from_secs(10)));
    let restarted = PollSchedule::new(now);
    assert!(restarted.is_due(now));
}

thread_local! {
    pub(super) static FAIL_STAGE_DELETE_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[test]
fn stage_unlink_fsync_failure_restores_private_ciphertext_for_retry() {
    use std::os::unix::fs::PermissionsExt;
    let (a, b) = devices();
    let envelope = a
        .seal(&b.device, "synthetic", &payload(1), 1000, TTL, None, None)
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ingress = dir.path().join("new/relay/ingress");
    crate::storage::private_dir(&ingress).unwrap();
    for child in [
        dir.path().join("new"),
        dir.path().join("new/relay"),
        ingress.clone(),
    ] {
        assert_eq!(
            fs::metadata(child).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let path = ingress.join("encrypted.json");
    write(&path, &envelope).unwrap();
    FAIL_STAGE_DELETE_SYNC.with(|f| f.set(true));
    assert!(remove_stage(&path, &envelope).is_err());
    assert!(path.exists());
    let recovered: Envelope = read(&path).unwrap();
    assert_eq!(recovered, envelope);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let stored = fs::read_to_string(&path).unwrap();
    assert!(!stored.contains("private message"));
    remove_stage(&path, &envelope).unwrap();
    assert!(!path.exists());
}

fn devices() -> (Keys, Keys) {
    let owner = json!(["https://issuer.example/", "org", "org-a"]).to_string();
    (
        Keys::new(owner.clone(), "agent-a".into(), [1; 32], [2; 32]),
        Keys::new(owner, "agent-b".into(), [3; 32], [4; 32]),
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
        external_untrusted: false,
    }
}

#[test]
fn device_labels_are_advisory_duplicate_safe_and_legacy_compatible() {
    let (a, b) = devices();
    let bundles = json!([a.device, b.device]);
    let before = bundles.clone();
    let labels = json!({"labels":{"agent-a":"same name","agent-b":"same name"}});
    let text = format_device_names(bundles.clone(), Some(labels), &a.device.owner).unwrap();
    assert_eq!(text.matches("\"same name\"").count(), 2);
    assert!(text.contains("agent-a") && text.contains("agent-b"));
    assert!(text.contains(&a.device.fingerprint()) && text.contains(&b.device.fingerprint()));
    assert_eq!(before, bundles);
    let legacy = format_device_names(bundles.clone(), None, &a.device.owner).unwrap();
    assert!(legacy.contains("Names unavailable") && legacy.contains("(unnamed)"));
    let malformed = format_device_names(
        bundles.clone(),
        Some(json!({"labels": false})),
        &a.device.owner,
    )
    .unwrap();
    assert!(malformed.contains("Names unavailable"));
    let partial = format_device_names(
        bundles.clone(),
        Some(json!({"labels":{"agent-a":"new name"}})),
        &a.device.owner,
    )
    .unwrap();
    assert!(partial.contains("new name") && partial.contains("(unnamed)"));
    let hostile = format_device_names(
        bundles.clone(),
        Some(json!({"labels":{"agent-a":"\x1b[31m"}})),
        &a.device.owner,
    )
    .unwrap();
    assert!(!hostile.contains('\x1b'));
    assert!(format_device_names(bundles.clone(), None, "other owner").is_err());
    let mut tampered = bundles;
    tampered[0]["deviceId"] = json!("forged");
    assert!(format_device_names(tampered, None, &a.device.owner).is_err());
}

#[test]
fn encrypted_round_trip_and_ciphertext_only() {
    let (a, b) = devices();
    let p = payload(1);
    let e = a.seal(&b.device, "key", &p, 1000, TTL, None, None).unwrap();
    assert_eq!(b.open(&a.device, &e, 1001).unwrap(), p);
    let stored = serde_json::to_string(&e).unwrap();
    assert!(!stored.contains("private message"));
    assert!(!stored.contains("codex"));
}

#[test]
fn ciphertext_signature_routing_and_aead_tampering_rejected() {
    let (a, b) = devices();
    let original = a
        .seal(&b.device, "key", &payload(1), 1000, TTL, None, None)
        .unwrap();
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
    let value = serde_json::to_value(&original).unwrap();
    for (field, replacement) in [
        ("owner", json!("wrong-org")),
        ("recipientOwner", json!("wrong-org")),
        ("grantId", json!("other-grant")),
        ("fromDevice", json!("other")),
        ("toDevice", json!("other")),
        ("messageId", json!("other")),
        ("idempotencyKey", json!("other")),
        ("hop", json!(2)),
        ("parent", json!({"owner":"other","messageId":"other"})),
        ("createdAt", json!(999)),
        ("expiresAt", json!(3601001)),
        ("senderFingerprint", json!("00".repeat(32))),
        ("recipientFingerprint", json!("00".repeat(32))),
        ("ephemeralKey", json!("00".repeat(32))),
        ("nonce", json!("00".repeat(12))),
    ] {
        let mut tampered = value.clone();
        tampered[field] = replacement;
        let e: Envelope = serde_json::from_value(tampered).unwrap();
        assert!(b.open(&a.device, &e, 1001).is_err(), "{field}");
    }
}

#[test]
fn wrong_org_missing_grant_changed_keys_ttl_and_hops_rejected() {
    let (a, b) = devices();
    let other = Keys::new(
        json!(["https://issuer.example/", "org", "org-b"]).to_string(),
        "agent-b".into(),
        [3; 32],
        [4; 32],
    );
    assert!(a
        .seal(&other.device, "key", &payload(1), 1000, TTL, None, None)
        .is_err());
    assert!(a
        .seal(
            &b.device,
            "key",
            &payload(1),
            1000,
            TTL,
            Some("grant".into()),
            None
        )
        .is_err());
    let e = a
        .seal(&b.device, "key", &payload(1), 1000, TTL, None, None)
        .unwrap();
    assert!(other.open(&a.device, &e, 1001).is_err());
    assert!(a.open(&a.device, &e, 1001).is_err());
    assert!(b.open(&a.device, &e, e.expires_at).is_err());
    let rotated = Keys::new(
        a.device.owner.clone(),
        a.device.device_id.clone(),
        [9; 32],
        [10; 32],
    );
    assert!(b.open(&rotated.device, &e, 1001).is_err());
    assert!(a
        .seal(&b.device, "key", &payload(65), 1000, TTL, None, None)
        .is_err());
    let parent = Some(Parent {
        owner: b.device.owner.clone(),
        message_id: "previous".into(),
    });
    let e = a
        .seal(&b.device, "key", &payload(2), 1000, TTL, None, parent)
        .unwrap();
    assert_eq!(b.open(&a.device, &e, 1001).unwrap(), payload(2));
    let mut too_big = payload(1);
    if let Payload::Inbox { text, .. } = &mut too_big {
        *text = "x".repeat(peer::BODY_CAP + 1);
    }
    assert!(a
        .seal(&b.device, "key", &too_big, 1000, TTL, None, None)
        .is_err());
}

#[test]
fn javascript_v2_encryption_signatures_rotation_and_observation_match() {
    let wire: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/relay-wire.json")).unwrap();
    let device: Device = serde_json::from_value(wire["device"].clone()).unwrap();
    let recipient: Device = serde_json::from_value(wire["recipient"].clone()).unwrap();
    let a = Keys::new(
        device.owner.clone(),
        device.device_id.clone(),
        [1; 32],
        [2; 32],
    );
    let b = Keys::new(
        recipient.owner.clone(),
        recipient.device_id.clone(),
        [3; 32],
        [4; 32],
    );
    assert_eq!(device, a.device);
    assert_eq!(recipient, b.device);
    assert_eq!(
        String::from_utf8(device.signed_bytes()).unwrap(),
        wire["deviceSignedUtf8"]
    );
    assert_eq!(device.fingerprint(), wire["fingerprint"]);
    let e: Envelope = serde_json::from_value(wire["envelope"].clone()).unwrap();
    assert_eq!(String::from_utf8(e.aad()).unwrap(), wire["aadUtf8"]);
    assert_eq!(
        String::from_utf8(e.signed_bytes()).unwrap(),
        wire["envelopeSignedUtf8"]
    );
    assert_eq!(a.sign(&e.signed_bytes()), e.signature);
    assert_eq!(
        String::from_utf8(b.open_bytes(&device, &e, 1001).unwrap()).unwrap(),
        wire["plaintextUtf8"]
    );
    assert_eq!(
        b.sign(wire["observationSignedUtf8"].as_str().unwrap().as_bytes()),
        wire["observation"]["signature"]
    );
    assert_eq!(
        a.sign(wire["rotationSignedUtf8"].as_str().unwrap().as_bytes()),
        wire["rotation"]["signature"]
    );
    let round_trip = serde_json::to_value(&e).unwrap();
    let decoded: Envelope = serde_json::from_value(round_trip).unwrap();
    assert_eq!(e, decoded);
    assert_eq!(e.signed_bytes(), decoded.signed_bytes());
}

#[test]
fn per_agent_key_paths_and_exact_idempotent_ciphertext_retry() {
    let (a, b) = devices();
    assert_ne!(
        key_path(&a.device.owner, &a.device.device_id),
        key_path(&b.device.owner, &b.device.device_id)
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retry.json");
    let pin = Paired {
        device: b.device.clone(),
        deployment: "https://relay.example".into(),
        grant_id: None,
    };
    let out = Outgoing {
        peer: "peer".into(),
        envelope: a
            .seal(&b.device, "key", &payload(1), 1000, TTL, None, None)
            .unwrap(),
        digest: "digest".into(),
        session: "sender".into(),
        harness: "claude".into(),
        posted: false,
        direct: None,
        posted_at: None,
        replied_at: None,
        relay_evidence: None,
        state: State::Queued,
        reply_pending: None,
    };
    write(&path, &out).unwrap();
    assert_eq!(
        reuse_outgoing(&path, "digest", &a, &pin)
            .unwrap()
            .unwrap()
            .envelope,
        out.envelope
    );
    assert!(reuse_outgoing(&path, "different-body", &a, &pin).is_err());
    let rotated = Keys::new(
        a.device.owner.clone(),
        a.device.device_id.clone(),
        [9; 32],
        [10; 32],
    );
    assert!(reuse_outgoing(&path, "digest", &rotated, &pin).is_err());
    let mut changed = pin;
    changed.grant_id = Some("changed-grant".into());
    assert!(reuse_outgoing(&path, "digest", &a, &changed).is_err());
}

#[test]
fn denied_agent_or_grant_observation_drops_cached_staging_before_publication() {
    let (a, b) = devices();
    let e = a
        .seal(&b.device, "key", &payload(1), 1000, TTL, None, None)
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    for reason in ["revoked agent", "revoked grant", "changed key", "wrong org"] {
        let path = dir.path().join("pending.json");
        write(&path, &e).unwrap();
        let err = EverettError::new(1, format!("Cloudflare relay denied: {reason}"));
        assert!(finish_staged(&b, &path, &StagedFile::Bare(e.clone()), &e, &Attestation::default(), Err(err)).is_err());
        assert!(
            !path.exists(),
            "denial must remove the cached encrypted row"
        );
    }
    let path = dir.path().join("uncertain.json");
    write(&path, &e).unwrap();
    assert!(finish_staged(&b, &path, &StagedFile::Bare(e.clone()), &e, &Attestation::default(), Err(bad("Cloudflare mutation timed out"))).is_err());
    assert!(path.exists(),"uncertain committed delivery retains encrypted durable recovery, never plaintext publication");
}

#[test]
fn empty_filtered_page_still_carries_cursor_and_requires_pagination() {
    let page: PendingPage = serde_json::from_value(
        json!({"serverNow":1000.0,"envelopes":[],"isDone":false,"continueCursor":"next-page"}),
    )
    .unwrap();
    assert!(!page.is_done);
    assert_eq!(page.continue_cursor, "next-page");
}

#[test]
fn unsafe_wire_numbers_are_rejected() {
    let wire: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/relay-wire.json")).unwrap();
    for field in ["createdAt", "expiresAt", "hop"] {
        for invalid in [json!(1.5), json!(9007199254740992_f64)] {
            let mut value = wire["envelope"].clone();
            value[field] = invalid;
            assert!(serde_json::from_value::<Envelope>(value).is_err());
        }
    }
}

#[test]
fn exact_device_query_recovers_a_committed_rotation_without_directory_scanning() {
    let (old, _) = devices();
    let new = Keys::new(
        old.device.owner.clone(),
        old.device.device_id.clone(),
        [9; 32],
        [10; 32],
    );
    let mut client = TestClient::fake(vec![("relay:device", Ok(json!(new.device.clone())))]);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let recovered = rt
        .block_on(commit_rotation(
            &mut client,
            &old,
            &new,
            "synthetic rotation",
        ))
        .unwrap();
    assert_eq!(recovered["fingerprint"], json!(new.device.fingerprint()));
    assert_eq!(
        client.calls.len(),
        1,
        "committed staged keys must not rotate again"
    );
    assert_eq!(client.calls[0].0, "relay:device");
    assert_eq!(
        client.calls[0].1,
        json!({"deviceId":old.device.device_id.clone()})
    );

    let mut client = TestClient::fake(vec![
        ("relay:device", Ok(json!(old.device.clone()))),
        ("relay:rotate", Ok(json!({"rotated":true}))),
    ]);
    assert_eq!(
        rt.block_on(commit_rotation(
            &mut client,
            &old,
            &new,
            "synthetic rotation"
        ))
        .unwrap()["rotated"],
        true
    );
    assert_eq!(client.calls[1].0, "relay:rotate");
    assert!(client.calls.iter().all(|(name, _)| name != "relay:devices"));
}

#[test]
fn staged_rotation_keeps_old_primary_on_errors_and_promotes_committed_stage() {
    let (old, _) = devices();
    let new = Keys::new(
        old.device.owner.clone(),
        old.device.device_id.clone(),
        [9; 32],
        [10; 32],
    );
    let old_private = PrivateKeys {
        signing: hex::encode([1; 32]),
        encryption: hex::encode([2; 32]),
    };
    let staged = PrivateKeys {
        signing: hex::encode([9; 32]),
        encryption: hex::encode([10; 32]),
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keys.json");
    let stage = path.with_extension("rotation");
    write(&path, &old_private).unwrap();

    for failure in ["lost response", "unknown registered key bundle"] {
        let result: Result<()> =
            staged_rotation(&path, &stage, || staged.clone(), |_| Err(bad(failure)));
        assert!(result.is_err());
        assert_eq!(
            read::<PrivateKeys>(&path).unwrap().signing,
            old_private.signing
        );
        assert_eq!(read::<PrivateKeys>(&stage).unwrap().signing, staged.signing);
    }

    let mut client = TestClient::fake(vec![("relay:device", Ok(json!(new.device.clone())))]);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (device, response) = staged_rotation(
        &path,
        &stage,
        || panic!("a saved stage must be reused"),
        |private| {
            let recovered = Keys::new(
                old.device.owner.clone(),
                old.device.device_id.clone(),
                crypto::decode(&private.signing)?,
                crypto::decode(&private.encryption)?,
            );
            let response = rt.block_on(commit_rotation(
                &mut client,
                &old,
                &recovered,
                "synthetic rotation",
            ))?;
            Ok((recovered.device, response))
        },
    )
    .unwrap();
    assert_eq!(device, new.device);
    assert_eq!(response["fingerprint"], new.device.fingerprint());
    assert_eq!(read::<PrivateKeys>(&path).unwrap().signing, staged.signing);
    assert!(!stage.exists());
    assert_eq!(
        client.calls.len(),
        1,
        "committed stage must not rotate again"
    );
    assert_eq!(client.calls[0].0, "relay:device");
}

#[test]
fn rejected_cache_is_bounded_and_expires_duplicate_suppression() {
    let mut rejected = RejectedCache::new();
    assert!(rejected.insert("long-lived".into(), i64::MAX, 1));
    assert!(!rejected.insert("long-lived".into(), i64::MAX, 2));
    // Reinsert one short-lived row thousands of times behind a live entry. The
    // old queue implementation accumulated stale versions without bound here.
    for now in 1..=(REJECTED_CACHE_CAP as i64 * 3) {
        assert!(rejected.insert("short-lived".into(), now, now));
        assert!(rejected.entries.len() <= REJECTED_CACHE_CAP);
    }
    rejected.prune(REJECTED_CACHE_CAP as i64 * 3);
    assert_eq!(rejected.entries.len(), 1);
    assert!(rejected.entries.contains_key("long-lived"));
    for i in 0..(REJECTED_CACHE_CAP + 8) {
        assert!(rejected.insert(format!("id-{i}"), i64::MAX, 2));
    }
    assert!(rejected.entries.len() <= REJECTED_CACHE_CAP);
    rejected.prune(i64::MAX);
    assert!(rejected.entries.is_empty());
    assert!(
        rejected.insert("duplicate".into(), 200, 101),
        "expired IDs must log again"
    );
}

pub(super) struct TestClient {
    native: Option<crate::cloudflare_auth::Credential>,
    responses: std::collections::VecDeque<(&'static str, std::result::Result<Value, &'static str>)>,
    calls: Vec<(String, Value)>,
    before_response: Option<fn(&str)>,
}

impl TestClient {
    fn fake(responses: Vec<(&'static str, std::result::Result<Value, &'static str>)>) -> Self {
        Self { native: None, responses: responses.into(), calls: vec![], before_response: None }
    }

    pub fn native(stored: crate::cloudflare_auth::Credential) -> Self {
        Self { native: Some(stored), ..Self::fake(vec![]) }
    }

    pub async fn call(&mut self, name: &str, value: Value) -> Result<Value> {
        if let Some(stored) = &self.native {
            if name == "relay:whoami" {
                return Ok(json!({"owner":stored.owner,"deviceId":stored.agent_id}));
            }
            if name == "relay:device" {
                return Ok(json!(NATIVE_DEVICES.lock().unwrap().iter().find(|d| value["deviceId"] == d.device_id)));
            }
            let (expected, result) = NATIVE_SCRIPT
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected native fixture request: {name}"));
            assert_eq!(name, expected);
            NATIVE_CALLS.lock().unwrap().push((name.to_string(), value));
            return result.map_err(|e| EverettError::new(1, e));
        }
        if let Some(callback) = self.before_response.take() {
            callback(name);
        }
        let (expected, result) = self.responses.pop_front().expect("unexpected relay request");
        assert_eq!(name, expected);
        self.calls.push((name.to_string(), value));
        result.map_err(|e| EverettError::new(1, e))
    }
}

fn queued_webhook_retry(keys: &Keys) -> PathBuf {
    crate::storage::private_dir(&root()).unwrap();
    let bearer = root().join("webhook-test.key");
    crate::storage::save(&bearer, b"synthetic-webhook-test-key").unwrap();
    write(&root().join("receiver-webhook.json"), &json!({
        "version":1,"enabled":true,"activated_at":1,"rules":[{
            "id":"retry-rule","owner":keys.device.owner,"recipient_device":keys.device.device_id,
            "session":"ext-webhook-test","harness":"grok-bot","cooldown_seconds":30,
            "url":"https://webhook-test.invalid/routine","bearer_file":bearer
        }]
    })).unwrap();
    let ledger = root().join("receiver-webhook/ledger.json");
    crate::storage::private_dir(ledger.parent().unwrap()).unwrap();
    let last = milliseconds() / 1000 - 31;
    write(&ledger, &json!({"version":1,"rules":{"retry-rule":last},"messages":{
        "retry-message":{"rule_id":"retry-rule","session":"ext-webhook-test","harness":"grok-bot",
            "state":"pending","attempts":1,"at":last}
    }})).unwrap();
    ledger
}

#[test]
fn due_webhook_retry_requires_authorization_during_an_idle_poll_gap() {
    if !crate::test_support::isolated("relay::tests::due_webhook_retry_requires_authorization_during_an_idle_poll_gap") {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("EVERETT_HOME", home.path());
    std::env::set_var("EVERETT_AUTH_PROVIDER", "cloudflare");
    let (a, b) = devices();
    configure(&b, vec![a.device.clone(), b.device.clone()]);
    let ledger = queued_webhook_retry(&b);
    let before = fs::read(&ledger).unwrap();
    let _capture = crate::receiver_webhook::testing_begin(200);
    let now = tokio::time::Instant::now();
    let mut polling = PollSchedule::new(now);
    polling.complete(now, false);
    assert!(!polling.is_due(now + Duration::from_secs(1)));
    let mut denied = TestClient::fake(vec![("relay:whoami", Err("Cloudflare relay denied: revoked receiver"))]);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let error = rt.block_on(poll_receiver_webhook(&mut denied, &b)).unwrap_err();
    assert!(super::denied(&error));
    assert_eq!(denied.calls.len(), 1);
    assert!(crate::receiver_webhook::testing_posts().is_empty());
    assert_eq!(fs::read(&ledger).unwrap(), before);
    let mut changed = TestClient::fake(vec![("relay:whoami", Ok(json!({"owner":"another-owner","deviceId":b.device.device_id})))]);
    let error = rt.block_on(poll_receiver_webhook(&mut changed, &b)).unwrap_err();
    assert_eq!(error.code, 2);
    assert!(crate::receiver_webhook::testing_posts().is_empty());
    assert_eq!(fs::read(ledger).unwrap(), before);
}

#[test]
fn webhook_maintenance_defers_uncertain_auth_without_spending_attempts() {
    if !crate::test_support::isolated("relay::tests::webhook_maintenance_defers_uncertain_auth_without_spending_attempts") {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("EVERETT_HOME", home.path());
    std::env::set_var("EVERETT_AUTH_PROVIDER", "cloudflare");
    let (a, b) = devices();
    configure(&b, vec![a.device.clone(), b.device.clone()]);
    let ledger = queued_webhook_retry(&b);
    let _capture = crate::receiver_webhook::testing_begin(200);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut saved: Value = read(&ledger).unwrap();
    saved["rules"]["retry-rule"] = json!(milliseconds() / 1000);
    write(&ledger, &saved).unwrap();
    let mut quiet = TestClient::fake(vec![]);
    rt.block_on(poll_receiver_webhook(&mut quiet, &b)).unwrap();
    assert!(quiet.calls.is_empty());
    assert!(crate::receiver_webhook::testing_posts().is_empty());
    let ledger = queued_webhook_retry(&b);
    let before = fs::read(&ledger).unwrap();
    let mut uncertain = TestClient::fake(vec![("relay:whoami", Err("network"))]);
    let error = rt.block_on(poll_receiver_webhook(&mut uncertain, &b)).unwrap_err();
    assert_eq!(error.code, 6);
    assert!(!super::denied(&error));
    assert_eq!(fs::read(&ledger).unwrap(), before);
    assert!(crate::receiver_webhook::testing_posts().is_empty());
    let mut recovered = TestClient::fake(vec![("relay:whoami", Ok(json!({"owner":b.device.owner,"deviceId":b.device.device_id})))]);
    rt.block_on(poll_receiver_webhook(&mut recovered, &b)).unwrap();
    assert_eq!(crate::receiver_webhook::testing_posts().len(), 1);
    let saved: Value = read(&ledger).unwrap();
    assert_eq!(saved["messages"]["retry-message"]["state"], "posted");
    assert_eq!(saved["messages"]["retry-message"]["attempts"], 2);
    rt.block_on(poll_receiver_webhook(&mut quiet, &b)).unwrap();
    assert!(quiet.calls.is_empty());
    assert_eq!(crate::receiver_webhook::testing_posts().len(), 1);
}

#[test]
fn webhook_metadata_errors_remain_nonfatal_and_do_not_authenticate() {
    if !crate::test_support::isolated("relay::tests::webhook_metadata_errors_remain_nonfatal_and_do_not_authenticate") {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("EVERETT_HOME", home.path());
    std::env::set_var("EVERETT_AUTH_PROVIDER", "cloudflare");
    let (a, b) = devices();
    configure(&b, vec![a.device.clone(), b.device.clone()]);
    let _capture = crate::receiver_webhook::testing_begin(200);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut client = TestClient::fake(vec![]);
    let ledger = queued_webhook_retry(&b);
    crate::storage::save(&ledger, b"{").unwrap();
    rt.block_on(poll_receiver_webhook(&mut client, &b)).unwrap();
    queued_webhook_retry(&b);
    crate::storage::save(&root().join("receiver-webhook.json"), b"{").unwrap();
    rt.block_on(poll_receiver_webhook(&mut client, &b)).unwrap();
    let ledger = queued_webhook_retry(&b);
    let before = fs::read(&ledger).unwrap();
    fs::remove_file(root().join("webhook-test.key")).unwrap();
    rt.block_on(poll_receiver_webhook(&mut client, &b)).unwrap();
    assert_eq!(fs::read(ledger).unwrap(), before);
    assert!(client.calls.is_empty());
    assert!(crate::receiver_webhook::testing_posts().is_empty());
}

#[test]
fn webhook_retry_reloads_after_authorization_without_holding_the_ledger_lock() {
    if !crate::test_support::isolated("relay::tests::webhook_retry_reloads_after_authorization_without_holding_the_ledger_lock") {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("EVERETT_HOME", home.path());
    std::env::set_var("EVERETT_AUTH_PROVIDER", "cloudflare");
    let (a, b) = devices();
    configure(&b, vec![a.device.clone(), b.device.clone()]);
    let ledger = queued_webhook_retry(&b);
    let _capture = crate::receiver_webhook::testing_begin(200);
    fn consume_retry(name: &str) {
        assert_eq!(name, "relay:whoami");
        let directory = root().join("receiver-webhook");
        let lock = fs::OpenOptions::new().read(true).write(true).open(directory.join("ledger.lock")).unwrap();
        fs2::FileExt::try_lock_exclusive(&lock).expect("ledger lock held while awaiting authorization");
        let path = directory.join("ledger.json");
        let mut saved: Value = read(&path).unwrap();
        saved["messages"]["retry-message"]["state"] = json!("posted");
        saved["messages"]["retry-message"]["attempts"] = json!(2);
        write(&path, &saved).unwrap();
    }
    let mut client = TestClient::fake(vec![("relay:whoami", Ok(json!({"owner":b.device.owner,"deviceId":b.device.device_id})))]);
    client.before_response = Some(consume_retry);
    tokio::runtime::Runtime::new().unwrap().block_on(poll_receiver_webhook(&mut client, &b)).unwrap();
    assert_eq!(client.calls.len(), 1);
    assert!(crate::receiver_webhook::testing_posts().is_empty());
    let saved: Value = read(&ledger).unwrap();
    assert_eq!(saved["messages"]["retry-message"]["state"], "posted");
    assert_eq!(saved["messages"]["retry-message"]["attempts"], 2);
}

#[test]
fn polling_activity_ignores_rejected_rows_and_keeps_uncertain_ingress_active() {
    if !crate::test_support::isolated("relay::tests::polling_activity_ignores_rejected_rows_and_keeps_uncertain_ingress_active") {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("EVERETT_HOME", home.path());
    std::env::set_var("EVERETT_NOTIFY", "none");
    std::env::set_var("EVERETT_AUTH_PROVIDER", "cloudflare");
    let (a, b) = devices();
    configure(&b, vec![a.device.clone(), b.device.clone()]);
    let ingress = root().join("ingress");
    crate::storage::private_dir(&ingress).unwrap();
    let transcript = home.path().join(".codex/sessions/2026/10/12/recipient.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(transcript, include_str!("../../tests/fixtures/codex.jsonl").replace("x-1", "recipient")).unwrap();
    let now = milliseconds();
    let envelope = a.seal(&b.device, "polling-activity", &payload(1), now, TTL, None, None).unwrap();
    let mut invalid = envelope.clone();
    invalid.signature = "00".repeat(64);
    let page = |rows: Vec<Envelope>, done: bool| json!({
        "serverNow":now, "envelopes":rows.into_iter().map(|envelope| json!({"envelope":envelope})).collect::<Vec<_>>(),
        "isDone":done, "continueCursor":"next"
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut rejected = RejectedCache::new();
    let mut invalid_client = TestClient::fake(vec![("relay:pending", Ok(page(vec![invalid], true)))]);
    let mut deferred = false;
    assert!(!rt.block_on(receive(&mut invalid_client, &b, &ingress, &mut rejected, &mut deferred)).unwrap());
    assert!(!deferred, "a forged signature is permanent, not a transient retry");
    let mut uncertain = TestClient::fake(vec![
        ("relay:pending", Ok(page(vec![], false))),
        ("relay:pending", Ok(page(vec![envelope.clone()], true))),
        ("relay:observe", Err("network")),
    ]);
    assert!(rt.block_on(receive(&mut uncertain, &b, &ingress, &mut rejected, &mut false)).unwrap());
    let path = ingress.join(format!("{}.json", local_message_id(&envelope)));
    assert!(path.exists());
    assert!(inbox::read("recipient").is_empty());
    let mut recovered = TestClient::fake(vec![
        ("relay:observe", Ok(json!({}))),
        ("relay:pending", Ok(page(vec![], true))),
    ]);
    assert!(rt.block_on(receive(&mut recovered, &b, &ingress, &mut rejected, &mut false)).unwrap());
    assert!(!path.exists());
    assert_eq!(inbox::read("recipient").len(), 1);
    let mut idle = TestClient::fake(vec![("relay:pending", Ok(page(vec![], true)))]);
    assert!(!rt.block_on(receive(&mut idle, &b, &ingress, &mut rejected, &mut false)).unwrap());
}

#[test]
fn newly_posted_local_requests_keep_polling_active_without_reposting() {
    if !crate::test_support::isolated("relay::tests::newly_posted_local_requests_keep_polling_active_without_reposting") {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("EVERETT_HOME", home.path());
    std::env::set_var("EVERETT_AUTH_PROVIDER", "cloudflare");
    let (a, b) = devices();
    configure(&a, vec![a.device.clone(), b.device.clone()]);
    let path = root().join("outgoing/posted.json");
    crate::storage::private_dir(path.parent().unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    for (age, state, active) in [
        (0, State::Queued, true),
        (61_000, State::Queued, false),
        (0, State::Replied, false),
        (TTL + 1, State::Queued, false),
    ] {
        let out = Outgoing {
            peer: "id:agent-b".into(),
            envelope: a.seal(&b.device, "posted-activity", &payload(1), milliseconds() - age, TTL, None, None).unwrap(),
            digest: "fixture".into(), session: "sender".into(), harness: "claude".into(),
            posted: true, direct: None, posted_at: None, replied_at: None, relay_evidence: None, state, reply_pending: None,
        };
        write(&path, &out).unwrap();
        let before = fs::read(&path).unwrap();
        let mut client = TestClient::fake(vec![]);
        assert_eq!(rt.block_on(reconcile(&mut client, &a, &mut RejectedCache::new())).unwrap(), active);
        assert!(client.calls.is_empty());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn relay_transport_denied_row_does_not_block_allowed_and_own_revocation_stops() {
    if !crate::test_support::isolated("relay::tests::relay_transport_denied_row_does_not_block_allowed_and_own_revocation_stops") {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("EVERETT_HOME", home.path());

    std::env::set_var("EVERETT_NOTIFY", "none");
    let (a, b) = devices();
    configure(&b, vec![a.device.clone(), b.device.clone()]);
    crate::storage::private_dir(&root().join("peers")).unwrap();
    crate::storage::private_dir(&root().join("ingress")).unwrap();
    write(
        &peer_path("peer").unwrap(),
        &Paired {
            device: a.device.clone(),
            deployment: "https://relay.example".into(),
            grant_id: None,
        },
    )
    .unwrap();
    let transcript = home.path().join(".codex/sessions/2026/10/12/x.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(transcript, include_str!("../../tests/fixtures/codex.jsonl")).unwrap();
    let incoming = |key| {
        a.seal(
            &b.device,
            key,
            &Payload::Inbox {
                session: "x-1".into(),
                harness: "codex".into(),
                text: "synthetic relay message".into(),
                from: "remote-session".into(),
                from_harness: "claude".into(),
                from_card: String::new(),
                reply_to: String::new(),
                hops: 1,
                external_untrusted: false,
            },
            milliseconds(),
            TTL,
            None,
            None,
        )
        .unwrap()
    };
    let denied_row = incoming("deny");
    let allowed_row = incoming("allow");
    let denied_path = root().join("ingress/deny.json");
    let allowed_path = root().join("ingress/allow.json");
    write(&denied_path, &denied_row).unwrap();
    write(&allowed_path, &allowed_row).unwrap();
    let mut client = TestClient::fake(vec![
        ("relay:observe", Err("Cloudflare relay denied: revoked sender")),
        (
            "relay:whoami",
            Ok(json!({"owner":b.device.owner,"deviceId":b.device.device_id})),
        ),
        ("relay:observe", Ok(json!({}))),
    ]);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        publish_staged(&mut client, &b, &denied_path, &StagedFile::Bare(denied_row.clone()), &denied_row, &Attestation::default())
            .await
            .unwrap();
        publish_staged(&mut client, &b, &allowed_path, &StagedFile::Bare(allowed_row.clone()), &allowed_row, &Attestation::default())
            .await
            .unwrap();
    });
    assert!(!denied_path.exists());
    assert!(!allowed_path.exists());
    assert_eq!(inbox::read("x-1").len(), 1);
    assert_eq!(
        inbox::read("x-1")[0]["relay_message_id"],
        allowed_row.message_id
    );
    assert_eq!(client.calls[0].1["state"], "delivered");
    assert_eq!(client.calls[0].1["senderOwner"], a.device.owner);
    assert_eq!(client.calls[0].1["messageId"], denied_row.message_id);
    for (own_response, terminal) in [
        (Err("network"), false),
        (Err("Cloudflare relay denied: revoked own agent"), true),
        (
            Ok(json!({"owner":"changed","deviceId":b.device.device_id})),
            true,
        ),
    ] {
        write(&denied_path, &denied_row).unwrap();
        let mut client = TestClient::fake(vec![
            ("relay:observe", Err("Cloudflare relay denied: revoked grant")),
            ("relay:whoami", own_response),
        ]);
        let outcome = rt.block_on(publish_staged(&mut client, &b, &denied_path, &StagedFile::Bare(denied_row.clone()), &denied_row, &Attestation::default()));
        assert_eq!(outcome.is_err(), terminal);
        assert!(
            denied_path.exists(),
            "uncertainty/own revocation must not delete the sole ciphertext inside publisher"
        );
    }
    let mut transient = TestClient::fake(vec![("relay:observe", Err("network"))]);
    rt.block_on(publish_staged(
        &mut transient,
        &b,
        &denied_path,
        &StagedFile::Bare(denied_row.clone()),
        &denied_row,
        &Attestation::default(),
    ))
    .unwrap();
    assert!(denied_path.exists());
    // Failure publishing to the inbox after the backend deleted the row must
    // retain encrypted staging. An invalid inbox path is deterministic offline.
    let blocked = incoming("blocked");
    let blocked_path = root().join("ingress/blocked.json");
    write(&blocked_path, &blocked).unwrap();
    fs::remove_file(inbox::path("x-1").unwrap()).unwrap();
    fs::create_dir(inbox::path("x-1").unwrap()).unwrap();
    let mut committed = TestClient::fake(vec![("relay:observe", Ok(json!({})))]);
    assert!(rt
        .block_on(publish_staged(&mut committed, &b, &blocked_path, &StagedFile::Bare(blocked.clone()), &blocked, &Attestation::default()))
        .is_err());
    assert!(blocked_path.exists());
    fs::remove_dir(inbox::path("x-1").unwrap()).unwrap();
    let mut retry = TestClient::fake(vec![("relay:observe", Ok(json!({})))]);
    rt.block_on(publish_staged(&mut retry, &b, &blocked_path, &StagedFile::Bare(blocked.clone()), &blocked, &Attestation::default()))
        .unwrap();
    assert!(!blocked_path.exists());
    // Claimed observations use the remote ID and remote sender owner, not the
    // independently generated local inbox ID. Reconcile retries idempotently.
    inbox::take("x-1");
    let mut claims = TestClient::fake(vec![("relay:observe", Ok(json!({})))]);
    assert!(rt.block_on(reconcile(&mut claims, &b, &mut RejectedCache::new())).unwrap());
    assert_eq!(claims.calls[0].1["state"], "claimed");
    assert_eq!(claims.calls[0].1["messageId"], blocked.message_id);
    assert_eq!(claims.calls[0].1["senderOwner"], a.device.owner);
    assert!(!rt.block_on(reconcile(&mut claims, &b, &mut RejectedCache::new())).unwrap());
    assert!(claims.responses.is_empty());
    let mut replies = TestClient::fake(vec![("relay:observe", Ok(json!({})))]);
    rt.block_on(signed_observation(
        &mut replies,
        &b,
        &a.device.owner,
        &blocked.message_id,
        "replied",
    ))
    .unwrap();
    for request in [&claims.calls[0].1, &replies.calls[0].1] {
        assert_eq!(request["messageId"], blocked.message_id);
        assert_eq!(request["senderOwner"], a.device.owner);
        assert_eq!(request["deviceId"], b.device.device_id);
        let bytes = json!([
            "everett-observe-v2",
            b.device.owner,
            b.device.device_id,
            a.device.owner,
            blocked.message_id,
            request["state"],
            request["at"].as_f64().unwrap() as i64
        ])
        .to_string();
        crypto::verify(
            &b.device.signing_key,
            request["signature"].as_str().unwrap(),
            bytes.as_bytes(),
        )
        .unwrap();
        let wrong = bytes.replace(&blocked.message_id, "unrelated-message");
        assert!(crypto::verify(
            &b.device.signing_key,
            request["signature"].as_str().unwrap(),
            wrong.as_bytes()
        )
        .is_err());
    }

    crate::storage::private_dir(&root().join("outgoing")).unwrap();
    let reply = Payload::Inbox {
        session: "remote-session".into(),
        harness: "claude".into(),
        text: "synthetic reply".into(),
        from: "x-1".into(),
        from_harness: "codex".into(),
        from_card: String::new(),
        reply_to: blocked.message_id.clone(),
        hops: 2,
        external_untrusted: false,
    };
    let (path, outgoing) = prepare(
        "peer",
        "retry-reply",
        &reply,
        &b,
        Some(Parent {
            owner: a.device.owner.clone(),
            message_id: blocked.message_id.clone(),
        }),
    )
    .unwrap();
    let mut unavailable = TestClient::fake(vec![("relay:enqueue", Err("network"))]);
    assert!(rt
        .block_on(post(&mut unavailable, &path, &outgoing))
        .is_err());
    assert!(!read::<Outgoing>(&path).unwrap().posted);
    let mut lost_observation = TestClient::fake(vec![
        ("relay:device", Ok(json!(a.device))),
        ("relay:enqueue", Ok(json!({}))),
        ("relay:observe", Err("network")),
    ]);
    rt.block_on(reconcile(&mut lost_observation, &b, &mut RejectedCache::new())).unwrap();
    let persisted: Outgoing = read(&path).unwrap();
    assert!(persisted.posted && persisted.reply_pending.is_some());
    let mut recovered = TestClient::fake(vec![
        ("relay:device", Ok(json!(a.device))),
        ("relay:observe", Ok(json!({}))),
    ]);
    rt.block_on(reconcile(&mut recovered, &b, &mut RejectedCache::new())).unwrap();
    assert_eq!(recovered.calls[1].1["state"], "replied");
    assert_eq!(recovered.calls[1].1["messageId"], blocked.message_id);
    assert_eq!(recovered.calls[1].1["senderOwner"], a.device.owner);
    assert!(read::<Outgoing>(&path).unwrap().reply_pending.is_none());
    rt.block_on(reconcile(&mut recovered, &b, &mut RejectedCache::new())).unwrap();
    assert_eq!(recovered.calls.len(), 2);
}

#[test]
fn signed_import_schedules_exact_receiver_job_and_fake_claude_attaches_once() {
    use std::os::unix::fs::PermissionsExt;

    if !crate::test_support::isolated("relay::tests::signed_import_schedules_exact_receiver_job_and_fake_claude_attaches_once") {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("EVERETT_HOME", home.path());

    std::env::set_var("EVERETT_NOTIFY", "none");
    let (a, b) = devices();
    configure(&b, vec![a.device.clone(), b.device.clone()]);
    crate::storage::private_dir(&root().join("peers")).unwrap();
    write(
        &peer_path("mac").unwrap(),
        &Paired {
            device: a.device.clone(),
            deployment: "https://relay.example".into(),
            grant_id: None,
        },
    )
    .unwrap();
    let project = home.path().join("real-project");
    fs::create_dir_all(&project).unwrap();
    let cwd = fs::canonicalize(project)
        .unwrap()
        .to_string_lossy()
        .to_string();
    let session_id = "38cab978-79c0-4f42-9aee-5c30d8aa6e4c";
    let policy_path = root().join("receiver-wake.json");
    write(
        &policy_path,
        &json!({"version":1,"enabled":false,"allow":[{"session_id":session_id,"cwd":cwd,"sender_device_id":a.device.device_id}]}),
    )
    .unwrap();
    assert_eq!(
        fs::metadata(&policy_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let now = milliseconds();
    let envelope = a
        .seal(
            &b.device,
            "receiver-test",
            &Payload::Inbox {
                session: session_id.into(),
                harness: "claude".into(),
                text: "synthetic trusted fixture request".into(),
                from: "mac-session".into(),
                from_harness: "claude".into(),
                from_card: String::new(),
                reply_to: String::new(),
                hops: 1,
                external_untrusted: false,
            },
            now,
            TTL,
            None,
            None,
        )
        .unwrap();

    assert!(import_inner(&b, &envelope, &Attestation::default(), now, true).is_err());
    assert!(inbox::pending(session_id, None).is_empty());
    write(
        &policy_path,
        &json!({"version":1,"enabled":true,"allow":[{"session_id":session_id,"cwd":cwd,"sender_device_id":a.device.device_id}]}),
    )
    .unwrap();
    import_inner(&b, &envelope, &Attestation::default(), now, true).unwrap();
    let local_id = local_message_id(&envelope);
    let jobs = receiver_wake::pending(&root(), now).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].imported.local_id, local_id);
    assert_eq!(
        jobs[0].imported.digest,
        format!("{:x}", Sha256::digest(envelope.signed_bytes()))
    );
    assert_eq!(jobs[0].imported.sender_session, "mac-session");
    assert_eq!(jobs[0].imported.hops, 1);
    assert_eq!(jobs[0].imported.expires_at, envelope.expires_at);

    let mut attaches = 0;
    let result = receiver_wake::attempt(
        &root(),
        &local_id,
        now,
        || Ok(()),
        |(), message_id| {
            attaches += 1;
            assert_eq!(message_id, local_id);
            receiver_wake::AttemptOutcome::PromptSent {
                client_pid: 20,
                target_pid: 10,
            }
        },
    )
    .unwrap();
    assert_eq!(
        result,
        Some(receiver_wake::AttemptOutcome::PromptSent {
            client_pid: 20,
            target_pid: 10
        })
    );
    assert_eq!(attaches, 1);
    import_inner(&b, &envelope, &Attestation::default(), now, true).unwrap();
    assert!(receiver_wake::pending(&root(), now).unwrap().is_empty());
    assert_eq!(attaches, 1);
}

#[test]
fn receiver_skips_consumed_or_digest_changed_inbox_rows() {
    use std::os::unix::fs::PermissionsExt;

    if !crate::test_support::isolated("relay::tests::receiver_skips_consumed_or_digest_changed_inbox_rows") {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("EVERETT_HOME", home.path());
    crate::storage::private_dir(&root()).unwrap();
    let session_id = "38cab978-79c0-4f42-9aee-5c30d8aa6e4c";
    let local_id = format!("relay-{}", "c".repeat(64));
    let digest = "d".repeat(64);
    let cwd = fs::canonicalize(home.path())
        .unwrap()
        .to_string_lossy()
        .to_string();
    let policy_path = root().join("receiver-wake.json");
    write(&policy_path, &json!({"version":1,"enabled":true,"allow":[{"session_id":session_id,"cwd":cwd,"sender_device_id":"mac-device"}]})).unwrap();
    assert_eq!(
        fs::metadata(&policy_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let imported = receiver_wake::Imported {
        local_id: local_id.clone(),
        digest: digest.clone(),
        relay_message_id: "mOriginal123".into(),
        reply_to: None,
        sender_session: "mac-session".into(),
        sender_harness: "claude".into(),
        relay_parent_owner: None,
        relay_parent_message_id: None,
        owner: "owner".into(),
        sender_owner: "owner".into(),
        recipient_owner: "owner".into(),
        sender_device_id: "mac-device".into(),
        recipient_device_id: "nuc-device".into(),
        grant_id: None,
        session_id: session_id.into(),
        harness: "claude".into(),
        cwd,
        expires_at: milliseconds() + TTL,
        hops: 1,
        external: false,
    };
    receiver_wake::schedule(&root(), imported.clone(), milliseconds()).unwrap();
    inbox::post(
        session_id,
        "verified fixture body",
        "mac-device/claude/mac-session",
        "message",
        "",
        1,
        "claude",
        "",
        Some(serde_json::from_value(json!({"id":local_id,"relay_digest":digest})).unwrap()),
    )
    .unwrap();
    let job = receiver_wake::pending(&root(), milliseconds())
        .unwrap()
        .remove(0);
    assert!(receiver_inbox_pending(&job));
    let mut changed_digest = job.clone();
    changed_digest.imported.digest = "e".repeat(64);
    assert!(!receiver_inbox_pending(&changed_digest));
    inbox::take(session_id);
    assert!(!receiver_inbox_pending(&job));
    receiver_wake::skip(
        &root(),
        &local_id,
        "inbox row already consumed",
        milliseconds(),
    )
    .unwrap();
    assert!(receiver_wake::pending(&root(), milliseconds())
        .unwrap()
        .is_empty());
    let attaches = std::cell::Cell::new(0);
    for pending in receiver_wake::pending(&root(), milliseconds()).unwrap() {
        let _: Option<receiver_wake::AttemptOutcome> = receiver_wake::attempt(
            &root(),
            &pending.imported.local_id,
            milliseconds(),
            || Ok(()),
            |(), _| {
                attaches.set(attaches.get() + 1);
                receiver_wake::AttemptOutcome::Cancelled
            },
        )
        .unwrap();
    }
    assert_eq!(attaches.get(), 0);
}

#[test]
fn imported_external_inbox_posts_one_webhook_without_the_message_text() {
    use std::os::unix::fs::PermissionsExt;

    if !crate::test_support::isolated(
        "relay::tests::imported_external_inbox_posts_one_webhook_without_the_message_text",
    ) {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("EVERETT_HOME", home.path());

    std::env::set_var("EVERETT_NOTIFY", "none");
    std::env::set_var("EVERETT_AUTH_PROVIDER", "cloudflare");
    let (a, b) = devices();
    configure(&b, vec![a.device.clone(), b.device.clone()]);
    crate::storage::private_dir(&root().join("peers")).unwrap();
    write(
        &peer_path("mac").unwrap(),
        &Paired {
            device: a.device.clone(),
            deployment: "https://relay.example".into(),
            grant_id: None,
        },
    )
    .unwrap();
    crate::adapters::external::register("ext-grok-acceptance", "grok-bot", "Grok", "/work", None, false)
        .unwrap();
    let bearer = home.path().join("webhook.key");
    fs::write(&bearer, "test-sender-key-0123456789").unwrap();
    fs::set_permissions(&bearer, fs::Permissions::from_mode(0o600)).unwrap();
    let policy = json!({
        "version": 1,
        "enabled": true,
        "activated_at": 1,
        "rules": [{
            "id": "grok-inbox",
            "owner": a.device.owner,
            "recipient_device": b.device.device_id,
            "session": "ext-grok-acceptance",
            "harness": "grok-bot",
            "sender_device": a.device.device_id,
            "sender_session": "human",
            "sender_harness": "codex",
            "cooldown_seconds": 30,
            "url": "https://api2.cursor.sh/automations/webhook/routine",
            "bearer_file": bearer
        }]
    });
    write(&root().join("receiver-webhook.json"), &policy).unwrap();
    let now = milliseconds();
    let phrase = "synthetic webhook phrase";
    let envelope = a
        .seal(
            &b.device,
            "webhook-test",
            &Payload::Inbox {
                session: "ext-grok-acceptance".into(),
                harness: "grok-bot".into(),
                text: phrase.into(),
                from: "human".into(),
                from_harness: "codex".into(),
                from_card: String::new(),
                reply_to: String::new(),
                hops: 1,
                external_untrusted: false,
            },
            now,
            TTL,
            None,
            None,
        )
        .unwrap();
    let _capture = crate::receiver_webhook::testing_begin(200);
    import_inner(&b, &envelope, &Attestation::default(), now, true).unwrap();
    let posts = crate::receiver_webhook::testing_posts();
    assert_eq!(posts.len(), 1, "{}", posts.first().map(|post| post.body.as_str()).unwrap_or(""));
    assert_eq!(posts[0].bearer, "test-sender-key-0123456789");
    assert!(posts[0].body.contains("ext-grok-acceptance"));
    assert!(!posts[0].body.contains(phrase));
    import_inner(&b, &envelope, &Attestation::default(), now, true).unwrap();
    assert_eq!(crate::receiver_webhook::testing_posts().len(), 1);
    let external = a
        .seal(
            &b.device,
            "webhook-external",
            &Payload::Inbox {
                session: "ext-grok-acceptance".into(),
                harness: "grok-bot".into(),
                text: phrase.into(),
                from: "human".into(),
                from_harness: "codex".into(),
                from_card: String::new(),
                reply_to: String::new(),
                hops: 1,
                external_untrusted: true,
            },
            now,
            TTL,
            None,
            None,
        )
        .unwrap();
    import_inner(&b, &external, &Attestation::default(), now, true).unwrap();
    assert_eq!(crate::receiver_webhook::testing_posts().len(), 1);
}
#[test]
fn pinned_peer_is_accepted_across_configured_relay_aliases() {
    if !crate::test_support::isolated(
        "relay::tests::pinned_peer_is_accepted_across_configured_relay_aliases",
    ) {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("EVERETT_HOME", home.path());
    let (a, b) = devices();
    *NATIVE_DEVICES.lock().unwrap() = vec![a.device.clone(), b.device.clone()];
    crate::storage::private_dir(crate::cloudflare_auth::credential_path().parent().unwrap()).unwrap();
    write(
        &crate::cloudflare_auth::credential_path(),
        &json!({
            "issuer": crate::cloudflare_auth::PUBLIC_ISSUER,
            "base_url": crate::cloudflare_auth::PUBLIC_RELAY,
            "agent_id": b.device.device_id,
            "owner": b.device.owner,
            "device_name": "fixture",
            "token": format!("evt_{}", "a".repeat(64)),
            "expires_at": u64::MAX
        }),
    )
    .unwrap();
    crate::storage::private_dir(&root().join("peers")).unwrap();
    let save = |deployment: &str| {
        write(
            &peer_path("peer").unwrap(),
            &Paired {
                device: a.device.clone(),
                deployment: deployment.into(),
                grant_id: None,
            },
        )
        .unwrap();
    };

    assert!(crate::cloudflare_auth::same_relay(
        crate::cloudflare_auth::PUBLIC_ISSUER,
        crate::cloudflare_auth::PUBLIC_RELAY
    ));
    assert!(!crate::cloudflare_auth::same_relay(
        "https://alias-a.example",
        "https://alias-b.example"
    ));
    save(crate::cloudflare_auth::PUBLIC_ISSUER);
    let accepted = paired("peer", &b.device.owner).unwrap();
    assert_eq!(accepted.device.device_id, a.device.device_id);
    assert_eq!(accepted.deployment, crate::cloudflare_auth::PUBLIC_RELAY);
    revoke_device_with(
        &a.device.device_id,
        &b.device.owner,
        crate::cloudflare_auth::PUBLIC_RELAY,
        |_, _, _| Ok(json!({})),
    )
    .unwrap();
    assert!(!peer_path("peer").unwrap().exists());

    save("https://foreign.example");
    let rejected = match paired("peer", &b.device.owner) {
        Err(error) => error,
        Ok(_) => panic!("foreign deployment was accepted"),
    };
    assert!(
        rejected.message.contains("relay deployment"),
        "{}",
        rejected.message
    );
    assert!(peer_path("peer").unwrap().exists());
    revoke_device_with(
        &a.device.device_id,
        &b.device.owner,
        crate::cloudflare_auth::PUBLIC_RELAY,
        |_, _, _| Ok(json!({})),
    )
    .unwrap();
    assert!(
        peer_path("peer").unwrap().exists(),
        "a foreign pin is a different relay"
    );

    std::env::set_var(
        "EVERETT_AUTH_RELAY_ALIASES",
        "https://alias-a.example, https://alias-b.example",
    );
    std::env::set_var("EVERETT_AUTH_BASE_URL", "https://alias-b.example");
    assert!(!crate::cloudflare_auth::same_relay(
        crate::cloudflare_auth::PUBLIC_ISSUER,
        crate::cloudflare_auth::PUBLIC_RELAY
    ));
    save("https://alias-a.example");
    let custom = paired("peer", &b.device.owner).unwrap();
    assert_eq!(custom.deployment, "https://alias-b.example");
    save(crate::cloudflare_auth::PUBLIC_ISSUER);
    match paired("peer", &b.device.owner) {
        Err(error) => assert!(
            error.message.contains("relay deployment"),
            "{}",
            error.message
        ),
        Ok(_) => panic!("explicit aliases must not keep the public pair"),
    }
}

#[test]
fn inbox_address_matches_the_relay_wire_rule() {
    for valid_address in ["claude@wmm.10462", "a@b", "x-y_z.1@z.9"] {
        assert!(inbox_address(valid_address), "{valid_address}");
    }
    for invalid_address in [
        "Claude@wmm.10462",
        "claude",
        "a@b@c",
        "a..b@zone",
        "a@z..one",
        ".a@zone",
        "a.@zone",
        "-a@zone",
        "a-@zone",
        "a@zone.",
        "a@-zone",
        "@zone",
        "a@",
        &format!("{}@zone", "a".repeat(65)),
        &format!("a@{}", "z".repeat(65)),
    ] {
        assert!(!inbox_address(invalid_address), "{invalid_address}");
    }
}

#[test]
fn legacy_bare_envelope_stage_files_still_load() {
    let (a, b) = devices();
    let envelope = a
        .seal(&b.device, "legacy", &payload(1), 1000, TTL, None, None)
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bare.json");
    write(&path, &envelope).unwrap();
    let (staged, recovered, attestation) = read_staged(&path).unwrap();
    assert!(matches!(staged, StagedFile::Bare(_)));
    assert_eq!(recovered, envelope);
    assert!(attestation.inbox.is_none() && attestation.sender_device.is_none());
    let staged_path = dir.path().join("staged.json");
    let record = StagedFile::Row(Staged::new(&envelope, &Attestation {
        inbox: Some(InboxBinding {
            address: "claude@wmm.10462".into(),
            harness: "claude".into(),
            session: "inbox-b".into(),
        }),
        sender_device: Some(a.device.clone()),
    }));
    write(&staged_path, &record).unwrap();
    let (staged, recovered, attestation) = read_staged(&staged_path).unwrap();
    assert!(matches!(staged, StagedFile::Row(_)));
    assert_eq!(recovered, envelope);
    assert_eq!(attestation.inbox.unwrap().session, "inbox-b");
    assert_eq!(attestation.sender_device.unwrap(), a.device);
}

fn cross_org_devices() -> (Keys, Keys) {
    let alice_owner = json!(["https://issuer.example/", "org", "org-a"]).to_string();
    let bob_owner = json!(["https://issuer.example/", "org", "org-b"]).to_string();
    (
        Keys::new(alice_owner, "agent-a".into(), [1; 32], [2; 32]),
        Keys::new(bob_owner, "agent-b".into(), [3; 32], [4; 32]),
    )
}

fn activate(home: &Path, keys: &Keys, seeds: ([u8; 32], [u8; 32]), devices: Vec<Device>) {
    std::env::set_var("HOME", home);
    std::env::set_var("EVERETT_HOME", home);
    configure(keys, devices);
    initialize().unwrap();
    write(
        &key_path(&keys.device.owner, &keys.device.device_id),
        &PrivateKeys {
            signing: hex::encode(seeds.0),
            encryption: hex::encode(seeds.1),
        },
    )
    .unwrap();
}

fn grant() -> String {
    format!("grant-{}", "ab".repeat(16))
}

fn cross_org_envelope(sender: &Keys, recipient: &Keys, session: &str, grant_id: &str) -> Envelope {
    sender
        .seal(
            &recipient.device,
            "cross-org-request",
            &Payload::Inbox {
                session: session.into(),
                harness: "claude".into(),
                text: "synthetic cross-org message".into(),
                from: "inbox-a".into(),
                from_harness: "codex".into(),
                from_card: String::new(),
                reply_to: String::new(),
                hops: 1,
                external_untrusted: true,
            },
            milliseconds(),
            TTL,
            Some(grant_id.into()),
            None,
        )
        .unwrap()
}

fn pending_page(rows: Vec<Value>, now: i64) -> Value {
    json!({
        "serverNow": now,
        "envelopes": rows,
        "isDone": true,
        "continueCursor": "0",
    })
}

#[test]
fn cross_org_address_send_and_correlated_reply_between_two_accounts() {
    if !crate::test_support::isolated("relay::tests::cross_org_address_send_and_correlated_reply_between_two_accounts") {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    std::env::set_var("EVERETT_NOTIFY", "none");
    std::env::set_var("EVERETT_AUTH_PROVIDER", "cloudflare");
    let (alice, bob) = cross_org_devices();
    let home_a = tempfile::tempdir().unwrap();
    let home_b = tempfile::tempdir().unwrap();
    let address = "claude@wmm.10462";
    let grant_id = grant();
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Alice resolves the named inbox and enqueues a cross-org root request.
    activate(home_a.path(), &alice, ([1; 32], [2; 32]), vec![]);
    *NATIVE_SCRIPT.lock().unwrap() = vec![
        (
            "relay:resolveInbox",
            Ok(json!({"address":address,"owner":bob.device.owner,"device":bob.device,"harness":"claude","session":"inbox-b","grantId":grant_id})),
        ),
        ("relay:enqueue", Ok(json!({"state":"queued"}))),
    ]
    .into();
    let sent = send_address_as(address, "Hello across orgs", Some("req-key"), Some("inbox-a"), Some("codex")).unwrap();
    assert_eq!(sent["external"], true);
    assert_eq!(sent["grant_id"], json!(grant_id));
    assert_eq!(sent["inbox"], json!({"harness":"claude","session":"inbox-b"}));
    let calls = NATIVE_CALLS.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "relay:resolveInbox");
    let request_envelope: Envelope = serde_json::from_value(calls[1].1["envelope"].clone()).unwrap();
    assert_eq!(request_envelope.owner, alice.device.owner);
    assert_eq!(request_envelope.recipient_owner, bob.device.owner);
    assert_eq!(request_envelope.grant_id.as_deref(), Some(grant_id.as_str()));
    assert_eq!(request_envelope.hop, 1);
    assert_eq!(request_envelope.parent, None);
    assert_eq!(request_envelope.to_device, "agent-b");
    match bob.open(&alice.device, &request_envelope, milliseconds()).unwrap() {
        Payload::Inbox { session, harness, external_untrusted, hops, .. } => {
            assert_eq!((session.as_str(), harness.as_str()), ("inbox-b", "claude"));
            assert!(external_untrusted);
            assert_eq!(hops, 1);
        }
        _ => panic!("request must be an inbox payload"),
    }
    drop(calls);

    // Bob picks up the request with its server attestation and imports it.
    activate(home_b.path(), &bob, ([3; 32], [4; 32]), vec![]);
    let ingress = root().join("ingress");
    crate::storage::private_dir(&ingress).unwrap();
    let now = milliseconds();
    let mut client = TestClient::fake(vec![
        (
            "relay:pending",
            Ok(pending_page(
                vec![json!({"envelope":request_envelope,"trust":"external-untrusted","inbox":{"address":address,"harness":"claude","session":"inbox-b"},"senderDevice":alice.device})],
                now,
            )),
        ),
        ("relay:observe", Ok(json!({}))),
    ]);
    let mut deferred = false;
    rt.block_on(receive(&mut client, &bob, &ingress, &mut RejectedCache::new(), &mut deferred)).unwrap();
    let messages = inbox::read("inbox-b");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["external"], true);
    assert_eq!(messages[0]["untrusted"], true);
    assert_eq!(messages[0]["trust"], "external-untrusted");
    assert_eq!(messages[0]["relay_grant_id"], json!(grant_id));
    assert_eq!(messages[0]["relay_inbox_address"], address);
    assert!(messages[0]["relay_sender_device"].is_object());
    assert!(crate::paths::glob(&root().join("peers"), "*.json").is_empty(), "attested senders never create local pins");
    assert!(receiver_wake::pending(&root(), now).unwrap().is_empty(), "cross-org imports never open receiver wake");

    // Bob replies; the reply binds the attested sender bundle and the same grant.
    let local_id = local_message_id(&request_envelope);
    *NATIVE_SCRIPT.lock().unwrap() = vec![
        ("relay:enqueue", Ok(json!({"state":"queued"}))),
        ("relay:observe", Ok(json!({}))),
    ]
    .into();
    let reply = crate::send::reply_as(&local_id, "Correlated reply from Bob", Some("inbox-b"), Some("claude")).unwrap();
    assert_eq!(reply["mode"], "relay-reply");
    let calls = NATIVE_CALLS.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    let reply_envelope: Envelope = serde_json::from_value(calls[0].1["envelope"].clone()).unwrap();
    assert_eq!(reply_envelope.parent, Some(Parent { owner: alice.device.owner.clone(), message_id: request_envelope.message_id.clone() }));
    assert_eq!(reply_envelope.hop, 2);
    assert_eq!(reply_envelope.grant_id.as_deref(), Some(grant_id.as_str()));
    assert_eq!(reply_envelope.to_device, "agent-a");
    assert_eq!(calls[1].1["state"], "replied");
    assert_eq!(calls[1].1["messageId"], json!(request_envelope.message_id));
    drop(calls);

    // Alice picks up the correlated reply; her outgoing record flips to replied.
    activate(home_a.path(), &alice, ([1; 32], [2; 32]), vec![]);
    let ingress = root().join("ingress");
    crate::storage::private_dir(&ingress).unwrap();
    let mut client = TestClient::fake(vec![
        (
            "relay:pending",
            Ok(pending_page(
                vec![json!({"envelope":reply_envelope,"trust":"external-untrusted","senderDevice":bob.device})],
                milliseconds(),
            )),
        ),
        ("relay:observe", Ok(json!({}))),
    ]);
    rt.block_on(receive(&mut client, &alice, &ingress, &mut RejectedCache::new(), &mut deferred)).unwrap();
    let (_, outgoing) = find_outgoing(&alice.device.owner, &request_envelope.message_id).unwrap();
    assert_eq!(outgoing.state, State::Replied);
    let replies = inbox::read("inbox-a");
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["external"], true);
    assert_eq!(replies[0]["untrusted"], true);
    assert_eq!(replies[0]["trust"], "external-untrusted");
}

#[test]
fn cross_org_exact_inbox_mismatch_and_forged_sender_bundle_are_rejected() {
    if !crate::test_support::isolated("relay::tests::cross_org_exact_inbox_mismatch_and_forged_sender_bundle_are_rejected") {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    std::env::set_var("EVERETT_NOTIFY", "none");
    std::env::set_var("EVERETT_AUTH_PROVIDER", "cloudflare");
    let (alice, bob) = cross_org_devices();
    let eve = Keys::new(
        json!(["https://issuer.example/", "org", "org-c"]).to_string(),
        "agent-e".into(),
        [9; 32],
        [10; 32],
    );
    let home_b = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    activate(home_b.path(), &bob, ([3; 32], [4; 32]), vec![]);
    let ingress = root().join("ingress");
    crate::storage::private_dir(&ingress).unwrap();
    let grant_id = grant();
    let attested_row = |envelope: &Envelope, session: &str| {
        json!({"envelope":envelope,"trust":"external-untrusted","inbox":{"address":"claude@wmm.10462","harness":"claude","session":session},"senderDevice":alice.device})
    };

    // (a) Payload sealed for another session than the attested inbox.
    for (payload_session, inbox_session, harness) in [
        ("other", "inbox-b", "claude"),
        ("inbox-b", "inbox-b", "codex"),
    ] {
        let envelope = cross_org_envelope(&alice, &bob, payload_session, &grant_id);
        let row = if harness == "codex" {
            json!({"envelope":envelope,"inbox":{"address":"claude@wmm.10462","harness":"codex","session":inbox_session},"senderDevice":alice.device})
        } else {
            attested_row(&envelope, inbox_session)
        };
        let mut client = TestClient::fake(vec![(
            "relay:pending",
            Ok(pending_page(vec![row], milliseconds())),
        )]);
        rt.block_on(receive(&mut client, &bob, &ingress, &mut RejectedCache::new(), &mut false)).unwrap();
        assert!(inbox::read("inbox-b").is_empty() && inbox::read("other").is_empty());
        assert_eq!(client.calls.len(), 1, "nothing authorized, observed, or imported");
    }

    // (c) A root row with a sender bundle but no inbox binding.
    let envelope = cross_org_envelope(&alice, &bob, "inbox-b", &grant_id);
    let mut client = TestClient::fake(vec![(
        "relay:pending",
        Ok(pending_page(
            vec![json!({"envelope":envelope,"trust":"external-untrusted","senderDevice":alice.device})],
            milliseconds(),
        )),
    )]);
    rt.block_on(receive(&mut client, &bob, &ingress, &mut RejectedCache::new(), &mut false)).unwrap();
    assert!(inbox::read("inbox-b").is_empty());
    assert_eq!(client.calls.len(), 1);

    // (d) A sender bundle that does not match the sealed envelope.
    let envelope = cross_org_envelope(&alice, &bob, "inbox-b", &grant_id);
    let mut client = TestClient::fake(vec![(
        "relay:pending",
        Ok(pending_page(
            vec![json!({"envelope":envelope,"trust":"external-untrusted","inbox":{"address":"claude@wmm.10462","harness":"claude","session":"inbox-b"},"senderDevice":eve.device})],
            milliseconds(),
        )),
    )]);
    rt.block_on(receive(&mut client, &bob, &ingress, &mut RejectedCache::new(), &mut false)).unwrap();
    assert!(inbox::read("inbox-b").is_empty());
    assert_eq!(client.calls.len(), 1);

    // (e) A staged record whose attested inbox was tampered with on disk.
    let envelope = cross_org_envelope(&alice, &bob, "inbox-b", &grant_id);
    let staged_path = ingress.join("tampered.json");
    write(
        &staged_path,
        &StagedFile::Row(Staged::new(
            &envelope,
            &Attestation {
                inbox: Some(InboxBinding {
                    address: "claude@wmm.10462".into(),
                    harness: "claude".into(),
                    session: "tampered".into(),
                }),
                sender_device: Some(alice.device.clone()),
            },
        )),
    )
    .unwrap();
    let mut client = TestClient::fake(vec![]);
    let (staged, recovered, attestation) = read_staged(&staged_path).unwrap();
    rt.block_on(publish_staged(&mut client, &bob, &staged_path, &staged, &recovered, &attestation)).unwrap();
    assert!(!staged_path.exists(), "a tampered stage is refused and removed");
    assert!(inbox::read("inbox-b").is_empty() && inbox::read("tampered").is_empty());
    assert!(client.calls.is_empty());
}

#[test]
fn cross_org_denied_sender_and_revoked_grant_are_rejected() {
    if !crate::test_support::isolated("relay::tests::cross_org_denied_sender_and_revoked_grant_are_rejected") {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    std::env::set_var("EVERETT_NOTIFY", "none");
    std::env::set_var("EVERETT_AUTH_PROVIDER", "cloudflare");
    let (alice, bob) = cross_org_devices();
    let home_a = tempfile::tempdir().unwrap();
    let home_b = tempfile::tempdir().unwrap();
    let address = "claude@wmm.10462";
    let grant_id = grant();
    let rt = tokio::runtime::Runtime::new().unwrap();

    // A denied resolve leaves no enqueue attempt and no outgoing record.
    activate(home_a.path(), &alice, ([1; 32], [2; 32]), vec![]);
    *NATIVE_SCRIPT.lock().unwrap() = vec![(
        "relay:resolveInbox",
        Err("Cloudflare relay denied resolveInbox (HTTP 403, inbox_not_allowed)"),
    )]
    .into();
    let error = send_address_as(address, "denied", None, None, None).unwrap_err();
    assert!(denied(&error));
    assert_eq!(NATIVE_CALLS.lock().unwrap().len(), 1);
    assert!(crate::paths::glob(&root().join("outgoing"), "*.json").is_empty());

    // A grant revoked between resolve and enqueue surfaces the denial.
    *NATIVE_SCRIPT.lock().unwrap() = vec![
        (
            "relay:resolveInbox",
            Ok(json!({"address":address,"owner":bob.device.owner,"device":bob.device,"harness":"claude","session":"inbox-b","grantId":grant_id})),
        ),
        ("relay:enqueue", Err("Cloudflare relay denied: revoked grant")),
    ]
    .into();
    let error = send_address_as(address, "revoked", Some("revoked-key"), None, None).unwrap_err();
    assert!(denied(&error));

    // Receiver side: a staged cross-org row whose observe reports revocation is
    // removed without import once our own identity is verified.
    activate(home_b.path(), &bob, ([3; 32], [4; 32]), vec![]);
    let ingress = root().join("ingress");
    crate::storage::private_dir(&ingress).unwrap();
    let envelope = cross_org_envelope(&alice, &bob, "inbox-b", &grant_id);
    let staged_path = ingress.join("revoked.json");
    write(
        &staged_path,
        &StagedFile::Row(Staged::new(
            &envelope,
            &Attestation {
                inbox: Some(InboxBinding {
                    address: address.into(),
                    harness: "claude".into(),
                    session: "inbox-b".into(),
                }),
                sender_device: Some(alice.device.clone()),
            },
        )),
    )
    .unwrap();
    let mut client = TestClient::fake(vec![
        ("relay:observe", Err("Cloudflare relay denied: revoked grant")),
        ("relay:whoami", Ok(json!({"owner":bob.device.owner,"deviceId":bob.device.device_id}))),
    ]);
    let (staged, recovered, attestation) = read_staged(&staged_path).unwrap();
    rt.block_on(publish_staged(&mut client, &bob, &staged_path, &staged, &recovered, &attestation)).unwrap();
    assert!(!staged_path.exists());
    assert!(inbox::read("inbox-b").is_empty());

    // A reply attempt after revocation errors instead of posting.
    let mut client = TestClient::fake(vec![
        (
            "relay:pending",
            Ok(pending_page(
                vec![json!({"envelope":envelope,"trust":"external-untrusted","inbox":{"address":address,"harness":"claude","session":"inbox-b"},"senderDevice":alice.device})],
                milliseconds(),
            )),
        ),
        ("relay:observe", Ok(json!({}))),
    ]);
    let mut deferred = false;
    rt.block_on(receive(&mut client, &bob, &ingress, &mut RejectedCache::new(), &mut deferred)).unwrap();
    assert_eq!(inbox::read("inbox-b").len(), 1);
    *NATIVE_SCRIPT.lock().unwrap() = vec![(
        "relay:enqueue",
        Err("Cloudflare relay denied: revoked grant"),
    )]
    .into();
    let local_id = local_message_id(&envelope);
    assert!(crate::send::reply_as(&local_id, "too late", Some("inbox-b"), Some("claude")).is_err());
    let paths = crate::paths::glob(&root().join("outgoing"), "*.json");
    assert_eq!(paths.len(), 1);
    let reply_out: Outgoing = read(&paths[0]).unwrap();
    assert!(!reply_out.posted, "the denied reply stays an unposted retry record");
    assert!(reply_out.reply_pending.is_some());
}

#[test]
fn local_access_policy_enforces_cross_org_send_and_receive_rules() {
    if !crate::test_support::isolated(
        "relay::tests::local_access_policy_enforces_cross_org_send_and_receive_rules",
    ) {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    let home_a = tempfile::tempdir().unwrap();
    let home_b = tempfile::tempdir().unwrap();
    let owner_a = r#"["https://issuer.example/","org","org-a"]"#;
    let owner_b = r#"["https://issuer.example/","org","org-b"]"#;
    let alice = Keys::new(owner_a.into(), "agent-a".into(), [1; 32], [2; 32]);
    let bob = Keys::new(owner_b.into(), "agent-b".into(), [3; 32], [4; 32]);
    let address = "claude@wmm.10462";
    let grant_id = "grant-0123456789abcdef0123456789abcdef";

    // Outbound cross-org send is blocked when local access disables external sends.
    activate(home_a.path(), &alice, ([1; 32], [2; 32]), vec![]);
    crate::access::set_settings(None, Some("allowlist"), Some(false), None).unwrap();
    *NATIVE_SCRIPT.lock().unwrap() = vec![(
        "relay:resolveInbox",
        Ok(json!({"address":address,"owner":bob.device.owner,"device":bob.device,"harness":"claude","session":"inbox-b","grantId":grant_id})),
    )]
    .into();
    let err = send_address_as(address, "hello", Some("blocked-out"), None, None).unwrap_err();
    assert!(err.message.contains("disabled by local access policy"), "{}", err.message);

    // Incoming cross-org receive respects mode, allowlist, and blocklist rules.
    activate(home_b.path(), &bob, ([3; 32], [4; 32]), vec![]);
    let envelope = cross_org_envelope(&alice, &bob, "inbox-b", grant_id);
    let attestation = Attestation {
        inbox: Some(InboxBinding {
            address: address.into(),
            harness: "claude".into(),
            session: "inbox-b".into(),
        }),
        sender_device: Some(alice.device.clone()),
    };
    let now = milliseconds();

    crate::access::set_settings(None, Some("same-account"), None, None).unwrap();
    assert!(import_inner(&bob, &envelope, &attestation, now, true).is_err());
    assert!(inbox::read("inbox-b").is_empty());

    crate::access::set_settings(None, Some("allowlist"), None, None).unwrap();
    crate::access::allow_sender(
        None,
        owner_a,
        "agent-other",
        Some(address),
        None,
        false,
    )
    .unwrap();
    assert!(import_inner(&bob, &envelope, &attestation, now, true).is_err());

    crate::access::allow_sender(
        None,
        owner_a,
        &alice.device.device_id,
        Some(address),
        Some("Alice"),
        false,
    )
    .unwrap();
    import_inner(&bob, &envelope, &attestation, now, true).unwrap();
    assert_eq!(inbox::read("inbox-b").len(), 1);

    // Turning off external sends also blocks cross-org replies and background reconcile retries.
    crate::access::set_settings(None, Some("allowlist"), Some(false), None).unwrap();
    let local_id = local_message_id(&envelope);
    let reply_err =
        crate::send::reply_as(&local_id, "blocked reply", Some("inbox-b"), Some("claude"))
            .unwrap_err();
    assert!(
        reply_err.message.contains("disabled by local access policy"),
        "{}",
        reply_err.message
    );
    assert!(crate::paths::glob(&root().join("outgoing"), "*.json").is_empty());

    // Simulate a pre-existing unposted cross-org outgoing record: reconcile must not post it while external sends are off.
    let pending_reply_pin = Paired {
        device: alice.device.clone(),
        deployment: deployment().unwrap(),
        grant_id: Some(grant_id.into()),
    };
    let reply_payload = Payload::Inbox {
        session: "inbox-a".into(),
        harness: "codex".into(),
        text: "queued earlier".into(),
        from: "inbox-b".into(),
        from_harness: "claude".into(),
        from_card: String::new(),
        reply_to: envelope.message_id.clone(),
        hops: 2,
        external_untrusted: true,
    };
    let (reply_path, _) = prepare_for(
        address,
        pending_reply_pin,
        true,
        "reconcile-blocked-key",
        &reply_payload,
        &bob,
        Some(Parent {
            owner: envelope.owner.clone(),
            message_id: envelope.message_id.clone(),
        }),
    )
    .unwrap();
    let mut client = TestClient::fake(vec![]);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(reconcile(&mut client, &bob, &mut RejectedCache::new()))
        .unwrap();
    let still_unposted: Outgoing = read(&reply_path).unwrap();
    assert!(!still_unposted.posted);

    crate::access::set_settings(None, Some("allowlist"), Some(true), None).unwrap();
    crate::access::deny_sender(None, owner_a, None, Some("stop")).unwrap();
    let envelope2 = cross_org_envelope(&alice, &bob, "inbox-b", grant_id);
    assert!(import_inner(&bob, &envelope2, &attestation, now, true).is_err());
    assert_eq!(inbox::read("inbox-b").len(), 1);
}
