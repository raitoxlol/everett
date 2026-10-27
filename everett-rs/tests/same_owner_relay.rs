mod common;
use common::{fixture, Fixture};
use everett::relay::crypto::{Device, Envelope};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const OWNER: &str = "[\"https://issuer.example\",\"account\",\"github:1\"]";
#[derive(Default)]
struct State {
    devices: BTreeMap<String, Device>,
    labels: BTreeMap<String, String>,
    messages: Vec<Envelope>,
    revoked: bool,
    duplicate: bool,
    foreign: bool,
    unavailable: bool,
    incomplete: bool,
    labels_unavailable: bool,
    enqueue_unavailable: bool,
    device_queries: usize,
    pending_empty: bool,
}
struct Server {
    url: String,
    state: Arc<Mutex<State>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let weak = Arc::downgrade(&state);
        std::thread::spawn(move || {
            while let Some(shared) = weak.upgrade() {
                let Ok((mut stream, _)) = listener.accept() else {
                    drop(shared);
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                let (path, caller, args) = request(&mut stream);
                let mut s = shared.lock().unwrap();
                let mut status = 200;
                let value = match path.as_str() {
                    "/v2/relay/whoami" => json!({"owner":OWNER,"deviceId":caller,"role":"admin"}),
                    "/v2/relay/register" => {
                        let device: Device =
                            serde_json::from_value(args["device"].clone()).unwrap();
                        device.verify().unwrap();
                        assert_eq!(device.device_id, caller);
                        s.labels.insert(
                            caller.clone(),
                            if caller == "agent-max" {
                                "Max NUC"
                            } else {
                                "Hugo Box"
                            }
                            .into(),
                        );
                        s.devices.insert(caller.clone(), device);
                        Value::Null
                    }
                    "/v2/relay/devices" => json!(s.devices.values().collect::<Vec<_>>()),
                    "/v2/relay/deviceLabels" => {
                        let mut labels = s.labels.clone();
                        if s.duplicate {
                            labels.insert("agent-third".into(), "Hugo Box".into());
                        }
                        if s.labels_unavailable {
                            status = 503;
                            json!({"error":"unavailable"})
                        } else {
                            json!({"labels":labels,"complete":!s.incomplete})
                        }
                    }
                    "/v2/relay/device" => {
                        s.device_queries += 1;
                        if s.revoked || s.unavailable {
                            status = if s.unavailable { 503 } else { 403 };
                            json!({"error":"device_unavailable"})
                        } else {
                            let mut d = s.devices.get(args["deviceId"].as_str().unwrap()).cloned();
                            if s.foreign {
                                d = Some(
                                    everett::relay::crypto::Keys::new(
                                        "other-owner".into(),
                                        args["deviceId"].as_str().unwrap().into(),
                                        [19; 32],
                                        [20; 32],
                                    )
                                    .device,
                                );
                            }
                            json!(d)
                        }
                    }
                    "/v2/relay/enqueue" if s.enqueue_unavailable => {
                        status = 503;
                        json!({"error":"unavailable"})
                    }
                    "/v2/relay/enqueue" => {
                        let e: Envelope = serde_json::from_value(args["envelope"].clone()).unwrap();
                        if !s.messages.iter().any(|m| m.message_id == e.message_id) {
                            s.messages.push(e);
                        }
                        json!({"state":"queued"})
                    }
                    "/v2/relay/pending" => {
                        let envelopes: Vec<_> = s
                            .messages
                            .iter()
                            .filter(|m| !s.pending_empty && m.to_device == caller)
                            .map(|e| json!({"envelope":e}))
                            .collect();
                        json!({"serverNow":(everett::session::now()*1000.0) as i64,"envelopes":envelopes,"isDone":true,"continueCursor":"0"})
                    }
                    "/v2/relay/observe" | "/v2/relay/accept" => json!({}),
                    "/v2/relay/status" => json!({"state":"queued"}),
                    _ => {
                        status = 404;
                        json!({"error":"unsupported"})
                    }
                };
                let body = if status == 200 {
                    json!({"result":value})
                } else {
                    value
                }
                .to_string();
                let _ = write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            }
        });
        Self { url, state }
    }
    fn configure(&self, f: &Fixture, id: &str) {
        f.write(
            ".everett/config.toml",
            &format!("[auth]\nprovider='cloudflare'\nissuer='{}'\n", self.url),
        );
        let token = format!(
            "evt_{}",
            if id == "agent-max" { "a" } else { "b" }.repeat(64)
        );
        f.write(".everett/cloudflare-auth.json", &json!({"issuer":self.url,"owner":OWNER,"agent_id":id,"device_name":id,"token":token,"expires_at":(everett::session::now()*1000.0) as u64+120000}).to_string());
        success(f, &["relay", "register"]);
    }
}
fn request(stream: &mut TcpStream) -> (String, String, Value) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut byte = [0; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
    }
    let headers = String::from_utf8(bytes).unwrap();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map(|(_, v)| v.trim().parse().unwrap())
        })
        .unwrap();
    let mut body = vec![0; length];
    stream.read_exact(&mut body).unwrap();
    let caller = if headers.contains(&"a".repeat(64)) {
        "agent-max"
    } else {
        "agent-hugo"
    };
    (
        headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .into(),
        caller.into(),
        serde_json::from_slice(&body).unwrap(),
    )
}
fn run(f: &Fixture, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(args)
        .env_clear()
        .env("HOME", f.home())
        .env("EVERETT_HOME", f.home())
        .env("PATH", "/usr/bin:/bin")
        .env("EVERETT_NOTIFY", "none")
        .output()
        .unwrap()
}
fn success(f: &Fixture, args: &[&str]) -> Value {
    let out = run(f, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn listen(f: &Fixture) -> Running {
    Running(
        Command::new(env!("CARGO_BIN_EXE_everett"))
            .arg("relay")
            .env_clear()
            .env("HOME", f.home())
            .env("EVERETT_HOME", f.home())
            .env("PATH", "/usr/bin:/bin")
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(f.home().join("relay-test.stderr")).unwrap())
            .spawn()
            .unwrap(),
    )
}
#[test]
fn unpaired_same_owner_names_deliver_encrypted_inbox_and_retry_once() {
    let server = Server::new();
    let a = fixture();
    let b = fixture();
    server.configure(&a, "agent-max");
    server.configure(&b, "agent-hugo");
    success(
        &b,
        &[
            "external",
            "add",
            "--id",
            "ext-hugo",
            "--harness",
            "grok-bot",
        ],
    );
    let args = [
        "send",
        "synthetic-private-marker",
        "--to",
        "Hugo Box/grok-bot/ext-hugo",
        "--idempotency-key",
        "same-owner-test",
        "--json",
    ];
    let first = success(&a, &args);
    let retry = success(&a, &args);
    assert_eq!(first["message_id"], retry["message_id"]);
    success(
        &a,
        &[
            "peer",
            "status",
            "Hugo Box",
            first["message_id"].as_str().unwrap(),
            "--json",
        ],
    );
    let names = run(&a, &["relay", "devices", "--names"]);
    assert!(names.status.success());
    assert!(String::from_utf8_lossy(&names.stdout).contains("Hugo Box"));
    let mut mcp = Command::new(env!("CARGO_BIN_EXE_everett"))
        .arg("mcp")
        .env_clear()
        .env("HOME", a.home())
        .env("EVERETT_HOME", a.home())
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let messages = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"issue88","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"everett_agent_relay","arguments":{"action":"names"}}}),
    ];
    let mut stdin = mcp.stdin.take().unwrap();
    for message in messages {
        writeln!(stdin, "{message}").unwrap();
    }
    drop(stdin);
    let output = mcp.wait_with_output().unwrap();
    assert!(output.status.success());
    let response: Value = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|response| response["id"] == 2)
        .unwrap();
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(response["result"]["isError"], true, "{response}");
    assert!(
        response["result"].to_string().contains("Hugo Box"),
        "{response}"
    );
    let _relay = listen(&b);
    let deadline = Instant::now() + Duration::from_secs(10);
    let received = loop {
        let inbox = success(&b, &["inbox", "--session", "ext-hugo", "--peek", "--json"]);
        if inbox.to_string().contains("synthetic-private-marker") {
            break inbox;
        }
        assert!(
            Instant::now() < deadline,
            "no imported inbox message: {inbox}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(server.state.lock().unwrap().messages.len(), 1);
    let ciphertext = serde_json::to_string(&server.state.lock().unwrap().messages).unwrap();
    assert!(!ciphertext.contains("synthetic-private-marker"));
    let local_id = received["messages"].as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap();
    let reply = run(&b, &["reply", local_id, "synthetic-answer"]);
    assert!(
        reply.status.success(),
        "{}",
        String::from_utf8_lossy(&reply.stderr)
    );
    let _sender_relay = listen(&a);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let inbox = success(&a, &["inbox", "--session", "human", "--peek", "--json"]);
        if inbox.to_string().contains("synthetic-answer") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no returned reply: {inbox}; relay logs {}",
            std::fs::read_to_string(a.home().join("relay-test.stderr")).unwrap()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    for f in [&a, &b] {
        assert!(f
            .home()
            .join(".everett/relay-cloudflare/peers")
            .read_dir()
            .unwrap()
            .next()
            .is_none());
    }
}
#[test]
fn duplicate_names_revocation_foreign_owner_and_unavailability_fail_closed() {
    let server = Server::new();
    let a = fixture();
    let b = fixture();
    server.configure(&a, "agent-max");
    server.configure(&b, "agent-hugo");
    for mode in [
        "duplicate",
        "revoked",
        "foreign",
        "unavailable",
        "incomplete",
    ] {
        {
            let mut s = server.state.lock().unwrap();
            s.duplicate = mode == "duplicate";
            s.revoked = mode == "revoked";
            s.foreign = mode == "foreign";
            s.unavailable = mode == "unavailable";
            s.incomplete = mode == "incomplete";
        }
        let out = run(
            &a,
            &[
                "send",
                "must-not-send",
                "--to",
                "Hugo Box/grok-bot/ext-hugo",
                "--json",
            ],
        );
        assert!(!out.status.success(), "accepted {mode}");
        assert!(server.state.lock().unwrap().messages.is_empty());
    }
}

#[test]
fn webhook_cli_binds_registered_inbox_without_device_or_sender_filters() {
    use std::os::unix::fs::PermissionsExt;
    let server = Server::new();
    let f = fixture();
    server.configure(&f, "agent-hugo");
    success(
        &f,
        &[
            "external",
            "add",
            "--id",
            "ext-hugo",
            "--harness",
            "grok-bot",
        ],
    );
    let url = f.write(
        "private/webhook.url",
        "https://api2.cursor.sh/automations/webhook/synthetic-route",
    );
    let key = f.write("private/webhook.key", "synthetic-private-bearer");
    for file in [&url, &key] {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let args = [
        "relay",
        "webhook",
        "ext-hugo",
        "--harness",
        "grok-bot",
        "--url-file",
        url.to_str().unwrap(),
        "--bearer-file",
        key.to_str().unwrap(),
    ];
    let result = success(&f, &args);
    assert_eq!(result["state"], "configured");
    assert!(!result.to_string().contains("synthetic-route"));
    let path = f
        .home()
        .join(".everett/relay-cloudflare/receiver-webhook.json");
    let first = std::fs::read(&path).unwrap();
    success(&f, &args);
    assert_eq!(std::fs::read(&path).unwrap(), first);
    let policy: Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(policy["rules"].as_array().unwrap().len(), 1);
    assert_eq!(policy["rules"][0]["owner"], OWNER);
    for field in [
        "sender_device",
        "recipient_device",
        "sender_session",
        "sender_harness",
    ] {
        assert!(policy["rules"][0].get(field).is_none());
    }
    assert!(!String::from_utf8_lossy(&first).contains("synthetic-private-bearer"));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(server.state.lock().unwrap().messages.is_empty());

    let _receiver = listen(&f);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = success(&f, &["relay", "status", "--json"]);
        if status["receiver"]["state"] == "running" { break; }
        assert!(Instant::now() < deadline, "compatible receiver did not publish its lease: {status}");
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut restricted = policy;
    restricted["rules"][0]["sender_device"] = json!("agent-max");
    restricted["rules"][0]["sender_session"] = json!("human");
    restricted["rules"][0]["sender_harness"] = json!("");
    let before = serde_json::to_vec(&restricted).unwrap();
    std::fs::write(&path, &before).unwrap();
    let preview = success(&f, &[
        "relay", "trust-account", "ext-hugo", "--harness", "grok-bot", "--dry-run",
    ]);
    assert_eq!(preview["state"], "planned");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let changed = success(&f, &[
        "relay", "trust-account", "ext-hugo", "--harness", "grok-bot",
    ]);
    assert_eq!(changed["changed"], true);
    assert_eq!(changed["backup_created"], true);
    let migrated: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(migrated["rules"][0]["recipient_device"], "agent-hugo");
    assert!(migrated["rules"][0].get("sender_device").is_none());
    assert_eq!(migrated["rules"][0]["url"], restricted["rules"][0]["url"]);
    assert_eq!(migrated["rules"][0]["bearer_file"], restricted["rules"][0]["bearer_file"]);
    assert!(server.state.lock().unwrap().messages.is_empty());
}

#[test]
fn exact_device_works_without_labels_and_same_owner_alias_follows_active_rotation() {
    let server = Server::new();
    let a = fixture();
    let b = fixture();
    server.configure(&a, "agent-max");
    server.configure(&b, "agent-hugo");
    let old = server.state.lock().unwrap().devices["agent-hugo"].clone();
    success(
        &a,
        &[
            "peer",
            "add-relay",
            "hugo",
            "--device",
            "agent-hugo",
            "--fingerprint",
            &old.fingerprint(),
        ],
    );
    let rotated =
        everett::relay::crypto::Keys::new(OWNER.into(), "agent-hugo".into(), [31; 32], [32; 32]);
    {
        let mut state = server.state.lock().unwrap();
        state
            .devices
            .insert("agent-hugo".into(), rotated.device.clone());
        state.labels_unavailable = true;
    }
    success(
        &a,
        &[
            "send",
            "rotated-marker",
            "--to",
            "hugo/grok-bot/ext-hugo",
            "--json",
        ],
    );
    success(
        &a,
        &[
            "send",
            "exact-marker",
            "--to",
            "id:agent-hugo/grok-bot/ext-hugo",
            "--json",
        ],
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.messages.len(), 2);
    for envelope in &state.messages {
        assert_eq!(envelope.recipient_fingerprint, rotated.device.fingerprint());
        assert!(matches!(
            rotated
                .open(
                    &state.devices["agent-max"],
                    envelope,
                    (everett::session::now() * 1000.0) as i64
                )
                .unwrap(),
            everett::relay::Payload::Inbox { .. }
        ));
    }
}

#[test]
fn queued_retry_survives_name_change_and_remote_denial_without_stopping_receiver() {
    let server = Server::new();
    let a = fixture();
    let b = fixture();
    server.configure(&a, "agent-max");
    server.configure(&b, "agent-hugo");
    server.state.lock().unwrap().enqueue_unavailable = true;
    let out = run(
        &a,
        &[
            "send",
            "offline-marker",
            "--to",
            "Hugo Box/grok-bot/ext-hugo",
            "--idempotency-key",
            "offline-test",
            "--json",
        ],
    );
    assert!(!out.status.success());
    let before = {
        let mut s = server.state.lock().unwrap();
        s.labels.insert("agent-hugo".into(), "Renamed Hugo".into());
        s.revoked = true;
        s.device_queries
    };
    let mut receiver = listen(&a);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if server.state.lock().unwrap().device_queries > before {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(receiver.0.try_wait().unwrap().is_none());
    {
        let mut s = server.state.lock().unwrap();
        s.revoked = false;
        s.enqueue_unavailable = false;
    }
    loop {
        if server.state.lock().unwrap().messages.len() == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "queued ciphertext did not retry after label changed"
        );
        assert!(receiver.0.try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        server.state.lock().unwrap().messages[0].to_device,
        "agent-hugo"
    );
}

#[test]
fn temporary_trust_lookup_failure_keeps_the_only_encrypted_stage_until_recovery() {
    let server = Server::new();
    let a = fixture();
    let b = fixture();
    server.configure(&a, "agent-max");
    server.configure(&b, "agent-hugo");
    success(
        &b,
        &[
            "external",
            "add",
            "--id",
            "ext-hugo",
            "--harness",
            "grok-bot",
        ],
    );
    success(
        &a,
        &[
            "send",
            "recover-staged-marker",
            "--to",
            "Hugo Box/grok-bot/ext-hugo",
            "--json",
        ],
    );
    let (envelope, before) = {
        let mut s = server.state.lock().unwrap();
        s.pending_empty = true;
        s.unavailable = true;
        (s.messages[0].clone(), s.device_queries)
    };
    let stage = b.write(
        ".everett/relay-cloudflare/ingress/recover.json",
        &serde_json::to_string(&envelope).unwrap(),
    );
    let _receiver = listen(&b);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if server.state.lock().unwrap().device_queries > before {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        stage.exists(),
        "temporary key lookup failure discarded the only encrypted copy"
    );
    assert!(
        success(&b, &["inbox", "--session", "ext-hugo", "--peek", "--json"])["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    server.state.lock().unwrap().unavailable = false;
    loop {
        let inbox = success(&b, &["inbox", "--session", "ext-hugo", "--peek", "--json"]);
        if inbox.to_string().contains("recover-staged-marker") && !stage.exists() {
            break;
        }
        assert!(Instant::now() < deadline, "stage did not recover: {inbox}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn renamed_device_keeps_cli_and_mcp_message_status() {
    let server = Server::new();
    let a = fixture();
    let b = fixture();
    server.configure(&a, "agent-max");
    server.configure(&b, "agent-hugo");
    let sent = success(
        &a,
        &[
            "send",
            "status-marker",
            "--to",
            "Hugo Box/grok-bot/ext-hugo",
            "--json",
        ],
    );
    let mid = sent["message_id"].as_str().unwrap();
    server
        .state
        .lock()
        .unwrap()
        .labels
        .insert("agent-hugo".into(), "Renamed Hugo".into());
    let status = success(&a, &["peer", "status", "Hugo Box", mid]);
    assert_eq!(status["state"], "queued");
    let results = common::mcp_calls(
        &a,
        &[],
        &[],
        &[("everett_agent_status", json!({"message_id":mid}))],
    );
    assert_ne!(results[0]["isError"], true, "{:?}", results);
    assert!(results[0].to_string().contains("queued"), "{:?}", results);
    server.state.lock().unwrap().revoked = true;
    assert!(!run(&a, &["peer", "status", "Hugo Box", mid])
        .status
        .success());
}
