mod common;

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use common::fixture;

/// In-process OIDC fake: scripts device-flow poll outcomes, refresh results,
/// and records every request body so tests can assert on grant parameters.
struct Fake {
    base: String,
    state: Arc<Mutex<State>>,
    _listener: Arc<TcpListener>,
}

struct State {
    token_seq: VecDeque<&'static str>,
    refresh_error: Option<&'static str>,
    rotated_refresh: &'static str,
    requests: Vec<String>,
    device_expires_in: u64,
}

impl Fake {
    fn start() -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(State {
            token_seq: VecDeque::new(),
            refresh_error: None,
            rotated_refresh: "rt-rotated",
            requests: Vec::new(),
            device_expires_in: 30,
        }));
        let st = state.clone();
        let l = listener.try_clone().unwrap();
        thread::spawn(move || loop {
            let Ok((mut stream, _)) = l.accept() else { break };
            let st = st.clone();
            thread::spawn(move || handle(&mut stream, &st, addr.port()));
        });
        Fake {
            base: format!("http://127.0.0.1:{}", addr.port()),
            state,
            _listener: Arc::new(listener),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.state.lock().unwrap().requests.clone()
    }
}

fn read_request(stream: &mut TcpStream) -> (String, String, String) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos;
        }
        if n == 0 {
            break buf.len();
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("").to_string();
    let content_length: usize = head
        .lines()
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-length")))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0);
    let mut body = if buf.len() > header_end + 4 {
        buf[header_end + 4..].to_vec()
    } else {
        Vec::new()
    };
    while body.len() < content_length {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let mut parts = request_line.split_whitespace();
    (
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("").to_string(),
        String::from_utf8_lossy(&body).to_string(),
    )
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Error",
    };
    let out = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(out.as_bytes());
}

fn handle(stream: &mut TcpStream, state: &Arc<Mutex<State>>, port: u16) {
    let (method, path, body) = read_request(stream);
    state.lock().unwrap().requests.push(format!("{method} {path} {body}"));
    let base = format!("http://127.0.0.1:{port}");
    match (method.as_str(), path.as_str()) {
        ("GET", "/.well-known/openid-configuration") => respond(
            stream,
            200,
            &format!(
                r#"{{"issuer":"{base}","device_authorization_endpoint":"{base}/device","token_endpoint":"{base}/token","userinfo_endpoint":"{base}/userinfo","revocation_endpoint":"{base}/revoke"}}"#
            ),
        ),
        ("POST", "/device") => {
            let expires = state.lock().unwrap().device_expires_in;
            respond(
                stream,
                200,
                &format!(
                    r#"{{"device_code":"dc-1","user_code":"WDJB-MJHT","verification_uri":"{base}/activate","verification_uri_complete":"{base}/activate?user_code=WDJB-MJHT","interval":0,"expires_in":{expires}}}"#
                ),
            );
        }
        ("POST", "/token") => {
            if body.contains("grant_type=refresh_token") {
                let mut st = state.lock().unwrap();
                if let Some(err) = st.refresh_error.take() {
                    respond(stream, 400, &format!(r#"{{"error":"{err}"}}"#));
                } else {
                    respond(
                        stream,
                        200,
                        &format!(
                            r#"{{"access_token":"at-new","refresh_token":"{}","expires_in":3600}}"#,
                            st.rotated_refresh
                        ),
                    );
                }
                return;
            }
            let mut st = state.lock().unwrap();
            match st.token_seq.pop_front().unwrap_or("pending") {
                "pending" => respond(stream, 400, r#"{"error":"authorization_pending"}"#),
                "slow_down" => respond(stream, 400, r#"{"error":"slow_down"}"#),
                "access_denied" => respond(stream, 400, r#"{"error":"access_denied"}"#),
                "expired" => respond(stream, 400, r#"{"error":"expired_token"}"#),
                _ => respond(
                    stream,
                    200,
                    r#"{"access_token":"at-1","refresh_token":"rt-1","expires_in":3600,"scope":"openid email profile offline_access"}"#,
                ),
            }
        }
        ("GET", "/userinfo") => respond(
            stream,
            200,
            r#"{"sub":"auth0|abc123","email":"alex@example.com","name":"Alex"}"#,
        ),
        ("POST", "/revoke") => respond(stream, 200, r#"{}"#),
        _ => respond(stream, 400, r#"{"error":"unknown_path"}"#),
    }
}

fn configure(home: &common::Fixture, issuer: &str) {
    home.write(
        ".everett/config.toml",
        &format!("[auth]\nissuer = \"{issuer}\"\nclient_id = \"client-1\"\n"),
    );
}

fn stored_json(issuer: &str, expires_at: i64) -> String {
    format!(
        r#"{{"issuer":"{issuer}","client_id":"client-1","sub":"auth0|abc123","email":"alex@example.com","name":"Alex","device_name":"mac-studio","access_token":"at-1","refresh_token":"rt-1","expires_at":{expires_at},"scope":"openid"}}"#
    )
}

fn auth_path(f: &common::Fixture) -> std::path::PathBuf {
    f.home().join(".everett/auth.json")
}

#[test]
fn login_happy_path_writes_auth_and_whoami_prints_email() {
    let f = fixture();
    let fake = Fake::start();
    fake.state.lock().unwrap().token_seq = ["pending", "slow_down", "success"].into();
    configure(&f, &fake.base);

    let out = f.run(&["login"]);
    assert_eq!(out.status.code(), Some(0), "{}", f.stdout(&out));
    let text = f.stdout(&out);
    assert!(text.contains("WDJB-MJHT"), "{text}");
    assert!(text.contains("Signed in as alex@example.com"), "{text}");

    let path = auth_path(&f);
    assert!(path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(stored["email"], "alex@example.com");
    assert_eq!(stored["sub"], "auth0|abc123");
    assert_eq!(stored["issuer"], fake.base);
    assert_eq!(stored["client_id"], "client-1");
    assert_eq!(stored["refresh_token"], "rt-1");
    assert!(stored["expires_at"].as_f64().unwrap() > 0.0);

    let out = f.run(&["whoami"]);
    assert_eq!(out.status.code(), Some(0), "{}", f.stdout(&out));
    assert!(f.stdout(&out).contains("alex@example.com"));

    let reqs = fake.requests();
    let device = reqs.iter().find(|r| r.contains("/device")).unwrap();
    assert!(device.contains("client_id=client-1"), "{device}");
    let poll = reqs
        .iter()
        .find(|r| r.contains("/token") && r.contains("grant_type=") && r.contains("device_code"))
        .unwrap();
    assert!(poll.contains("device_code=dc-1"), "{poll}");
    assert!(poll.contains("client_id=client-1"), "{poll}");
}

#[test]
fn login_access_denied_exits_5_without_auth_file() {
    let f = fixture();
    let fake = Fake::start();
    fake.state.lock().unwrap().token_seq = ["access_denied"].into();
    configure(&f, &fake.base);
    let out = f.run(&["login"]);
    assert_eq!(out.status.code(), Some(5), "{}", f.stdout(&out));
    assert!(!auth_path(&f).exists());
}

#[test]
fn login_deadline_exits_5() {
    let f = fixture();
    let fake = Fake::start();
    fake.state.lock().unwrap().device_expires_in = 1;
    configure(&f, &fake.base);
    let out = f.run(&["login"]);
    assert_eq!(out.status.code(), Some(5), "{}", f.stdout(&out));
    assert!(!auth_path(&f).exists());
}

#[test]
fn login_unconfigured_exits_2() {
    let f = fixture();
    let out = f.run(&["login"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("auth.issuer"));
}

#[test]
fn whoami_rotates_refresh_token() {
    let f = fixture();
    let fake = Fake::start();
    configure(&f, &fake.base);
    f.write(".everett/auth.json", &stored_json(&fake.base, 1));

    let out = f.run(&["whoami"]);
    assert_eq!(out.status.code(), Some(0), "{}", f.stdout(&out));
    assert!(f.stdout(&out).contains("alex@example.com"));
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(auth_path(&f)).unwrap()).unwrap();
    assert_eq!(stored["refresh_token"], "rt-rotated");
    assert_eq!(stored["access_token"], "at-new");
}

#[test]
fn whoami_invalid_grant_clears_file() {
    let f = fixture();
    let fake = Fake::start();
    configure(&f, &fake.base);
    fake.state.lock().unwrap().refresh_error = Some("invalid_grant");
    f.write(".everett/auth.json", &stored_json(&fake.base, 1));

    let out = f.run(&["whoami"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!auth_path(&f).exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("everett login"));
}

#[test]
fn whoami_not_signed_in_exits_1() {
    let f = fixture();
    let out = f.run(&["whoami"]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn logout_revokes_and_deletes() {
    let f = fixture();
    let fake = Fake::start();
    configure(&f, &fake.base);
    f.write(".everett/auth.json", &stored_json(&fake.base, 9999999999));

    let out = f.run(&["logout"]);
    assert_eq!(out.status.code(), Some(0), "{}", f.stdout(&out));
    assert!(!auth_path(&f).exists());
    let reqs = fake.requests();
    let revoke = reqs.iter().find(|r| r.contains("/revoke")).expect("revocation call missing");
    assert!(revoke.contains("token=rt-1"), "{revoke}");
    assert!(revoke.contains("token_type_hint=refresh_token"), "{revoke}");
}

#[test]
fn logout_provider_down_still_deletes() {
    let f = fixture();
    // Point the stored session at a dead issuer: nothing listens on port 1.
    let dead = "http://127.0.0.1:1";
    configure(&f, dead);
    f.write(".everett/auth.json", &stored_json(dead, 9999999999));
    let out = f.run(&["logout"]);
    assert_eq!(out.status.code(), Some(0), "{}", f.stdout(&out));
    assert!(!auth_path(&f).exists());
    assert!(f.stdout(&out).contains("did not happen"), "{}", f.stdout(&out));
}

#[test]
fn json_outputs_never_contain_tokens() {
    let f = fixture();
    let fake = Fake::start();
    fake.state.lock().unwrap().token_seq = ["success"].into();
    configure(&f, &fake.base);

    let out = f.run(&["login", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", f.stdout(&out));
    let text = f.stdout(&out);
    assert!(!text.contains("at-1") && !text.contains("rt-1") && !text.contains("dc-1"), "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(v["expires_at"].as_f64().is_some());
    assert_eq!(v["email"], "alex@example.com");
    assert_eq!(v["device_name"].as_str().map(|s| !s.is_empty()), Some(true));

    let out = f.run(&["whoami", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let text = f.stdout(&out);
    assert!(!text.contains("at-1") && !text.contains("rt-1"), "{text}");

    let out = f.run(&["logout", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let text = f.stdout(&out);
    assert!(!text.contains("at-1") && !text.contains("rt-1"), "{text}");
}

#[test]
fn login_is_noop_when_signed_in_unless_force() {
    let f = fixture();
    let fake = Fake::start();
    fake.state.lock().unwrap().token_seq = ["success"].into();
    configure(&f, &fake.base);
    f.write(".everett/auth.json", &stored_json(&fake.base, 9999999999));

    let out = f.run(&["login"]);
    assert_eq!(out.status.code(), Some(0), "{}", f.stdout(&out));
    assert!(f.stdout(&out).contains("Already signed in"));

    let out = f.run(&["login", "--force"]);
    assert_eq!(out.status.code(), Some(0), "{}", f.stdout(&out));
    assert!(f.stdout(&out).contains("Signed in as alex@example.com"));
}

#[test]
fn doctor_reports_account_line() {
    let f = fixture();
    let fake = Fake::start();
    configure(&f, &fake.base);
    let out = f.run(&["doctor"]);
    assert!(f.stdout(&out).contains("account: not signed in"), "{}", f.stdout(&out));

    f.write(".everett/auth.json", &stored_json(&fake.base, 9999999999));
    let out = f.run(&["doctor"]);
    let text = f.stdout(&out);
    assert!(text.contains("account: alex@example.com"), "{text}");
    assert!(text.contains("mac-studio"), "{text}");
}
