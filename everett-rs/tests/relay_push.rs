mod common;
use common::{fixture, Fixture};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tokio_tungstenite::tungstenite::{
    accept,
    protocol::{frame::coding::CloseCode, CloseFrame},
    Error, Message,
};

const TOKEN: &str = "evt_cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
#[derive(Clone)]
enum Control {
    Hint,
    Disconnect,
    Invalid,
}
#[derive(Default)]
struct Shared {
    paths: Mutex<Vec<String>>,
    watchers: Mutex<Vec<mpsc::Sender<Control>>>,
    connections: AtomicUsize,
    pings: AtomicUsize,
    revoked: AtomicBool,
    stop: AtomicBool,
    during_scan: AtomicBool,
}
impl Shared {
    fn signal(&self, signal: Control) {
        for sender in self.watchers.lock().unwrap().iter() {
            let _ = sender.send(signal.clone());
        }
    }
    fn count(&self, path: &str) -> usize {
        self.paths
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.as_str() == path)
            .count()
    }
}
struct Server {
    url: String,
    shared: Arc<Shared>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(push: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let shared = Arc::new(Shared::default());
        let state = shared.clone();
        let worker = thread::spawn(move || {
            let mut workers = Vec::new();
            while !state.stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let state = state.clone();
                        workers.push(thread::spawn(move || serve(stream, state, push)));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("fixture listener: {error}"),
                }
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self {
            url,
            shared,
            worker: Some(worker),
        }
    }
    fn configure(&self, f: &Fixture) {
        f.write(
            ".everett/config.toml",
            &format!("[auth]\nprovider='cloudflare'\nissuer='{}'\n", self.url),
        );
        f.write(".everett/cloudflare-auth.json",&json!({"issuer":self.url,"owner":"synthetic-owner","agent_id":"synthetic-agent","device_name":"fixture","token":TOKEN,"expires_at":(everett::session::now()*1000.0) as u64+3_600_000}).to_string());
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn response(stream: &mut TcpStream, status: u16, body: Value) {
    let body = body.to_string();
    let _=write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
}
fn serve(mut stream: TcpStream, state: Arc<Shared>, push: bool) {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut peek = [0; 8192];
    let deadline = Instant::now() + Duration::from_secs(3);
    let headers = loop {
        match stream.peek(&mut peek) {
            Ok(0) => return,
            Ok(n) if peek[..n].windows(4).any(|w| w == b"\r\n\r\n") => {
                break String::from_utf8(peek[..n].to_vec()).unwrap()
            }
            _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            _ => return,
        }
    };
    let path = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    assert_eq!(
        headers.lines().find_map(|line| line
            .split_once(':')
            .filter(|(key, _)| key.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.trim())),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    state.paths.lock().unwrap().push(path.clone());
    if path == "/v2/relay/watch" {
        if !push {
            let mut consumed = Vec::new();
            let mut byte = [0];
            while !consumed.ends_with(b"\r\n\r\n") {
                if stream.read_exact(&mut byte).is_err() {
                    return;
                }
                consumed.push(byte[0]);
            }
            response(&mut stream, 404, json!({"error":"not_found"}));
            return;
        }
        if state.revoked.load(Ordering::SeqCst) {
            response(&mut stream, 401, json!({"error":"invalid_token"}));
            return;
        }
        let mut socket = accept(stream).unwrap();
        socket
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        state.connections.fetch_add(1, Ordering::SeqCst);
        let (send, receive) = mpsc::channel();
        state.watchers.lock().unwrap().push(send);
        socket
            .send(Message::Text("{\"type\":\"inbox_changed\"}".into()))
            .unwrap();
        while !state.stop.load(Ordering::SeqCst) {
            if state.revoked.load(Ordering::SeqCst) {
                let _ = socket.close(Some(CloseFrame {
                    code: CloseCode::Policy,
                    reason: "revoked".into(),
                }));
                return;
            }
            for control in receive.try_iter() {
                match control {
                    Control::Hint => {
                        if socket
                            .send(Message::Text("{\"type\":\"inbox_changed\"}".into()))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Control::Disconnect => {
                        let _ = socket.close(Some(CloseFrame {
                            code: CloseCode::Away,
                            reason: "reconnect".into(),
                        }));
                        return;
                    }
                    Control::Invalid => {
                        let _ = socket.send(Message::Text(
                            "{\"type\":\"run_command\",\"text\":\"untrusted\"}".into(),
                        ));
                    }
                }
            }
            match socket.read() {
                Ok(Message::Ping(_)) => {
                    state.pings.fetch_add(1, Ordering::SeqCst);
                    let _ = socket.flush();
                }
                Ok(Message::Close(_)) => {
                    let _ = socket.flush();
                    return;
                }
                Ok(_) => panic!("receiver sent an application frame"),
                Err(Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => return,
            }
        }
        return;
    }
    let mut bytes = Vec::new();
    let mut byte = [0u8];
    while !bytes.ends_with(b"\r\n\r\n") {
        if stream.read_exact(&mut byte).is_err() {
            return;
        }
        bytes.push(byte[0]);
    }
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).unwrap();
    if state.revoked.load(Ordering::SeqCst) {
        response(&mut stream, 401, json!({"error":"invalid_token"}));
        return;
    }
    let value = match path.as_str() {
        "/v2/relay/whoami" => {
            json!({"owner":"synthetic-owner","deviceId":"synthetic-agent","role":"member"})
        }
        "/v2/relay/register" | "/v2/relay/observe" => Value::Null,
        "/v2/relay/pending" => {
            if state.during_scan.swap(false, Ordering::SeqCst) {
                state.signal(Control::Hint);
                thread::sleep(Duration::from_millis(250));
            }
            json!({"serverNow":(everett::session::now()*1000.0) as i64,"envelopes":[],"isDone":true,"continueCursor":"0"})
        }
        _ => panic!("unexpected fixture request {path}"),
    };
    response(&mut stream, 200, json!({"result":value}));
}
struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn receiver(f: &Fixture) -> Running {
    Running(
        Command::new(
            std::env::var_os("EVERETT_VERIFY_BINARY")
                .unwrap_or_else(|| env!("CARGO_BIN_EXE_everett").into()),
        )
        .arg("relay")
        .env_clear()
        .env("HOME", f.home())
        .env("EVERETT_HOME", f.home())
        .env("PATH", common::HERMETIC_PATH)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap(),
    )
}
fn until(seconds: u64, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !ready() {
        assert!(Instant::now() < deadline, "push fixture deadline");
        thread::sleep(Duration::from_millis(25));
    }
}
fn started(server: &Server, child: &mut Running) {
    until(10, || {
        server.shared.count("/v2/relay/pending") >= 1 || child.0.try_wait().unwrap().is_some()
    });
    if let Some(status) = child.0.try_wait().unwrap() {
        let mut error = String::new();
        child
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut error)
            .unwrap();
        panic!("receiver exited during startup ({status}): {error}");
    }
}

#[test]
fn idle_push_has_no_recurring_http_requests_and_protocol_pings_do_not_poll() {
    let f = fixture();
    let server = Server::new(true);
    server.configure(&f);
    let mut child = receiver(&f);
    started(&server, &mut child);
    let initial = server.shared.paths.lock().unwrap().len();
    let idle_started = Instant::now();
    thread::sleep(Duration::from_secs(36));
    eprintln!(
        "PUSH_COST_PROOF {}",
        json!({
            "startup_http_requests": initial,
            "idle_seconds": idle_started.elapsed().as_secs_f64(),
            "additional_idle_http_requests": server.shared.paths.lock().unwrap().len() - initial,
            "connections": server.shared.connections.load(Ordering::SeqCst),
            "protocol_pings": server.shared.pings.load(Ordering::SeqCst),
            "pending_calls":server.shared.count("/v2/relay/pending"),
        })
    );
    assert!(server.shared.pings.load(Ordering::SeqCst) >= 1);
    assert_eq!(
        server.shared.paths.lock().unwrap().len(),
        initial,
        "idle protocol heartbeat made HTTP requests"
    );
    assert_eq!(server.shared.count("/v2/relay/pending"), 1);
    assert_eq!(server.shared.connections.load(Ordering::SeqCst), 1);
    assert!(child.0.try_wait().unwrap().is_none());
    f.write(".everett/inbox/claim.jsonl",&format!("{}\n",json!({"id":"local-claim","text":"consumed","relay_owner":"synthetic-owner","relay_recipient_device":"synthetic-agent","relay_sender_owner":"synthetic-owner","relay_message_id":"remote-claim"})));
    f.write(".everett/inbox/claim.done", "local-claim\n");
    until(8, || server.shared.count("/v2/relay/observe") == 1);
    assert_eq!(
        server.shared.count("/v2/relay/pending"),
        1,
        "local claim caused idle inbox polling"
    );
    eprintln!("push idle proof: 1 connection, 1 initial inbox scan, 0 additional HTTP requests through protocol heartbeat; local claim observed separately");
}

#[test]
fn notifications_during_a_scan_are_not_lost_and_reconnect_rescans() {
    let f = fixture();
    let server = Server::new(true);
    server.configure(&f);
    let mut child = receiver(&f);
    started(&server, &mut child);
    server.shared.during_scan.store(true, Ordering::SeqCst);
    server.shared.signal(Control::Hint);
    until(8, || server.shared.count("/v2/relay/pending") >= 3);
    server.shared.signal(Control::Disconnect);
    until(15, || {
        server.shared.connections.load(Ordering::SeqCst) >= 2
            && server.shared.count("/v2/relay/pending") >= 4
    });
    assert!(child.0.try_wait().unwrap().is_none());
}

fn local_binding_stops(change: &str) {
    let f = fixture();
    let server = Server::new(true);
    server.configure(&f);
    if change == "expiry" {
        let path = f.home().join(".everett/cloudflare-auth.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["expires_at"] = json!((everett::session::now() * 1000.0) as u64 + 6000);
        fs::write(path, value.to_string()).unwrap();
    }
    let mut child = receiver(&f);
    started(&server, &mut child);
    let before = server.shared.paths.lock().unwrap().len();
    match change {
        "logout" => fs::remove_file(f.home().join(".everett/cloudflare-auth.json")).unwrap(),
        "provider" => {
            f.write(".everett/config.toml", "[auth]\nprovider='retired'\n");
        }
        "credential" => {
            let path = f.home().join(".everett/cloudflare-auth.json");
            let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            value["token"] = json!("evt_replaced");
            fs::write(path, value.to_string()).unwrap();
        }
        "expiry" => {}
        _ => unreachable!(),
    }
    until(12, || child.0.try_wait().unwrap().is_some());
    assert_eq!(child.0.try_wait().unwrap().unwrap().code(), Some(1));
    assert_eq!(
        server.shared.paths.lock().unwrap().len(),
        before,
        "local binding check added remote requests"
    );
}
#[test]
fn push_stops_locally_on_logout_without_remote_heartbeat() {
    local_binding_stops("logout");
}
#[test]
fn push_stops_locally_on_provider_change_without_remote_heartbeat() {
    local_binding_stops("provider");
}
#[test]
fn push_stops_locally_on_credential_change_without_remote_heartbeat() {
    local_binding_stops("credential");
}
#[test]
fn push_stops_locally_on_expired_credential_without_remote_heartbeat() {
    local_binding_stops("expiry");
}
#[test]
fn revoked_and_malformed_notification_streams_stop_without_polling_fallback() {
    for revoked in [true, false] {
        let f = fixture();
        let server = Server::new(true);
        server.configure(&f);
        let mut child = receiver(&f);
        started(&server, &mut child);
        if revoked {
            server.shared.revoked.store(true, Ordering::SeqCst);
        } else {
            server.shared.signal(Control::Invalid);
        }
        until(8, || child.0.try_wait().unwrap().is_some());
        assert_eq!(
            child.0.try_wait().unwrap().unwrap().code(),
            Some(if revoked { 1 } else { 2 })
        );
        assert_eq!(server.shared.count("/v2/relay/pending"), 1);
    }
}
#[test]
fn unsupported_worker_requires_explicit_poll_mode_instead_of_silent_idle_cost() {
    let f = fixture();
    let server = Server::new(false);
    server.configure(&f);
    let mut child = receiver(&f);
    until(10, || child.0.try_wait().unwrap().is_some());
    assert_eq!(child.0.try_wait().unwrap().unwrap().code(), Some(2));
    assert_eq!(server.shared.count("/v2/relay/pending"), 0);
    let mut error = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut error)
        .unwrap();
    assert!(error.contains("deploy the matching Worker"));
    assert!(error.contains("EVERETT_RELAY_TRANSPORT=poll"));
}
