use super::*;
use rusqlite::{params, Connection};

fn await_condition(check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !check() {
        assert!(Instant::now() < deadline, "Events pump did not converge");
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub(super) fn modern_discovery_and_lifecycle() {
    const VERSION: &str = "2026-07-28";
    let request = |method: &str, mut params: Value| {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": VERSION,
            "io.modelcontextprotocol/clientCapabilities": {}
        });
        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
    };
    let call = |port, method: &str, params| {
        let response = post(
            port,
            &request(method, params),
            &[("MCP-Protocol-Version", Some(VERSION)), ("Mcp-Method", Some(method))],
        );
        assert_eq!(response.status, 200);
        body_json(&response)["result"].clone()
    };
    let c = ctx();
    for enabled in [false, true] {
        let port = c.http_server(&json!({"events":enabled}));
        let legacy = body_json(&post(
            port,
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":initialize()}),
            &[],
        ));
        assert_eq!(legacy["result"]["capabilities"].get("events").is_some(), enabled);
        let discovery = call(port, "server/discover", json!({}));
        assert_eq!(discovery["resultType"], "complete");
        assert!(discovery["supportedVersions"].as_array().unwrap().contains(&json!(VERSION)));
        assert_eq!(discovery["capabilities"]["tools"], json!({}));
        assert_eq!(discovery["capabilities"].get("events").is_some(), enabled);
    }

    let port = c.http_server(&json!({"events":true}));
    let discover = request("server/discover", json!({}));
    assert_eq!(post(port, &discover, &[
        ("Authorization", None),
        ("MCP-Protocol-Version", Some(VERSION)),
        ("Mcp-Method", Some("server/discover")),
    ]).status, 401);
    let event_list = request("events/list", json!({}));
    let mismatched_method = post(port, &event_list, &[
        ("MCP-Protocol-Version", Some(VERSION)), ("Mcp-Method", Some("tools/list")),
    ]);
    assert_eq!(mismatched_method.status, 400);
    assert_eq!(body_json(&mismatched_method)["error"]["code"], -32020);
    let mut missing_metadata = event_list.clone();
    missing_metadata["params"] = json!({});
    let invalid = post(port, &missing_metadata, &[
        ("MCP-Protocol-Version", Some(VERSION)), ("Mcp-Method", Some("events/list")),
    ]);
    assert_eq!(invalid.status, 400);
    assert_eq!(body_json(&invalid)["error"]["code"], -32602);

    let catalog = call(port, "events/list", json!({}));
    assert_eq!(catalog["resultType"], "complete");
    assert_eq!(catalog["events"][0]["name"], "message.created");
    let legacy = body_json(&post(port, &json!({"jsonrpc":"2.0","id":1,"method":"events/list"}), &[]));
    assert!(legacy["result"].get("resultType").is_none());
    assert_eq!(legacy["result"]["events"], catalog["events"]);

    // Seed a verified subscription so refresh and unsubscribe exercise the HTTP wire without sending callbacks.
    use sha2::{Digest, Sha256};
    let dir = c.home.join(".everett/gateway-events");
    let db = Connection::open(std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "sqlite3")).unwrap()).unwrap();
    let owner = json!([hex::encode(Sha256::digest(TOKEN)), "dot-one"]).to_string();
    let url = "https://callback.example/";
    let key = "whsec_BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
    let id = format!("sub_{}", hex::encode(Sha256::digest(json!([owner, "dot-one", "message.created", url]).to_string())));
    let at = chrono::Utc::now().timestamp_millis();
    db.execute(
        "INSERT INTO subscriptions(id,owner,agent,token,url,secret,rotation_until,created,expires,verified) VALUES(?,?,'dot-one',?,?,?,0,?,?,?)",
        params![id, owner, json!({"access":null,"handle":null}).to_string(), url, key, at, at + 60_000, at],
    ).unwrap();
    let mut params = json!({"name":"message.created","arguments":{},"delivery":{"mode":"webhook","url":url,"secret":key}});
    let refreshed = call(port, "events/subscribe", params.clone());
    assert_eq!(refreshed["resultType"], "complete");
    assert_eq!(refreshed["id"], id);
    params["delivery"].as_object_mut().unwrap().remove("secret");
    assert_eq!(call(port, "events/unsubscribe", params), json!({"resultType":"complete"}));
    assert_eq!(db.query_row("SELECT count(*) FROM subscriptions", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

pub(super) fn identity_and_lifetime() {
    let c = ctx();
    let issuer = fake_issuer();
    for (token, account, client) in [
        ('a', "github:alice", "https://chatgpt.com/oauth/client.json"),
        ('b', "github:alice", "https://chatgpt.com/oauth/client.json"),
        ('c', "github:bob", "https://chatgpt.com/oauth/client.json"),
        ('d', "github:alice", "https://other.example/client"),
    ] {
        issuer.grant(
            &access(token),
            &format!("agent-{token}"),
            json!({"everett_account":account,"client_id":client}),
        );
    }
    let port = c.spawn_http(
        c.oauth_command(&json!({"events":true,"require_identity_handle":true,"oauth":{"issuer":issuer.url,"resource":RESOURCE}})),
    );
    let identity = success(&native_call(
        port,
        'a',
        "everett_identity",
        json!({"action":"create","registration_key":"events-private-registration-key-12345","name":"Events fixture","kind":"chat"}),
    ));
    let request = json!({"name":"message.created","arguments":{"identity_handle":identity["identity_handle"]},"delivery":{"mode":"webhook","url":"https://callback.example/","secret":"invalid-synthetic"}});
    for token in ['a', 'b'] {
        assert_eq!(
            oauth_rpc(port, token, "events/subscribe", request.clone())["error"]["message"],
            "invalid_signing_secret"
        );
    }
    for token in ['c', 'd'] {
        assert_eq!(
            oauth_rpc(port, token, "events/subscribe", request.clone())["error"]["message"],
            "invalid_identity_handle"
        );
    }
    let init = oauth_rpc(port, 'a', "initialize", initialize());
    assert_eq!(init["result"]["capabilities"]["events"], json!({}));
    let path = std::fs::read_dir(c.home.join(".everett/gateway-events"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|s| s == "sqlite3"))
        .unwrap();
    let db = Connection::open(path).unwrap();
    db.busy_timeout(Duration::from_secs(2)).unwrap();
    let scope = serde_json::to_string(&json!([issuer.url, RESOURCE])).unwrap();
    let identity_owner = serde_json::to_string(&json!([
        "github:alice",
        "https://chatgpt.com/oauth/client.json"
    ]))
    .unwrap();
    let owner = serde_json::to_string(&json!([scope, identity_owner])).unwrap();
    let credentials = |token: char| {
        json!({"access":access(token),"handle":identity["identity_handle"]}).to_string()
    };
    let count = || {
        db.query_row(
            "SELECT count(*) FROM subscriptions WHERE id='fixture'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    let seed = || {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        db.execute("INSERT INTO subscriptions(id,owner,agent,token,url,secret,rotation_until,created,expires,verified) VALUES('fixture',?,?,?,'https://127.0.0.1/','unused-synthetic',0,?,?,?)",params![owner,identity["session_id"].as_str().unwrap(),credentials('b'),at,at+60000,at]).unwrap();
    };
    seed();
    // The strict gateway must leave this legacy lease and its pending delivery
    // alone even after its stored OAuth token is revoked.
    db.execute(
        "INSERT INTO subscriptions(id,owner,agent,token,url,secret,rotation_until,created,expires,verified)
         SELECT 'legacy',owner,'agent-a',?,url,secret,rotation_until,created,expires,verified
         FROM subscriptions WHERE id='fixture'",
        [json!({"access":access('a'),"handle":null}).to_string()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO deliveries(subscription,message,event,body)
         VALUES('legacy','pending','evt_legacy','{}')",
        [],
    )
    .unwrap();
    db.execute(
        "UPDATE subscriptions SET secret=? WHERE id='fixture'",
        ["whsec_BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="],
    )
    .unwrap();
    db.execute("INSERT INTO deliveries(subscription,message,event,body) VALUES('fixture','pending','evt_pending','{}')", []).unwrap();
    let attempts = || {
        db.query_row(
            "SELECT attempts FROM deliveries WHERE subscription='fixture'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    issuer.grant(&access('a'), "agent-a", json!({"active":false}));
    let before = *issuer.calls.lock().unwrap();
    await_condition(|| *issuer.calls.lock().unwrap() > before);
    assert_eq!(count(), 1, "replacement credential must remain active");
    await_condition(|| attempts() > 0);
    let attempted = attempts();
    issuer.grant(&access('b'), "agent-b", json!({"fixture_unavailable":true}));
    let before = *issuer.calls.lock().unwrap();
    await_condition(|| *issuer.calls.lock().unwrap() > before);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        count(),
        1,
        "introspection outage must preserve subscription without delivery"
    );
    assert_eq!(
        attempts(),
        attempted,
        "introspection outage must defer pending delivery"
    );
    issuer.grant(&access('b'), "agent-b", json!({"active":false}));
    await_condition(|| count() == 0);
    issuer.grant(
        &access('b'),
        "agent-b",
        json!({"everett_account":"github:alice","exp":1}),
    );
    seed();
    await_condition(|| count() == 0);
    issuer.grant(
        &access('b'),
        "agent-b",
        json!({"everett_account":"github:alice"}),
    );
    seed();
    success(&native_call(
        port,
        'b',
        "everett_identity",
        json!({"action":"revoke","identity_handle":identity["identity_handle"]}),
    ));
    await_condition(|| count() == 0);
    assert_eq!(
        oauth_rpc(port, 'b', "events/subscribe", request)["error"]["message"],
        "invalid_identity_handle"
    );

    assert_eq!(
        db.query_row(
            "SELECT attempts FROM deliveries WHERE subscription='legacy' AND done=0",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap(),
        0,
        "strict mode must retain and skip pending delivery on legacy leases"
    );

    let c = ctx();
    let port = c.http_server(&json!({"events":true}));
    let call = |name: &str, args: Value| {
        body_json(&post(
            port,
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}}),
            &[],
        ))
    };
    let own = success(&call(
        "everett_identity",
        json!({"action":"create","registration_key":"bearer-events-private-registration-key","name":"Bearer events","kind":"chat"}),
    ));
    let req = json!({"jsonrpc":"2.0","id":2,"method":"events/subscribe","params":{"name":"message.created","arguments":{"identity_handle":own["identity_handle"]},"delivery":{"mode":"webhook","url":"https://callback.example/","secret":"invalid-synthetic"}}});
    assert_eq!(
        body_json(&post(port, &req, &[]))["error"]["message"],
        "invalid_signing_secret"
    );
    success(&call(
        "everett_identity",
        json!({"action":"revoke","identity_handle":own["identity_handle"]}),
    ));
    assert_eq!(
        body_json(&post(port, &req, &[]))["error"]["message"],
        "invalid_identity_handle"
    );
}


pub(super) fn verified_token_lease() {
    use sha2::{Digest, Sha256};
    for requested in [None, Some(86_400_000)] {
        let c=ctx(); let issuer=fake_issuer();
        let at=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let expiry=at+120;
        issuer.grant(&access('a'),"agent-a",json!({"everett_account":"github:alice","exp":expiry}));
        issuer.grant(&access('b'),"agent-a",json!({"everett_account":"github:alice","exp":at+600}));
        let port=c.spawn_http(c.oauth_command(&json!({"events":true,"oauth":{"issuer":issuer.url,"resource":RESOURCE}})));
        let path=std::fs::read_dir(c.home.join(".everett/gateway-events")).unwrap().map(|e|e.unwrap().path()).find(|p|p.extension().is_some_and(|s|s=="sqlite3")).unwrap();
        let db=Connection::open(path).unwrap(); db.busy_timeout(Duration::from_secs(2)).unwrap();
        let scope=json!([issuer.url,RESOURCE]).to_string();
        let owner=json!([scope,json!(["github:alice","https://chatgpt.com/oauth/client.json"]).to_string()]).to_string();
        let url="https://callback.example/";
        let id=format!("sub_{}",hex::encode(Sha256::digest(json!([owner,"agent-a","message.created",url]).to_string().as_bytes())));
        let key="whsec_BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
        let credentials=json!({"access":access('a'),"handle":null}).to_string();
        db.execute("INSERT INTO subscriptions(id,owner,agent,token,url,secret,rotation_until,created,expires,verified) VALUES(?,?,'agent-a',?,?,?,0,?,?,?)",params![id,owner,credentials,url,key,at as i64*1000-1000,(at+86400) as i64*1000,at as i64*1000]).unwrap();
        db.execute("INSERT INTO deliveries(subscription,message,event,body,done) VALUES(?,'historic','evt_historic','{}',1)",[&id]).unwrap();
        let mut args=json!({"name":"message.created","arguments":{"session_id":"agent-a"},"delivery":{"mode":"webhook","url":url,"secret":key},"exp":at+999999});
        if let Some(ttl)=requested {args["ttlMs"]=json!(ttl);}
        let result=oauth_rpc(port,'a',"events/subscribe",args.clone());
        let deadline=chrono::DateTime::parse_from_rfc3339(result["result"]["refreshBefore"].as_str().expect("subscribe response")).unwrap().timestamp_millis();
        assert_eq!(deadline,expiry as i64*1000,"advertised lease must use verified token expiry");
        assert_eq!(result["result"]["id"],id);
        let renewed=oauth_rpc(port,'b',"events/subscribe",args);
        assert_eq!(renewed["result"]["id"],id);
        assert_eq!(chrono::DateTime::parse_from_rfc3339(renewed["result"]["refreshBefore"].as_str().unwrap()).unwrap().timestamp_millis(),(at+600) as i64*1000);
        assert_eq!(db.query_row("SELECT count(*) FROM deliveries WHERE subscription=?",[&id],|r|r.get::<_,i64>(0)).unwrap(),1);
        issuer.grant(&access('a'),"agent-a",json!({"exp":1}));
        let before=*issuer.calls.lock().unwrap();
        await_condition(|| *issuer.calls.lock().unwrap()>before);
        assert_eq!(db.query_row("SELECT count(*) FROM subscriptions WHERE id=?",[&id],|r|r.get::<_,i64>(0)).unwrap(),1);
    }
}
