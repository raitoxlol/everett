use super::*;

pub(super) fn gateway() {
    let c=ctx(); let issuer=fake_issuer();
    assert!(c.cli(&["external","add","--id","agent-two","--harness","openai-dot"]).status.success());
    issuer.grant(&access('a'),"legacy-client",json!({"everett_account":"private-owner","everett_agent_name":"OAuth client label"}));
    let port=c.oauth_server(&issuer.url);
    let legacy=success(&native_call(port,'a',"everett_whoami",json!({})));
    assert_eq!(legacy["gateway_auth"]["agent_name"],"OAuth client label");
    assert_eq!(legacy["identity"]["account"]["status"],"verified");
    assert!(legacy["identity"]["account"]["subject"].is_null());
    assert_eq!(legacy["identity"]["client"]["name"],"OAuth client label");
    assert_eq!(legacy["identity"]["session"]["provenance"],"legacy_client_binding");
    assert_eq!(legacy["identity"]["session"]["ownership"],"unmapped");
    let first=success(&native_call(port,'a',"everett_identity",json!({"action":"create","registration_key":"p".repeat(32),"name":"Same label","kind":"chat"})));
    let second=success(&native_call(port,'a',"everett_identity",json!({"action":"create","registration_key":"q".repeat(32),"name":"Same label","kind":"chat"})));
    let a=success(&native_call(port,'a',"everett_whoami",json!({"identity_handle":first["identity_handle"]})));
    let b=success(&native_call(port,'a',"everett_whoami",json!({"identity_handle":second["identity_handle"]})));
    for (result, registered) in [(&a,&first),(&b,&second)] {
        assert_eq!(result["session_id"],registered["session_id"]);
        assert_eq!(result["identity"]["session"]["id"],registered["session_id"]);
        assert_eq!(result["identity"]["session"]["provenance"],"owner_registered_logical");
        assert_eq!(result["identity"]["session"]["ownership"],"verified_account_client");
        assert_eq!(result["authorized_destinations"],json!(["agent-two"]));
        let raw=result.to_string();
        for secret in [access('a'),registered["identity_handle"].as_str().unwrap().into(),INTROSPECTION_SECRET.into(),"private-owner".into()] { assert!(!raw.contains(&secret)); }
    }
    assert_eq!(a["identity"]["client"],b["identity"]["client"]);
    assert_ne!(a["identity"]["session"]["label"],b["identity"]["session"]["label"]);
    denied(&native_call(port,'a',"everett_whoami",json!({"identity_handle":first["identity_handle"],"session_id":"forged-session"})));
    denied(&native_call(port,'a',"everett_send",json!({"identity_handle":first["identity_handle"],"to":"not-granted","text":"fixture"})));
    let sent=success(&native_call(port,'a',"everett_send",json!({"identity_handle":first["identity_handle"],"to":"agent-two","text":"fixture"})));
    assert_eq!(sent["queued"],true);assert_eq!(sent["hooked"],false);
    success(&native_call(port,'a',"everett_identity",json!({"action":"revoke","identity_handle":first["identity_handle"]})));
    denied(&native_call(port,'a',"everett_whoami",json!({"identity_handle":first["identity_handle"]})));
    let mut fixed=c.peer(&json!({}));
    let fixed=success(&fixed.call("everett_whoami",json!({})));
    assert_eq!(fixed["identity"]["account"]["status"],"unknown");
    assert_eq!(fixed["identity"]["session"]["provenance"],"legacy_client_binding");
}

pub(super) fn native() {
    let c=ctx();
    for sid in ["native-one","native-two"] {
        let mut cmd=c.command(&["mcp"]);cmd.env("CODEX_THREAD_ID",sid);
        let mut peer=Wire::new(cmd);
        denied(&peer.call("everett_whoami",json!({"session_id":"forged-session"})));
        let result=success(&peer.call("everett_whoami",json!({})));
        assert_eq!(result["session_id"],sid);
        assert_eq!(result["identity"]["session"]["id"],sid);
        assert_eq!(result["identity"]["session"]["provenance"],"native_host_bound");
        assert_eq!(result["identity"]["session"]["ownership"],"unmapped");
        assert_eq!(result["identity"]["account"]["status"],"unknown");
    }
    let mut peer=Wire::new(c.command(&["mcp"]));
    denied(&peer.call("everett_whoami",json!({"session_id":"forged-session","_meta":{"threadId":"also-forged"}})));
    let unbound=success(&peer.call("everett_whoami",json!({})));
    assert!(unbound["identity"]["session"]["id"].is_null());
    assert_eq!(unbound["identity"]["session"]["provenance"],"unbound");
    let meta=success(&peer.request("tools/call",Some(json!({"name":"everett_whoami","arguments":{},"_meta":{"threadId":"metadata-session"}}))));
    assert_eq!(meta["identity"]["session"]["provenance"],"request_metadata");
    assert_eq!(meta["identity"]["session"]["ownership"],"unmapped");
}
