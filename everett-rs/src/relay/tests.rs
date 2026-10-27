use super::*;
use crate::receiver_wake;
use std::sync::Mutex;

static RECEIVER_ENV_LOCK: Mutex<()> = Mutex::new(());

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
    peer::private_dir(&ingress).unwrap();
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
        .seal(&b.device, "key", &payload(4), 1000, TTL, None, None)
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
    let converted = args(json!({"envelope":e})).unwrap();
    let round_trip: Value = converted["envelope"].clone().into();
    let decoded: Envelope = serde_json::from_value(round_trip).unwrap();
    assert_eq!(e, decoded);
    assert_eq!(e.signed_bytes(), decoded.signed_bytes());
}

#[test]
fn local_id_token_claim_consistency_and_namespace_checks() {
    let stored: auth::Stored = serde_json::from_value(json!({
        "issuer":"https://issuer.example/","client_id":"client","sub":"alice","email":"",
        "access_token":"","id_token":"","expires_at":5000.0
    }))
    .unwrap();
    let claim = json!({"iss":stored.issuer,"aud":"client","sub":"alice","iat":900,"exp":2000,"everett_agent":"agent-a","everett_org":"org-a"});
    assert_eq!(
        checked_identity(&stored, &claim, 1000.0).unwrap(),
        json!([stored.issuer, "org", "org-a"]).to_string()
    );
    for (key, value) in [
        ("iss", json!("wrong")),
        ("aud", json!("wrong")),
        ("sub", json!("wrong")),
        ("exp", json!(999)),
        ("everett_agent", Value::Null),
        ("everett_org", json!(4)),
        ("iat", json!(1100)),
        ("nbf", json!(1100)),
    ] {
        let mut invalid = claim.clone();
        invalid[key] = value;
        assert!(
            checked_identity(&stored, &invalid, 1000.0).is_err(),
            "{key}"
        );
    }
    let mut account = claim.clone();
    account.as_object_mut().unwrap().remove("everett_org");
    assert_eq!(
        checked_identity(&stored, &account, 1000.0).unwrap(),
        json!([stored.issuer, "account", "alice"]).to_string()
    );
    let mut audience_array = claim;
    audience_array["aud"] = json!(["client", "other"]);
    audience_array["azp"] = json!("client");
    assert!(checked_identity(&stored, &audience_array, 1000.0).is_ok());
    audience_array["azp"] = json!("other");
    assert!(checked_identity(&stored, &audience_array, 1000.0).is_err());
    assert!(claims("opaque-token").is_err());
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
        deployment: "https://test.convex.cloud".into(),
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
        let err = bad(format!("Convex rejected the request: {reason}"));
        assert!(finish_staged(&b, &path, &e, Err(err)).is_err());
        assert!(
            !path.exists(),
            "denial must remove the cached encrypted row"
        );
    }
    let path = dir.path().join("uncertain.json");
    write(&path, &e).unwrap();
    assert!(finish_staged(&b, &path, &e, Err(bad("Convex mutation timed out"))).is_err());
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
fn unsafe_convex_numbers_are_rejected() {
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
    let mut client = ConvexClient::fake(vec![("relay:device", Ok(json!(new.device.clone())))]);
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

    let mut client = ConvexClient::fake(vec![
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

    let mut client = ConvexClient::fake(vec![("relay:device", Ok(json!(new.device.clone())))]);
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

// Test-only transport around the official Rust SDK's sans-I/O protocol core.
// Relay production args/result handling, signing, observation and publication
// execute unchanged; there are no sockets and no mock of authorization helpers.
pub(super) struct ConvexClient {
    live: Option<super::transport::Client>,
    base: convex::base_client::BaseConvexClient,
    version: convex_sync_types::StateVersion,
    responses: std::collections::VecDeque<(&'static str, std::result::Result<Value, &'static str>)>,
    calls: Vec<(String, Value)>,
}

impl ConvexClient {
    fn fake(responses: Vec<(&'static str, std::result::Result<Value, &'static str>)>) -> Self {
        Self {
            live: None,
            base: convex::base_client::BaseConvexClient::new(),
            version: convex_sync_types::StateVersion::initial(),
            responses: responses.into(),
            calls: vec![],
        }
    }

    pub fn native(stored: crate::cloudflare_auth::Credential) -> Self {
        let mut value = Self::fake(vec![]);
        value.live = Some(super::transport::Client::native(stored));
        value
    }
    pub fn is_native(&self) -> bool {
        self.live.as_ref().is_some_and(|client| client.is_native())
    }
    pub async fn native_call(&self, name: &str, value: Value) -> Result<Value> {
        self.live.as_ref().unwrap().native_call(name, value).await
    }

    pub async fn new(url: &str) -> anyhow::Result<Self> {
        let mut value = Self::fake(vec![]);
        value.live = Some(super::transport::Client::new(url).await?);
        Ok(value)
    }

    pub async fn tls_error(&mut self) -> anyhow::Error {
        match self.live.as_mut() {
            Some(live) => live.tls_error().await,
            None => std::future::pending().await,
        }
    }

    pub async fn set_auth_callback(
        &mut self,
        fetcher: Option<convex::base_client::AuthTokenFetcher>,
    ) {
        self.live.as_mut().unwrap().set_auth_callback(fetcher).await;
    }

    pub async fn subscribe(
        &mut self,
        name: &str,
        arguments: BTreeMap<String, convex::Value>,
    ) -> anyhow::Result<convex::QuerySubscription> {
        self.live
            .as_mut()
            .expect("subscriptions use the live SDK")
            .subscribe(name, arguments)
            .await
    }

    fn respond(
        &mut self,
        path: String,
        arguments: &convex_sync_types::types::SerializedArgs,
    ) -> anyhow::Result<
        std::result::Result<convex::Value, convex_sync_types::ErrorPayload<convex::Value>>,
    > {
        let (expected, response) = self
            .responses
            .pop_front()
            .expect("unexpected Convex request");
        assert_eq!(path, expected);
        self.calls.push((
            expected.into(),
            serde_json::from_str::<Value>(arguments.get())?[0].clone(),
        ));
        if response == Err("network") {
            return Err(anyhow::anyhow!("synthetic network failure"));
        }
        Ok(match response {
            Ok(value) => Ok(convex::Value::try_from(value)?),
            Err(error) => Err(convex_sync_types::ErrorPayload::Message(error.into())),
        })
    }

    pub async fn mutation(
        &mut self,
        name: &str,
        arguments: BTreeMap<String, convex::Value>,
    ) -> anyhow::Result<FunctionResult> {
        use convex_sync_types::{ClientMessage, LogLinesMessage, ServerMessage};
        if let Some(live) = self.live.as_mut() {
            return live.mutation(name, arguments).await;
        }
        let future = self.base.mutation(name.parse()?, arguments);
        let Some(ClientMessage::Mutation {
            request_id,
            udf_path,
            args,
            ..
        }) = self.base.pop_next_message()
        else {
            panic!("missing SDK mutation");
        };
        let result = self.respond(udf_path.to_string(), &args)?;
        self.base
            .receive_message(ServerMessage::MutationResponse {
                request_id,
                result,
                ts: Some(self.version.ts),
                log_lines: LogLinesMessage(vec![]),
            })
            .map_err(anyhow::Error::msg)?;
        let end_version = convex_sync_types::StateVersion {
            ts: self.version.ts.succ()?,
            ..self.version
        };
        self.base
            .receive_message(ServerMessage::Transition {
                start_version: self.version,
                end_version,
                modifications: vec![],
                client_clock_skew: None,
                server_ts: None,
            })
            .map_err(anyhow::Error::msg)?;
        self.version = end_version;
        Ok(future.await?)
    }

    pub async fn query(
        &mut self,
        name: &str,
        arguments: BTreeMap<String, convex::Value>,
    ) -> anyhow::Result<FunctionResult> {
        use convex_sync_types::{
            ClientMessage, LogLinesMessage, QuerySetModification, ServerMessage, StateModification,
        };
        if let Some(live) = self.live.as_mut() {
            return live.query(name, arguments).await;
        }
        let id = self.base.subscribe(name.parse()?, arguments);
        let Some(ClientMessage::ModifyQuerySet {
            new_version,
            modifications,
            ..
        }) = self.base.pop_next_message()
        else {
            panic!("missing SDK query");
        };
        let QuerySetModification::Add(query) = &modifications[0] else {
            panic!("missing SDK query add");
        };
        let result = self.respond(query.udf_path.to_string(), &query.args)?;
        let modification = match result {
            Ok(value) => StateModification::QueryUpdated {
                query_id: query.query_id,
                value,
                log_lines: LogLinesMessage(vec![]),
                journal: None,
            },
            Err(error) => StateModification::QueryFailed {
                query_id: query.query_id,
                error_message: error.get_message().into(),
                log_lines: LogLinesMessage(vec![]),
                journal: None,
                error_data: None,
            },
        };
        let end_version = convex_sync_types::StateVersion {
            query_set: new_version,
            ts: self.version.ts.succ()?,
            ..self.version
        };
        self.base
            .receive_message(ServerMessage::Transition {
                start_version: self.version,
                end_version,
                modifications: vec![modification],
                client_clock_skew: None,
                server_ts: None,
            })
            .map_err(anyhow::Error::msg)?;
        self.version = end_version;
        let value = self.base.get_query(query.query_id).unwrap();
        self.base.unsubscribe(id);
        self.base.pop_next_message();
        Ok(value)
    }
}

#[test]
fn sdk_transport_denied_row_does_not_block_allowed_and_own_revocation_stops() {
    if !crate::test_support::isolated("relay::tests::sdk_transport_denied_row_does_not_block_allowed_and_own_revocation_stops") {
        return;
    }
    let _env = RECEIVER_ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("EVERETT_HOME", home.path());
    std::env::set_var("EVERETT_CONVEX_URL", "https://test.convex.cloud");
    std::env::set_var("EVERETT_NOTIFY", "none");
    let (a, b) = devices();
    peer::private_dir(&root().join("peers")).unwrap();
    peer::private_dir(&root().join("ingress")).unwrap();
    write(
        &peer_path("peer").unwrap(),
        &Paired {
            device: a.device.clone(),
            deployment: "https://test.convex.cloud".into(),
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
                text: "synthetic SDK message".into(),
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
    let mut client = ConvexClient::fake(vec![
        ("relay:observe", Err("revoked sender")),
        (
            "relay:whoami",
            Ok(json!({"owner":b.device.owner,"deviceId":b.device.device_id})),
        ),
        ("relay:observe", Ok(json!({}))),
    ]);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        publish_staged(&mut client, &b, &denied_path, &denied_row)
            .await
            .unwrap();
        publish_staged(&mut client, &b, &allowed_path, &allowed_row)
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
        (Err("revoked own agent"), true),
        (
            Ok(json!({"owner":"changed","deviceId":b.device.device_id})),
            true,
        ),
    ] {
        write(&denied_path, &denied_row).unwrap();
        let mut client = ConvexClient::fake(vec![
            ("relay:observe", Err("revoked grant")),
            ("relay:whoami", own_response),
        ]);
        let outcome = rt.block_on(publish_staged(&mut client, &b, &denied_path, &denied_row));
        assert_eq!(outcome.is_err(), terminal);
        assert!(
            denied_path.exists(),
            "uncertainty/own revocation must not delete the sole ciphertext inside publisher"
        );
    }
    let mut transient = ConvexClient::fake(vec![("relay:observe", Err("network"))]);
    rt.block_on(publish_staged(
        &mut transient,
        &b,
        &denied_path,
        &denied_row,
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
    let mut committed = ConvexClient::fake(vec![("relay:observe", Ok(json!({})))]);
    assert!(rt
        .block_on(publish_staged(&mut committed, &b, &blocked_path, &blocked))
        .is_err());
    assert!(blocked_path.exists());
    fs::remove_dir(inbox::path("x-1").unwrap()).unwrap();
    let mut retry = ConvexClient::fake(vec![("relay:observe", Ok(json!({})))]);
    rt.block_on(publish_staged(&mut retry, &b, &blocked_path, &blocked))
        .unwrap();
    assert!(!blocked_path.exists());
    // Claimed observations use the remote ID and remote sender owner, not the
    // independently generated local inbox ID. Reconcile retries idempotently.
    inbox::take("x-1");
    let mut claims = ConvexClient::fake(vec![("relay:observe", Ok(json!({})))]);
    rt.block_on(reconcile(&mut claims, &b, &mut RejectedCache::new())).unwrap();
    assert_eq!(claims.calls[0].1["state"], "claimed");
    assert_eq!(claims.calls[0].1["messageId"], blocked.message_id);
    assert_eq!(claims.calls[0].1["senderOwner"], a.device.owner);
    rt.block_on(reconcile(&mut claims, &b, &mut RejectedCache::new())).unwrap();
    assert!(claims.responses.is_empty());
    let mut replies = ConvexClient::fake(vec![("relay:observe", Ok(json!({})))]);
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

    peer::private_dir(&root().join("outgoing")).unwrap();
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
    let mut unavailable = ConvexClient::fake(vec![("relay:enqueue", Err("network"))]);
    assert!(rt
        .block_on(post(&mut unavailable, &path, &outgoing))
        .is_err());
    assert!(!read::<Outgoing>(&path).unwrap().posted);
    let mut lost_observation = ConvexClient::fake(vec![
        ("relay:enqueue", Ok(json!({}))),
        ("relay:observe", Err("network")),
    ]);
    rt.block_on(reconcile(&mut lost_observation, &b, &mut RejectedCache::new())).unwrap();
    let persisted: Outgoing = read(&path).unwrap();
    assert!(persisted.posted && persisted.reply_pending.is_some());
    let mut recovered = ConvexClient::fake(vec![("relay:observe", Ok(json!({})))]);
    rt.block_on(reconcile(&mut recovered, &b, &mut RejectedCache::new())).unwrap();
    assert_eq!(recovered.calls[0].1["state"], "replied");
    assert_eq!(recovered.calls[0].1["messageId"], blocked.message_id);
    assert_eq!(recovered.calls[0].1["senderOwner"], a.device.owner);
    assert!(read::<Outgoing>(&path).unwrap().reply_pending.is_none());
    rt.block_on(reconcile(&mut recovered, &b, &mut RejectedCache::new())).unwrap();
    assert_eq!(recovered.calls.len(), 1);
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
    std::env::set_var("EVERETT_CONVEX_URL", "https://test.convex.cloud");
    std::env::set_var("EVERETT_NOTIFY", "none");
    let (a, b) = devices();
    peer::private_dir(&root().join("peers")).unwrap();
    write(
        &peer_path("mac").unwrap(),
        &Paired {
            device: a.device.clone(),
            deployment: "https://test.convex.cloud".into(),
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

    assert!(import_inner(&b, &envelope, now, true).is_err());
    assert!(inbox::pending(session_id, None).is_empty());
    write(
        &policy_path,
        &json!({"version":1,"enabled":true,"allow":[{"session_id":session_id,"cwd":cwd,"sender_device_id":a.device.device_id}]}),
    )
    .unwrap();
    import_inner(&b, &envelope, now, true).unwrap();
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
    import_inner(&b, &envelope, now, true).unwrap();
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
    peer::private_dir(&root()).unwrap();
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
    std::env::set_var("EVERETT_CONVEX_URL", "https://test.convex.cloud");
    std::env::set_var("EVERETT_NOTIFY", "none");
    std::env::set_var("EVERETT_AUTH_PROVIDER", "oidc");
    let (a, b) = devices();
    peer::private_dir(&root().join("peers")).unwrap();
    write(
        &peer_path("mac").unwrap(),
        &Paired {
            device: a.device.clone(),
            deployment: "https://test.convex.cloud".into(),
            grant_id: None,
        },
    )
    .unwrap();
    crate::adapters::external::register("ext-grok-acceptance", "grok-bot", "Grok", "/work", false)
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
    import_inner(&b, &envelope, now, true).unwrap();
    let posts = crate::receiver_webhook::testing_posts();
    assert_eq!(posts.len(), 1, "{}", posts.first().map(|post| post.body.as_str()).unwrap_or(""));
    assert_eq!(posts[0].bearer, "test-sender-key-0123456789");
    assert!(posts[0].body.contains("ext-grok-acceptance"));
    assert!(!posts[0].body.contains(phrase));
    import_inner(&b, &envelope, now, true).unwrap();
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
    import_inner(&b, &external, now, true).unwrap();
    assert_eq!(crate::receiver_webhook::testing_posts().len(), 1);
}
