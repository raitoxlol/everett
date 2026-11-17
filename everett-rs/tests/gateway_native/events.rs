use super::*;
use rusqlite::{params, Connection};

fn await_condition(check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !check() {
        assert!(Instant::now() < deadline, "Events pump did not converge");
        std::thread::sleep(Duration::from_millis(50));
    }
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
        c.oauth_command(&json!({"events":true,"oauth":{"issuer":issuer.url,"resource":RESOURCE}})),
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
