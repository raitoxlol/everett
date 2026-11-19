use super::*;

const HEADER: &str = "Everett-Identity-Handle";
const ADMIN_HEADER: &str = "Everett-Identity-Admin";
const ADMIN_TOKEN: &str = "synthetic-owner-identity-credential-not-real";
const LIFECYCLE: &str = "/mcp/identity";
const OTHER_TOKEN: &str = "synthetic-independent-gateway-credential-not-real";
const LEGACY_TOKEN: &str = "synthetic-legacy-owner-gateway-credential-not-real";

fn manage(port: u16, token: &str, request: &Value) -> Reply {
    http_at(
        port,
        "POST",
        LIFECYCLE,
        &request.to_string(),
        &[
            ("Authorization", Some(&format!("Bearer {token}"))),
            (ADMIN_HEADER, Some(ADMIN_TOKEN)),
        ],
    )
}

fn rpc(port: u16, token: &str, handle: Option<&str>, method: &str, params: Value) -> Value {
    let request = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
    let response = post(
        port,
        &request,
        &[
            ("Authorization", Some(&format!("Bearer {token}"))),
            (HEADER, handle),
        ],
    );
    assert_eq!(response.status, 200);
    body_json(&response)
}

fn call(port: u16, token: &str, handle: Option<&str>, name: &str, arguments: Value) -> Value {
    rpc(
        port,
        token,
        handle,
        "tools/call",
        json!({"name":name,"arguments":arguments}),
    )
}

fn create(key: char) -> Value {
    json!({"action":"create","registration_key":key.to_string().repeat(48),
        "name":"Same display name","kind":"chat"})
}

fn registered(port: u16, token: &str, request: &Value) -> Value {
    let response = manage(port, token, request);
    assert_eq!(
        response.status, 200,
        "registration endpoint must be available"
    );
    assert!(response
        .headers
        .iter()
        .any(|(name, value)| name == "cache-control" && value == "no-store"));
    body_json(&response)
}

fn catalog(port: u16, token: &str) -> Vec<String> {
    let result = rpc(port, token, None, "tools/list", json!({}));
    result["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| {
            let schema = tool["inputSchema"]["properties"].as_object().unwrap();
            assert!(!schema.contains_key("identity_handle"));
            tool["name"].as_str().unwrap().to_owned()
        })
        .collect()
}

fn card(what: &str) -> Value {
    json!({"what":what,"state":"Testing","next":"Report"})
}

fn bearer_command(c: &Ctx, binding: &Value) -> Command {
    let path = c.config_path(binding);
    let mut command = c.command(&[
        "gateway",
        "--config",
        path.to_str().unwrap(),
        "--transport",
        "http",
        "--port",
        "0",
    ]);
    command.env("EVERETT_GATEWAY_IDENTITY_ADMIN_TOKEN", ADMIN_TOKEN);
    command
}

pub(super) fn bearer() {
    let c = ctx();
    common::native_mcp_account(&c.home);
    let grant = c.home.join("permissions.json");
    let update_grant = |args: &[&str]| {
        let mut command = vec!["gateway-permissions", "--file", grant.to_str().unwrap()];
        command.extend_from_slice(args);
        assert!(c.cli(&command).status.success());
    };
    update_grant(&["--all"]);
    let mut legacy_command = bearer_command(&c, &json!({"agent_tool_permissions":grant}));
    legacy_command.env("EVERETT_GATEWAY_TOKEN", LEGACY_TOKEN);
    let legacy = c.spawn_http(legacy_command);
    let binding = json!({"agent_tool_permissions":grant,"conversation_identity":true,
        "allow_shared_core":false,"harness":"devin-bot","destinations":["agent-two","queue-only-local"]});
    let port = c.spawn_http(bearer_command(&c, &binding));
    for changes in [
        json!({"agent_tool_permissions":null}),
        json!({"agent_tool_permissions":null,"events":true}),
        json!({"agent_tool_permissions":null,"allow_shared_core":true}),
    ] {
        assert_eq!(
            c.gateway_code(&c.config_path(&merged(&binding, &changes))),
            Some(2)
        );
    }
    assert_eq!(manage(legacy, LEGACY_TOKEN, &create('a')).status, 404);
    assert_eq!(
        success(&call(
            legacy,
            LEGACY_TOKEN,
            None,
            "everett_whoami",
            json!({})
        ))["session_id"],
        "dot-one"
    );
    assert_eq!(
        catalog(legacy, LEGACY_TOKEN).len(),
        everett::mcp::tools().len()
    );
    let whoami =
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"everett_whoami"}});
    assert_eq!(post(legacy, &whoami, &[]).status, 401);
    assert_eq!(
        post(
            port,
            &whoami,
            &[("Authorization", Some(&format!("Bearer {LEGACY_TOKEN}")))]
        )
        .status,
        401
    );
    assert_eq!(
        catalog(port, TOKEN),
        [
            "everett_whoami",
            "everett_inbox",
            "everett_card",
            "everett_send"
        ]
    );
    assert_eq!(manage(port, OTHER_TOKEN, &create('a')).status, 401);
    assert_eq!(
        http_at(port, "POST", LIFECYCLE, &create('a').to_string(), &[]).status,
        403
    );
    assert_eq!(
        http_at(
            port,
            "POST",
            LIFECYCLE,
            &create('a').to_string(),
            &[
                (ADMIN_HEADER, Some(ADMIN_TOKEN)),
                ("Content-Type", Some("text/plain")),
            ]
        )
        .status,
        415
    );
    denied(&call(port, TOKEN, None, "everett_identity", create('a')));

    let a = registered(port, TOKEN, &create('a'));
    let b = registered(port, TOKEN, &create('b'));
    let a_id = a["session_id"].as_str().unwrap();
    let b_id = b["session_id"].as_str().unwrap();
    let a_handle = a["identity_handle"].as_str().unwrap();
    let b_handle = b["identity_handle"].as_str().unwrap();
    assert_ne!(a_id, b_id);
    assert_ne!(a_handle, b_handle);
    assert_ne!(a_id, "dot-one");
    assert_eq!(registered(port, TOKEN, &create('a')), a);
    let restarted = c.spawn_http(bearer_command(&c, &binding));
    assert_eq!(registered(restarted, TOKEN, &create('a')), a);
    assert_eq!(registered(restarted, TOKEN, &create('b')), b);
    let backend = c.home.join("backend-link");
    let binary = std::env::var_os("EVERETT_VERIFY_BINARY").unwrap_or_else(|| BIN.into());
    std::os::unix::fs::symlink(binary, &backend).unwrap();
    let changed_backend = c.spawn_http(bearer_command(
        &c,
        &merged(&binding, &json!({"backend_command":[backend,"mcp"]})),
    ));
    let fallback = c.home.join("fallback-backend");
    std::fs::write(&fallback, "#!/bin/sh\n: > \"$HOME/fallback-started\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_file(&backend).unwrap();
    std::os::unix::fs::symlink(fallback, backend).unwrap();
    denied(&call(
        changed_backend,
        TOKEN,
        Some(a_handle),
        "everett_whoami",
        json!({}),
    ));
    assert!(!c.home.join("fallback-started").exists());
    let mut command = bearer_command(&c, &binding);
    command.env("EVERETT_GATEWAY_TOKEN", OTHER_TOKEN);
    let independent = c.spawn_http(command);
    let foreign = registered(independent, OTHER_TOKEN, &create('a'));
    let foreign_handle = foreign["identity_handle"].as_str().unwrap();
    assert_ne!(foreign["session_id"], a_id);
    assert_ne!(foreign_handle, a_handle);

    let inboxes = c.home.join(".everett/inbox");
    std::fs::create_dir_all(&inboxes).unwrap();
    for (id, message, handle, description) in [
        (a_id, "only-a", a_handle, "Card A"),
        (b_id, "only-b", b_handle, "Card B"),
    ] {
        let row = json!({"id":message,"to":id,"from":"agent-two","text":message,
            "ts":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64()});
        std::fs::write(inboxes.join(format!("{id}.jsonl")), format!("{row}\n")).unwrap();
        let identity = call(restarted, TOKEN, Some(handle), "everett_whoami", json!({}));
        assert!(!identity.to_string().contains(handle));
        let identity = success(&identity);
        assert_eq!(identity["session_id"], id);
        assert_eq!(identity["harness"], "devin-bot");
        assert_eq!(
            success(&call(
                port,
                TOKEN,
                Some(handle),
                "everett_inbox",
                json!({"peek":true})
            ))["messages"][0]["id"],
            message
        );
        success(&call(
            port,
            TOKEN,
            Some(handle),
            "everett_card",
            card(description),
        ));
    }
    let b_path = c.home.join(format!(".everett/cards/{b_id}.md"));
    let baseline = std::fs::read_to_string(&b_path).unwrap();
    assert!(baseline.contains("Card B"));
    assert!(
        std::fs::read_to_string(c.home.join(format!(".everett/cards/{a_id}.md")))
            .unwrap()
            .contains("Card A")
    );

    for handle in [None, Some(b_id), Some(foreign_handle)] {
        denied(&call(
            port,
            TOKEN,
            handle,
            "everett_inbox",
            json!({"session_id":b_id}),
        ));
        denied(&call(port, TOKEN, handle, "everett_card", card("Denied")));
    }
    denied(&call(port, TOKEN, None, "everett_whoami", json!({})));
    denied(&call(
        port,
        TOKEN,
        None,
        "everett_inbox",
        json!({"identity_handle":b_handle}),
    ));
    denied(&call(
        port,
        TOKEN,
        Some(a_handle),
        "everett_inbox",
        json!({"session_id":b_id}),
    ));
    denied(&call(
        port,
        TOKEN,
        Some(a_handle),
        "everett_inbox",
        json!({"identity_handle":b_handle}),
    ));
    denied(&call(
        independent,
        OTHER_TOKEN,
        Some(b_handle),
        "everett_inbox",
        json!({}),
    ));
    denied(&call(
        independent,
        OTHER_TOKEN,
        Some(b_handle),
        "everett_card",
        card("Denied"),
    ));
    let mut wrong_card = card("Denied");
    wrong_card["session_id"] = json!(b_id);
    denied(&call(
        port,
        TOKEN,
        Some(a_handle),
        "everett_card",
        wrong_card,
    ));
    let spoofed = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
        "name":"everett_inbox","_meta":{"openai/session":b_id,"identity_handle":b_handle},"arguments":{}
    }});
    denied(&body_json(&post(
        port,
        &spoofed,
        &[
            ("Mcp-Session-Id", Some(b_id)),
            ("X-Devin-Session-Id", Some(b_id)),
        ],
    )));
    assert_eq!(
        post(
            legacy,
            &spoofed,
            &[
                (HEADER, Some(a_handle)),
                ("Authorization", Some(&format!("Bearer {LEGACY_TOKEN}")))
            ]
        )
        .status,
        400
    );
    let duplicate = format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nAuthorization: Bearer {TOKEN}\r\n{HEADER}: {a_handle}\r\n{HEADER}: {b_handle}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    assert!(raw_exchange(port, duplicate.as_bytes()).starts_with(b"HTTP/1.1 400"));
    denied(&call(
        port,
        TOKEN,
        None,
        "everett_send",
        json!({"reply_to":"only-b","text":"Denied"}),
    ));
    denied(&call(
        port,
        TOKEN,
        Some(a_handle),
        "everett_send",
        json!({"reply_to":"only-b","text":"Denied"}),
    ));
    denied(&call(
        port,
        TOKEN,
        Some(a_handle),
        "everett_send",
        json!({"to":b_id,"text":"No destination grant"}),
    ));
    success(&call(
        port,
        TOKEN,
        Some(a_handle),
        "everett_send",
        json!({"reply_to":"only-a","text":"Authorized reply"}),
    ));
    assert!(c
        .cli(&[
            "external",
            "add",
            "--id",
            "agent-two",
            "--harness",
            "grok-bot"
        ])
        .status
        .success());
    std::fs::write(
        c.home.join(".everett/cards/agent-two.md"),
        "Synthetic destination card secret",
    )
    .unwrap();
    let relay = c.home.join(".everett/relay-cloudflare");
    std::fs::create_dir_all(&relay).unwrap();
    std::fs::write(
        relay.join("receiver-webhook.json"),
        "Invalid synthetic policy",
    )
    .unwrap();
    let send = call(
        port,
        TOKEN,
        Some(a_handle),
        "everett_send",
        json!({"to":"agent-two","text":"Direct queue only"}),
    );
    assert!(!send
        .to_string()
        .contains("Synthetic destination card secret"));
    let receipt = success(&send);
    assert_eq!(receipt["queued"], true);
    assert!(receipt.get("webhook").is_none());
    let rows = std::fs::read_to_string(inboxes.join("agent-two.jsonl")).unwrap();
    let last: Value = serde_json::from_str(rows.lines().last().unwrap()).unwrap();
    assert_eq!(last["from"], a_id);
    assert_eq!(last["id"], receipt["message_id"]);
    let queued = success(&call(
        port,
        TOKEN,
        Some(a_handle),
        "everett_send",
        json!({"to":"queue-only-local","text":"Queue without a destination registry scan"}),
    ));
    assert_eq!(queued["queued"], true);
    assert_eq!(queued["to"], "queue-only-local");
    assert!(inboxes.join("queue-only-local.jsonl").is_file());
    for name in [
        "everett_ls",
        "everett_core",
        "everett_control",
        "everett_agent_relay",
    ] {
        denied(&call(port, TOKEN, Some(a_handle), name, json!({})));
    }

    update_grant(&["--allow", "everett_whoami,everett_core"]);
    assert_eq!(catalog(port, TOKEN), ["everett_whoami"]);
    denied(&call(
        port,
        TOKEN,
        Some(a_handle),
        "everett_card",
        card("Denied"),
    ));
    denied(&call(
        port,
        TOKEN,
        Some(a_handle),
        "everett_core",
        json!({}),
    ));
    update_grant(&["--deny-all"]);
    assert!(catalog(port, TOKEN).is_empty());
    denied(&call(
        port,
        TOKEN,
        Some(a_handle),
        "everett_whoami",
        json!({}),
    ));
    update_grant(&["--all"]);

    let revoke = json!({"action":"revoke","identity_handle":a_handle});
    assert_eq!(manage(independent, OTHER_TOKEN, &revoke).status, 400);
    let revoked = json!({"session_id":a_id,"revoked":true});
    assert_eq!(body_json(&manage(port, TOKEN, &revoke)), revoked);
    assert_eq!(body_json(&manage(port, TOKEN, &revoke)), revoked);
    assert_eq!(manage(port, TOKEN, &create('a')).status, 400);
    denied(&call(
        port,
        TOKEN,
        Some(a_handle),
        "everett_inbox",
        json!({}),
    ));
    denied(&call(
        port,
        TOKEN,
        Some(a_handle),
        "everett_card",
        card("Denied"),
    ));
    assert_eq!(
        success(&call(
            port,
            TOKEN,
            Some(b_handle),
            "everett_inbox",
            json!({})
        ))["messages"][0]["id"],
        "only-b"
    );
    assert_eq!(std::fs::read_to_string(b_path).unwrap(), baseline);
    assert!(!c.home.join(".everett/cards/dot-one.md").exists());
    assert!(!c
        .home
        .join(format!(".everett/external/{a_id}.json"))
        .exists());
    let survivor = c.cli(&["ls", "--harness", "devin-bot", "--json"]);
    assert!(survivor.status.success());
    let sessions: Value = serde_json::from_slice(&survivor.stdout).unwrap();
    assert!(sessions
        .as_array()
        .unwrap()
        .iter()
        .any(|session| session["id"] == b_id));
    let revoke_by_address = json!({"action":"revoke","session_id":b_id});
    assert_eq!(
        manage(independent, OTHER_TOKEN, &revoke_by_address).status,
        400
    );
    assert_eq!(
        http_at(port, "POST", LIFECYCLE, &revoke_by_address.to_string(), &[]).status,
        403
    );
    let revoked = json!({"session_id":b_id,"revoked":true});
    assert_eq!(body_json(&manage(port, TOKEN, &revoke_by_address)), revoked);
    assert_eq!(body_json(&manage(port, TOKEN, &revoke_by_address)), revoked);
    denied(&call(
        port,
        TOKEN,
        Some(b_handle),
        "everett_card",
        card("Denied"),
    ));
}

pub(super) fn oauth() {
    let c = ctx();
    let issuer = fake_issuer();
    let legacy_token = access('f');
    let legacy_resource = "https://legacy.example.test/mcp";
    let (a, b, renewed, old, other_client) = (
        access('a'),
        access('b'),
        access('c'),
        access('d'),
        access('e'),
    );
    issuer.grant(&a, "agent-a", json!({"everett_account":"github:alice"}));
    issuer.grant(&b, "agent-b", json!({"everett_account":"github:bob"}));
    issuer.grant(
        &renewed,
        "agent-renewed",
        json!({"everett_account":"github:alice"}),
    );
    issuer.grant(&old, "agent-legacy", json!({}));
    issuer.grant(
        &other_client,
        "agent-other-client",
        json!({"everett_account":"github:alice", "client_id":"https://other.example/client"}),
    );
    issuer.grant(
        &legacy_token,
        "agent-original",
        json!({"everett_account":"github:alice","aud":legacy_resource}),
    );
    let legacy = c.spawn_http(
        c.oauth_command(&json!({"oauth":{"issuer":issuer.url,"resource":legacy_resource}})),
    );
    let mut command = c.oauth_command(&json!({
        "oauth":{"issuer":issuer.url,"resource":RESOURCE,"owner_account":"github:alice"},
        "conversation_identity":true,"allow_shared_core":false
    }));
    command.env("EVERETT_GATEWAY_IDENTITY_ADMIN_TOKEN", ADMIN_TOKEN);
    let port = c.spawn_http(command);
    assert_eq!(
        success(&call(
            legacy,
            &legacy_token,
            None,
            "everett_whoami",
            json!({})
        ))["session_id"],
        "agent-original"
    );
    let whoami =
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"everett_whoami"}});
    assert_eq!(
        post(
            legacy,
            &whoami,
            &[("Authorization", Some(&format!("Bearer {a}")))]
        )
        .status,
        401
    );
    assert_eq!(
        post(
            port,
            &whoami,
            &[("Authorization", Some(&format!("Bearer {legacy_token}")))]
        )
        .status,
        401
    );
    assert_eq!(manage(legacy, &legacy_token, &create('a')).status, 404);
    let legacy_identity = success(&call(
        legacy,
        &legacy_token,
        None,
        "everett_identity",
        create('a'),
    ));
    assert_eq!(
        catalog(port, &a),
        [
            "everett_whoami",
            "everett_inbox",
            "everett_card",
            "everett_send"
        ]
    );
    assert_eq!(manage(port, &old, &create('a')).status, 401);
    assert_eq!(manage(port, &b, &create('a')).status, 401);
    let first = registered(port, &a, &create('a'));
    let handle = first["identity_handle"].as_str().unwrap();
    assert_ne!(first["session_id"], legacy_identity["session_id"]);
    assert_eq!(registered(port, &renewed, &create('a')), first);
    assert_eq!(
        success(&call(
            port,
            &renewed,
            Some(handle),
            "everett_whoami",
            json!({})
        ))["session_id"],
        first["session_id"]
    );
    denied(&call(
        port,
        &a,
        legacy_identity["identity_handle"].as_str(),
        "everett_inbox",
        json!({}),
    ));
    denied(&call(
        port,
        &other_client,
        Some(handle),
        "everett_inbox",
        json!({}),
    ));
    denied(&call(
        legacy,
        &legacy_token,
        None,
        "everett_inbox",
        json!({"identity_handle":handle}),
    ));
}
