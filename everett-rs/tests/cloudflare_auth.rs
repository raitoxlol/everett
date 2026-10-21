mod common;
use common::{fixture, Fixture};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CODE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN: &str = "evt_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
struct Fake {
    issuer: String,
    requests: Arc<Mutex<Vec<(String, String, Value)>>>,
    polls: Arc<Mutex<VecDeque<(u16, Value)>>>,
    drop_polls: Arc<Mutex<usize>>,
    poll_delay: Arc<Mutex<Duration>>,
    device_started: Arc<Mutex<Option<Instant>>>,
    identity_status: Arc<Mutex<u16>>,
    identity: Arc<Mutex<Value>>,
    cancel_state: Arc<Mutex<&'static str>>,
    interval: Arc<Mutex<u64>>,
    expires: Arc<Mutex<u64>>,
    fail_save: Arc<Mutex<Option<std::path::PathBuf>>>,
    revoke_status: Arc<Mutex<u16>>,
}
impl Fake {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let polls = Arc::new(Mutex::new(VecDeque::new()));
        let drop_polls = Arc::new(Mutex::new(0usize));
        let poll_delay = Arc::new(Mutex::new(Duration::ZERO));
        let device_started = Arc::new(Mutex::new(None));
        let started = device_started.clone();
        let identity_status = Arc::new(Mutex::new(200));
        let identity = Arc::new(Mutex::new(
            json!({"result":{"owner":"owner-synthetic","deviceId":"immutable-agent","role":"member"}}),
        ));
        let cancel_state = Arc::new(Mutex::new("cancelled"));
        let interval = Arc::new(Mutex::new(1));
        let expires = Arc::new(Mutex::new(30));
        let fail_save = Arc::new(Mutex::new(None::<std::path::PathBuf>));
        let revoke_status = Arc::new(Mutex::new(200));
        let (save_failure, revoke) = (fail_save.clone(), revoke_status.clone());
        let (drops, delay) = (drop_polls.clone(), poll_delay.clone());
        let (r, p, s, i, c, n, e, origin) = (
            requests.clone(),
            polls.clone(),
            identity_status.clone(),
            identity.clone(),
            cancel_state.clone(),
            interval.clone(),
            expires.clone(),
            issuer.clone(),
        );
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let (path, headers, body) = read_request(&mut stream);
                r.lock().unwrap().push((path.clone(), headers, body));
                if path == "/v2/auth/poll" {
                    let mut remaining = drops.lock().unwrap();
                    if *remaining > 0 {
                        *remaining -= 1;
                        continue;
                    }
                    drop(remaining);
                    std::thread::sleep(*delay.lock().unwrap());
                }
                let (status, body) = match path.as_str() {
                    "/v2/auth/device" => (
                        200,
                        json!({"device_code":CODE,"user_code":"ABCDEF123456","verification_uri":format!("{origin}/enroll"),"verification_uri_complete":format!("{origin}/enroll?code=ABCDEF123456"),"expires_in":*e.lock().unwrap(),"interval":*n.lock().unwrap()}),
                    ),
                    "/v2/auth/poll" => p.lock().unwrap().pop_front().unwrap_or((
                        200,
                        json!({"state":"pending","interval":*n.lock().unwrap()}),
                    )),
                    "/v2/auth/cancel" => (200, json!({"state":*c.lock().unwrap()})),
                    "/v2/relay/whoami" => (*s.lock().unwrap(), i.lock().unwrap().clone()),
                    "/v2/relay/revokeAgent" => (*revoke.lock().unwrap(), json!({"result":null})),
                    _ => (404, json!({"error":"unknown"})),
                };
                if path == "/v2/auth/poll" && body["state"] == "approved" {
                    if let Some(path) = save_failure.lock().unwrap().take() {
                        std::fs::create_dir(path).unwrap();
                    }
                }
                let body = body.to_string();
                if path == "/v2/auth/device" {
                    *started.lock().unwrap() = Some(Instant::now());
                }
                let _=write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            }
        });
        Self {
            issuer,
            requests,
            polls,
            drop_polls,
            poll_delay,
            device_started,
            identity_status,
            identity,
            cancel_state,
            interval,
            expires,
            fail_save,
            revoke_status,
        }
    }
    fn configure(&self, f: &Fixture) {
        f.write(
            ".everett/config.toml",
            &format!(
                "[auth]\nprovider = 'cloudflare'\nissuer = '{}'\n",
                self.issuer
            ),
        );
    }
    fn approved(&self) {
        self.polls.lock().unwrap().push_back((200,json!({"state":"approved","token":TOKEN,"agent_id":"immutable-agent","owner":"owner-synthetic","expires_at":(everett::session::now()*1000.0) as u64+60_000})));
    }
    fn requests(&self) -> Vec<(String, String, Value)> {
        self.requests.lock().unwrap().clone()
    }
    fn approval_elapsed(&self) -> Duration {
        self.device_started.lock().unwrap().unwrap().elapsed()
    }
}
fn read_request(stream: &mut TcpStream) -> (String, String, Value) {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buf = [0; 4096];
    let end = loop {
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buf[..n]);
        if let Some(p) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
            break p + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..end]).to_string();
    let len = headers
        .lines()
        .find_map(|l| {
            l.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        })
        .map(|(_, v)| v.trim().parse::<usize>().unwrap())
        .unwrap_or(0);
    while bytes.len() < end + len {
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buf[..n]);
    }
    let path = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    (
        path,
        headers,
        serde_json::from_slice(&bytes[end..end + len]).unwrap(),
    )
}
fn redact(out: &std::process::Output) {
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!all.contains(CODE), "device secret leaked");
    assert!(!all.contains(TOKEN), "native credential leaked");
}
#[test]
fn pending_approval_persists_private_credential_then_server_verifies() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    fake.polls
        .lock()
        .unwrap()
        .push_back((200, json!({"state":"pending","interval":1})));
    fake.approved();
    let out = f.run(&["login", "--device", "my-computer", "--json"]);
    redact(&out);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["verified"], true);
    assert_eq!(v["agent_id"], "immutable-agent");
    let path = f.home().join(".everett/cloudflare-auth.json");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(!f.home().join(".everett/cloudflare-pending.json").exists());
    assert!(!f.home().join(".everett/auth.json").exists());
    let requests = fake.requests();
    assert_eq!(requests[0].2, json!({"name":"my-computer"}));
    assert!(requests
        .last()
        .unwrap()
        .1
        .contains(&format!("Bearer {TOKEN}")));
    assert!(f.run(&["whoami", "--json"]).status.success());
    assert!(f.run(&["login", "--json"]).status.success());
    assert_eq!(
        fake.requests()
            .iter()
            .filter(|r| r.0 == "/v2/auth/device")
            .count(),
        1
    );
}
#[test]
fn terminal_states_do_not_enroll_and_allow_retry() {
    for state in ["denied", "cancelled", "expired", "consumed"] {
        let f = fixture();
        let fake = Fake::new();
        fake.configure(&f);
        fake.polls
            .lock()
            .unwrap()
            .push_back((200, json!({"state":state,"interval":1})));
        let out = f.run(&["login", "--json"]);
        redact(&out);
        assert_eq!(out.status.code(), Some(5));
        assert!(!f.home().join(".everett/cloudflare-auth.json").exists());
        assert!(!f.home().join(".everett/cloudflare-pending.json").exists());
        fake.approved();
        assert!(f.run(&["login", "--json"]).status.success());
    }
}
#[test]
fn start_cancel_resume_and_ctrl_c_use_real_cli() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    let out = f.run(&["login", "--no-wait", "--json"]);
    redact(&out);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["state"],
        "pending"
    );
    let pending = f.home().join(".everett/cloudflare-pending.json");
    assert_eq!(
        std::fs::metadata(&pending).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let out = f.run(&["login", "--cancel", "--json"]);
    assert!(out.status.success());
    assert!(!pending.exists());
    assert_eq!(fake.requests().last().unwrap().0, "/v2/auth/cancel");
    assert!(f.run(&["login", "--no-wait"]).status.success());
    let child = Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(["login", "--json"])
        .env_clear()
        .env("EVERETT_HOME", f.home())
        .env("HOME", f.home())
        .env("PATH", "/usr/bin:/bin")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    let out = child.wait_with_output().unwrap();
    redact(&out);
    assert_eq!(out.status.code(), Some(130));
    assert!(!pending.exists());
}
#[test]
fn pending_can_resume_without_issuer_environment_or_config() {
    let f = fixture();
    let fake = Fake::new();
    let out = f.run(&[
        "login",
        "--provider",
        "cloudflare",
        "--issuer",
        &fake.issuer,
        "--no-wait",
    ]);
    assert!(out.status.success());
    fake.approved();
    let out = f.run(&["login", "--provider", "cloudflare", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
#[test]
fn approved_cancel_is_not_claimed_as_cancelled() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    assert!(f.run(&["login", "--no-wait"]).status.success());
    *fake.cancel_state.lock().unwrap() = "approved";
    let out = f.run(&["login", "--cancel", "--json"]);
    assert_eq!(out.status.code(), Some(5));
    assert!(out.stdout.is_empty());
    assert!(f.home().join(".everett/cloudflare-pending.json").exists());
}
#[test]
fn transient_poll_error_keeps_pending_and_existing_oidc_file() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    f.write(".everett/auth.json", "synthetic-existing-oidc");
    fake.polls
        .lock()
        .unwrap()
        .push_back((503, json!({"error":TOKEN})));
    let out = f.run(&["login"]);
    redact(&out);
    assert_eq!(out.status.code(), Some(6));
    assert!(f.home().join(".everett/cloudflare-pending.json").exists());
    assert_eq!(
        std::fs::read_to_string(f.home().join(".everett/auth.json")).unwrap(),
        "synthetic-existing-oidc"
    );
    fake.approved();
    assert!(f.run(&["login"]).status.success());
}

#[test]
fn dropped_poll_connection_resumes_the_same_approval() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    *fake.drop_polls.lock().unwrap() = 1;
    fake.approved();
    let before = Instant::now();
    let out = f.run(&["login", "--json"]);
    redact(&out);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let identity: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(identity["verified"], true);
    assert_eq!(identity["agent_id"], "immutable-agent");
    assert!(before.elapsed() >= Duration::from_secs(3));
    assert!(!f.home().join(".everett/cloudflare-pending.json").exists());
    let requests = fake.requests();
    assert_eq!(requests.iter().filter(|r| r.0 == "/v2/auth/device").count(), 1);
    assert_eq!(requests.iter().filter(|r| r.0 == "/v2/auth/poll").count(), 2);
}

#[test]
fn dropped_poll_connections_stop_at_the_approval_deadline() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    *fake.drop_polls.lock().unwrap() = usize::MAX;
    *fake.expires.lock().unwrap() = 4;
    let out = f.run(&["login", "--json"]);
    redact(&out);
    assert_eq!(out.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&out.stderr).contains("approval code expired"));
    assert!(fake.approval_elapsed() >= Duration::from_secs(4));
    assert!(fake.approval_elapsed() < Duration::from_secs(7));
    assert!(!f.home().join(".everett/cloudflare-pending.json").exists());
    assert!(!f.home().join(".everett/cloudflare-auth.json").exists());
    let requests = fake.requests();
    assert_eq!(requests.iter().filter(|r| r.0 == "/v2/auth/device").count(), 1);
    assert_eq!(requests.iter().filter(|r| r.0 == "/v2/auth/poll").count(), 2);
}

#[test]
fn stalled_poll_timeout_is_bounded_by_the_approval_deadline() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    *fake.expires.lock().unwrap() = 2;
    *fake.poll_delay.lock().unwrap() = Duration::from_secs(4);
    let out = f.run(&["login", "--json"]);
    redact(&out);
    assert_eq!(out.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&out.stderr).contains("approval code expired"));
    assert!(fake.approval_elapsed() < Duration::from_millis(3500));
    assert!(!f.home().join(".everett/cloudflare-pending.json").exists());
    assert!(!f.home().join(".everett/cloudflare-auth.json").exists());
}

#[test]
fn ctrl_c_cancels_during_poll_transport_backoff() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    *fake.drop_polls.lock().unwrap() = usize::MAX;
    let mut child = Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(["login", "--json"])
        .env_clear()
        .env("EVERETT_HOME", f.home())
        .env("HOME", f.home())
        .env("PATH", "/usr/bin:/bin")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !fake.requests().iter().any(|r| r.0 == "/v2/auth/poll") {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("login did not reach its first approval poll within fifteen seconds");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
    let before = Instant::now();
    unsafe { libc::kill(child.id() as i32, libc::SIGINT); }
    let out = child.wait_with_output().unwrap();
    redact(&out);
    assert_eq!(out.status.code(), Some(130));
    assert!(before.elapsed() < Duration::from_secs(2));
    assert!(!f.home().join(".everett/cloudflare-pending.json").exists());
    assert!(!f.home().join(".everett/cloudflare-auth.json").exists());
    let requests = fake.requests();
    assert_eq!(requests.iter().filter(|r| r.0 == "/v2/auth/device").count(), 1);
    assert_eq!(requests.last().unwrap().0, "/v2/auth/cancel");
}

#[test]
fn poll_retry_preserves_authoritative_and_invalid_response_errors() {
    for (status, body, exit) in [
        (401, json!({"error":"unauthorized"}), 1),
        (403, json!({"error":"forbidden"}), 1),
        (503, json!({"error":"unconfigured"}), 6),
        (200, json!({"state":"consumed"}), 5),
        (200, json!({"state":"invalid"}), 6),
    ] {
        let f = fixture();
        let fake = Fake::new();
        fake.configure(&f);
        *fake.drop_polls.lock().unwrap() = 1;
        fake.polls.lock().unwrap().push_back((status, body));
        fake.approved();
        let out = f.run(&["login", "--json"]);
        redact(&out);
        assert_eq!(out.status.code(), Some(exit));
        assert!(!f.home().join(".everett/cloudflare-auth.json").exists());
        if exit == 5 {
            assert!(String::from_utf8_lossy(&out.stderr).contains("already collected"));
            assert!(!f.home().join(".everett/cloudflare-pending.json").exists());
        }
        let requests = fake.requests();
        assert_eq!(requests.iter().filter(|r| r.0 == "/v2/auth/device").count(), 1);
        assert_eq!(requests.iter().filter(|r| r.0 == "/v2/auth/poll").count(), 2);
    }
}
#[test]
fn slow_down_backs_off_before_collecting_approval() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    fake.polls
        .lock()
        .unwrap()
        .push_back((429, json!({"error":"slow_down"})));
    fake.approved();
    let before = Instant::now();
    assert!(f.run(&["login"]).status.success());
    assert!(before.elapsed() >= Duration::from_secs(7));
}
#[test]
fn identity_expiry_revocation_mismatch_and_transient_preserve_credentials() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    fake.approved();
    assert!(f.run(&["login"]).status.success());
    let path = f.home().join(".everett/cloudflare-auth.json");
    let original = std::fs::read(&path).unwrap();
    for status in [401, 403, 503] {
        *fake.identity_status.lock().unwrap() = status;
        let out = f.run(&["whoami", "--json"]);
        redact(&out);
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
    *fake.identity_status.lock().unwrap() = 200;
    fake.identity.lock().unwrap()["result"]["deviceId"] = json!("another-agent");
    assert!(!f.run(&["whoami"]).status.success());
    let mut stored: Value = serde_json::from_slice(&original).unwrap();
    stored["expires_at"] = json!(1);
    std::fs::write(&path, stored.to_string()).unwrap();
    let before = fake.requests().len();
    let out = f.run(&["whoami"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(fake.requests().len(), before);
}
#[test]
fn failed_credential_save_never_reports_success_or_clears_pending() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    assert!(f.run(&["login", "--no-wait"]).status.success());
    *fake.fail_save.lock().unwrap() = Some(f.home().join(".everett/cloudflare-auth.json"));
    fake.approved();
    let out = f.run(&["login", "--json"]);
    redact(&out);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(f.home().join(".everett/cloudflare-pending.json").exists());
}
#[test]
fn local_logout_and_confirmed_remote_revoke_are_distinct() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    fake.approved();
    assert!(f.run(&["login"]).status.success());
    let out = f.run(&["logout", "--json"]);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["revoked"],
        false
    );
    assert!(!fake
        .requests()
        .iter()
        .any(|r| r.0 == "/v2/relay/revokeAgent"));
    fake.approved();
    assert!(f.run(&["login"]).status.success());
    let out = f.run(&["logout", "--revoke-device", "--json"]);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["revoked"],
        true
    );
    assert_eq!(
        fake.requests().last().unwrap().2,
        json!({"deviceId":"immutable-agent"})
    );
}
#[test]
fn unsafe_origins_fail_without_network_or_pending_state() {
    for issuer in [
        "http://example.com",
        "https://x.example/path",
        "https://user:secret@x.example",
        "https://x.example?token=secret",
    ] {
        let f = fixture();
        let out = f.run(&[
            "login",
            "--provider",
            "cloudflare",
            "--issuer",
            issuer,
            "--no-wait",
        ]);
        assert_eq!(out.status.code(), Some(2));
        assert!(!f.home().join(".everett/cloudflare-pending.json").exists());
    }
    let f = fixture();
    let out = f.run(&["login", "--provider", "oidc", "--no-wait"]);
    assert_eq!(out.status.code(), Some(2));
}
#[test]
fn local_approval_deadline_stops_before_next_poll() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    *fake.expires.lock().unwrap() = 1;
    *fake.interval.lock().unwrap() = 5;
    let out = f.run(&["login"]);
    assert_eq!(out.status.code(), Some(5));
    assert!(!fake.requests().iter().any(|r| r.0 == "/v2/auth/poll"));
}

#[test]
fn failed_remote_revoke_and_failed_verification_keep_private_credential() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    fake.approved();
    *fake.identity_status.lock().unwrap() = 503;
    let out = f.run(&["login", "--json"]);
    redact(&out);
    assert_eq!(out.status.code(), Some(6));
    assert!(out.stdout.is_empty());
    let path = f.home().join(".everett/cloudflare-auth.json");
    let original = std::fs::read(&path).unwrap();
    assert!(!f.home().join(".everett/cloudflare-pending.json").exists());
    *fake.revoke_status.lock().unwrap() = 403;
    let out = f.run(&["logout", "--revoke-device", "--json"]);
    assert_eq!(out.status.code(), Some(6));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    *fake.identity_status.lock().unwrap() = 200;
    assert!(f.run(&["whoami", "--json"]).status.success());
}

#[cfg(unix)]
#[test]
fn browser_approval_resumes_onboarding_then_guides_and_prints_review_prompt() {
    for (cols, rows) in [(80, 24), (54, 14)] {
        let f = fixture();
        let fake = Fake::new();
        fake.approved();
        common::plant(&f, "claude.jsonl", ".claude/projects/app/claude.jsonl");
        f.write("AGENTS.md", "Existing project instructions.\n");
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let script = f.write(
            &format!("bin/{opener}"),
            "#!/bin/sh\nprintf '%s' \"$1\" > \"$EVERETT_HOME/browser-url\"\n",
        );
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_everett"));
        cmd.env_clear()
            .env("HOME", f.home())
            .env("EVERETT_HOME", f.home())
            .env("EVERETT_AUTH_ISSUER", &fake.issuer)
            .env("PATH", f.home().join("bin"))
            .env("TERM", "xterm")
            .env("LANG", "en_US.UTF-8")
            .env("EVERETT_NO_ANIM", "1");
        let mut session = expectrl::Session::spawn(cmd).unwrap();
        session
            .get_process_mut()
            .set_window_size(cols, rows)
            .unwrap();
        let mut vt = vt100::Parser::new(rows, cols, 0);
        fn see(session: &mut expectrl::Session, vt: &mut vt100::Parser, needle: &str) {
            let end = Instant::now() + Duration::from_secs(10);
            loop {
                let mut buf = [0u8; 8192];
                while let Ok(n) = session.try_read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    vt.process(&buf[..n]);
                }
                if vt.screen().contents().contains(needle) {
                    return;
                }
                assert!(
                    Instant::now() < end,
                    "missing {needle}: {}",
                    vt.screen().contents()
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        see(&mut session, &mut vt, "Welcome to Everett");
        session.send("\r").unwrap();
        see(&mut session, &mut vt, "Choose your agents");
        let url = std::fs::read_to_string(f.home().join("browser-url")).unwrap();
        assert_eq!(url, format!("{}/enroll?code=ABCDEF123456", fake.issuer));
        assert!(f.home().join(".everett/cloudflare-auth.json").exists());
        let configured = std::fs::read_to_string(f.home().join(".everett/config.toml")).unwrap();
        assert!(configured.contains("cloudflare"), "{configured}");
        assert!(!f.home().join(".everett/onboarding-complete").exists());
        let mut repeat = Command::new(env!("CARGO_BIN_EXE_everett"));
        repeat.env_clear().env("HOME", f.home()).env("EVERETT_HOME", f.home())
            .env("PATH", "/nonexistent").env("TERM", "dumb");
        let mut repeat = expectrl::Session::spawn(repeat).unwrap();
        repeat.set_expect_timeout(Some(Duration::from_secs(5)));
        repeat.expect("Usage: everett").unwrap();
        repeat.expect(expectrl::Eof).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&f.run(&["whoami", "--json"]).stdout).unwrap()["verified"], true);
        session.send("\r").unwrap();
        see(&mut session, &mut vt, "Claude Code / setup guide");
        session.send("\x1b[F").unwrap();
        see(
            &mut session,
            &mut vt,
            "Instruction Markdown is never edited",
        );
        session.send("\r").unwrap();
        see(&mut session, &mut vt, "Review your setup");
        session.send("a").unwrap();
        see(&mut session, &mut vt, "Local setup applied");
        session.send("\r").unwrap();
        see(&mut session, &mut vt, "Teach your agents about Everett");
        session.send("\x1b[F").unwrap();
        see(&mut session, &mut vt, "Do not include credentials.");
        session.send("\r").unwrap();
        see(&mut session, &mut vt, "Do not include credentials.");
        session.set_expect_timeout(Some(Duration::from_secs(5)));
        session.expect(expectrl::Eof).unwrap();
        assert_eq!(
            std::fs::read_to_string(f.home().join("AGENTS.md")).unwrap(),
            "Existing project instructions.\n"
        );
        assert!(f.home().join(".everett/onboarding-complete").exists());
    }
}

#[test]
fn native_profile_and_convex_account_never_fall_back_to_each_other() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    fake.approved();
    f.write(".everett/auth.json", "synthetic-existing-oidc");
    assert!(f.run(&["login"]).status.success());
    let out = f.run(&["relay", "whoami"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&out.stdout).unwrap()["deviceId"], "immutable-agent");
    assert!(fake.requests().last().unwrap().1.contains(TOKEN));
    assert!(!f.home().join(".everett/relay/keys.json").exists());
    f.write(".everett/config.toml", "[auth]\nprovider = 'oidc'\nissuer = 'https://old-oidc.example/realm'\nclient_id = 'old-client'\n");
    let out = f.run(&["login", "--provider", "cloudflare", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(f.home().join(".everett/auth.json")).unwrap(),
        "synthetic-existing-oidc"
    );
}

#[test]
fn cancel_reports_authoritative_state_instead_of_claiming_cancellation() {
    for state in ["cancelled", "denied", "expired"] {
        let f = fixture();
        let fake = Fake::new();
        fake.configure(&f);
        assert!(f.run(&["login", "--no-wait"]).status.success());
        *fake.cancel_state.lock().unwrap() = state;
        let out = f.run(&["login", "--cancel", "--json"]);
        assert!(out.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap()["state"],
            state
        );
        assert!(!f.home().join(".everett/cloudflare-pending.json").exists());
    }
}

#[test]
fn interrupted_blocking_browser_opener_cancels_pending_login() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let script = f.write(&format!("bin/{opener}"), "#!/bin/sh\nprintf started > \"$EVERETT_HOME/opener-started\"\nexec /bin/sleep 30\n");
    std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(["login", "--open", "--json"]).env_clear()
        .env("HOME", f.home()).env("EVERETT_HOME", f.home()).env("PATH", f.home().join("bin"))
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f.home().join("opener-started").exists() {
        assert!(Instant::now() < deadline, "opener did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    unsafe { libc::kill(child.id() as i32, libc::SIGINT); }
    let status = wait_timeout::ChildExt::wait_timeout(&mut child, Duration::from_secs(3)).unwrap();
    if status.is_none() { child.kill().unwrap(); }
    let out = child.wait_with_output().unwrap();
    redact(&out);
    assert!(fake.requests().iter().any(|r| r.0 == "/v2/auth/cancel"), "Ctrl-C did not cancel pending approval");
    assert_eq!(out.status.code(), Some(130));
    assert!(!f.home().join(".everett/cloudflare-pending.json").exists());
}

#[test]
fn rejected_credential_starts_new_approval_but_uncertainty_keeps_it() {
    let f = fixture();
    let fake = Fake::new();
    fake.configure(&f);
    fake.approved();
    assert!(f.run(&["login"]).status.success());
    let path = f.home().join(".everett/cloudflare-auth.json");
    let original = std::fs::read(&path).unwrap();
    for status in [503, 403] {
        *fake.identity_status.lock().unwrap() = status;
        assert!(!f.run(&["login", "--no-wait"]).status.success());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(!f.home().join(".everett/cloudflare-pending.json").exists());
    }
    *fake.identity_status.lock().unwrap() = 200;
    fake.identity.lock().unwrap()["result"]["deviceId"] = json!("other-agent");
    assert!(!f.run(&["login", "--no-wait"]).status.success());
    assert!(!f.home().join(".everett/cloudflare-pending.json").exists());
    fake.identity.lock().unwrap()["result"]["deviceId"] = json!("immutable-agent");
    *fake.identity_status.lock().unwrap() = 401;
    let out = f.run(&["login", "--no-wait", "--json"]);
    redact(&out);
    assert!(out.status.success(), "revoked credential could not restart enrollment: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&out.stdout).unwrap()["state"], "pending");
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(fake.requests().iter().filter(|r| r.0 == "/v2/auth/device").count(), 2);
    *fake.identity_status.lock().unwrap() = 200;
    fake.approved();
    assert!(f.run(&["login"]).status.success());
}
