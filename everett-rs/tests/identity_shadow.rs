#[path = "../src/identity_shadow.rs"]
mod identity_shadow;
use identity_shadow::*;

fn account(subject: &str) -> AccountPrincipal {
    AccountPrincipal {
        issuer: "fixture-issuer".into(),
        subject: subject.into(),
    }
}
fn fixture() -> Snapshot {
    Snapshot {
        clients: vec![
            ClientDevice {
                id: "plugin".into(),
                account: account("owner"),
                revoked: false,
            },
            ClientDevice {
                id: "native".into(),
                account: account("owner"),
                revoked: false,
            },
            ClientDevice {
                id: "foreign".into(),
                account: account("other"),
                revoked: false,
            },
        ],
        sessions: [
            ("chat-a", "plugin", "chat"),
            ("chat-b", "plugin", "chat"),
            ("dot-a", "plugin", "dot"),
            ("codex-a", "native", "codex"),
            ("other-a", "foreign", "chat"),
        ]
        .into_iter()
        .map(|(id, client, kind)| SessionIdentity {
            client_id: client.into(),
            session_id: id.into(),
            name: "Same name".into(),
            kind: kind.into(),
            provenance: if client == "native" {
                Provenance::HostAttested
            } else {
                Provenance::OwnerRegistered
            },
            registration: Registration::Registered,
            reachability: Reachability::Unknown,
            aliases: vec![],
        })
        .collect(),
    }
}
fn request(to: &str) -> Request {
    Request {
        account: account("owner"),
        from: "chat-a".into(),
        to: to.into(),
        capability: Capability::Message,
        current: Decision::Deny,
    }
}
#[test]
fn account_discovery_keeps_four_distinct_sessions_with_duplicate_names() {
    let snapshot = fixture();
    let rows = snapshot.discover(&account("owner")).unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(
        rows.iter()
            .map(|r| &r.label)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
    assert!(rows.iter().all(|r| !r.client_revoked));
    assert_eq!(rows[0].identity.provenance, Provenance::OwnerRegistered);
    assert_eq!(rows[3].identity.provenance, Provenance::HostAttested);
}
#[test]
fn same_owner_is_only_a_shadow_difference() {
    let result = fixture().compare(request("codex-a")).unwrap();
    assert_eq!(result.current, Decision::Deny);
    assert_eq!(result.proposed, Decision::Allow);
    assert!(result.changed && result.shadow_only);
    assert_eq!(result.canonical_recipient.as_deref(), Some("codex-a"));
}
#[test]
fn foreign_forged_and_display_names_do_not_route() {
    for target in ["other-a", "invented", "Same name", "plugin"] {
        assert_eq!(
            fixture().compare(request(target)).unwrap().proposed,
            Decision::Deny
        );
    }
    let mut forged = request("codex-a");
    forged.account = account("other");
    assert_eq!(fixture().compare(forged).unwrap().proposed, Decision::Deny);
}
#[test]
fn revocation_and_missing_registration_fail_closed() {
    for state in [Registration::Revoked, Registration::Unavailable] {
        let mut snapshot = fixture();
        snapshot.sessions[3].registration = state;
        assert_eq!(
            snapshot.compare(request("codex-a")).unwrap().proposed,
            Decision::Deny
        );
    }
    let mut snapshot = fixture();
    snapshot.clients[0].revoked = true;
    assert_eq!(
        snapshot.compare(request("codex-a")).unwrap().proposed,
        Decision::Deny
    );
}
#[test]
fn reconnect_and_stale_metadata_preserve_ids_and_messaging_policy() {
    let mut snapshot = fixture();
    for state in [
        Reachability::Online,
        Reachability::Stale,
        Reachability::Offline,
        Reachability::Unknown,
    ] {
        snapshot.sessions[3].reachability = state;
        assert_eq!(
            snapshot
                .compare(request("codex-a"))
                .unwrap()
                .canonical_recipient
                .as_deref(),
            Some("codex-a")
        );
        assert_eq!(
            snapshot.compare(request("codex-a")).unwrap().proposed,
            Decision::Allow
        );
    }
}
#[test]
fn aliases_preserve_canonical_inbox_and_cannot_shadow_other_identity() {
    let mut snapshot = fixture();
    snapshot.sessions[3].aliases.push("old-codex".into());
    assert_eq!(
        snapshot
            .compare(request("old-codex"))
            .unwrap()
            .canonical_recipient
            .as_deref(),
        Some("codex-a")
    );
    snapshot.sessions[3].aliases.push("chat-a".into());
    assert!(snapshot.validate().is_err());
}
#[test]
fn duplicate_ids_and_unmapped_clients_are_rejected() {
    let mut snapshot = fixture();
    snapshot.sessions[1].session_id = "chat-a".into();
    assert!(snapshot.validate().is_err());
    snapshot = fixture();
    snapshot.sessions[1].client_id = "unmapped".into();
    assert!(snapshot.validate().is_err());
}
#[test]
fn messaging_never_grants_other_capabilities_or_changes_current_allow() {
    for capability in [
        Capability::Wake,
        Capability::Execute,
        Capability::TranscriptRead,
        Capability::SharedMemory,
    ] {
        let mut r = request("codex-a");
        r.capability = capability;
        r.current = Decision::Allow;
        let result = fixture().compare(r).unwrap();
        assert_eq!(result.proposed, Decision::Deny);
        assert_eq!(result.current, Decision::Allow);
    }
}
#[test]
fn issuer_is_part_of_account_identity() {
    let mut r = request("codex-a");
    r.account.issuer = "forged-issuer".into();
    assert_eq!(fixture().compare(r).unwrap().proposed, Decision::Deny);
}

#[test]
fn current_native_and_gateway_records_project_without_invented_online_state() {
    let native: everett::session::Session = serde_json::from_value(serde_json::json!({
        "id":"native-existing", "harness":"codex", "title":"Work", "cwd":"/fixture",
        "path":"", "started":"", "last_active":1.0, "running":true
    }))
    .unwrap();
    let registered = serde_json::json!({"session_id":"chat-existing", "name":"Work", "kind":"chat",
        "state":"registered", "created_at":1.0, "availability":"unknown"});
    let n = SessionIdentity::from_native("native", &native);
    let c = SessionIdentity::from_registered("plugin", &registered).unwrap();
    assert_eq!(n.session_id, "native-existing");
    assert_eq!(n.reachability, Reachability::Unknown);
    assert_eq!(c.session_id, "chat-existing");
    assert_eq!(c.reachability, Reachability::Unknown);
    assert_eq!(c.provenance, Provenance::OwnerRegistered);
    let mut invalid = registered;
    invalid["state"] = serde_json::json!("new_unsupported_state");
    assert!(SessionIdentity::from_registered("plugin", &invalid).is_err());
}
