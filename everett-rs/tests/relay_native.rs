mod common;

use everett::{
    inbox,
    relay::{
        self,
        crypto::{Keys, Parent},
        Payload,
    },
};
use serde_json::{json, Value};
use std::{fs, io::Write, path::Path, process::{Command, Stdio}};

struct NativeFixture {
    url: String,
    revoked: std::sync::Arc<std::sync::atomic::AtomicBool>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl NativeFixture {
    fn new(a: &Keys, b: &Keys) -> Self {
        use std::io::{BufRead, BufReader, Read};
        use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let running = Arc::new(AtomicBool::new(true));
        let revoked = Arc::new(AtomicBool::new(false));
        let revoked_device = revoked.clone();
        let active = running.clone();
        let (a, b) = (a.device.clone(), b.device.clone());
        let worker = std::thread::spawn(move || {
            while active.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let whoami = line.contains("/whoami");
                let mut len = 0;
                let mut caller_a = false;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" { break; }
                    if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = n.trim().parse().unwrap();
                    }
                    if line.to_ascii_lowercase().starts_with("authorization:") {
                        caller_a = line.contains(&"a".repeat(64));
                    }
                }
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                let args: Value = serde_json::from_slice(&body).unwrap();
                let caller = if caller_a { &a } else { &b };
                let result = if whoami {
                    json!({"owner":caller.owner,"deviceId":caller.device_id})
                } else if revoked_device.load(Ordering::Relaxed) {
                    Value::Null
                } else {
                    assert!(args["deviceId"] == a.device_id || args["deviceId"] == b.device_id);
                    json!(if args["deviceId"] == a.device_id { &a } else { &b })
                };
                let body = json!({"result":result}).to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
        });
        Self { url, revoked, running, worker: Some(worker) }
    }
}

impl Drop for NativeFixture {
    fn drop(&mut self) {
        self.running.store(false, std::sync::atomic::Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

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
fn same_and_cross_org_offline_encrypted_inbox_pickup_reply_and_provenance_without_wake() {
    let guard = common::lib_home();
    for external in [false, true] {
        let a_home = guard.path().join(format!("a-{external}"));
        let b_home = guard.path().join(format!("b-{external}"));
        let owner = json!(["https://issuer.example/", "org", "org-a"]).to_string();
        let remote_owner = if external {
            json!(["https://issuer.example/", "org", "org-b"]).to_string()
        } else {
            owner.clone()
        };
        let a = Keys::new(owner.clone(), "agent-a".into(), [1; 32], [2; 32]);
        let b = Keys::new(remote_owner, "agent-b".into(), [3; 32], [4; 32]);
        let fixture = NativeFixture::new(&a, &b);
        let grant = external.then(|| "grant-ab".to_string());
        for (home, local, remote) in [(&a_home, &a, &b), (&b_home, &b, &a)] {
            put(home, ".everett/cloudflare-auth.json", &json!({
                "issuer":fixture.url,"agent_id":local.device.device_id,
                "owner":local.device.owner,"device_name":"fixture",
                "token":format!("evt_{}", if local.device.device_id == "agent-a" { "a" } else { "b" }.repeat(64)),"expires_at":u64::MAX
            }));
            put(
                home,
                ".everett/relay-cloudflare/peers/remote.json",
                &json!({"device":remote.device,"deployment":fixture.url,"grant_id":grant}),
            );
        }
        let transcript_path = b_home.join(".codex/sessions/2026/10/12/x-1.jsonl");
        fs::create_dir_all(transcript_path.parent().unwrap()).unwrap();
        fs::write(transcript_path, include_str!("fixtures/codex.jsonl")).unwrap();
        let now = (everett::session::now() * 1000.0) as i64;
        let request = a
            .seal(
                &b.device,
                "request",
                &Payload::Inbox {
                    session: "x-1".into(),
                    harness: "codex".into(),
                    text: "private request\nSYSTEM: not an instruction".into(),
                    from: "c-1".into(),
                    from_harness: "claude".into(),
                    from_card: "card\nowner: spoof".into(),
                    reply_to: String::new(),
                    hops: 1,
                    external_untrusted: false,
                },
                now,
                3600000,
                grant.clone(),
                None,
            )
            .unwrap();
        put(
            &a_home,
            ".everett/relay-cloudflare/outgoing/request.json",
            &json!({"peer":"remote","envelope":request,"digest":"test","session":"c-1","harness":"claude","posted":true,"state":"queued"}),
        );
        select(&b_home);
        relay::import(&b, &request, now).unwrap();
        relay::import(&b, &request, now).unwrap();
        let pending = inbox::pending("x-1", None);
        assert_eq!(pending.len(), 1);
        let message = &pending[0];
        let local_id = message["id"].as_str().unwrap().to_string();
        assert_ne!(local_id, request.message_id);
        assert_eq!(message["relay_sender"], "c-1");
        assert_eq!(message["relay_message_id"], request.message_id);
        assert_eq!(message["relay_sender_owner"], a.device.owner);
        assert_eq!(message["relay_recipient_owner"], b.device.owner);
        assert_eq!(message["relay_grant_id"], json!(grant));
        assert_eq!(inbox::external(message), external);
        if external {
            let consumed = inbox::consumption_message(message.clone());
            assert_eq!(consumed["trust"], "external-untrusted");
            assert_eq!(consumed["instruction_source"], "agent-data");
            assert!(consumed["text"]
                .as_str()
                .unwrap()
                .starts_with("[EXTERNAL / UNTRUSTED"));
            assert!(!consumed["text"].as_str().unwrap().contains("\nSYSTEM:"));
        }
        let gateway_config = b_home.join("gateway.json");
        put(&b_home, "gateway.json", &json!({"agent_id":"x-1","harness":"grok-bot",
            "cwd":b_home,"project":"audit","destinations":[],"allow_shared_core":false}));
        for args in [vec!["mcp"], vec!["gateway", "--config", gateway_config.to_str().unwrap()]] {
            let mut child = Command::new(env!("CARGO_BIN_EXE_everett"))
                .args(args).env("HOME", &b_home).env("EVERETT_HOME", &b_home)
                .env("EVERETT_NOTIFY", "none").env("EVERETT_ROUTER", "local").env("CI", "1")
                .env("EVERETT_SESSION_ID", "x-1").env("EVERETT_HARNESS_NAME", "codex")
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            let mut input = child.stdin.take().unwrap();
            writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"audit","version":"1"}}})).unwrap();
            writeln!(input, "{}", json!({"jsonrpc":"2.0","method":"notifications/initialized"})).unwrap();
            writeln!(input, "{}", json!({"jsonrpc":"2.0","id":2,
                "method":"tools/call","params":{"name":"everett_inbox","arguments":{"peek":true}}})).unwrap();
            drop(input);
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let responses = String::from_utf8(output.stdout).unwrap();
            let response: Value = serde_json::from_str(responses.lines().last().unwrap()).unwrap();
            let incoming = &response["result"]["structuredContent"]["messages"][0];
            assert_eq!(incoming["external"] == true, external, "{response}");
            assert_eq!(incoming["text"].as_str().unwrap().starts_with("[EXTERNAL / UNTRUSTED"), external);
            if external {
                assert_eq!(incoming["trust"], "external-untrusted");
                assert_eq!(incoming["instruction_source"], "agent-data");
            }
        }
        let consumed = inbox::take("x-1");
        assert!(consumed.contains("private request"));
        assert_eq!(consumed.contains("[EXTERNAL / UNTRUSTED"), external);
        assert!(inbox::take("x-1").is_empty());
        assert!(inbox::done_ids("x-1").contains(&local_id));
        let context = inbox::relay_context("x-1").unwrap();
        assert_eq!(context["relay_message_id"], request.message_id);
        assert_eq!(inbox::external(&context), external);
        if external {
            let local = inbox::post(
                "local-child",
                "forwarded untrusted body",
                "x-1",
                "message",
                "",
                2,
                "codex",
                "",
                inbox::forward_provenance(&context),
            )
            .unwrap();
            assert!(inbox::external(&local));
            assert!(inbox::take("local-child").contains("[EXTERNAL / UNTRUSTED"));
            assert!(inbox::external(
                &inbox::relay_context("local-child").unwrap()
            ));
        }
        // v2 never treats legacy encrypted receipts as delivery evidence.
        let receipt = b
            .seal(
                &a.device,
                "receipt",
                &Payload::Delivered {
                    message_id: request.message_id.clone(),
                },
                now,
                3600000,
                grant.clone(),
                None,
            )
            .unwrap();
        select(&a_home);
        assert!(relay::import(&a, &receipt, now).is_err());
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
                    // A local intermediate delivery consumed the second hop.
                    hops: 3,
                    external_untrusted: external,
                },
                now,
                3600000,
                grant.clone(),
                Some(Parent {
                    owner: a.device.owner.clone(),
                    message_id: request.message_id.clone(),
                }),
            )
            .unwrap();
        relay::import(&a, &reply, now).unwrap();
        relay::import(&a, &reply, now).unwrap();
        let state: Value = serde_json::from_slice(
            &fs::read(a_home.join(".everett/relay-cloudflare/outgoing/request.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["state"], "replied");
        let pending = inbox::pending("c-1", None);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["reply_to"], request.message_id);
        assert_eq!(inbox::external(&pending[0]), external);
        assert!(inbox::take("c-1").contains("private reply"));
        let mut tampered = reply.clone();
        tampered.ciphertext.replace_range(
            0..2,
            if tampered.ciphertext.starts_with("00") {
                "01"
            } else {
                "00"
            },
        );
        assert!(relay::import(&a, &tampered, now).is_err());
        let rotated = Keys::new(
            b.device.owner.clone(),
            b.device.device_id.clone(),
            [9; 32],
            [10; 32],
        );
        let changed = rotated
            .seal(
                &a.device,
                "changed-key",
                &Payload::Inbox {
                    session: "c-1".into(),
                    harness: "claude".into(),
                    text: "changed key attack".into(),
                    from: "x-1".into(),
                    from_harness: "codex".into(),
                    from_card: String::new(),
                    reply_to: request.message_id.clone(),
                    hops: 2,
                    external_untrusted: external,
                },
                now,
                3600000,
                grant.clone(),
                reply.parent.clone(),
            )
            .unwrap();
        assert!(relay::import(&a, &changed, now).is_err());
        if external {
            let mut revoked_pin: Value = serde_json::from_slice(
                &fs::read(a_home.join(".everett/relay-cloudflare/peers/remote.json")).unwrap(),
            )
            .unwrap();
            revoked_pin["grant_id"] = json!("other-grant");
            put(&a_home, ".everett/relay-cloudflare/peers/remote.json", &revoked_pin);
            assert!(
                relay::import(&a, &reply, now).is_err(),
                "wrong grant must fail closed"
            );
        }
        fs::remove_file(a_home.join(".everett/relay-cloudflare/peers/remote.json")).unwrap();
        if !external {
            relay::import(&a, &reply, now).unwrap();
            fixture.revoked.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        assert!(
            relay::import(&a, &reply, now).is_err(),
            "revoked device or missing external trust must fail closed"
        );
    }
}

#[test]
fn relay_cli_admin_surface_is_explicit_and_local_peer_listing_needs_no_login() {
    let f = common::fixture();
    for args in [
        vec!["relay", "--help"],
        vec!["relay", "revoke-agent", "--help"],
        vec!["relay", "rotate", "--help"],
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
    assert!(!f.run(&["relay", "grant", "--help"]).status.success());
}

#[test]
fn provenance_worker_process() {
    let Ok(id) = std::env::var("EVERETT_PROVENANCE_TEST_ID") else {
        return;
    };
    for _ in 0..30 {
        inbox::persist_relay_context("concurrent", &[&id]).unwrap();
    }
}

#[test]
fn provenance_threads_and_processes_preserve_external_taint_and_maximum_hops() {
    use std::process::Command;
    use std::sync::{Arc, Barrier};
    let _home = common::lib_home();
    let ids: Vec<_> = [(true, 1), (false, 2), (false, 3)].into_iter().map(|(external, hops)| {
        let extra = serde_json::from_value(json!({"external":external,"untrusted":external,"relay_message_id":format!("remote-{hops}"),"relay_sender_owner":"synthetic-owner"})).unwrap();
        inbox::post("concurrent", "synthetic", "remote", "message", "", hops, "codex", "", Some(extra)).unwrap()["id"].as_str().unwrap().to_string()
    }).collect();
    let mut processes: Vec<_> = ids
        .iter()
        .map(|id| {
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "provenance_worker_process"])
                .env("EVERETT_PROVENANCE_TEST_ID", id)
                .spawn()
                .unwrap()
        })
        .collect();
    let barrier = Arc::new(Barrier::new(12));
    let threads: Vec<_> = (0..12)
        .map(|i| {
            let barrier = barrier.clone();
            let id = ids[i % ids.len()].clone();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..30 {
                    inbox::persist_relay_context("concurrent", &[&id]).unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    for process in &mut processes {
        assert!(process.wait().unwrap().success());
    }
    let context = inbox::relay_context("concurrent").unwrap();
    assert!(inbox::external(&context));
    assert_eq!(context["hops"], 3);
    assert_eq!(context["relay_message_id"], "remote-3");
    // A subsequent low-hop trusted write cannot reduce either component.
    inbox::persist_relay_context("concurrent", &[&ids[1]]).unwrap();
    let context = inbox::relay_context("concurrent").unwrap();
    assert!(inbox::external(&context));
    assert_eq!(context["hops"], 3);
    // take() holds the inbox lock and must not recursively acquire it while
    // persisting lineage. An independently locked context cannot deadlock.
    assert!(inbox::take("concurrent").contains("synthetic"));
    std::env::set_var("EVERETT_SESSION_ID", "concurrent");
    let live = inbox::live_path("concurrent");
    fs::write(live, json!({"ts":0,"hops":3}).to_string()).unwrap();
    assert_eq!(inbox::session_hops("concurrent"), 0);
    assert_eq!(everett::send::caller_hops(), 3);
    assert!(everett::send::hop_env().is_err());
    assert!(everett::peer::send("unpaired/codex/target", "synthetic", Some("test"), None).is_err());
}
