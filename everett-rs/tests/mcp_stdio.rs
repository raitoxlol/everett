mod common;

use common::{fixture, plant, Fixture};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Server {
    fn new(fx: &Fixture, env: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_everett"));
        cmd.arg("mcp")
            .env_clear()
            .env("HOME", fx.home())
            .env("EVERETT_HOME", fx.home())
            .env("EVERETT_NOTIFY", "none")
            .env("PATH", "/nonexistent")
            .current_dir(fx.home())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Server {
            child,
            stdin,
            stdout,
            next_id: 0,
        }
    }

    fn start(fx: &Fixture, env: &[(&str, &str)]) -> Self {
        let mut s = Server::new(fx, env);
        let init = s.request(
            "initialize",
            Some(json!({"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}})),
        );
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        s.send_line(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string());
        s
    }

    fn send_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(format!("{}\n", line).as_bytes()).unwrap();
        stdin.flush().unwrap();
    }

    fn raw(&mut self, line: &str) -> Value {
        self.send_line(line);
        let mut buf = String::new();
        self.stdout.read_line(&mut buf).unwrap();
        serde_json::from_str(&buf).unwrap()
    }

    fn request(&mut self, method: &str, params: Option<Value>) -> Value {
        self.next_id += 1;
        let mut msg = json!({"jsonrpc": "2.0", "id": self.next_id, "method": method});
        if let Some(p) = params {
            msg["params"] = p;
        }
        let response = self.raw(&msg.to_string());
        assert_eq!(response["id"], self.next_id, "{response}");
        response
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request(
            "tools/call",
            Some(json!({"name": name, "arguments": arguments})),
        )["result"]
            .clone()
    }

    fn close(mut self) {
        drop(self.stdin.take());
        self.child.wait().unwrap();
    }
}

fn text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn home_with_claude_session() -> Fixture {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/-work-app/c-1.jsonl");
    fx
}

#[test]
fn concurrent_card_updates_are_atomic_and_do_not_share_temporary_files() {
    let fx = home_with_claude_session();
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for worker in 0..8 {
            let fx = &fx;
            let barrier = &barrier;
            workers.push(scope.spawn(move || {
                let mut server = Server::start(
                    fx,
                    &[("EVERETT_SESSION_ID", "c-1"), ("EVERETT_HARNESS_NAME", "claude")],
                );
                let mut results = Vec::new();
                for round in 0..16 {
                    barrier.wait();
                    results.push(server.call(
                        "everett_card",
                        json!({"what":format!("worker-{worker}-round-{round}"),
                            "state":"working", "next":"finish"}),
                    ));
                }
                server.close();
                results
            }));
        }
        for worker in workers {
            for result in worker.join().unwrap() {
                assert_eq!(result["isError"], false, "{result}");
            }
        }
    });
    let card = std::fs::read_to_string(fx.home().join(".everett/cards/c-1.md")).unwrap();
    assert!(
        (0..8).any(|worker| card == format!("What: worker-{worker}-round-15\nState: working\nNext: finish\n")),
        "{card}"
    );
    assert_eq!(
        std::fs::read_dir(fx.home().join(".everett/cards")).unwrap().count(),
        1
    );
}

#[test]
fn round_trip_over_pipes() {
    let fx = home_with_claude_session();
    let mut s = Server::start(&fx, &[]);
    assert_eq!(s.request("ping", None)["result"], json!({}));
    let tools = s.request("tools/list", None)["result"]["tools"].clone();
    let mut names: Vec<String> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().into())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "everett_agent_grants",
            "everett_agent_relay",
            "everett_agent_send",
            "everett_agent_status",
            "everett_card",
            "everett_core",
            "everett_event",
            "everett_inbox",
            "everett_learn",
            "everett_ls",
            "everett_route",
            "everett_send",
            "everett_subscribe",
            "everett_whoami"
        ]
    );
    for tool in tools.as_array().unwrap() {
        assert!(!tool["description"].as_str().unwrap().trim().is_empty());
        let schema = &tool["inputSchema"];
        assert_eq!(
            (
                schema["type"].clone(),
                schema["additionalProperties"].clone()
            ),
            (json!("object"), json!(false))
        );
        for key in schema["required"].as_array().into_iter().flatten() {
            assert!(
                schema["properties"].get(key.as_str().unwrap()).is_some(),
                "{tool}"
            );
        }
    }
    let listing = s.call("everett_ls", json!({}));
    assert_eq!(listing["isError"], false);
    let ids: Vec<_> = listing["structuredContent"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].clone())
        .collect();
    assert_eq!(ids, vec![json!("c-1")]);
    assert_eq!(
        serde_json::from_str::<Value>(&text(&listing)).unwrap(),
        listing["structuredContent"]
    );
    s.close();
}

#[test]
fn version_negotiation() {
    let fx = home_with_claude_session();
    let mut s = Server::new(&fx, &[]);
    let old = s.request(
        "initialize",
        Some(json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {}})),
    );
    assert_eq!(old["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(old["result"]["serverInfo"]["name"], "everett");
    let future = s.request(
        "initialize",
        Some(json!({"protocolVersion": "2099-01-01", "capabilities": {}, "clientInfo": {}})),
    );
    assert_eq!(
        future["result"]["protocolVersion"],
        everett::mcp::PROTOCOL_VERSIONS[0]
    );
    s.close();
}

#[test]
fn error_codes() {
    let fx = home_with_claude_session();
    let mut s = Server::start(&fx, &[]);
    assert_eq!(s.raw("{not json")["error"]["code"], -32700);
    assert_eq!(
        s.raw(r#"{"jsonrpc": "1.0", "id": 9, "method": "ping"}"#)["error"]["code"],
        -32600
    );
    assert_eq!(s.request("resources/list", None)["error"]["code"], -32601);
    assert_eq!(
        s.request("tools/call", Some(json!({"name": "nope"})))["error"]["code"],
        -32602
    );
    assert_eq!(
        s.request(
            "tools/call",
            Some(json!({"name": "everett_route", "arguments": {}}))
        )["error"]["code"],
        -32602
    );
    assert_eq!(
        s.request(
            "tools/call",
            Some(json!({"name": "everett_ls", "arguments": {"hours": "x"}}))
        )["error"]["code"],
        -32602
    );
    let batch = s.raw(r#"[{"jsonrpc":"2.0","id":"a","method":"ping"},{"jsonrpc":"2.0","method":"notifications/x"}]"#);
    assert_eq!(batch, json!([{"jsonrpc": "2.0", "id": "a", "result": {}}]));
    s.close();
}

#[test]
fn hop_guard_refuses_past_three() {
    let fx = home_with_claude_session();
    let mut s = Server::start(&fx, &[("EVERETT_HOPS", "3")]);
    let result = s.call("everett_send", json!({"text": "keep going", "to": "c-1"}));
    assert_eq!(result["isError"], true);
    assert!(text(&result).contains("Hop limit"), "{result}");
    s.close();
}

#[test]
fn self_send_refused() {
    let fx = home_with_claude_session();
    let mut s = Server::start(&fx, &[("CLAUDE_CODE_SESSION_ID", "c-1")]);
    let who = s.call("everett_whoami", json!({}))["structuredContent"].clone();
    assert_eq!(
        (
            who["session_id"].clone(),
            who["harness"].clone(),
            who["source"].clone()
        ),
        (
            json!("c-1"),
            json!("claude"),
            json!("env CLAUDE_CODE_SESSION_ID")
        )
    );
    assert_eq!(
        s.call("everett_ls", json!({}))["structuredContent"]["sessions"][0]["you"],
        true
    );
    let result = s.call("everett_send", json!({"text": "hello me", "to": "c-1"}));
    assert_eq!(result["isError"], true);
    assert!(text(&result).contains("calling session itself"), "{result}");
    s.close();
    let mut explicit = Server::start(&fx, &[]);
    let result = explicit.request("tools/call", Some(json!({"name":"everett_send", "arguments":
        {"text": "hello me", "to": "c-1", "session_id": "c-1"}
    })));
    assert_eq!(result["error"]["code"], -32602, "{result}");
    explicit.close();
}

#[test]
fn codex_call_metadata_binds_identity_without_process_environment() {
    let fx = home_with_claude_session();
    plant(
        &fx,
        "codex.jsonl",
        ".codex/sessions/2026/10/01/x-1.jsonl",
    );
    fx.write(
        ".everett/inbox/c-1.jsonl",
        &json!({
            "id":"codex-message", "from":"owner", "to":"c-1",
            "text":"bound by call metadata", "ts":everett::session::now()
        })
        .to_string(),
    );
    let mut server = Server::start(&fx, &[]);
    let call = |name: &str, arguments: Value| {
        json!({
            "name":name,
            "arguments":arguments,
            "_meta":{"threadId":"c-1"}
        })
    };
    let who = server.request(
        "tools/call",
        Some(call("everett_whoami", json!({}))),
    )["result"]["structuredContent"]
        .clone();
    assert_eq!(
        (
            who["session_id"].clone(),
            who["harness"].clone(),
            who["source"].clone()
        ),
        (
            json!("c-1"),
            json!("codex"),
            json!("request _meta.threadId")
        )
    );
    let inbox = server.request(
        "tools/call",
        Some(call("everett_inbox", json!({"peek":true}))),
    )["result"]["structuredContent"]
        .clone();
    assert_eq!(inbox["session"], "c-1");
    assert_eq!(inbox["messages"][0]["id"], "codex-message");
    let card = server.request(
        "tools/call",
        Some(call(
            "everett_card",
            json!({"what":"Codex metadata","state":"working","next":"finish"}),
        )),
    )["result"]
        .clone();
    assert_eq!(card["isError"], false, "{card}");
    assert!(fx.home().join(".everett/cards/c-1.md").exists());
    let sent = server.request(
        "tools/call",
        Some(call(
            "everett_send",
            json!({"to":"x-1","text":"host-bound sender","mode":"inbox"}),
        )),
    )["result"]
        .clone();
    assert_eq!(sent["isError"], false, "{sent}");
    let queued: Value = serde_json::from_str(
        std::fs::read_to_string(fx.home().join(".everett/inbox/x-1.jsonl"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(queued["from"], "c-1");
    assert_eq!(queued["from_harness"], "codex");
    let self_send = server.request(
        "tools/call",
        Some(call(
            "everett_send",
            json!({"to":"c-1","text":"must identify the caller"}),
        )),
    )["result"]
        .clone();
    assert!(text(&self_send).contains("calling session itself"), "{self_send}");
    let conflict = server.request(
        "tools/call",
        Some(call(
            "everett_card",
            json!({
                "session_id":"victim", "what":"spoofed",
                "state":"working", "next":"finish"
            }),
        )),
    );
    assert_eq!(conflict["error"]["code"], -32602, "{conflict}");
    assert!(conflict["error"]["message"]
        .as_str()
        .unwrap()
        .contains("conflicts with the host-detected caller"));
    assert!(!fx.home().join(".everett/cards/victim.md").exists());
    server.close();
}

#[test]
fn new_without_spawn_sends_nothing() {
    let fx = home_with_claude_session();
    let mut s = Server::start(&fx, &[]);
    let result = s.call(
        "everett_send",
        json!({"text": "compose a sonnet about penguins"}),
    )["structuredContent"]
        .clone();
    assert_eq!(
        (result["decision"].clone(), result["delivered"].clone()),
        (json!("NEW"), json!(false))
    );
    assert!(result["note"].as_str().unwrap().contains("spawn=true"));
    s.close();
}

#[test]
fn learn_secret_filter_core_and_card() {
    let fx = home_with_claude_session();
    let mut s = Server::start(&fx, &[("CODEX_THREAD_ID", "x-9")]);
    let bad = s.call(
        "everett_learn",
        json!({"fact": "the api_key = abcdef123456"}),
    );
    assert_eq!(bad["isError"], true);
    assert!(
        text(&bad).contains("Secrets never go into the shared core"),
        "{bad}"
    );
    assert_eq!(
        s.call(
            "everett_learn",
            json!({"fact": "Deploys go out from main only"})
        )["isError"],
        false
    );
    assert!(s.call("everett_core", json!({}))["structuredContent"]
        .get("pending_learnings")
        .is_some());
    let card = s.call("everett_card", json!({"what": "Deploy tooling", "state": "writing the rollback script", "next": "test it"}));
    assert_eq!(card["isError"], false, "{card}");
    s.close();
    let body = std::fs::read_to_string(fx.home().join(".everett/cards/x-9.md")).unwrap();
    assert_eq!(
        body,
        "What: Deploy tooling\nState: writing the rollback script\nNext: test it\n"
    );
    let inbox = std::fs::read_to_string(fx.home().join(".everett/core/inbox.jsonl")).unwrap();
    let lines: Vec<_> = inbox.lines().collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(lines[0]).unwrap()["harness"],
        "codex"
    );
    let log = std::fs::read_to_string(fx.home().join(".everett/mcp.log")).unwrap();
    assert!(!log.contains("abcdef123456") && !log.contains("Deploys go out from main only"));
}

#[test]
fn card_needs_identity() {
    let fx = home_with_claude_session();
    let mut s = Server::start(&fx, &[]);
    let result = s.call(
        "everett_card",
        json!({"what": "a", "state": "b", "next": "c"}),
    );
    assert_eq!(result["isError"], true);
    assert!(text(&result).contains("EVERETT_SESSION_ID"), "{result}");
    s.close();
}

#[test]
fn agent_grants_require_explicit_key_confirmation_and_bounded_limits() {
    let fx = home_with_claude_session();
    let mut s = Server::start(&fx, &[]);
    for action in ["invite", "propose", "accept", "approve", "reconfirm"] {
        let result = s.request(
            "tools/call",
            Some(
                json!({"name":"everett_agent_grants","arguments":{"action":action,"id":"grant-1"}}),
            ),
        );
        assert_eq!(result["error"]["code"], -32602, "{result}");
        assert!(
            result["error"]["message"]
                .as_str()
                .unwrap()
                .contains("local_fingerprint"),
            "{result}"
        );
    }
    let mut invite = json!({"action":"invite","id":"grant-1","device":"machine-a",
        "peer_owner":"remote-org","remote_agent":"machine-b",
        "local_fingerprint":"a".repeat(64),"peer_fingerprint":"b".repeat(64)});
    for (key, value) in [
        ("rate_per_minute", json!(0)),
        ("rate_per_minute", json!(121)),
        ("rate_per_minute", json!(1.5)),
        ("rate_per_minute", json!("30")),
        ("body_cap_bytes", json!(65537)),
        ("expires_at", json!(-1)),
    ] {
        invite[key] = value;
        let result = s.request(
            "tools/call",
            Some(json!({"name":"everett_agent_grants","arguments":invite.clone()})),
        );
        assert_eq!(result["error"]["code"], -32602, "{result}");
        assert!(
            result["error"]["message"].as_str().unwrap().contains(key),
            "{result}"
        );
        invite.as_object_mut().unwrap().remove(key);
    }
    for (tool, args) in [
        ("everett_agent_grants", invite),
        ("everett_agent_relay", json!({"action":"credentials"})),
        ("everett_agent_relay", json!({"action":"memberships"})),
        (
            "everett_agent_relay",
            json!({"action":"revoke-agent","id":"machine-b"}),
        ),
        (
            "everett_agent_relay",
            json!({"action":"revoke-membership","id":"user-b"}),
        ),
    ] {
        let result = s.call(tool, args);
        assert_eq!(result["isError"], true, "{result}");
        assert!(text(&result).contains("login"), "{result}");
    }
    s.close();
}

#[test]
fn log_records_metadata_without_bodies() {
    let fx = home_with_claude_session();
    let mut s = Server::start(&fx, &[]);
    s.call(
        "everett_route",
        json!({"text": "x".repeat(1000), "router": "local"}),
    );
    s.close();
    let log = std::fs::read_to_string(fx.home().join(".everett/mcp.log")).unwrap();
    let lines: Vec<Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let call = lines.iter().find(|l| l["event"] == "call").unwrap();
    assert_eq!(call["tool"], "everett_route");
    assert_eq!(
        serde_json::from_str::<Value>(call["args"].as_str().unwrap()).unwrap(),
        json!(["router", "text"])
    );
    assert!(lines.iter().any(|l| l["event"] == "result"));
    assert!(lines.iter().all(|l| l.to_string().len() < 1000));
}

#[test]
fn install_mcp_print_writes_nothing() {
    let fx = fixture();
    let out = fx.stdout(&fx.run(&["install-mcp"]));
    for needle in [
        "claude mcp add --scope user",
        "[mcp_servers.everett]",
        "\"mcpServers\"",
    ] {
        assert!(out.contains(needle), "{needle}: {out}");
    }
    assert!(!fx.home().join(".claude.json").exists());
}

fn backups(dir: &std::path::Path, prefix: &str) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(prefix)
        })
        .count()
}

#[test]
fn install_mcp_apply_backs_up_and_is_idempotent() {
    let fx = fixture();
    let claude = fx.write(
        ".claude.json",
        &json!({"numStartups": 3, "mcpServers": {"other": {"command": "x"}}}).to_string(),
    );
    let codex = fx.write(
        ".codex/config.toml",
        "model = \"gpt\"\n[mcp_servers.other]\ncommand = \"x\"\n",
    );
    std::fs::create_dir_all(fx.home().join(".omp/agent")).unwrap();
    for _ in 0..2 {
        let out = fx.run(&["install-mcp", "--claude", "--codex", "--omp", "--apply"]);
        assert_eq!(out.status.code(), Some(0), "{}", fx.stdout(&out));
    }
    let data: Value = serde_json::from_str(&std::fs::read_to_string(&claude).unwrap()).unwrap();
    assert_eq!(data["numStartups"], 3);
    let mut servers: Vec<_> = data["mcpServers"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    servers.sort();
    assert_eq!(servers, ["everett", "other"]);
    assert_eq!(data["mcpServers"]["everett"]["args"], json!(["mcp"]));
    let toml_text = std::fs::read_to_string(&codex).unwrap();
    assert_eq!(toml_text.matches("[mcp_servers.everett]").count(), 1);
    assert!(toml_text.starts_with("model = \"gpt\"\n[mcp_servers.other]"));
    let parsed: toml::Value = toml_text.parse().unwrap();
    assert_eq!(
        parsed["mcp_servers"]["everett"]["args"].as_array().unwrap()[0].as_str(),
        Some("mcp")
    );
    let omp: Value = serde_json::from_str(
        &std::fs::read_to_string(fx.home().join(".omp/agent/mcp.json")).unwrap(),
    )
    .unwrap();
    assert!(omp["mcpServers"].get("everett").is_some());
    assert_eq!(backups(&fx.home(), ".claude.json.everett-bak-"), 1);
    assert_eq!(
        backups(codex.parent().unwrap(), "config.toml.everett-bak-"),
        1
    );
}

#[test]
fn install_mcp_grok_toml_idempotent_with_backup() {
    let fx = fixture();
    let cfg = fx.write(
        ".grok/config.toml",
        "model = \"grok-4\"\n\n[mcp_servers.other]\ncommand = \"x\"\n",
    );
    assert!(fx
        .stdout(&fx.run(&["install-mcp", "--grok"]))
        .contains("grok mcp add"));
    let first = fx.stdout(&fx.run(&["install-mcp", "--grok", "--apply"]));
    assert!(first.contains("backup"), "{first}");
    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("[mcp_servers.other]"));
    assert_eq!(text.matches("[mcp_servers.everett]").count(), 1);
    assert!(fx
        .stdout(&fx.run(&["install-mcp", "--grok", "--apply"]))
        .contains("already registered"));
    assert_eq!(
        std::fs::read_to_string(&cfg)
            .unwrap()
            .matches("[mcp_servers.everett]")
            .count(),
        1
    );
}

#[test]
fn registered_command_actually_serves() {
    let fx = fixture();
    fx.run(&["install-mcp", "--claude", "--apply"]);
    let data: Value =
        serde_json::from_str(&std::fs::read_to_string(fx.home().join(".claude.json")).unwrap())
            .unwrap();
    let entry = &data["mcpServers"]["everett"];
    let args: Vec<String> = entry["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().into())
        .collect();
    let mut child = Command::new(entry["command"].as_str().unwrap())
        .args(&args)
        .env_clear()
        .env("HOME", fx.home())
        .env("PATH", "/nonexistent")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"jsonrpc": "2.0", "id": 1, "result": {}})
    );
}

#[test]
fn remote_lineage_guard_binds_host_caller_and_preserves_durable_hops_over_stdio() {
    let fx = home_with_claude_session();
    fx.write(".everett/inbox/relay-context/remote.json", &json!({"relay_message_id":"parent","relay_sender_owner":"synthetic-owner","hops":2,"external":true,"untrusted":true}).to_string());
    fx.write(
        ".everett/inbox/live/remote.json",
        &json!({"hops_ts":0,"hops":3}).to_string(),
    );
    let mut server = Server::start(&fx, &[("EVERETT_SESSION_ID", "remote")]);
    for args in [
        json!({"text":"untrusted","to":"c-1","mode":"resume","session_id":"remote"}),
        json!({"text":"untrusted","spawn":true,"session_id":"remote"}),
    ] {
        let result = server.call("everett_send", args);
        assert_eq!(result["isError"], true);
        assert!(
            text(&result).contains("Remote-derived callers are inbox-only"),
            "{result}"
        );
    }
    let result = server.call(
        "everett_send",
        json!({"text":"untrusted","to":"c-1","session_id":"remote"}),
    );
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(result["structuredContent"]["mode"], "inbox");
    let messages = std::fs::read_to_string(fx.home().join(".everett/inbox/c-1.jsonl")).unwrap();
    let message: Value = serde_json::from_str(messages.lines().next().unwrap()).unwrap();
    assert_eq!(message["hops"], 3);
    assert_eq!(message["external"], true);
    assert_eq!(message["relay_lineage"]["relay_message_id"], "parent");
    let mismatch = server.request(
        "tools/call",
        Some(json!({
            "name":"everett_send",
            "arguments":{"text":"spoof","to":"c-1","mode":"inbox","session_id":"clean"}
        })),
    );
    assert_eq!(mismatch["error"]["code"], -32602, "{mismatch}");
    assert!(
        mismatch["error"]["message"]
            .as_str()
            .unwrap()
            .contains("conflicts with the host-detected caller"),
        "{mismatch}"
    );
    for (tool, arguments) in [
        (
            "everett_send",
            json!({"text":"ssh spoof","to":"ssh-peer/codex/remote","session_id":"clean"}),
        ),
        (
            "everett_send",
            json!({"text":"spawn spoof","spawn":true,"session_id":"clean"}),
        ),
        (
            "everett_send",
            json!({"text":"resume spoof","to":"c-1","mode":"resume","session_id":"clean"}),
        ),
        (
            "everett_send",
            json!({"text":"reply spoof","reply_to":"parent","session_id":"clean"}),
        ),
        (
            "everett_agent_send",
            json!({"agent":"agent","harness":"codex","session":"remote","text":"agent spoof","idempotency_key":"agent-spoof","session_id":"clean"}),
        ),
    ] {
        let response = server.request(
            "tools/call",
            Some(json!({"name":tool,"arguments":arguments})),
        );
        assert_eq!(response["error"]["code"], -32602, "{response}");
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("conflicts with the host-detected caller"));
    }
    let inbox = server.request(
        "tools/call",
        Some(json!({"name":"everett_inbox","arguments":{"session_id":"clean"}})),
    );
    assert_eq!(inbox["error"]["code"], -32602, "{inbox}");
    assert!(
        inbox["error"]["message"]
            .as_str()
            .unwrap()
            .contains("conflicts with the host-detected caller"),
        "{inbox}"
    );
    let messages = std::fs::read_to_string(fx.home().join(".everett/inbox/c-1.jsonl")).unwrap();
    assert_eq!(
        messages.lines().count(),
        1,
        "spoofed caller must not create a clean local forward"
    );
    fx.write(
        ".everett/inbox/relay-context/remote.json",
        &json!({"relay_message_id":"parent","hops":3,"external":true}).to_string(),
    );
    let result = server.call(
        "everett_send",
        json!({"text":"hop 4","to":"c-1","mode":"inbox","session_id":"remote"}),
    );
    assert!(text(&result).contains("Hop limit"), "{result}");
    server.close();
}

#[test]
fn unbound_mcp_cannot_deliver_or_reply_with_model_provenance() {
    let fx = home_with_claude_session();
    fx.write(
        ".everett/inbox/c-1.jsonl",
        &json!({"id":"reply-parent","from":"remote","harness":"codex","text":"parent"}).to_string(),
    );
    let mut server = Server::start(&fx, &[]);
    for args in [
        json!({"text":"no identity","to":"c-1","mode":"inbox"}),
        json!({"text":"spoofed identity","to":"c-1","mode":"inbox","session_id":"model-session"}),
        json!({"text":"reply without identity","reply_to":"reply-parent"}),
        json!({"text":"spoofed reply","reply_to":"reply-parent","session_id":"model-session"}),
    ] {
        let explicit = args.get("session_id").is_some();
        let response = server.request("tools/call", Some(json!({"name":"everett_send", "arguments":args})));
        if explicit {
            assert_eq!(response["error"]["code"], -32602, "{response}");
        } else {
            let result = &response["result"];
            assert_eq!(result["isError"], true, "{response}");
            assert!(text(result).contains("Cannot authenticate the calling session for delivery"), "{response}");
        }
    }
    assert_eq!(
        std::fs::read_to_string(fx.home().join(".everett/inbox/c-1.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    server.close();
}

#[test]
fn unbound_mcp_cannot_read_consume_or_overwrite_another_sessions_state() {
    let fx = home_with_claude_session();
    fx.write(
        ".everett/inbox/victim.jsonl",
        &json!({"id":"private-marker","from":"owner","to":"victim","text":"synthetic private marker","ts":everett::session::now()}).to_string(),
    );
    let mut server = Server::start(&fx, &[]);
    for (name, arguments) in [
        ("everett_inbox", json!({"session_id":"victim","peek":true})),
        ("everett_inbox", json!({"session_id":"victim"})),
        ("everett_card", json!({"session_id":"victim","what":"impersonated","state":"working","next":"done"})),
    ] {
        let response = server.request("tools/call", Some(json!({"name":name,"arguments":arguments})));
        assert!(response.get("error").is_some() || response["result"]["isError"] == true, "{response}");
    }
    assert!(!fx.home().join(".everett/cards/victim.md").exists());
    assert!(!fx.home().join(".everett/inbox/victim.done").exists());
    server.close();
}

#[test]
fn malformed_jsonrpc_ids_never_dispatch_tools() {
    let fx = home_with_claude_session();
    let mut server = Server::start(&fx, &[("EVERETT_SESSION_ID", "victim")]);
    for id in [json!(true), json!({}), json!([])] {
        let response = server.raw(&json!({"jsonrpc":"2.0","id":id,"method":"tools/call", "params":{
            "name":"everett_card","arguments":{"what":"invalid frame","state":"working","next":"done"}
        }}).to_string());
        assert_eq!(response["error"]["code"], -32600, "{response}");
        assert_eq!(response["id"], Value::Null, "{response}");
    }
    assert!(!fx.home().join(".everett/cards/victim.md").exists());
    assert_eq!(server.request("ping", None)["result"], json!({}));
    server.close();
}

#[test]
fn explicit_remote_dispatch_is_bounded_and_gateway_cannot_gain_remote_power() {
    let fx = home_with_claude_session();
    let mut server = Server::start(&fx, &[]);
    let tools = server.request("tools/list", None);
    let send = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "everett_send")
        .unwrap();
    assert_eq!(
        send["inputSchema"]["properties"]["idempotency_key"]["type"],
        "string"
    );
    for extra in [
        json!({"mode":"resume"}),
        json!({"spawn":true}),
        json!({"wait":1}),
    ] {
        let mut args =
            json!({"to":"remote/codex/session","text":"synthetic","idempotency_key":"test-retry"});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let result = server.call("everett_send", args);
        assert!(
            text(&result).contains("Remote sends are inbox-only")
                || text(&result).contains("Caller identity is not host-detected")
                || text(&result).contains("Cannot authenticate the calling session"),
            "{result}"
        );
    }
    server.close();
    let mut gateway = Server::start(
        &fx,
        &[
            ("EVERETT_GATEWAY_EXACT_IDS", "1"),
            ("EVERETT_SESSION_ID", "gateway"),
        ],
    );
    let result = gateway.call(
        "everett_send",
        json!({"to":"remote/codex/session","text":"synthetic","mode":"inbox"}),
    );
    assert!(
        text(&result).contains("Gateway remote destinations are not allowed"),
        "{result}"
    );
    gateway.close();
}
