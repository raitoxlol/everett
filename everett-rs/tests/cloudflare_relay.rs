mod common;
use common::{fixture, Fixture};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TOKEN: &str = "evt_cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[test]
fn corrupted_outgoing_record_is_retained_without_killing_or_flooding_the_receiver() {
    let f = fixture();
    let server = Server::new();
    server.configure(&f);
    server.pending.store(1, Ordering::SeqCst);
    let path = f.write(".everett/relay-cloudflare/outgoing/corrupted.json", "{private-corrupt-receipt");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut child = listener(&f);
    until(|| server.pending.load(Ordering::SeqCst) >= 3 || child.0.try_wait().unwrap().is_some());
    if child.0.try_wait().unwrap().is_some() {
        let mut errors = String::new();
        child.0.stderr.take().unwrap().read_to_string(&mut errors).unwrap();
        panic!("corrupted local receipt killed receiver: {errors}");
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{private-corrupt-receipt");
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let mut errors = String::new();
    child.0.stderr.take().unwrap().read_to_string(&mut errors).unwrap();
    assert_eq!(errors.matches("local outgoing record is unreadable").count(), 1);
    assert!(!errors.contains("private-corrupt-receipt"));
}

#[test]
fn reconciliation_skips_local_and_incomplete_inbox_messages() {
    let f = fixture();
    let server = Server::new();
    server.configure(&f);
    server.pending.store(1, Ordering::SeqCst);
    let messages = [
        json!({}),
        json!({"id":"local","text":"ordinary local message"}),
        json!({"id":"no-recipient","relay_owner":"synthetic-owner"}),
        json!({"id":"no-sender","relay_owner":"synthetic-owner","relay_recipient_device":"synthetic-agent"}),
        json!({"id":"no-remote-id","relay_owner":"synthetic-owner","relay_recipient_device":"synthetic-agent","relay_sender_owner":"synthetic-owner"}),
    ];
    f.write(
        ".everett/inbox/receiver-regression.jsonl",
        &messages.iter().map(Value::to_string).collect::<Vec<_>>().join("\n"),
    );
    f.write(
        ".everett/inbox/receiver-regression.done",
        "local\nno-recipient\nno-sender\nno-remote-id\n",
    );
    let mut child = listener(&f);
    until(|| server.pending.load(Ordering::SeqCst) >= 3 || child.0.try_wait().unwrap().is_some());
    if child.0.try_wait().unwrap().is_some() {
        let mut errors = String::new();
        child.0.stderr.take().unwrap().read_to_string(&mut errors).unwrap();
        panic!("receiver exited while reconciling unrelated inbox rows: {errors}");
    }
    assert!(!server.calls.lock().unwrap().iter().any(|path| path == "/v2/relay/observe"));
    child.0.kill().unwrap();
    child.0.wait().unwrap();
}

struct Server {
    url: String,
    pending: Arc<AtomicUsize>,
    revoked: Arc<AtomicBool>,
    calls: Arc<Mutex<Vec<String>>>,
    labels_missing: Arc<AtomicBool>,
    device_error: Arc<Mutex<Option<String>>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let pending = Arc::new(AtomicUsize::new(0));
        let revoked = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let labels_missing = Arc::new(AtomicBool::new(false));
        let missing = labels_missing.clone();
        let device_error = Arc::new(Mutex::new(None::<String>));
        let denial = device_error.clone();
        let (p, r, c) = (pending.clone(), revoked.clone(), calls.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let (path, args) = request(&mut stream);
                c.lock().unwrap().push(path.clone());
                let (status, result) = if r.load(Ordering::SeqCst) {
                    (401, json!({"error":"invalid_token"}))
                } else {
                    match path.as_str() {
                        "/v2/relay/whoami" => (
                            200,
                            json!({"result":{"owner":"synthetic-owner","deviceId":"synthetic-agent","role":"member"}}),
                        ),
                        "/v2/relay/register" => (200, json!({"result":null})),
                        "/v2/relay/devices" if denial.lock().unwrap().is_some() => {
                            (403, json!({"error":denial.lock().unwrap().clone().unwrap(),"detail":TOKEN}))
                        }
                        "/v2/relay/devices" => {
                            let device = everett::relay::crypto::Keys::new(
                                "synthetic-owner".into(),
                                "synthetic-agent".into(),
                                [1; 32],
                                [2; 32],
                            )
                            .device;
                            (200, json!({"result":[device]}))
                        }
                        "/v2/relay/deviceLabels" if missing.load(Ordering::SeqCst) => {
                            (404, json!({"error":"not_found"}))
                        }
                        "/v2/relay/renameDevice" => {
                            assert_eq!(args, json!({"name":"renamed-fixture"}));
                            (
                                200,
                                json!({"result":{"labels":{"synthetic-agent":"renamed-fixture"}}}),
                            )
                        }
                        "/v2/relay/deviceLabels" => (
                            200,
                            json!({"result":{"labels":{"synthetic-agent":"fixture-nuc"}}}),
                        ),
                        "/v2/relay/pending" if p.fetch_add(1, Ordering::SeqCst) == 0 => {
                            (503, json!({"error":"service_unavailable"}))
                        }
                        "/v2/relay/pending" => (
                            200,
                            json!({"result":{"serverNow":(everett::session::now()*1000.0) as i64,"envelopes":[],"isDone":true,"continueCursor":"0","padding":"x".repeat(100_000)}}),
                        ),
                        _ => (404, json!({"error":"unsupported"})),
                    }
                };
                let body = result.to_string();
                let _ = write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        Self {
            url,
            pending,
            revoked,
            calls,
            labels_missing,
            device_error,
        }
    }
    fn configure(&self, f: &Fixture) {
        f.write(
            ".everett/config.toml",
            &format!("[auth]\nprovider = 'cloudflare'\nissuer = '{}'\n", self.url),
        );
        f.write(".everett/cloudflare-auth.json", &json!({"issuer":self.url,"owner":"synthetic-owner","agent_id":"synthetic-agent","device_name":"test-device","token":TOKEN,"expires_at":(everett::session::now()*1000.0) as u64+120_000}).to_string());
        f.write(".everett/auth.json", "synthetic-old-oidc");
    }
}
fn request(stream: &mut TcpStream) -> (String, Value) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut data = Vec::new();
    let mut byte = [0u8; 1];
    while !data.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        data.push(byte[0]);
        assert!(data.len() < 8192);
    }
    let headers = String::from_utf8(data).unwrap();
    assert!(headers.contains(TOKEN));
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
    (
        headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_string(),
        serde_json::from_slice(&body).unwrap(),
    )
}
struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn listener(f: &Fixture) -> Running {
    Running(
        Command::new(env!("CARGO_BIN_EXE_everett"))
            .arg("relay")
            .env_clear()
            .env("HOME", f.home())
            .env("EVERETT_HOME", f.home())
            .env("PATH", "/usr/bin:/bin")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}
fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "native receiver did not reach expected state"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}
#[test]
fn transient_pending_failure_reconnects_large_page_then_revocation_stops() {
    let f = fixture();
    let server = Server::new();
    server.configure(&f);
    let original = std::fs::read(f.home().join(".everett/cloudflare-auth.json")).unwrap();
    let mut child = listener(&f);
    until(|| server.pending.load(Ordering::SeqCst) >= 2);
    assert!(child.0.try_wait().unwrap().is_none());
    server.revoked.store(true, Ordering::SeqCst);
    until(|| child.0.try_wait().unwrap().is_some());
    assert_eq!(child.0.try_wait().unwrap().unwrap().code(), Some(1));
    let mut errors = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut errors)
        .unwrap();
    assert!(errors.contains("retrying in 5s"), "{errors}");
    assert!(
        !errors.contains("invalid response"),
        "large pending response was rejected: {errors}"
    );
    assert!(!errors.contains(TOKEN));
    assert_eq!(
        std::fs::read(f.home().join(".everett/cloudflare-auth.json")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read_to_string(f.home().join(".everett/auth.json")).unwrap(),
        "synthetic-old-oidc"
    );
    assert!(!f.home().join(".everett/relay").exists());
}
#[test]
fn provider_switch_stops_native_listener_without_using_oidc() {
    let f = fixture();
    let server = Server::new();
    server.configure(&f);
    let mut child = listener(&f);
    until(|| server.pending.load(Ordering::SeqCst) >= 1);
    f.write(".everett/config.toml", "[auth]\nprovider = 'oidc'\n");
    let count = server.calls.lock().unwrap().len();
    until(|| child.0.try_wait().unwrap().is_some());
    assert_eq!(child.0.try_wait().unwrap().unwrap().code(), Some(1));
    assert_eq!(server.calls.lock().unwrap().len(), count);
    assert!(!f.home().join(".everett/relay").exists());
}
#[test]
fn same_owner_credential_replacement_stops_before_another_request() {
    let f = fixture();
    let server = Server::new();
    server.configure(&f);
    let mut child = listener(&f);
    until(|| server.pending.load(Ordering::SeqCst) >= 1);
    let path = f.home().join(".everett/cloudflare-auth.json");
    let mut replacement: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    replacement["agent_id"] = json!("synthetic-replacement-agent");
    replacement["token"] =
        json!("evt_dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd");
    let bytes = serde_json::to_vec(&replacement).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let count = server.calls.lock().unwrap().len();
    until(|| child.0.try_wait().unwrap().is_some());
    assert_eq!(child.0.try_wait().unwrap().unwrap().code(), Some(1));
    assert_eq!(server.calls.lock().unwrap().len(), count);
    let mut errors = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut errors)
        .unwrap();
    assert!(errors.contains("signed-in identity changed"), "{errors}");
    assert!(!errors.contains(TOKEN));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(
        std::fs::read_to_string(f.home().join(".everett/auth.json")).unwrap(),
        "synthetic-old-oidc"
    );
    assert!(!f.home().join(".everett/relay").exists());
}
#[test]
fn relay_device_listing_shows_advisory_names_and_preserves_json_bundles() {
    let f = fixture();
    let server = Server::new();
    server.configure(&f);
    let out = f.run(&["relay", "devices", "--names"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = f.stdout(&out);
    assert!(text.contains("fixture-nuc"), "{text}");
    assert!(text.contains("synthetic-agent"));
    assert!(text.contains("not signed"));
    let raw = f.run(&["relay", "devices", "--json"]);
    assert!(raw.status.success());
    let bundles: Value = serde_json::from_slice(&raw.stdout).unwrap();
    assert_eq!(bundles[0].as_object().unwrap().len(), 5);
    assert!(bundles[0].get("name").is_none());
    let legacy = f.run(&["relay", "devices"]);
    assert_eq!(raw.stdout, legacy.stdout);
    server.labels_missing.store(true, Ordering::SeqCst);
    let fallback = f.run(&["relay", "devices", "--names"]);
    assert!(fallback.status.success());
    assert!(f.stdout(&fallback).contains("Names unavailable"));
    assert!(f.stdout(&fallback).contains("(unnamed)"));
    let renamed = f.run(&["relay", "rename", "renamed-fixture"]);
    assert!(renamed.status.success());
    assert!(f.stdout(&renamed).contains("renamed-fixture"));
    let count = server.calls.lock().unwrap().len();
    let invalid = f.run(&["relay", "rename", "\x1b[31m"]);
    assert!(!invalid.status.success());
    assert_eq!(server.calls.lock().unwrap().len(), count);
}

#[test]
fn unsupported_registry_operation_never_falls_back_to_oidc() {
    let f = fixture();
    let server = Server::new();
    server.configure(&f);
    let out = f.run(&["devices", "--json"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("requires OIDC/Convex"));
    assert!(server.calls.lock().unwrap().is_empty());
}

#[test]
fn missing_device_bundle_has_an_actionable_error_without_echoing_server_secrets() {
    let f = fixture();
    let server = Server::new();
    server.configure(&f);
    *server.device_error.lock().unwrap() = Some("device_unavailable".into());
    let out = f.run(&["relay", "devices"]);
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("device_unavailable"), "{text}");
    assert!(text.contains("registered key bundle"), "{text}");
    assert!(!text.contains(TOKEN));
    *server.device_error.lock().unwrap() = Some(TOKEN.into());
    let unknown = f.run(&["relay", "devices"]);
    assert!(!unknown.status.success());
    assert!(!String::from_utf8_lossy(&unknown.stderr).contains(TOKEN));
    assert!(!String::from_utf8_lossy(&unknown.stdout).contains(TOKEN));
}
