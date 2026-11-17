use super::super::{OAuth, OAuthAgent};
use super::*;
use model::{Capability, Decision, Reachability, Request, Snapshot};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    os::fd::AsRawFd,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};

struct Server {
    issuer: String,
    response: Arc<Mutex<(u16, Value)>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn prepare_connection(stream: &std::net::TcpStream) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))
}

#[test]
fn fixture_issuer_restores_blocking_on_accepted_sockets() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let _client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (stream, _) = listener.accept().unwrap();
    // Emulate the listener flag inherited by accepted sockets on macOS.
    stream.set_nonblocking(true).unwrap();
    prepare_connection(&stream).unwrap();
    // SAFETY: the stream owns a live descriptor; F_GETFL only reads its flags.
    let flags = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0);
    assert_eq!(flags & libc::O_NONBLOCK, 0, "issuer reads must wait for the complete request within their timeout");
    assert_eq!(stream.read_timeout().unwrap(), Some(Duration::from_secs(2)));
}

fn read_request(stream: &mut impl Read) -> std::io::Result<()> {
    const LIMIT: usize = 16 * 1024;
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid fixture request");
    let mut bytes = Vec::new();
    loop {
        if bytes.len() == LIMIT { return Err(invalid()); }
        let mut chunk = [0; 1024];
        let size = chunk.len().min(LIMIT - bytes.len());
        let read = stream.read(&mut chunk[..size])?;
        if read == 0 { return Err(std::io::ErrorKind::UnexpectedEof.into()); }
        bytes.extend_from_slice(&chunk[..read]);
        let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
        let headers = std::str::from_utf8(&bytes[..end]).map_err(|_| invalid())?;
        let length = headers.lines().filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim().parse::<usize>().map_err(|_| invalid()))
            .transpose()?.unwrap_or(0);
        if length > LIMIT - (end + 4) { return Err(invalid()); }
        if bytes.len() >= end + 4 + length { return Ok(()); }
    }
}

#[test]
fn fixture_issuer_drains_fragmented_request_before_reply() {
    struct Fragmented(std::io::Cursor<Vec<u8>>);
    impl Read for Fragmented {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let size = bytes.len().min(7);
            self.0.read(&mut bytes[..size])
        }
    }
    let body = "token=synthetic-fixture-token";
    let request = format!("POST /oauth/introspect HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    let mut stream = Fragmented(std::io::Cursor::new(request.as_bytes().to_vec()));
    read_request(&mut stream).unwrap();
    assert_eq!(stream.0.position() as usize, request.len(), "receive the complete POST before replying and closing");
    let mut incomplete = std::io::Cursor::new(&request.as_bytes()[..request.len() - 1]);
    assert_eq!(read_request(&mut incomplete).unwrap_err().kind(), std::io::ErrorKind::UnexpectedEof);
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let response = Arc::new(Mutex::new((200, json!({}))));
        let stop = Arc::new(AtomicBool::new(false));
        let (reply, done) = (response.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    prepare_connection(&stream).unwrap();
                    if read_request(&mut stream).is_err() {
                        continue;
                    }
                    let (status, value) = reply.lock().unwrap().clone();
                    let body = value.to_string();
                    let _ = write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                } else {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        });
        Self {
            issuer,
            response,
            stop,
            thread: Some(thread),
        }
    }
    fn reply(&self, status: u16, value: Value) {
        *self.response.lock().unwrap() = (status, value);
    }
    fn oauth(&self, owner: &str, client: &str) {
        self.reply(
            200,
            json!({"active":true,"iss":self.issuer,"aud":"http://localhost/mcp",
            "exp":(session::now() as u64)+3600,"scope":"everett","sub":"legacy-chat",
            "everett_agent":"legacy-chat","client_id":client,"everett_account":owner}),
        );
    }
    fn native(&self, owner: &str) {
        self.reply(
            200,
            json!({"result":{"owner":owner,"deviceId":"native-device","role":"member"}}),
        );
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn token() -> String {
    format!("eva_{}", "a".repeat(64))
}
fn oauth(server: &Server) -> OAuth {
    OAuth {
        issuer: server.issuer.clone(),
        resource: "http://localhost/mcp".into(),
        scope: "everett".into(),
        introspection_endpoint: None,
        owner_account: None,
    }
}
fn binding(home: &std::path::Path, server: &Server) -> Binding {
    let mut binding: Binding =
        serde_json::from_value(json!({"agent_id":"legacy-chat", "harness":"openai-dot",
        "cwd":home,"project":"fixture","destinations":[],"allow_shared_core":false}))
        .unwrap();
    binding.home = home.into();
    binding.oauth = Some(oauth(server));
    binding
}
fn register_logical(binding: &Binding, owner: &str, client: &str, key: &str) -> Value {
    let selected = binding.for_oauth(&OAuthAgent {
        expires_at: u64::MAX,
        agent_id: "legacy-chat".into(),
        name: String::new(),
        client_id: client.into(),
        account: Some(owner.into()),
    });
    identities::create(&selected, &key.repeat(32), "Duplicate name", "chat").unwrap()
}
fn store_native(server: &Server, owner: &str) {
    let path = cloudflare_auth::credential_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, json!({"issuer":server.issuer,"agent_id":"native-device","owner":owner,
        "device_name":"fixture","token":"native-fixture-secret","expires_at":((session::now()+3600.0)*1000.0) as u64}).to_string()).unwrap();
}
fn hook(id: &str, pid: i32) -> host::Host {
    let host = host::Host {
        harness: "codex".into(),
        pid,
        started: session::now() - 1.0,
        cwd: "/fixture".into(),
    };
    let path = crate::inbox::live_path(id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        json!({"pid":pid,"ts":session::now(),"harness":"codex"}).to_string(),
    )
    .unwrap();
    host
}

#[test]
fn verified_ownership_journey() {
    if !crate::test_support::isolated(
        "gateway::ownership_shadow::tests::verified_ownership_journey",
    ) {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("EVERETT_HOME", home.path());
    let server = Server::new();
    let binding = binding(home.path(), &server);
    let verifier = Arc::new(
        OAuthVerifier::new(oauth(&server), "fixture-introspection-secret".into()).unwrap(),
    );
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let one = register_logical(&binding, "owner", "plugin", "a");
    let two = register_logical(&binding, "owner", "plugin", "b");
    server.oauth("owner", "plugin");
    let first = runtime
        .block_on(logical(
            &binding,
            &verifier,
            token(),
            one["identity_handle"].as_str().unwrap(),
        ))
        .unwrap();
    let second = runtime
        .block_on(logical(
            &binding,
            &verifier,
            token(),
            two["identity_handle"].as_str().unwrap(),
        ))
        .unwrap();
    assert_ne!(first.session.session_id, second.session.session_id);
    let again = runtime
        .block_on(logical(
            &binding,
            &verifier,
            token(),
            one["identity_handle"].as_str().unwrap(),
        ))
        .unwrap();
    assert_eq!(again.session.session_id, first.session.session_id);
    for (owner, client) in [("foreign", "plugin"), ("owner", "forged-client")] {
        server.oauth(owner, client);
        assert!(runtime
            .block_on(logical(
                &binding,
                &verifier,
                token(),
                one["identity_handle"].as_str().unwrap()
            ))
            .is_err());
    }
    server.oauth("owner", "plugin");
    assert!(runtime
        .block_on(logical(
            &binding,
            &verifier,
            token(),
            one["session_id"].as_str().unwrap()
        ))
        .is_err());
    server.reply(200, json!({"active":false}));
    assert!(runtime
        .block_on(logical(
            &binding,
            &verifier,
            token(),
            one["identity_handle"].as_str().unwrap()
        ))
        .is_err());
    server.oauth("owner", "plugin");
    let selected = binding.for_oauth(&OAuthAgent {
        expires_at: u64::MAX,
        agent_id: "legacy-chat".into(),
        name: String::new(),
        client_id: "plugin".into(),
        account: Some("owner".into()),
    });
    identities::revoke(&selected, one["identity_handle"].as_str().unwrap()).unwrap();
    assert!(runtime
        .block_on(logical(
            &binding,
            &verifier,
            token(),
            one["identity_handle"].as_str().unwrap()
        ))
        .is_err());
    let other_issuer = Server::new();
    other_issuer.oauth("owner", "plugin");
    let foreign_verifier =
        Arc::new(OAuthVerifier::new(oauth(&other_issuer), "fixture".into()).unwrap());
    assert!(runtime
        .block_on(logical(
            &binding,
            &foreign_verifier,
            token(),
            two["identity_handle"].as_str().unwrap()
        ))
        .is_err());

    server.native("owner");
    store_native(&server, "owner");
    let host_one = hook("native-one", 12345);
    let host_two = hook("native-two", 12346);
    assert!(native(&host_one, None).unwrap().is_none());
    let receipt_one = NativeReceipt::register(&host_one).unwrap();
    let receipt_two = NativeReceipt::register(&host_two).unwrap();
    let n1 = native(&host_one, Some(&receipt_one)).unwrap().unwrap();
    let n2 = native(&host_two, Some(&receipt_two)).unwrap().unwrap();
    assert_eq!(
        native(&host_one, Some(&receipt_one))
            .unwrap()
            .unwrap()
            .session
            .session_id,
        "native-one"
    );
    assert!(native(&host_two, Some(&receipt_one)).is_err());
    store_native(&server, "forged-owner");
    assert!(native(&host_one, Some(&receipt_one)).is_err());
    server.native("foreign");
    store_native(&server, "foreign");
    assert!(native(&host_one, Some(&receipt_one)).is_err());
    server.native("owner");
    store_native(&server, "owner");
    server.reply(401, json!({"error":"revoked"}));
    assert!(native(&host_one, Some(&receipt_one)).is_err());
    other_issuer.native("owner");
    store_native(&other_issuer, "owner");
    assert!(native(&host_one, Some(&receipt_one)).is_err());
    server.native("owner");
    store_native(&server, "owner");
    std::fs::remove_file(crate::inbox::live_path("native-one")).unwrap();
    assert!(native(&host_one, Some(&receipt_one)).is_err());

    let account = first.client.account.clone();
    let snapshot = Snapshot {
        clients: vec![first.client, n1.client],
        sessions: vec![first.session, second.session, n1.session, n2.session],
    };
    assert_eq!(snapshot.discover(&account).unwrap().len(), 4);
    assert!(snapshot
        .sessions
        .iter()
        .all(|s| s.reachability == Reachability::Unknown));
    let comparison = snapshot
        .compare(Request {
            account,
            from: snapshot.sessions[0].session_id.clone(),
            to: "native-two".into(),
            capability: Capability::Message,
            current: Decision::Deny,
        })
        .unwrap();
    assert_eq!(comparison.current, Decision::Deny);
    assert_eq!(comparison.proposed, Decision::Allow);
    let output = serde_json::to_string(&snapshot).unwrap();
    for secret in [
        token(),
        "native-fixture-secret".into(),
        one["identity_handle"].as_str().unwrap().into(),
        two["identity_handle"].as_str().unwrap().into(),
    ] {
        assert!(!output.contains(&secret));
    }
}

#[test]
fn mismatched_verifier_policy_is_rejected() {
    let home = tempfile::tempdir().unwrap();
    let server = Server::new();
    let mut binding = binding(home.path(), &server);
    let registered = register_logical(&binding, "foreign", "plugin", "c");
    binding.oauth.as_mut().unwrap().owner_account = Some("owner".into());
    server.oauth("foreign", "plugin");
    let verifier = Arc::new(OAuthVerifier::new(oauth(&server), "fixture".into()).unwrap());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    assert!(runtime
        .block_on(logical(
            &binding,
            &verifier,
            token(),
            registered["identity_handle"].as_str().unwrap()
        ))
        .is_err());
}
