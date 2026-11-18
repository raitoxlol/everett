//! Native gateway journeys over stdio and HTTP. The test binary doubles as the synthetic
//! backend (`--fixture-backend`), so no model, harness, or other runtime is involved.
#[path = "gateway_native/events.rs"]
mod events_acceptance;
#[path = "gateway_native/identity_diagnostics.rs"]
mod identity_diagnostics;
#[path = "gateway_native/whoami_provenance.rs"]
mod whoami_provenance;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_everett");
const TOKEN: &str = "synthetic-gateway-test-token-not-a-real-secret";
const INTROSPECTION_SECRET: &str = "synthetic-introspection-secret-not-real-000";
const RESOURCE: &str = "https://dot.example.com/mcp";
const TOOLS: [&str; 6] = ["everett_identity", "everett_core", "everett_whoami", "everett_inbox", "everett_card", "everett_send"];

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
                let has_secret = ["EVERETT_GATEWAY_TOKEN", "EVERETT_OAUTH_INTROSPECTION_SECRET", "OPENAI_API_KEY", "TYPESAFE_API_KEY"].iter().any(|k| std::env::var_os(k).is_some());
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
        let binary = std::env::var_os("EVERETT_VERIFY_BINARY").unwrap_or_else(|| BIN.into());
        let mut command = Command::new(binary);
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
        self.spawn_http(self.command(&["gateway", "--config", path.to_str().unwrap(), "--transport", "http", "--port", "0"]))
    }

    fn oauth_command(&self, changes: &Value) -> Command {
        let path = self.config_path(changes);
        let mut command = self.command(&["gateway", "--config", path.to_str().unwrap(), "--transport", "http", "--port", "0"]);
        command.env_remove("EVERETT_GATEWAY_TOKEN").env("EVERETT_OAUTH_INTROSPECTION_SECRET", INTROSPECTION_SECRET);
        command
    }

    fn oauth_server(&self, issuer: &str) -> u16 {
        self.spawn_http(self.oauth_command(&json!({"oauth": {"issuer": issuer, "resource": RESOURCE}})))
    }

    fn with_backend_oauth_server(&self, issuer: &str) -> u16 {
        self.spawn_http(self.oauth_command(&with(fixture(&[]), json!({"oauth": {"issuer": issuer, "resource": RESOURCE}}))))
    }

    fn spawn_http(&self, mut command: Command) -> u16 {
        let mut child = command.stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
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
    http_at(port, method, "/mcp", body, overrides)
}

fn http_at(port: u16, method: &str, path: &str, body: &str, overrides: &[(&str, Option<&str>)]) -> Reply {
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
    let mut request = format!("{} {} HTTP/1.1\r\n", method, path);
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

/// Synthetic RFC 7662 introspection endpoint; tokens map to the response it returns.
struct FakeIssuer {
    url: String,
    tokens: Arc<Mutex<HashMap<String, Value>>>,
    calls: Arc<Mutex<u32>>,
}

fn fake_issuer() -> FakeIssuer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let tokens: Arc<Mutex<HashMap<String, Value>>> = Arc::default();
    let calls: Arc<Mutex<u32>> = Arc::default();
    let (map, count) = (tokens.clone(), calls.clone());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().map_while(Result::ok) {
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            let body_start = loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 { break None; }
                raw.extend_from_slice(&buf[..n]);
                if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") { break Some(i + 4); }
            };
            let Some(start) = body_start else { continue };
            let head = String::from_utf8_lossy(&raw[..start]).to_lowercase();
            let length: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).map_or(0, |v| v.trim().parse().unwrap());
            while raw.len() < start + length {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 { break; }
                raw.extend_from_slice(&buf[..n]);
            }
            *count.lock().unwrap() += 1;
            let body = String::from_utf8_lossy(&raw[start..]).to_string();
            let token = body.split('&').find_map(|kv| kv.strip_prefix("token=")).unwrap_or_default().to_string();
            let authorized = head.contains(&format!("authorization: bearer {}", INTROSPECTION_SECRET.to_lowercase()));
            let (status, reply) = if authorized {
                {
                    let value = map.lock().unwrap().get(&token).cloned().unwrap_or(json!({"active":false}));
                    (if value["fixture_unavailable"] == true { "503 Unavailable" } else { "200 OK" }, value)
                }
            } else {
                ("401 Unauthorized", json!({"error": "invalid_client"}))
            };
            let reply = reply.to_string();
            let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
        }
    });
    FakeIssuer { url, tokens, calls }
}

impl FakeIssuer {
    fn grant(&self, token: &str, agent: &str, changes: Value) {
        let exp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 600;
        let claims = merged(&json!({"active": true, "iss": self.url, "aud": RESOURCE, "exp": exp, "scope": "everett", "sub": agent,
            "everett_agent": agent, "everett_agent_name": "ChatGPT dot", "client_id": "https://chatgpt.com/oauth/client.json"}), &changes);
        self.tokens.lock().unwrap().insert(token.to_string(), claims);
    }
}

fn access(c: char) -> String {
    format!("eva_{}", c.to_string().repeat(64))
}

// A process fixture for the real T3 CLI's stdin and receipt contract, never a model.
fn native_cli_fixture(args: &[String]) {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).unwrap();
    let mut calls = std::fs::OpenOptions::new().create(true).append(true).open(home.join("native-start-calls.jsonl")).unwrap();
    let has_secret=["EVERETT_GATEWAY_TOKEN","EVERETT_OAUTH_INTROSPECTION_SECRET","OPENAI_API_KEY","TYPESAFE_API_KEY"].iter().any(|k|std::env::var_os(k).is_some());
    writeln!(calls,"{}",json!({"pid":std::process::id(),"args":args,"text":text,"has_secret":has_secret})).unwrap();
    let mode = std::fs::read_to_string(home.join("native-start-mode")).unwrap_or_default();
    match mode.as_str() {
        "hang" => std::thread::sleep(Duration::from_secs(30)),
        "slow" => std::thread::sleep(Duration::from_millis(300)),
        "rejected" => { println!("{{\"accepted\":false}}"); return; },
        "oversized" => { println!("{}", "x".repeat(300 * 1024)); return; },
        "empty" => { println!("{{}}"); return; },
        "array" => { println!("[]"); return; },
        "exit" => std::process::exit(7),
        "invalid-id" => { println!("{}",json!({"threadId":"mcp:bad\nthread","projectId":"fixed-project","runId":"run:1"})); return; },
        "long-id" => { println!("{}",json!({"threadId":"x".repeat(513),"projectId":"fixed-project","runId":"run:1"})); return; },
        "no-run" => { println!("{}",json!({"threadId":"native-thread-1","projectId":"fixed-project","runId":null})); return; },
        _ => {},
    }
    let project = if mode == "wrong-project" { "foreign-project" } else { "fixed-project" };
    println!("{}",json!({"threadId":"mcp:fixture-thread","projectId":project,"runId":"run:thread:mcp%3Afixture-thread:ordinal:1"}));
}

fn native_config(issuer: &str) -> Value {
    json!({"oauth":{"issuer":issuer,"resource":RESOURCE}, "native_start":{
        "owner_account":"github:alice","client_id":"https://chatgpt.com/oauth/client.json",
        "command":std::env::current_exe().unwrap(),"project_id":"fixed-project","provider":"fixed-provider","model":"fixed-model"}})
}

fn oauth_rpc(port: u16, token: char, method: &str, params: Value) -> Value {
    let auth = bearer(&access(token));
    body_json(&post(port,&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),&[("Authorization",Some(&auth))]))
}

fn native_call(port: u16, token: char, name: &str, args: Value) -> Value {
    oauth_rpc(port,token,"tools/call",json!({"name":name,"arguments":args}))
}

fn native_identity(port: u16, key: &str) -> Value {
    success(&native_call(port,'a',"everett_identity",json!({"action":"create","registration_key":key,"name":"Disposable fixture chat","kind":"chat"})))
}

fn native_records(c: &Ctx) -> Vec<Value> {
    // The fixture appends while the parent polls. A visible final fragment is
    // not a completed record until its newline has been written.
    let text = std::fs::read_to_string(c.home.join("native-start-calls.jsonl")).unwrap_or_default();
    text.split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .map(|line| serde_json::from_str(line).expect("completed fixture record must be valid JSON"))
        .collect()
}

fn native_outcome(response: &Value) -> Value {
    assert!(response.get("error").is_none(),"{response}");
    let data=response["result"]["structuredContent"].clone();
    assert!(data.is_object(),"{response}");
    assert_eq!(response["result"]["isError"]==true,data["dispatch_confirmed"]!=true,"{response}");
    data
}

fn owner_native_start_is_explicit_isolated_and_durable() {
    let c = ctx();
    let issuer = fake_issuer();
    issuer.grant(&access('a'),"agent-old",json!({"everett_account":"github:alice"}));
    issuer.grant(&access('b'),"agent-new",json!({"everett_account":"github:alice"}));
    issuer.grant(&access('c'),"agent-bob",json!({"everett_account":"github:bob"}));
    issuer.grant(&access('d'),"agent-other-client",json!({"everett_account":"github:alice","client_id":"other-client"}));
    issuer.grant(&access('e'),"agent-no-account",json!({}));
    let cfg = native_config(&issuer.url);
    let port = c.spawn_http(c.oauth_command(&cfg));
    for token in ['a','c','d','e'] {
        let listed = oauth_rpc(port,token,"tools/list",json!({}));
        let present = listed["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"]=="everett_start");
        assert_eq!(present,token=='a',"{listed}");
    }
    let identity = native_identity(port,"native-start-registration-key-12345");
    let args = json!({"identity_handle":identity["identity_handle"],"request_id":"native-start-request-id-1234567890","text":"Synthetic request --model never-a-flag"});
    let info=success(&native_call(port,'a',"everett_whoami",json!({"identity_handle":identity["identity_handle"]})));
    assert_eq!(info["capabilities"]["native_session_start"],true);
    assert_eq!(info["gateway_resource"],RESOURCE);
    assert_eq!(info["gateway_runtime"]["version"],env!("CARGO_PKG_VERSION"));
    for token in ['c','d','e'] { denied(&native_call(port,token,"everett_start",args.clone())); }
    denied(&native_call(port,'a',"everett_start",json!({"request_id":args["request_id"],"text":"without handle"})));
    for (key,value) in [("cwd",json!("/tmp")),("project",json!("foreign")),("provider",json!("other")),("model",json!("other")),("wait",json!(true)),("spawn",json!(true))] {
        let mut bad=args.clone();bad[key]=value;denied(&native_call(port,'a',"everett_start",bad));
    }
    assert!(native_records(&c).is_empty());
    let receipt = success(&native_call(port,'a',"everett_start",args.clone()));
    assert_eq!(receipt["state"],"dispatch_confirmed");
    assert_eq!(receipt["thread_id"],"mcp:fixture-thread");
    assert_eq!(receipt["run_id"],"run:thread:mcp%3Afixture-thread:ordinal:1");
    for key in ["pickup_confirmed","completion_confirmed","reply_confirmed"] { assert_eq!(receipt[key],false); }
    assert!(receipt["native_session_id"].is_null());
    assert_eq!(success(&native_call(port,'a',"everett_start",args.clone())),receipt);
    let mut changed=args.clone();changed["text"]=json!("changed");
    denied(&native_call(port,'a',"everett_start",changed));
    let restarted = c.spawn_http(c.oauth_command(&cfg));
    assert_eq!(success(&native_call(restarted,'b',"everett_start",args.clone())),receipt);
    let mut changed_policy=cfg.clone();changed_policy["native_start"]["model"]=json!("another-owner-model");
    let changed_server=c.spawn_http(c.oauth_command(&changed_policy));
    denied(&native_call(changed_server,'b',"everett_start",args.clone()));
    let disabled_server=c.spawn_http(c.oauth_command(&json!({"oauth":{"issuer":issuer.url,"resource":RESOURCE}})));
    denied(&native_call(disabled_server,'b',"everett_start",args.clone()));
    assert_eq!(success(&native_call(disabled_server,'b',"everett_whoami",json!({"identity_handle":identity["identity_handle"]})))["capabilities"]["native_session_start"],false);
    let records=native_records(&c);
    assert_eq!(records.len(),1);
    assert_eq!(records[0]["text"],args["text"]);
    assert_eq!(records[0]["has_secret"],false);
    let journal=std::fs::read(c.home.join(".everett/gateway-starts.sqlite")).unwrap();
    for private in [args["text"].as_str().unwrap(),identity["identity_handle"].as_str().unwrap(),INTROSPECTION_SECRET] {
        assert!(!journal.windows(private.len()).any(|w|w==private.as_bytes()),"start journal must not contain prompt, handle or token");
    }
    assert_eq!(records[0]["args"],json!(["thread","start","--json","--title","Everett owner request","--provider","fixed-provider","--model","fixed-model","fixed-project","-"]));
    let second=native_identity(port,"native-start-second-chat-key-123456");
    let mut own=args.clone();own["identity_handle"]=second["identity_handle"].clone();
    success(&native_call(port,'a',"everett_start",own));
    assert_eq!(native_records(&c).len(),2,"chat retry scopes must be separate");
    success(&native_call(port,'a',"everett_identity",json!({"action":"revoke","identity_handle":identity["identity_handle"]})));
    denied(&native_call(port,'a',"everett_start",args));
    assert_eq!(native_records(&c).len(),2);
    denied(&native_call(port,'a',"everett_control",json!({"action":"send","thread":receipt["thread_id"],"text":"no"})));
    denied(&native_call(port,'a',"everett_send",json!({"to":receipt["thread_id"],"text":"no automatic destination grant"})));
    issuer.grant(&access('b'),"agent-new",json!({"active":false}));
    let forbidden=post(port,&json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"everett_start","arguments":{}}}),&[("Authorization",Some(&bearer(&access('b'))))]);
    assert_eq!(forbidden.status,401,"revoked OAuth must deny even cached starts");
}

fn native_start_policy_and_journal_fail_closed() {
    use std::os::unix::fs::{symlink,PermissionsExt};
    let c=ctx();let issuer=fake_issuer();
    issuer.grant(&access('a'),"agent-one",json!({"everett_account":"github:alice"}));
    let cfg=native_config(&issuer.url);
    for (key,value) in [("owner_account",json!("")),("client_id",json!("")),("command",json!("relative-t3")),
        ("project_id",json!("--injected-flag")),("model",json!("")),("provider",json!("")),("unexpected",json!(true))] {
        let mut bad=cfg.clone();bad["native_start"][key]=value;
        let path=c.config_path(&bad);assert_eq!(c.gateway_code(&path),Some(2));
    }
    let mut no_oauth=cfg.clone();no_oauth.as_object_mut().unwrap().remove("oauth");
    assert_eq!(c.gateway_code(&c.config_path(&no_oauth)),Some(2));
    let default=c.with_backend_oauth_server(&issuer.url);
    let identity=native_identity(default,"native-default-denied-registration123");
    let args=json!({"identity_handle":identity["identity_handle"],"request_id":"native-denied-request-id-123456789","text":"Synthetic test"});
    denied(&native_call(default,'a',"everett_start",args.clone()));
    let stdio=c.peer(&cfg).request("tools/list",None);
    assert!(!stdio["result"]["tools"].as_array().unwrap().iter().any(|t|t["name"]=="everett_start"));
    let port=c.spawn_http(c.oauth_command(&cfg));
    let journal=c.home.join(".everett/gateway-starts.sqlite");
    let other=c.home.join("untouched");std::fs::write(&other,"unchanged").unwrap();
    symlink(&other,&journal).unwrap();
    denied(&native_call(port,'a',"everett_start",args.clone()));
    assert_eq!(std::fs::read_to_string(&other).unwrap(),"unchanged");
    std::fs::remove_file(&journal).unwrap();
    std::fs::write(&journal,"").unwrap();std::fs::set_permissions(&journal,std::fs::Permissions::from_mode(0o644)).unwrap();
    denied(&native_call(port,'a',"everett_start",args.clone()));
    assert!(native_records(&c).is_empty());
    std::fs::set_permissions(&journal,std::fs::Permissions::from_mode(0o600)).unwrap();
    success(&native_call(port,'a',"everett_start",args.clone()));
    let con=rusqlite::Connection::open(&journal).unwrap();
    for corrupted in [json!({"dispatch_confirmed":true}),json!([]),json!({"accepted":false})] {
        con.execute("UPDATE starts SET result=?1",[corrupted.to_string()]).unwrap();
        denied(&native_call(port,'a',"everett_start",args.clone()));
        assert_eq!(native_records(&c).len(),1,"corrupt receipts must never claim dispatch or launch again");
    }
}

fn native_start_uncertain_receipts_never_dispatch_twice() {
    for mode in ["rejected","empty","array","wrong-project","oversized","hang","exit","no-run","invalid-id","long-id"] {
        let c=ctx();let issuer=fake_issuer();
        issuer.grant(&access('a'),"agent-one",json!({"everett_account":"github:alice"}));
        let cfg=with(native_config(&issuer.url),json!({"timeout":1}));
        let port=c.spawn_http(c.oauth_command(&cfg));
        let identity=native_identity(port,"uncertain-native-registration-key-123");
        std::fs::write(c.home.join("native-start-mode"),mode).unwrap();
        let args=json!({"identity_handle":identity["identity_handle"],"request_id":"uncertain-native-request-key-12345","text":"Synthetic uncertain dispatch"});
        let result=native_outcome(&native_call(port,'a',"everett_start",args.clone()));
        assert_eq!(result["state"],"dispatch_unconfirmed","{mode}: {result}");
        assert_eq!(result["dispatch_confirmed"],false);
        let code=match mode { "hang"=>"cli_timeout", "oversized"=>"cli_output_too_large", "wrong-project"|"no-run"|"invalid-id"|"long-id"=>"destination_mismatch_or_missing_run", "exit"=>"cli_exit",_=>"invalid_receipt" };
        assert_eq!(result["diagnostic"],code,"{mode}: {result}");
        std::fs::remove_file(c.home.join("native-start-mode")).unwrap();
        let restarted=c.spawn_http(c.oauth_command(&cfg));
        assert_eq!(native_outcome(&native_call(restarted,'a',"everett_start",args)),result);
        assert_eq!(native_records(&c).len(),1,"{mode} must not be retried automatically");
    }
}

fn concurrent_native_start_and_gateway_crash_keep_reservation() {
    let c=ctx();let issuer=fake_issuer();
    issuer.grant(&access('a'),"agent-one",json!({"everett_account":"github:alice"}));
    let cfg=native_config(&issuer.url);
    let port=c.spawn_http(c.oauth_command(&cfg));
    let identity=native_identity(port,"concurrent-native-registration-key123");
    std::fs::write(c.home.join("native-start-mode"),"slow").unwrap();
    let args=json!({"identity_handle":identity["identity_handle"],"request_id":"concurrent-native-request-key-12345","text":"Synthetic concurrent dispatch"});
    let a=args.clone();let b=args.clone();
    let first=std::thread::spawn(move ||native_call(port,'a',"everett_start",a));
    let second=std::thread::spawn(move ||native_call(port,'a',"everett_start",b));
    let results=[native_outcome(&first.join().unwrap()),native_outcome(&second.join().unwrap())];
    assert!(results.iter().any(|r|r["dispatch_confirmed"]==true));
    assert_eq!(native_records(&c).len(),1);
    // An interrupted gateway leaves a committed uncertainty record before the CLI answers.
    std::fs::write(c.home.join("native-start-mode"),"hang").unwrap();
    let mut crash=args;crash["request_id"]=json!("interrupted-native-request-key-12345");
    let retry=crash.clone();
    let pending=std::thread::spawn(move || {
        let body=json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"everett_start","arguments":crash}}).to_string();
        raw_exchange(port,format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nAuthorization: {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2025-06-18\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",bearer(&access('a')),body.len(),body).as_bytes())
    });
    let started=Instant::now();
    while native_records(&c).len()<2 { assert!(started.elapsed()<Duration::from_secs(3));std::thread::sleep(Duration::from_millis(10)); }
    c.servers.borrow_mut()[0].kill().unwrap();
    c.servers.borrow_mut()[0].wait().unwrap();
    // Hard-killed parents cannot run Drop. Clean up only this fixture CLI's group.
    let fixture_pid=native_records(&c)[1]["pid"].as_i64().unwrap() as i32;
    unsafe { libc::kill(-fixture_pid,libc::SIGKILL); }
    let _=pending.join();
    std::fs::remove_file(c.home.join("native-start-mode")).unwrap();
    let restarted=c.spawn_http(c.oauth_command(&cfg));
    let recovered=native_outcome(&native_call(restarted,'a',"everett_start",retry));
    assert_eq!(recovered["state"],"dispatch_unconfirmed");
    assert_eq!(recovered["diagnostic"],"interrupted_or_pending");
    assert_eq!(native_records(&c).len(),2);
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

fn header<'a>(reply: &'a Reply, name: &str) -> Option<&'a str> {
    reply.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn whoami() -> Value {
    json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "everett_whoami", "arguments": {}}})
}

fn oauth_serves_protected_resource_metadata_and_challenges() {
    let c = ctx();
    let issuer = fake_issuer();
    let port = c.oauth_server(&issuer.url);
    for path in ["/.well-known/oauth-protected-resource", "/.well-known/oauth-protected-resource/mcp"] {
        let reply = http_at(port, "GET", path, "", &[("Authorization", None)]);
        assert_eq!(reply.status, 200, "{path}");
        let metadata: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(metadata["resource"], RESOURCE);
        assert_eq!(metadata["authorization_servers"], json!([issuer.url]));
        assert_eq!(metadata["scopes_supported"], json!(["everett"]));
    }
    assert_eq!(http_at(port, "GET", "/.well-known/oauth-protected-resource", "", &[("Host", Some("evil.example"))]).status, 421);
    let message = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize()});
    let missing = post(port, &message, &[("Authorization", None)]);
    assert_eq!(missing.status, 401);
    let challenge = header(&missing, "www-authenticate").unwrap();
    assert!(challenge.starts_with("Bearer resource_metadata=\"https://dot.example.com/.well-known/oauth-protected-resource/mcp\""), "{challenge}");
    assert!(challenge.contains("scope=\"everett\"") && !challenge.contains("invalid_token"), "{challenge}");
    let static_bearer = post(port, &message, &[]);
    assert_eq!(static_bearer.status, 401);
    assert!(header(&static_bearer, "www-authenticate").unwrap().contains("error=\"invalid_token\""));
    assert_eq!(*issuer.calls.lock().unwrap(), 0, "malformed tokens never reach the issuer");
}

fn oauth_owner_account_restricts_every_request_before_backend_dispatch() {
    let c = ctx();
    let issuer = fake_issuer();
    let config = with(fixture(&[]), json!({"oauth": {
        "issuer":issuer.url, "resource":RESOURCE, "owner_account":"github:alice"
    }}));
    for owner in [Value::Null, json!(""), json!("github:alice bob"), json!("a".repeat(257)), json!(false)] {
        let mut invalid = config.clone();
        invalid["oauth"]["owner_account"] = owner;
        assert_eq!(c.gateway_code(&c.config_path(&invalid)), Some(2));
    }
    let port = c.spawn_http(c.oauth_command(&config));
    let auth = bearer(&access('a'));
    let auth = [("Authorization", Some(auth.as_str()))];
    issuer.grant(&access('a'), "agent-aaaa", json!({"everett_account":"github:alice"}));
    assert_eq!(success(&body_json(&post(port, &whoami(), &auth)))["session_id"], "agent-aaaa");
    assert_eq!(c.records().len(), 1);
    for account in [json!("github:bob"), Value::Null, json!(["github:alice"]), json!("github:alice ")] {
        issuer.grant(&access('a'), "agent-aaaa", json!({"everett_account":account}));
        for message in [whoami(), json!({"jsonrpc":"2.0","id":7,"method":"tools/list"})] {
            assert_eq!(post(port, &message, &auth).status, 401);
        }
        assert_eq!(c.records().len(), 1, "denied account must never reach a worker");
    }
    issuer.grant(&access('a'), "agent-aaaa", json!({"everett_account":"github:alice"}));
    assert_eq!(post(port, &whoami(), &auth).status, 200);
    assert_eq!(c.records().len(), 2);
}

fn oauth_token_maps_to_its_dot_identity() {
    let c = ctx();
    let issuer = fake_issuer();
    issuer.grant(&access('a'), "agent-aaaa", json!({}));
    issuer.grant(&access('b'), "agent-bbbb", json!({"aud": [RESOURCE, "https://other.example/mcp"]}));
    let port = c.with_backend_oauth_server(&issuer.url);
    for (token, agent) in [(access('a'), "agent-aaaa"), (access('b'), "agent-bbbb")] {
        let auth = bearer(&token);
        let auth = [("Authorization", Some(auth.as_str()))];
        let list = body_json(&post(port, &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}), &auth));
        let tools = list["result"]["tools"].as_array().unwrap();
        let card = tools.iter().find(|t| t["name"] == "everett_card").unwrap();
        assert_eq!(card["inputSchema"]["properties"]["session_id"]["type"], "string", "{card}");
        let data = success(&body_json(&post(port, &whoami(), &auth)));
        assert_eq!(data["session_id"], agent);
        assert_eq!(data["gateway_auth"], json!({"mode": "oauth", "agent_name": "ChatGPT dot", "client_id": "https://chatgpt.com/oauth/client.json"}));
        let foreign = json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "everett_card", "arguments": {"session_id": "dot-one", "summary": "x"}}});
        denied(&body_json(&post(port, &foreign, &auth)));
    }
    let agents: Vec<Value> = c.records().iter().map(|r| r["agent_id"].clone()).collect();
    assert_eq!(agents, vec![json!("agent-aaaa"), json!("agent-bbbb")]);
    assert!(c.records().iter().all(|r| r["has_secret"] == false), "introspection secret must not reach workers");
}

fn oauth_chat_identity_survives_reapproval_but_rejects_another_account() {
    let c = ctx();
    let issuer = fake_issuer();
    issuer.grant(&access('a'), "agent-old", json!({"everett_account":"github:alice"}));
    issuer.grant(&access('b'), "agent-new", json!({"everett_account":"github:alice"}));
    issuer.grant(&access('c'), "agent-bob", json!({"everett_account":"github:bob"}));
    issuer.grant(&access('d'), "agent-other-client", json!({"everett_account":"github:alice","client_id":"https://other.example/client"}));
    let port = c.with_backend_oauth_server(&issuer.url);
    let call = |token: char, name: &str, args: Value| {
        let authorization = bearer(&access(token));
        body_json(&post(port, &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}}), &[("Authorization", Some(authorization.as_str()))]))
    };
    let create = json!({"action":"create","registration_key":"private-key-for-chat-registration-123","name":"Alice chat","kind":"chat"});
    let identity = success(&call('a',"everett_identity",create.clone()));
    assert_eq!(success(&call('b',"everett_identity",create.clone())),identity);
    let own = json!({"identity_handle":identity["identity_handle"]});
    assert_eq!(success(&call('b',"everett_whoami",own.clone()))["session_id"],identity["session_id"]);
    denied(&call('c',"everett_whoami",own.clone()));
    denied(&call('d',"everett_whoami",own.clone()));
    denied(&call('c',"everett_identity",json!({"action":"revoke","identity_handle":identity["identity_handle"]})));
    assert_eq!(success(&call('c',"everett_identity",json!({"action":"list"})))["identities"], json!([]));
    let bob = success(&call('c',"everett_identity",create));
    assert_ne!(bob["session_id"],identity["session_id"]);
    assert_ne!(bob["identity_handle"],identity["identity_handle"]);
    issuer.grant(&access('b'),"agent-new",json!({"active":false}));
    assert_eq!(post(port,&whoami(),&[("Authorization",Some(bearer(&access('b')).as_str()))]).status,401);
    // A new account-level approval remains required after revocation; a handle is insufficient.
    assert_eq!(success(&call('a',"everett_whoami",own))["session_id"],identity["session_id"]);
}

fn oauth_rejects_wrong_issuer_audience_expiry_scope_identity_and_revoked_tokens() {
    let c = ctx();
    let issuer = fake_issuer();
    let cases = [("issuer", json!({"iss": "https://evil.example"})), ("audience", json!({"aud": "https://other.example/mcp"})),
        ("expired", json!({"exp": 1})), ("scope", json!({"scope": "other"})), ("agent", json!({"everett_agent": "agent-other"})),
        ("reserved", json!({"sub": "live", "everett_agent": "live"})), ("destination", json!({"sub": "agent-two", "everett_agent": "agent-two"}))];
    for (i, (_, changes)) in cases.iter().enumerate() {
        issuer.grant(&access(char::from(b'0' + i as u8)), "agent-aaaa", changes.clone());
    }
    issuer.grant(&access('f'), "agent-ffff", json!({}));
    let port = c.with_backend_oauth_server(&issuer.url);
    for (i, (name, _)) in cases.iter().enumerate() {
        let auth = bearer(&access(char::from(b'0' + i as u8)));
        let reply = post(port, &whoami(), &[("Authorization", Some(auth.as_str()))]);
        assert_eq!(reply.status, 401, "{name}");
        assert!(header(&reply, "www-authenticate").unwrap().contains("error=\"invalid_token\""), "{name}");
    }
    let auth = bearer(&access('f'));
    let auth = [("Authorization", Some(auth.as_str()))];
    assert_eq!(success(&body_json(&post(port, &whoami(), &auth)))["session_id"], "agent-ffff");
    issuer.tokens.lock().unwrap().insert(access('f'), json!({"active": false}));
    let revoked = post(port, &whoami(), &auth);
    assert_eq!(revoked.status, 401);
    assert!(String::from_utf8_lossy(&revoked.body).contains("revoked"));
    assert!(c.records().iter().all(|r| r["agent_id"] == "agent-ffff"));
    issuer.grant(&access('e'), "agent-eeee", json!({}));
    let independent = bearer(&access('e'));
    let reply = post(port, &whoami(), &[("Authorization", Some(independent.as_str()))]);
    assert_eq!(reply.status, 200);
    assert_eq!(success(&body_json(&reply))["session_id"], "agent-eeee");
    assert_eq!(post(port, &whoami(), &auth).status, 401);
}

fn oauth_configuration_fails_closed_and_stdio_keeps_fixed_identity() {
    let c = ctx();
    let issuer = fake_issuer();
    let oauth = json!({"oauth": {"issuer": issuer.url, "resource": RESOURCE}});
    let run = |mut command: Command| command.output().unwrap().status.code();
    let mut without_secret = c.oauth_command(&oauth);
    without_secret.env_remove("EVERETT_OAUTH_INTROSPECTION_SECRET");
    assert_eq!(run(without_secret), Some(2));
    let mut weak = c.oauth_command(&oauth);
    weak.env("EVERETT_OAUTH_INTROSPECTION_SECRET", "short");
    assert_eq!(run(weak), Some(2));
    let mut both = c.oauth_command(&oauth);
    both.env("EVERETT_GATEWAY_TOKEN", TOKEN);
    assert_eq!(run(both), Some(2));
    for bad in [json!({"issuer": "http://issuer.example", "resource": RESOURCE}), json!({"issuer": issuer.url, "resource": "ftp://x/mcp"}),
        json!({"issuer": issuer.url, "resource": RESOURCE, "scope": "a\"b"}), json!({"issuer": issuer.url, "resource": RESOURCE, "extra": 1})] {
        assert_eq!(run(c.oauth_command(&json!({"oauth": bad}))), Some(2), "{bad}");
    }
    let mut peer = c.peer(&with(fixture(&[]), oauth));
    assert_eq!(success(&peer.call("everett_whoami", json!({})))["session_id"], "dot-one");
    let down = Ctx::with_backend_oauth_server(&c, "http://127.0.0.1:9");
    let auth = bearer(&access('a'));
    assert_eq!(post(down, &whoami(), &[("Authorization", Some(auth.as_str()))]).status, 503);
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
    for harness in ["openai-dot", "grok-bot"] {
        let c = ctx();
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
        assert!(identity["gateway_runtime"]["revision"].is_string());
        assert_eq!(identity["capabilities"]["native_session_start"], false);
        assert_eq!(identity["capabilities"]["delivery"], "inbox");
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
        let mut recipient = Wire::new({
            let mut command = c.command(&["mcp"]);
            command.env("EVERETT_SESSION_ID", "agent-two").env("EVERETT_HARNESS_NAME", "claude");
            command
        });
        let received = success(&recipient.call("everett_inbox", json!({})));
        let messages = received["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        for message in messages {
            assert_eq!(message["external"], true, "{message}");
            assert_eq!(message["untrusted"], true, "{message}");
            assert!(message["text"].as_str().unwrap().starts_with("[EXTERNAL / UNTRUSTED"));
        }
        let owned = messages[0]["id"].as_str().unwrap().to_string();
        let forward = success(&recipient.call("everett_send", json!({"to":"dot-one","text":"Derived external data","mode":"inbox"})));
        assert_eq!(forward["delivered"], false);
        assert_eq!(forward["queued"], true);
        let reply = success(&recipient.call("everett_send", json!({"reply_to":owned,"text":"Synthetic answer"})));
        assert_eq!(reply["to"], "dot-one");
        let derived = success(&peer.call("everett_inbox", json!({})));
        let messages = derived["messages"].as_array().unwrap();
        let direct = messages.iter().find(|message| message["reply_to"] == "").unwrap();
        assert_eq!(direct["external"], Value::Null);
        let reply = messages.iter().find(|message| message["reply_to"] == owned).unwrap();
        assert_eq!(reply["untrusted"], true);
        assert!(reply["text"].as_str().unwrap().starts_with("[EXTERNAL / UNTRUSTED"));
        drop(recipient);
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
    let mut cases: Vec<(&str, Value)> = ["everett_ls", "everett_control", "everett_route", "everett_learn", "everett_event", "everett_subscribe"]
        .iter().map(|n| (*n, json!({}))).collect();
    cases.extend(["everett_inbox", "everett_card", "everett_send"].iter().map(|n| (*n, json!({"session_id": "other"}))));
    cases.extend([
        ("everett_status", json!({"message_id": "m0000000000"})),
        ("everett_core", json!({"project": "elsewhere"})), ("everett_inbox", json!({"peek": 1})),
        ("everett_core", json!({"action": "forget", "fact": "Owner fact"})),
        ("everett_core", json!({"action": "correct", "fact": "Owner fact", "replacement": "New fact"})),
        ("everett_core", json!({"action": "search", "query": "Owner fact"})),
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

fn conversation_metadata_cannot_replace_the_gateway_binding() {
    let c = ctx();
    let mut peer = c.peer(&fixture(&[]));
    for conversation in ["chat-one", "chat-two"] {
        let response = peer.request("tools/call", Some(json!({
            "name": "everett_whoami", "arguments": {},
            "_meta": {"openai/session": conversation, "openai/subject": "other-owner",
                      "threadId": "foreign-session"}
        })));
        assert_eq!(success(&response)["session_id"], "dot-one");
    }
    assert_eq!(c.records().len(), 2);
    assert!(c.records().iter().all(|r| r["agent_id"] == "dot-one"));
}

fn identity_listing_filters_same_name_registrations_by_kind() {
    let c = ctx();
    let mut peer = c.peer(&fixture(&[]));
    let mut expected = Vec::new();
    for (key, kind) in [("a", "chat"), ("b", "dot"), ("c", "chat")] {
        let identity = success(&peer.call("everett_identity", json!({"action":"create",
            "registration_key":key.repeat(32), "name":"Planning", "kind":kind})));
        if kind == "chat" {
            expected.push(identity["session_id"].as_str().unwrap().to_owned());
        }
    }
    let listed = success(&peer.call("everett_identity", json!({"action":"list","kind":"chat"})));
    let mut found: Vec<_> = listed["identities"].as_array().unwrap().iter().map(|row| {
        assert!(row.get("identity_handle").is_none());
        row["session_id"].as_str().unwrap().to_owned()
    }).collect();
    expected.sort();
    found.sort();
    assert_eq!(found, expected);
    let empty = success(&peer.call("everett_identity", json!({"action":"list","kind":"codex"})));
    assert_eq!(empty["identities"], json!([]));
    denied(&peer.call("everett_identity", json!({"action":"list","kind":"unknown"})));
    denied(&peer.call("everett_identity", json!({"action":"list","kind":null})));
}

fn per_chat_identities_are_private_durable_and_revocable() {
    let c = ctx();
    let mut peer = c.peer(&fixture(&[]));
    let mut identities = Vec::new();
    for (key, kind) in [("a", "chat"), ("b", "chat"), ("c", "dot"), ("d", "codex")] {
        let args = json!({"action":"create", "registration_key":key.repeat(32), "name":"Same name", "kind":kind});
        let created = success(&peer.call("everett_identity", args.clone()));
        assert!(created["session_id"].as_str().unwrap().starts_with("chat-"), "{created}");
        assert_ne!(created["session_id"], "dot-one");
        assert_eq!(success(&peer.call("everett_identity", args)), created);
        let own = success(&peer.call("everett_whoami", json!({"identity_handle":created["identity_handle"]})));
        assert_eq!(own["session_id"], created["session_id"]);
        assert!(!own.to_string().contains(created["identity_handle"].as_str().unwrap()));
        identities.push(created);
    }
    let list = success(&peer.call("everett_identity", json!({"action":"list"})));
    assert_eq!(list["identities"].as_array().unwrap().len(), 4);
    for identity in &identities {
        assert!(!list.to_string().contains(identity["identity_handle"].as_str().unwrap()));
    }
    assert_ne!(identities[0]["session_id"], identities[1]["session_id"]);
    denied(&peer.call("everett_inbox", json!({"identity_handle":identities[0]["identity_handle"], "session_id":identities[1]["session_id"]})));
    denied(&peer.call("everett_control", json!({"identity_handle":identities[0]["identity_handle"],"action":"start"})));
    drop(peer);
    let mut peer = c.peer(&fixture(&[]));
    assert_eq!(success(&peer.call("everett_whoami", json!({"identity_handle":identities[0]["identity_handle"]})))["session_id"], identities[0]["session_id"]);
    success(&peer.call("everett_identity", json!({"action":"revoke","identity_handle":identities[0]["identity_handle"]})));
    denied(&peer.call("everett_whoami", json!({"identity_handle":identities[0]["identity_handle"]})));
    assert_eq!(success(&peer.call("everett_whoami", json!({"identity_handle":identities[1]["identity_handle"]})))["session_id"], identities[1]["session_id"]);
    assert_eq!(success(&peer.call("everett_whoami", json!({})))["session_id"], "dot-one");
    assert!(c.records().iter().all(|r| r["arguments"].get("identity_handle").is_none()));
}

fn chat_identities_discover_and_exchange_correlated_inbox_messages() {
    let mut c = ctx();
    let mut peer = c.peer(&json!({}));
    let a = success(&peer.call("everett_identity",json!({"action":"create","registration_key":"a".repeat(32),"name":"First chat","kind":"chat"})));
    let b = success(&peer.call("everett_identity",json!({"action":"create","registration_key":"b".repeat(32),"name":"Second chat","kind":"dot"})));
    drop(peer);
    let listing: Value = serde_json::from_slice(&c.cli(&["external","list"]).stdout).unwrap();
    assert_eq!(listing.as_array().unwrap().len(),2);
    // The local owner grants destinations; chat registration does not grant them.
    c.config["destinations"] = json!([a["session_id"],b["session_id"]]);
    let mut peer = c.peer(&json!({}));
    let sent = success(&peer.call("everett_send",json!({"identity_handle":a["identity_handle"],"to":b["session_id"],"text":"Exact request"})));
    assert_eq!(sent["delivered"],false);
    let received = success(&peer.call("everett_inbox",json!({"identity_handle":b["identity_handle"]})));
    assert_eq!(received["messages"][0]["id"],sent["message_id"]);
    assert_eq!(received["messages"][0]["from"],a["session_id"]);
    assert!(received["messages"][0]["text"].as_str().unwrap().ends_with("\"Exact request\""));
    assert_eq!(received["messages"][0]["untrusted"],true);
    success(&peer.call("everett_send",json!({"identity_handle":b["identity_handle"],"reply_to":sent["message_id"],"text":"Exact answer"})));
    let reply = success(&peer.call("everett_inbox",json!({"identity_handle":a["identity_handle"]})));
    assert_eq!(reply["messages"][0]["reply_to"],sent["message_id"]);
    assert!(reply["messages"][0]["text"].as_str().unwrap().ends_with("\"Exact answer\""));
    assert_eq!(success(&peer.call("everett_inbox",json!({"identity_handle":b["identity_handle"]})))["messages"],json!([]));
}

fn revoked_identity_retries_finish_failed_registration_cleanup() {
    let c = ctx();
    let mut peer = c.peer(&fixture(&[]));
    let created = success(&peer.call("everett_identity",json!({"action":"create","registration_key":"r".repeat(32),"name":"Retry cleanup","kind":"chat"})));
    let path = c.home.join(".everett/external").join(format!("{}.json",created["session_id"].as_str().unwrap()));
    let registration = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let revoke = json!({"action":"revoke","identity_handle":created["identity_handle"]});
    denied(&peer.call("everett_identity",revoke.clone()));
    denied(&peer.call("everett_whoami",json!({"identity_handle":created["identity_handle"]})));
    // Repair the cleanup obstacle; the already-revoked handle must still clean up.
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path,registration).unwrap();
    assert_eq!(success(&peer.call("everett_identity",revoke.clone()))["revoked"],true);
    assert!(!path.exists());
    assert_eq!(success(&peer.call("everett_identity",revoke))["revoked"],true);
    denied(&peer.call("everett_whoami",json!({"identity_handle":created["identity_handle"]})));
}

fn identity_retry_rejects_invalid_discovery_records_and_repairs_missing_ones() {
    let c = ctx();
    let mut peer = c.peer(&fixture(&[]));
    let args = json!({"action":"create","registration_key":"s".repeat(32),"name":"Discovery retry","kind":"chat"});
    let created = success(&peer.call("everett_identity",args.clone()));
    let path = c.home.join(".everett/external").join(format!("{}.json",created["session_id"].as_str().unwrap()));
    let original = std::fs::read(&path).unwrap();
    for contents in [b"broken JSON".to_vec(), {
        let mut record: Value = serde_json::from_slice(&original).unwrap();
        record["device"] = json!("other/codex/foreign");
        serde_json::to_vec(&record).unwrap()
    }] {
        std::fs::write(&path, &contents).unwrap();
        denied(&peer.call("everett_identity",args.clone()));
        assert_eq!(std::fs::read(&path).unwrap(), contents, "never overwrite a conflicting registration");
        let listed = success(&peer.call("everett_identity",json!({"action":"list"})));
        assert_eq!(listed["identities"][0]["state"],"registration_unavailable");
    }
    std::fs::remove_file(&path).unwrap();
    assert_eq!(success(&peer.call("everett_identity",args)),created);
    let listed = success(&peer.call("everett_identity",json!({"action":"list"})));
    assert_eq!(listed["identities"][0]["state"],"registered");
    assert_eq!(listed["identities"][0]["availability"],"unknown");
    // Offline identity metadata remains durable, but never claims connectivity.
    std::fs::remove_file(&path).unwrap();
    let listed = success(&peer.call("everett_identity",json!({"action":"list"})));
    assert_eq!(listed["identities"][0]["state"],"registration_unavailable");
    let elsewhere = c.home.join("other-registration.json");
    std::fs::write(&elsewhere,&original).unwrap();
    std::os::unix::fs::symlink(&elsewhere,&path).unwrap();
    denied(&peer.call("everett_identity",json!({"action":"create","registration_key":"s".repeat(32),"name":"Discovery retry","kind":"chat"})));
    assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read(&elsewhere).unwrap(),original);
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

fn live_destinations_change_without_changing_cached_send_schema() {
    let c = ctx();
    let issuer = fake_issuer();
    issuer.grant(&access('a'), "agent-one", json!({"everett_account":"github:alice"}));
    let cfg = with(fixture(&[]), json!({"oauth":{"issuer":issuer.url,"resource":RESOURCE}}));
    let first = c.spawn_http(c.oauth_command(&cfg));
    let send_schema = |port| {
        let response = oauth_rpc(port, 'a', "tools/list", json!({}));
        response["result"]["tools"].as_array().unwrap().iter()
            .find(|tool| tool["name"] == "everett_send").unwrap()["inputSchema"].clone()
    };
    let cached_schema = send_schema(first);
    let changed = with(cfg, json!({"destinations":["agent-three"]}));
    let second = c.spawn_http(c.oauth_command(&changed));
    assert_eq!(cached_schema, send_schema(second), "owner grant updates must not stale a cached client schema");
    assert!(cached_schema["properties"]["to"].get("enum").is_none());
    assert_eq!(success(&native_call(first, 'a', "everett_whoami", json!({})))["authorized_destinations"], json!(["agent-two"]));
    assert_eq!(success(&native_call(second, 'a', "everett_whoami", json!({})))["authorized_destinations"], json!(["agent-three"]));
    let before = c.records().len();
    for to in [json!("agent-two"), json!("agent"), json!("agent-three-suffix"), json!("foreign"), json!("") , json!(1)] {
        denied(&native_call(second, 'a', "everett_send", json!({"to":to,"text":"must remain denied"})));
    }
    assert_eq!(c.records().len(), before, "denied destinations must not reach backend");
    success(&native_call(second, 'a', "everett_send", json!({"to":"agent-three","text":"new explicit owner grant"})));
    assert_eq!(c.records().last().unwrap()["arguments"]["to"], "agent-three");
}

fn native_inbox_batches_encoded_results_before_consuming() {
    for text in ["x".repeat(15000), "\"\\\n🦀".repeat(1500), "\"\\\n🦀".repeat(70000)] {
        let c = ctx();
        assert!(c.cli(&["external", "add", "--id", "dot-one", "--harness", "openai-dot"]).status.success());
        let count = if text.len() > 240 * 1024 { 1 } else { 10 };
        let path = c.inbox("agent-two", "dot-one", "placeholder");
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
        let rows: Vec<Value> = (0..count).map(|i| json!({"id":format!("budget-{i}"), "to":"dot-one", "from":"agent-two", "text":text, "ts":ts})).collect();
        std::fs::write(path, rows.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
        let port = c.http_server(&json!({}));
        let token = bearer(TOKEN);
        let auth = [("Authorization", Some(token.as_str()))];
        let read = |peek| {
            let response = post(port, &json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"everett_inbox", "arguments":{"peek":peek}}}), &auth);
            assert_eq!(response.status, 200);
            assert!(response.body.len() <= 256 * 1024, "encoded response must fit gateway budget");
            success(&body_json(&response))
        };
        let peeked = read(true);
        assert!(!peeked["messages"].as_array().unwrap().is_empty());
        assert!(!c.home.join(".everett/inbox/dot-one.done").exists());
        if count > 1 {
            assert!(peeked["messages"].as_array().unwrap().len() < count);
        }
        let mut delivered = Vec::new();
        for _ in 0..=count {
            let batch = read(false);
            let messages = batch["messages"].as_array().unwrap();
            if messages.is_empty() { break; }
            for message in messages {
                let id = message["id"].as_str().unwrap().to_string();
                assert!(!delivered.contains(&id), "a delivered message must not repeat");
                if count > 1 {
                    assert_eq!(message["text"], text);
                    assert!(message.get("truncated").is_none());
                } else {
                    assert_eq!(message["truncated"], true);
                    assert!(text.starts_with(message["text"].as_str().unwrap()));
                }
                delivered.push(id);
            }
            let done = std::fs::read_to_string(c.home.join(".everett/inbox/dot-one.done")).unwrap();
            let mut actual: Vec<_> = done.lines().collect();
            let mut expected: Vec<_> = delivered.iter().map(String::as_str).collect();
            actual.sort(); expected.sort();
            assert_eq!(actual, expected, "consume only IDs returned in a successful bounded batch");
        }
        assert_eq!(delivered.len(), count);
        assert_eq!(read(false)["messages"], json!([]));
    }
    let c = ctx();
    let path = c.inbox("agent-two", "dot-one", "oversized-metadata");
    let mut row: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    row["from_card"] = json!("x".repeat(300 * 1024));
    std::fs::write(path, format!("{row}\n")).unwrap();
    let mut peer = c.peer(&json!({}));
    let response = peer.call("everett_inbox", json!({}));
    denied(&response);
    assert!(!c.home.join(".everett/inbox/dot-one.done").exists(), "irreducible metadata must fail before consumption");
}

type Case = (&'static str, fn());

fn events_catalog_is_authenticated_and_scoped() {
    let c = ctx();
    let port = c.http_server(&json!({"events":true}));
    let request = json!({"jsonrpc":"2.0","id":1,"method":"events/list"});
    let result = body_json(&post(port, &request, &[]));
    assert_eq!(result["result"]["events"][0]["name"], "message.created");
    assert_eq!(post(port, &request, &[("Authorization", None)]).status, 401);
    let wrong = json!({"jsonrpc":"2.0","id":2,"method":"events/subscribe","params":{"name":"message.created","arguments":{"session_id":"foreign"},"delivery":{"mode":"webhook","url":"https://127.0.0.1/","secret":"synthetic"}}});
    assert_eq!(body_json(&post(port, &wrong, &[]))["error"]["code"], -32602);
}

const CASES: &[Case] = &[
    ("dot_picks_up_only_the_event_message_and_replies", dot_picks_up_only_the_event_message_and_replies),
    ("events_verified_token_lease", events_acceptance::verified_token_lease),
    ("events_identity_and_lifetime", events_acceptance::identity_and_lifetime),
    ("events_catalog_is_authenticated_and_scoped", events_catalog_is_authenticated_and_scoped),
    ("whoami_provenance_gateway", whoami_provenance::gateway),
    ("whoami_provenance_native", whoami_provenance::native),
    ("identity_diagnostic_native", identity_diagnostics::native),
    ("identity_diagnostic_gateway", identity_diagnostics::gateway),
    ("identity_diagnostic_invalid_input", identity_diagnostics::invalid_input),
    ("native_inbox_batches_encoded_results_before_consuming", native_inbox_batches_encoded_results_before_consuming),
    ("live_destinations_change_without_changing_cached_send_schema", live_destinations_change_without_changing_cached_send_schema),
    ("native_start_policy_and_journal_fail_closed", native_start_policy_and_journal_fail_closed),
    ("owner_native_start_is_explicit_isolated_and_durable", owner_native_start_is_explicit_isolated_and_durable),
    ("native_start_uncertain_receipts_never_dispatch_twice", native_start_uncertain_receipts_never_dispatch_twice),
    ("native_records_waits_for_complete_fixture_lines", native_records_waits_for_complete_fixture_lines),
    ("concurrent_native_start_and_gateway_crash_keep_reservation", concurrent_native_start_and_gateway_crash_keep_reservation),
    ("revoked_identity_retries_finish_failed_registration_cleanup", revoked_identity_retries_finish_failed_registration_cleanup),
    ("identity_retry_rejects_invalid_discovery_records_and_repairs_missing_ones", identity_retry_rejects_invalid_discovery_records_and_repairs_missing_ones),
    ("chat_identities_discover_and_exchange_correlated_inbox_messages", chat_identities_discover_and_exchange_correlated_inbox_messages),
    ("oauth_chat_identity_survives_reapproval_but_rejects_another_account", oauth_chat_identity_survives_reapproval_but_rejects_another_account),
    ("identity_listing_filters_same_name_registrations_by_kind", identity_listing_filters_same_name_registrations_by_kind),
    ("per_chat_identities_are_private_durable_and_revocable", per_chat_identities_are_private_durable_and_revocable),
    ("native_stdio_journey_for_both_external_harnesses", native_stdio_journey_for_both_external_harnesses),
    ("policy_rejects_foreign_identity_project_and_unsafe_arguments", policy_rejects_foreign_identity_project_and_unsafe_arguments),
    ("conversation_metadata_cannot_replace_the_gateway_binding", conversation_metadata_cannot_replace_the_gateway_binding),
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
    ("oauth_serves_protected_resource_metadata_and_challenges", oauth_serves_protected_resource_metadata_and_challenges),
    ("oauth_token_maps_to_its_dot_identity", oauth_token_maps_to_its_dot_identity),
    ("oauth_rejects_wrong_issuer_audience_expiry_scope_identity_and_revoked_tokens", oauth_rejects_wrong_issuer_audience_expiry_scope_identity_and_revoked_tokens),
    ("oauth_owner_account_restricts_every_request_before_backend_dispatch", oauth_owner_account_restricts_every_request_before_backend_dispatch),
    ("oauth_configuration_fails_closed_and_stdio_keeps_fixed_identity", oauth_configuration_fails_closed_and_stdio_keeps_fixed_identity),
];

fn dot_picks_up_only_the_event_message_and_replies() {
    let c = ctx();
    assert!(c.cli(&["external", "add", "--id", "dot-one", "--harness", "openai-dot"]).status.success());
    let path = c.inbox("agent-two", "dot-one", "event-message");
    let event = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, event.replace("event-message", "unrelated") + &event).unwrap();
    let port = c.http_server(&json!({"events":true}));
    let call = |name, args| body_json(&post(port, &json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":name,"arguments":args}}), &[]));
    let rejected = call("everett_inbox", json!({"message_id":"../foreign"}));
    denied(&rejected);
    denied(&call("everett_inbox", json!({"session_id":"agent-two","message_id":"event-message"})));
    let picked = success(&call("everett_inbox", json!({"message_id":"event-message"})));
    let items = picked["messages"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "event-message");
    assert_eq!(success(&call("everett_inbox", json!({"message_id":"event-message"})))["messages"], json!([]));
    let remaining = success(&call("everett_inbox", json!({"peek":true})));
    let items = remaining["messages"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "unrelated");
    let reply = success(&call("everett_send", json!({"reply_to":"event-message","text":"Dot response"})));
    assert_eq!(reply["to"], "agent-two");
    let received = std::fs::read_to_string(c.home.join(".everett/inbox/agent-two.jsonl")).unwrap();
    let received: Value = serde_json::from_str(&received).unwrap();
    assert_eq!(received["reply_to"], "event-message");
    assert_eq!(received["text"], "Dot response");

    let old_backend = c.http_server(&fixture(&["--legacy"]));
    let reply = body_json(&post(old_backend, &json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"everett_inbox","arguments":{"message_id":"unrelated"}}}), &[]));
    denied(&reply);
    assert!(c.records().is_empty(), "the older backend must not receive a consuming call");
    assert_eq!(success(&call("everett_inbox", json!({"peek":true})))["messages"][0]["id"], "unrelated");

    private_dot_picks_up_and_replies_to_its_event();
}

fn private_dot_picks_up_and_replies_to_its_event() {
    let c = ctx();
    let issuer = fake_issuer();
    issuer.grant(&access('a'), "agent-a", json!({"everett_account":"github:alice"}));
    let port = c.spawn_http(c.oauth_command(&json!({"events":true,"oauth":{"issuer":issuer.url,"resource":RESOURCE}})));
    let register = |key: char| success(&native_call(port, 'a', "everett_identity", json!({
        "action":"create", "registration_key":key.to_string().repeat(32), "name":"Fixture Dot", "kind":"dot"
    })));
    let dot = register('a');
    let other = register('b');
    let dot_id = dot["session_id"].as_str().unwrap();
    let handle = &dot["identity_handle"];
    let from = other["session_id"].as_str().unwrap();
    let path = c.inbox(from, dot_id, "requested-work");
    let row = std::fs::read_to_string(&path).unwrap();
    std::fs::rename(path, c.home.join(format!(".everett/inbox/{dot_id}.jsonl"))).unwrap();
    std::fs::write(c.home.join(format!(".everett/inbox/{from}.jsonl")), row
        .replace("requested-work", "other-work").replace(dot_id, from)).unwrap();
    let sub = json!({"name":"message.created","arguments":{"identity_handle":handle,"session_id":dot_id},
        "delivery":{"mode":"webhook","url":"https://callback.example/","secret":"invalid-synthetic"}});
    assert_eq!(oauth_rpc(port, 'a', "events/subscribe", sub.clone())["error"]["message"], "invalid_signing_secret");
    let mut wrong = sub;
    wrong["arguments"]["identity_handle"] = other["identity_handle"].clone();
    assert_eq!(oauth_rpc(port, 'a', "events/subscribe", wrong)["error"]["message"], "unsupported_event_or_filter");
    let call = |name, args| native_call(port, 'a', name, args);
    let take = |handle: &Value, id: &str| success(&call("everett_inbox", json!({"identity_handle":handle,"message_id":id})));
    assert_eq!(take(&other["identity_handle"], "requested-work")["messages"], json!([]));
    assert_eq!(take(handle, "requested-work")["messages"][0]["id"], "requested-work");
    assert_eq!(take(handle, "requested-work")["messages"], json!([]));
    denied(&call("everett_send", json!({"identity_handle":other["identity_handle"],"reply_to":"requested-work","text":"wrong inbox"})));
    let reply = success(&call("everett_send", json!({"identity_handle":handle,"reply_to":"requested-work","text":"Dot reply"})));
    assert_eq!(reply["to"], from);
    let results = success(&call("everett_inbox", json!({"identity_handle":other["identity_handle"],"peek":true})));
    assert_eq!(results["messages"].as_array().unwrap().len(), 2);
    assert_eq!(results["messages"][0]["id"], "other-work");
    assert_eq!(results["messages"][1]["reply_to"], "requested-work");
    assert_eq!(results["messages"][1]["kind"], "reply");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str)==Some("thread") { native_cli_fixture(&args[1..]); return; }
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

fn native_records_waits_for_complete_fixture_lines() {
    let c = ctx();
    let path = c.home.join("native-start-calls.jsonl");
    std::fs::write(&path, "{\"pid\":1}\n{\"pid\":").unwrap();
    assert_eq!(native_records(&c), vec![json!({"pid":1})]);
    std::fs::write(&path, "{\"pid\":1}\n{\"pid\":2}\n").unwrap();
    assert_eq!(native_records(&c), vec![json!({"pid":1}), json!({"pid":2})]);
}
