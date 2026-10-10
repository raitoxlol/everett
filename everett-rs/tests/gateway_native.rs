//! Native gateway journeys over stdio and HTTP. The test binary doubles as the synthetic
//! backend (`--fixture-backend`), so no model, harness, or other runtime is involved.
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_everett");
const TOKEN: &str = "synthetic-gateway-test-token-not-a-real-secret";
const TOOLS: [&str; 5] = ["everett_core", "everett_whoami", "everett_inbox", "everett_card", "everett_send"];

fn initialize() -> Value {
    json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "synthetic-gateway-tests", "version": "0"}})
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

fn backend(args: &[String]) {
    let home = PathBuf::from(std::env::var("EVERETT_HOME").unwrap());
    let own = std::env::var("EVERETT_SESSION_ID").unwrap();
    let harness = std::env::var("EVERETT_HARNESS_NAME").unwrap();
    let inbox = home.join(".everett/inbox").join(format!("{}.jsonl", own));
    let rows = || -> Vec<Value> {
        std::fs::read_to_string(&inbox).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    };
    for line in std::io::stdin().lock().lines() {
        let message: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let Some(id) = message.get("id").cloned() else { continue };
        let result = match message["method"].as_str().unwrap() {
            "initialize" => {
                let mut capabilities = json!({"tools": {}});
                if !args.iter().any(|a| a == "--legacy") && std::env::var("EVERETT_GATEWAY_EXACT_IDS").as_deref() == Ok("1") {
                    let exact = option(args, "--capability").map(|c| serde_json::from_str(c).unwrap()).unwrap_or(json!({"enforced": true}));
                    capabilities["experimental"] = json!({"everettExactDestinationIds": exact});
                }
                json!({"protocolVersion": "2025-06-18", "capabilities": capabilities, "serverInfo": {"name": "synthetic-everett", "version": "0"}})
            }
            "tools/list" => json!({"tools": TOOLS.iter().map(|n| json!({"name": n, "inputSchema": {"type": "object"}})).collect::<Vec<_>>()}),
            "tools/call" => {
                let name = message["params"]["name"].as_str().unwrap().to_string();
                let a = message["params"]["arguments"].clone();
                let has_secret = ["EVERETT_GATEWAY_TOKEN", "OPENAI_API_KEY", "TYPESAFE_API_KEY"].iter().any(|k| std::env::var_os(k).is_some());
                let record = json!({"name": name, "arguments": a, "pid": std::process::id(),
                    "cwd": std::env::current_dir().unwrap(), "agent_id": own, "harness": harness, "has_secret": has_secret});
                let mut log = std::fs::OpenOptions::new().create(true).append(true).open(home.join("worker-calls.jsonl")).unwrap();
                writeln!(log, "{}", record).unwrap();
                if args.iter().any(|a| a == "--hang") {
                    std::thread::sleep(Duration::from_secs(60));
                }
                let data = match name.as_str() {
                    "everett_whoami" => json!({"session_id": own, "harness": harness}),
                    "everett_core" => json!({"core": format!("Synthetic global + project {}", a["project"].as_str().unwrap()), "pending_learnings": 99}),
                    "everett_inbox" => json!({"messages": rows()}),
                    "everett_send" => {
                        assert!(a["mode"] == "inbox" && a["wait"] == 0 && a["spawn"] == false, "unsafe send arguments");
                        let to = match a.get("reply_to") {
                            Some(r) => rows().into_iter().find(|row| &row["id"] == r).unwrap()["from"].clone(),
                            None => a["to"].clone(),
                        };
                        json!({"delivered": true, "mode": "inbox", "message_id": "synthetic-reply", "to": to})
                    }
                    _ => json!({"session_id": own, "written": true}),
                };
                let mut result = json!({"content": [{"type": "text", "text": data.to_string()}], "structuredContent": data, "isError": false});
                match option(args, "--result-mode") {
                    Some("text-only") => { result.as_object_mut().unwrap().remove("structuredContent"); }
                    Some("oversized") => result = json!({"content": [], "structuredContent": {"core": "x".repeat(256 * 1024 + 1)}}),
                    Some("amplified") => result = json!({"content": [], "structuredContent": {"core": "x".repeat(140 * 1024)}}),
                    Some("wrong-identity") => result["structuredContent"]["session_id"] = json!("foreign"),
                    Some("wrong-harness") => result["structuredContent"]["harness"] = json!("foreign"),
                    Some("non-object") => result["structuredContent"] = json!(["invalid"]),
                    _ => {}
                }
                result
            }
            _ => json!({}),
        };
        println!("{}", json!({"jsonrpc": "2.0", "id": id, "result": result}));
        std::io::stdout().flush().unwrap();
    }
}

struct Wire {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    counter: i64,
}

fn line_reader(stream: impl Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

impl Wire {
    fn new(mut command: Command) -> Wire {
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let lines = line_reader(child.stdout.take().unwrap());
        let mut wire = Wire { stdin: child.stdin.take(), child, lines, counter: 0 };
        wire.request("initialize", Some(initialize()));
        wire.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        wire
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", message).unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Option<Value>) -> Value {
        self.counter += 1;
        let mut message = json!({"jsonrpc": "2.0", "id": self.counter, "method": method});
        if let Some(p) = params {
            message["params"] = p;
        }
        self.send(&message);
        let line = self.lines.recv_timeout(Duration::from_secs(6)).expect("native gateway response timed out");
        serde_json::from_str(&line).unwrap()
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", Some(json!({"name": name, "arguments": arguments})))
    }
}

impl Drop for Wire {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Ctx {
    _dir: tempfile::TempDir,
    home: PathBuf,
    config: Value,
    number: Cell<u32>,
    servers: RefCell<Vec<Child>>,
}

impl Drop for Ctx {
    fn drop(&mut self) {
        for child in self.servers.borrow_mut().iter_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn ctx() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(dir.path()).unwrap();
    let config = json!({"agent_id": "dot-one", "harness": "openai-dot", "cwd": home, "project": "everett",
        "destinations": ["agent-two"], "allow_shared_core": true});
    Ctx { _dir: dir, home, config, number: Cell::new(0), servers: RefCell::new(Vec::new()) }
}

fn merged(base: &Value, changes: &Value) -> Value {
    let mut out = base.clone();
    for (k, v) in changes.as_object().unwrap() {
        out[k] = v.clone();
    }
    out
}

impl Ctx {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command.args(args).env_clear().current_dir(&self.home);
        for (k, v) in [("HOME", self.home.to_str().unwrap()), ("EVERETT_HOME", self.home.to_str().unwrap()),
            ("PATH", "/nonexistent"), ("EVERETT_NOTIFY", "none"), ("EVERETT_ROUTER", "local"),
            ("EVERETT_GATEWAY_TOKEN", TOKEN), ("OPENAI_API_KEY", "synthetic"), ("TYPESAFE_API_KEY", "synthetic")] {
            command.env(k, v);
        }
        command
    }

    fn write_config(&self, config: &Value) -> PathBuf {
        self.number.set(self.number.get() + 1);
        let path = self.home.join(format!("binding-{}.json", self.number.get()));
        std::fs::write(&path, config.to_string()).unwrap();
        path
    }

    fn config_path(&self, changes: &Value) -> PathBuf {
        self.write_config(&merged(&self.config, changes))
    }

    fn cli(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn gateway_code(&self, path: &Path) -> Option<i32> {
        self.cli(&["gateway", "--config", path.to_str().unwrap()]).status.code()
    }

    fn peer(&self, changes: &Value) -> Wire {
        let path = self.config_path(changes);
        Wire::new(self.command(&["gateway", "--config", path.to_str().unwrap()]))
    }

    fn inbox(&self, sender: &str, to: &str, id: &str) -> PathBuf {
        let dir = self.home.join(".everett/inbox");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dot-one.jsonl");
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
        std::fs::write(&path, format!("{}\n", json!({"id": id, "to": to, "from": sender, "text": "Synthetic handoff", "ts": ts}))).unwrap();
        path
    }

    fn records(&self) -> Vec<Value> {
        std::fs::read_to_string(self.home.join("worker-calls.jsonl")).unwrap_or_default()
            .lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    fn http_server(&self, changes: &Value) -> u16 {
        let path = self.config_path(changes);
        let mut child = self.command(&["gateway", "--config", path.to_str().unwrap(), "--transport", "http", "--port", "0"])
            .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
        let lines = line_reader(child.stderr.take().unwrap());
        self.servers.borrow_mut().push(child);
        let line = lines.recv_timeout(Duration::from_secs(5)).expect("HTTP startup timed out");
        std::thread::spawn(move || while lines.recv().is_ok() {});
        assert!(line.contains("listening on 127.0.0.1:"), "{line}");
        line.rsplit(':').next().unwrap().trim().parse().unwrap()
    }
}

fn fixture(options: &[&str]) -> Value {
    let exe = std::env::current_exe().unwrap();
    let mut command = vec![exe.to_string_lossy().to_string(), "--fixture-backend".into()];
    command.extend(options.iter().map(|o| o.to_string()));
    json!({"backend_command": command})
}

fn with(base: Value, extra: Value) -> Value {
    merged(&base, &extra)
}

fn success(response: &Value) -> Value {
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(response["result"]["isError"], true, "{response}");
    response["result"]["structuredContent"].clone()
}

fn denied(response: &Value) {
    assert!(response.get("error").is_some() || response["result"]["isError"] == true, "{response}");
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn raw_exchange(port: u16, request: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
    let _ = stream.write_all(request);
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    out
}

fn dechunk(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(end) = body.windows(2).position(|w| w == b"\r\n") {
        let size = usize::from_str_radix(std::str::from_utf8(&body[..end]).unwrap().trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        out.extend_from_slice(&body[end + 2..end + 2 + size]);
        body = &body[end + 4 + size..];
    }
    out
}

fn http(port: u16, method: &str, body: &str, overrides: &[(&str, Option<&str>)]) -> Reply {
    let auth = format!("Bearer {}", TOKEN);
    let mut headers: Vec<(String, String)> = [("Authorization", auth.as_str()), ("Host", "127.0.0.1:8788"),
        ("Content-Type", "application/json"), ("Accept", "application/json, text/event-stream"),
        ("MCP-Protocol-Version", "2025-06-18")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    for (key, value) in overrides {
        headers.retain(|(k, _)| k != key);
        if let Some(v) = value {
            headers.push((key.to_string(), v.to_string()));
        }
    }
    let mut request = format!("{} /mcp HTTP/1.1\r\n", method);
    for (k, v) in &headers {
        request += &format!("{}: {}\r\n", k, v);
    }
    request += &format!("Content-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
    let raw = raw_exchange(port, request.as_bytes());
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("HTTP response");
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())).collect();
    let mut body = raw[split + 4..].to_vec();
    if headers.iter().any(|(k, v)| k == "transfer-encoding" && v.contains("chunked")) {
        body = dechunk(&body);
    }
    Reply { status, headers, body }
}

fn post(port: u16, message: &Value, overrides: &[(&str, Option<&str>)]) -> Reply {
    http(port, "POST", &message.to_string(), overrides)
}

fn body_json(reply: &Reply) -> Value {
    let text = String::from_utf8_lossy(&reply.body).to_string();
    let data = text.lines().find_map(|l| l.strip_prefix("data:")).map(|d| d.trim().to_string()).unwrap_or(text);
    serde_json::from_str(&data).unwrap_or_else(|_| panic!("{}", data))
}

fn pid_alive(pid: u64) -> bool {
    Command::new("/bin/kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn eventually_dead(pid: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while pid_alive(pid) {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

fn native_stdio_journey_for_both_external_harnesses() {
    let c = ctx();
    for harness in ["openai-dot", "grok-bot"] {
        for id in ["agent-two", "dot-one"] {
            assert!(c.cli(&["external", "add", "--id", id, "--harness", harness]).status.success());
        }
        let mut peer = c.peer(&json!({"harness": harness}));
        let tools: Vec<String> = peer.request("tools/list", None)["result"]["tools"].as_array().unwrap()
            .iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        let mut sorted = tools.clone();
        sorted.sort();
        let mut want: Vec<String> = TOOLS.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(sorted, want);
        let identity = success(&peer.call("everett_whoami", json!({})));
        assert_eq!((identity["session_id"].clone(), identity["harness"].clone()), (json!("dot-one"), json!(harness)));
        let incoming = format!("message-{}", harness);
        c.inbox("agent-two", "dot-one", &incoming);
        assert_eq!(success(&peer.call("everett_inbox", json!({"peek": true})))["messages"][0]["id"], json!(incoming));
        assert_eq!(success(&peer.call("everett_inbox", json!({})))["messages"][0]["id"], json!(incoming));
        assert_eq!(success(&peer.call("everett_inbox", json!({})))["messages"], json!([]));
        let reply = success(&peer.call("everett_send", json!({"text": "Synthetic reply", "reply_to": incoming})));
        assert_eq!(reply["to"], "agent-two");
        let direct = success(&peer.call("everett_send", json!({"text": "Synthetic direct", "to": "agent-two"})));
        assert_eq!((direct["session"]["id"].clone(), direct["mode"].clone()), (json!("agent-two"), json!("inbox")));
        assert_eq!((direct["queued"].clone(), direct["hooked"].clone()), (json!(true), json!(false)));
        success(&peer.call("everett_card", json!({"what": "Synthetic", "state": "working", "next": "reply"})));
        assert!(std::fs::read_to_string(c.home.join(".everett/cards/dot-one.md")).unwrap().contains("Synthetic"));
        assert!(!c.home.join(".everett/cards/agent-two.md").exists());
        let core = success(&peer.call("everett_core", json!({})));
        assert_eq!(core.as_object().unwrap().keys().collect::<Vec<_>>(), ["core"]);
        drop(peer);
        for id in ["agent-two", "dot-one"] {
            assert!(c.cli(&["external", "remove", "--id", id]).status.success());
        }
    }
}

fn policy_rejects_foreign_identity_project_and_unsafe_arguments() {
    let c = ctx();
    let mut peer = c.peer(&fixture(&[]));
    let mut cases: Vec<(&str, Value)> = ["everett_ls", "everett_route", "everett_learn", "everett_event", "everett_subscribe"]
        .iter().map(|n| (*n, json!({}))).collect();
    cases.extend(["everett_inbox", "everett_card", "everett_send"].iter().map(|n| (*n, json!({"session_id": "other"}))));
    cases.extend([
        ("everett_core", json!({"project": "elsewhere"})), ("everett_inbox", json!({"peek": 1})),
        ("everett_send", json!({"text": "task"})), ("everett_send", json!({"text": "task", "to": "agent"})),
        ("everett_send", json!({"text": "task", "to": "dot-one"})), ("everett_send", json!({"text": "task", "to": "foreign"})),
        ("everett_send", json!({"text": "task", "to": "agent-two", "reply_to": "one"})),
        ("everett_card", json!({"what": "x", "state": " ", "next": "x"})),
        ("everett_send", json!({"text": "x".repeat(16001), "to": "agent-two"})),
    ]);
    for key in ["spawn", "dir", "harness", "mode", "wait", "timeout", "router"] {
        let mut a = json!({"text": "task", "to": "agent-two"});
        a[key] = json!(false);
        cases.push(("everett_send", a));
    }
    for (name, arguments) in cases {
        denied(&peer.call(name, arguments));
    }
    assert!(c.records().is_empty());
}

fn core_is_an_explicit_grant() {
    let c = ctx();
    let mut peer = c.peer(&with(fixture(&[]), json!({"allow_shared_core": false})));
    let listed = peer.request("tools/list", None).to_string();
    assert!(!listed.contains("everett_core"));
    denied(&peer.call("everett_core", json!({})));
    let mut peer = c.peer(&fixture(&[]));
    assert_eq!(success(&peer.call("everett_core", json!({}))), json!({"core": "Synthetic global + project everett"}));
}

fn reply_authorization_fails_closed() {
    let c = ctx();
    let mut peer = c.peer(&fixture(&[]));
    for (sender, to) in [("foreign", "dot-one"), ("agent-two", "other")] {
        c.inbox(sender, to, "message-one");
        denied(&peer.call("everett_send", json!({"text": "reply", "reply_to": "message-one"})));
    }
    let path = c.inbox("agent-two", "dot-one", "message-one");
    let once = std::fs::read_to_string(&path).unwrap();
    for raw in [once.repeat(2), "not json\n".to_string(), " ".repeat(8 * 1024 * 1024 + 1)] {
        std::fs::write(&path, raw).unwrap();
        denied(&peer.call("everett_send", json!({"text": "reply", "reply_to": "message-one"})));
    }
    assert!(c.records().is_empty());
}

fn legacy_or_malformed_exact_id_capability_blocks_direct_send_only() {
    let c = ctx();
    for options in [vec!["--legacy"], vec!["--capability", "true"], vec!["--capability", r#"{"enforced":"true"}"#],
        vec!["--capability", r#"{"enforced":false}"#]] {
        let mut peer = c.peer(&fixture(&options));
        denied(&peer.call("everett_send", json!({"text": "task", "to": "agent-two"})));
        c.inbox("agent-two", "dot-one", "message-one");
        let reply = peer.call("everett_send", json!({"text": "reply", "reply_to": "message-one"}));
        if options == ["--capability", "true"] {
            denied(&reply);
            continue;
        }
        assert_eq!(success(&reply)["to"], "agent-two", "{options:?}");
        assert!(success(&peer.call("everett_core", json!({}))).get("core").is_some());
    }
}

fn worker_identity_cwd_credentials_and_lifecycle() {
    let c = ctx();
    let mut peer = c.peer(&fixture(&[]));
    success(&peer.call("everett_whoami", json!({})));
    success(&peer.call("everett_whoami", json!({})));
    let mut other = c.peer(&with(fixture(&[]), json!({"agent_id": "grok-one", "harness": "grok-bot"})));
    assert_eq!(success(&other.call("everett_whoami", json!({})))["session_id"], "grok-one");
    let records = c.records();
    let pids: std::collections::HashSet<u64> = records.iter().map(|r| r["pid"].as_u64().unwrap()).collect();
    assert_eq!(pids.len(), 3);
    for record in &records {
        assert_eq!(record["has_secret"], false);
        assert_eq!(std::fs::canonicalize(record["cwd"].as_str().unwrap()).unwrap(), c.home);
        assert!(eventually_dead(record["pid"].as_u64().unwrap()));
    }
}

fn worker_timeout_and_concurrency_are_bounded_and_reap_children() {
    let c = ctx();
    let port = c.http_server(&with(fixture(&["--hang"]), json!({"max_workers": 1, "timeout": 1})));
    let message = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "everett_whoami", "arguments": {}}});
    let m = message.clone();
    let first = std::thread::spawn(move || post(port, &m, &[]));
    let deadline = Instant::now() + Duration::from_secs(3);
    while c.records().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!c.records().is_empty());
    let start = Instant::now();
    let busy = post(port, &message, &[]);
    assert_eq!(busy.status, 200);
    denied(&body_json(&busy));
    assert!(String::from_utf8_lossy(&busy.body).contains("busy"));
    assert!(start.elapsed() < Duration::from_millis(800));
    let first = first.join().unwrap();
    assert_eq!(first.status, 200);
    denied(&body_json(&first));
    for record in c.records() {
        assert!(eventually_dead(record["pid"].as_u64().unwrap()));
    }
}

fn configuration_rejects_bad_types_reserved_ids_paths_and_unknown_fields() {
    let c = ctx();
    for fields in [json!({"agent_id": "../dot"}), json!({"agent_id": "HUMAN"}), json!({"agent_id": "LIVE"}),
        json!({"harness": "grok"}), json!({"project": "../other"}), json!({"project": ""}), json!({"cwd": "relative"}),
        json!({"max_workers": 0}), json!({"max_workers": 9}), json!({"max_workers": true}), json!({"timeout": 31}),
        json!({"timeout": true}), json!({"allow_shared_core": "yes"}), json!({"extra": true}), json!({"backend_command": []}),
        json!({"backend_command": ["everett", "mcp"]}), json!({"allowed_hosts": []}), json!({"allowed_hosts": ["*.example.com"]}),
        json!({"allowed_hosts": ["example.com:99999"]}), json!({"allowed_origins": ["https://example.com/path"]})] {
        assert_eq!(c.gateway_code(&c.config_path(&fields)), Some(2), "{fields}");
    }
    for key in ["allow_shared_core", "agent_id", "harness", "cwd", "project", "destinations"] {
        let mut config = c.config.clone();
        config.as_object_mut().unwrap().remove(key);
        assert_eq!(c.gateway_code(&c.write_config(&config)), Some(2), "{key}");
    }
    let path = c.config_path(&json!({}));
    std::fs::write(&path, "x".repeat(16385)).unwrap();
    assert_eq!(c.gateway_code(&path), Some(2));
}

fn http_requires_a_strong_token_without_logging_it() {
    let c = ctx();
    let long = "x".repeat(513);
    let spaced = format!("{} ", "x".repeat(32));
    let accented = "é".repeat(32);
    for token in [None, Some(""), Some("short"), Some(long.as_str()), Some(spaced.as_str()), Some(accented.as_str())] {
        let path = c.config_path(&json!({}));
        let mut command = c.command(&["gateway", "--config", path.to_str().unwrap(), "--transport", "http"]);
        match token {
            None => { command.env_remove("EVERETT_GATEWAY_TOKEN"); }
            Some(t) => { command.env("EVERETT_GATEWAY_TOKEN", t); }
        }
        let out = command.output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{token:?}");
        if let Some(t) = token.filter(|t| t.len() >= 32) {
            assert!(!String::from_utf8_lossy(&out.stderr).contains(t));
        }
    }
}

fn http_sdk_roundtrip_is_stateless_and_native() {
    let c = ctx();
    let port = c.http_server(&json!({}));
    for message in [json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize()}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "everett_whoami", "arguments": {}}})] {
        let reply = post(port, &message, &[]);
        assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.body));
        assert!(!reply.headers.iter().any(|(k, _)| k == "mcp-session-id"));
        let response = body_json(&reply);
        assert!(response.get("error").is_none(), "{response}");
        if message["method"] == "tools/call" {
            assert_eq!(success(&response)["session_id"], "dot-one");
        }
    }
    assert!([405, 400].contains(&http(port, "GET", "", &[]).status));
}

fn http_rejects_auth_host_origin_oversized_and_non_json_requests() {
    let c = ctx();
    let port = c.http_server(&json!({}));
    let message = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize()});
    for (header, value, expected) in [("Authorization", None, 401), ("Authorization", Some("Bearer wrong"), 401),
        ("Host", Some("evil.example"), 421), ("Origin", Some("https://evil.example"), 403), ("Content-Type", Some("text/plain"), 415)] {
        assert_eq!(post(port, &message, &[(header, value)]).status, expected, "{header}");
    }
    assert_eq!(http(port, "POST", &"x".repeat(65537), &[]).status, 413);
    assert_eq!(post(port, &message, &[("Content-Type", Some("application/json; charset=utf-8"))]).status, 200);
}

fn exact_public_host_and_allowed_origin_work() {
    let c = ctx();
    let port = c.http_server(&json!({"allowed_hosts": ["gateway.example:8443"], "allowed_origins": ["https://client.example"]}));
    let message = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize()});
    let host = ("Host", Some("gateway.example:8443"));
    assert_eq!(post(port, &message, &[host, ("Origin", Some("https://client.example"))]).status, 200);
    assert_eq!(post(port, &message, &[host, ("Origin", Some("https://client.example/"))]).status, 403);
}

fn duplicate_security_headers_fail_closed() {
    let c = ctx();
    let port = c.http_server(&json!({}));
    let auth = format!("Bearer {}", TOKEN);
    for (header, value) in [("Host", "127.0.0.1:8788"), ("Origin", "https://evil.example"), ("Authorization", auth.as_str()),
        ("Content-Length", "2"), ("Transfer-Encoding", "chunked")] {
        let mut raw = format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nAuthorization: {auth}\r\nContent-Type: application/json\r\n\
            Content-Length: 2\r\n{header}: {value}\r\nConnection: close\r\n\r\n{{}}");
        if header == "Origin" {
            raw = raw.replace("Connection: close", "Origin: https://evil.example\r\nConnection: close");
        }
        if header == "Transfer-Encoding" {
            raw = raw.replace("Content-Length: 2", "Transfer-Encoding: chunked");
        }
        let reply = String::from_utf8_lossy(&raw_exchange(port, raw.as_bytes())).to_string();
        assert!(reply.contains(" 400 "), "{header}: {reply}");
    }
}

fn http_body_read_timeout_and_request_capacity() {
    let c = ctx();
    let port = c.http_server(&json!({"max_workers": 1}));
    let partial = format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nAuthorization: Bearer {TOKEN}\r\n\
        Content-Type: application/json\r\nContent-Length: 10\r\nConnection: close\r\n\r\nx");
    let clients: Vec<_> = (0..2).map(|_| {
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
        client.write_all(partial.as_bytes()).unwrap();
        client
    }).collect();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut status = 200;
    while status != 503 && Instant::now() < deadline {
        status = post(port, &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}), &[]).status;
    }
    assert_eq!(status, 503);
    for mut client in clients {
        let mut buf = [0u8; 8192];
        let n = client.read(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains(" 504 "));
    }
}

fn oversized_stdio_frame_closes_without_dispatching() {
    let c = ctx();
    let mut peer = c.peer(&fixture(&[]));
    let stdin = peer.stdin.as_mut().unwrap();
    let _ = stdin.write_all(&[b'x'; 65537]);
    let _ = stdin.write_all(b"\n");
    let _ = stdin.flush();
    let deadline = Instant::now() + Duration::from_secs(5);
    while peer.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "gateway kept serving after an oversized frame");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(c.records().is_empty());
}

fn backend_results_are_structured_bounded_and_identity_checked() {
    let c = ctx();
    for mode in ["text-only", "oversized", "amplified", "non-object", "wrong-identity", "wrong-harness"] {
        let mut peer = c.peer(&with(fixture(&["--result-mode", mode]), json!({"timeout": 1})));
        let tool = if mode.starts_with("wrong-") { "everett_whoami" } else { "everett_core" };
        let response = peer.call(tool, json!({}));
        denied(&response);
        assert!(response.to_string().len() < 1024, "{mode}");
    }
}

fn native_backend_never_resolves_destination_prefix_or_title() {
    let c = ctx();
    assert!(c.cli(&["external", "add", "--id", "agent-two-suffix", "--harness", "grok-bot", "--title", "agent-two"]).status.success());
    let mut peer = c.peer(&json!({}));
    denied(&peer.call("everett_send", json!({"text": "Must not route", "to": "agent-two"})));
    assert!(!c.home.join(".everett/inbox/agent-two-suffix.jsonl").exists());
}

fn symlinked_working_directory_is_canonicalized() {
    let c = ctx();
    let link = c.home.join("linked-project");
    std::os::unix::fs::symlink(&c.home, &link).unwrap();
    let mut peer = c.peer(&with(fixture(&[]), json!({"cwd": link})));
    success(&peer.call("everett_whoami", json!({})));
    assert_eq!(c.records()[0]["cwd"], json!(c.home));
}

type Case = (&'static str, fn());

const CASES: &[Case] = &[
    ("native_stdio_journey_for_both_external_harnesses", native_stdio_journey_for_both_external_harnesses),
    ("policy_rejects_foreign_identity_project_and_unsafe_arguments", policy_rejects_foreign_identity_project_and_unsafe_arguments),
    ("core_is_an_explicit_grant", core_is_an_explicit_grant),
    ("reply_authorization_fails_closed", reply_authorization_fails_closed),
    ("legacy_or_malformed_exact_id_capability_blocks_direct_send_only", legacy_or_malformed_exact_id_capability_blocks_direct_send_only),
    ("worker_identity_cwd_credentials_and_lifecycle", worker_identity_cwd_credentials_and_lifecycle),
    ("worker_timeout_and_concurrency_are_bounded_and_reap_children", worker_timeout_and_concurrency_are_bounded_and_reap_children),
    ("configuration_rejects_bad_types_reserved_ids_paths_and_unknown_fields", configuration_rejects_bad_types_reserved_ids_paths_and_unknown_fields),
    ("http_requires_a_strong_token_without_logging_it", http_requires_a_strong_token_without_logging_it),
    ("http_sdk_roundtrip_is_stateless_and_native", http_sdk_roundtrip_is_stateless_and_native),
    ("http_rejects_auth_host_origin_oversized_and_non_json_requests", http_rejects_auth_host_origin_oversized_and_non_json_requests),
    ("exact_public_host_and_allowed_origin_work", exact_public_host_and_allowed_origin_work),
    ("duplicate_security_headers_fail_closed", duplicate_security_headers_fail_closed),
    ("http_body_read_timeout_and_request_capacity", http_body_read_timeout_and_request_capacity),
    ("oversized_stdio_frame_closes_without_dispatching", oversized_stdio_frame_closes_without_dispatching),
    ("backend_results_are_structured_bounded_and_identity_checked", backend_results_are_structured_bounded_and_identity_checked),
    ("native_backend_never_resolves_destination_prefix_or_title", native_backend_never_resolves_destination_prefix_or_title),
    ("symlinked_working_directory_is_canonicalized", symlinked_working_directory_is_canonicalized),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--fixture-backend") {
        backend(&args[2..]);
        return;
    }
    let filters: Vec<&String> = args[1..].iter().filter(|a| !a.starts_with('-')).collect();
    let selected: Vec<&Case> = CASES.iter().filter(|(n, _)| filters.is_empty() || filters.iter().any(|f| n.contains(f.as_str()))).collect();
    println!("\nrunning {} tests", selected.len());
    let mut failed = Vec::new();
    for (name, case) in selected.iter() {
        let ok = std::panic::catch_unwind(*case).is_ok();
        println!("test {} ... {}", name, if ok { "ok" } else { "FAILED" });
        if !ok {
            failed.push(*name);
        }
    }
    println!("\ntest result: {}. {} passed; {} failed", if failed.is_empty() { "ok" } else { "FAILED" },
        selected.len() - failed.len(), failed.len());
    if !failed.is_empty() {
        std::process::exit(1);
    }
}

