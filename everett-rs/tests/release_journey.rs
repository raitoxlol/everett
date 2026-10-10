//! The release journey: CLI, all ten stdio MCP tools, routing, messaging, subscriptions and shared
//! memory over an isolated home. Set `EVERETT_VERIFY_BINARY` to check an installed or packaged binary.

use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, SystemTime};

const TOOLS: [&str; 10] = ["everett_card", "everett_core", "everett_event", "everett_inbox", "everett_learn",
    "everett_ls", "everett_route", "everett_send", "everett_subscribe", "everett_whoami"];

fn binary() -> PathBuf {
    std::env::var_os("EVERETT_VERIFY_BINARY").map(PathBuf::from).unwrap_or_else(|| env!("CARGO_BIN_EXE_everett").into())
}

struct Home {
    root: tempfile::TempDir,
    bin: PathBuf,
}

impl Home {
    fn new() -> Self {
        let root = tempfile::Builder::new().prefix("everett-release-check-").tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let home = Home { root, bin };
        home.stub("claude", "#!/bin/sh\n# Detection only: a model call fails this verification.\nexit 88\n");
        home
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn stub(&self, name: &str, script: &str) {
        let path = self.bin.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn command(&self, args: &[&str], caller: &str) -> Command {
        let mut cmd = Command::new(binary());
        cmd.args(args).current_dir(self.path()).env_clear()
            .env("HOME", self.path()).env("EVERETT_HOME", self.path())
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("EVERETT_NOTIFY", "none").env("EVERETT_ROUTER", "local").env("CI", "1");
        if !caller.is_empty() {
            cmd.env("EVERETT_SESSION_ID", caller);
        }
        cmd
    }

    fn run(&self, args: &[&str], caller: &str) -> String {
        let out = self.command(args, caller).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }

    fn json(&self, args: &[&str], caller: &str) -> Value {
        serde_json::from_str(&self.run(args, caller)).unwrap()
    }

    fn seed(&self, sid: &str, cwd: &Path, text: &str) {
        let path = self.path().join(".claude/projects/verification").join(format!("{sid}.jsonl"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let rows = [json!({"type": "user", "sessionId": sid, "cwd": cwd, "message": {"role": "user", "content": text}}),
            json!({"type": "assistant", "sessionId": sid, "message": {"role": "assistant", "content": [{"type": "text", "text": "Ready for review."}]}})];
        std::fs::write(&path, rows.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
        age(&path, 120);
    }
}

fn age(path: &Path, secs: u64) {
    std::fs::File::options().write(true).open(path).unwrap().set_modified(SystemTime::now() - Duration::from_secs(secs)).unwrap();
}

struct Wire {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    seen: BTreeSet<String>,
}

impl Wire {
    fn start(home: &Home, caller: &str) -> Self {
        let mut cmd = home.command(&["mcp"], caller);
        cmd.env("EVERETT_HARNESS_NAME", "claude").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut wire = Wire { child, stdin, stdout, next_id: 0, seen: BTreeSet::new() };
        let info = wire.request("initialize", Some(json!({"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "everett-release-check", "version": "1"}})));
        assert!(info["capabilities"].get("tools").is_some(), "{info}");
        wire.line(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        let names: BTreeSet<String> = wire.request("tools/list", None)["tools"].as_array().unwrap()
            .iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, TOOLS.iter().map(|t| t.to_string()).collect());
        wire
    }

    fn line(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Option<Value>) -> Value {
        self.next_id += 1;
        let mut message = json!({"jsonrpc": "2.0", "id": self.next_id, "method": method});
        if let Some(params) = params {
            message["params"] = params;
        }
        self.line(&message);
        let mut buf = String::new();
        self.stdout.read_line(&mut buf).unwrap();
        let response: Value = serde_json::from_str(&buf).unwrap_or_else(|e| panic!("{method}: {e}: {buf:?}"));
        assert_eq!(response["id"], self.next_id);
        assert!(response.get("error").is_none(), "{method}: {response}");
        response["result"].clone()
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let result = self.request("tools/call", Some(json!({"name": name, "arguments": arguments})));
        assert_ne!(result["isError"], true, "{name} failed: {result}");
        self.seen.insert(name.to_string());
        result["structuredContent"].clone()
    }

    fn close(mut self) -> BTreeSet<String> {
        drop(self.stdin.take());
        assert!(self.child.wait().unwrap().success(), "MCP server did not exit cleanly");
        self.seen
    }
}

fn ids(sessions: &Value) -> BTreeSet<String> {
    sessions.as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap().to_string()).collect()
}

#[test]
fn release_journey_over_cli_and_all_ten_mcp_tools() {
    let home = Home::new();
    let root = home.path().to_path_buf();
    assert!(home.run(&["--version"], "").starts_with("everett "));
    assert!(home.run(&["doctor"], "").contains("10 tools over stdio"));
    home.run(&["onboard", "--yes", "--no-backfill"], "");
    assert!(root.join(".claude.json").is_file(), "onboarding missed the CLI without a session store");
    assert!(!home.run(&["doctor"], "").contains("setup needs attention"));

    let (sender_dir, worker_dir) = (root.join("docs"), root.join("uploads"));
    std::fs::create_dir_all(&sender_dir).unwrap();
    std::fs::create_dir_all(&worker_dir).unwrap();
    home.seed("verify-sender", &sender_dir, "Document email templates");
    let task = "Fix upload retries for storage client";
    home.seed("verify-worker", &worker_dir, task);
    assert_eq!(home.json(&["ls", "--json"], "").as_array().unwrap().len(), 2);
    let routed = home.json(&["route", task, "--router", "local", "--json"], "");
    assert_eq!((routed["decision"].clone(), routed["session"]["id"].clone()), (json!("SESSION"), json!("verify-worker")));

    let mut wire = Wire::start(&home, "verify-sender");
    assert_eq!(wire.call("everett_whoami", json!({}))["session_id"], "verify-sender");
    for (sid, what) in [("verify-sender", "Email templates"), ("verify-worker", "Upload retries")] {
        wire.call("everett_card", json!({"session_id": sid, "what": what, "state": "Reviewing", "next": "Run checks"}));
        assert!(root.join(".everett/cards").join(format!("{sid}.md")).is_file());
    }
    let listed = wire.call("everett_ls", json!({}));
    assert_eq!(ids(&listed["sessions"]), ["verify-sender", "verify-worker"].map(String::from).into());
    let routed = wire.call("everett_route", json!({"text": task, "router": "local", "session_id": "verify-sender"}));
    assert_eq!(routed["session"]["id"], "verify-worker");

    let sent = wire.call("everett_send", json!({"text": "Review the upload tests", "to": "verify-worker", "mode": "inbox"}));
    let received = wire.call("everett_inbox", json!({"session_id": "verify-worker"}))["messages"].clone();
    assert!(received.as_array().unwrap().len() == 1 && received[0]["id"] == sent["message_id"], "{received}");
    assert_eq!(wire.call("everett_inbox", json!({"session_id": "verify-worker"}))["messages"], json!([]));
    home.run(&["reply", sent["message_id"].as_str().unwrap(), "Upload tests pass"], "verify-worker");
    let replies = wire.call("everett_inbox", json!({}))["messages"].clone();
    assert_eq!((replies[0]["text"].clone(), replies[0]["reply_to"].clone()), (json!("Upload tests pass"), sent["message_id"].clone()));
    let cli_sent = home.json(&["send", "Check the retry cap", "--to", "verify-worker", "--mode", "inbox", "--json"], "verify-sender");
    wire.call("everett_send", json!({"reply_to": cli_sent["message_id"], "text": "Cap is five", "session_id": "verify-worker"}));
    assert_eq!(home.json(&["inbox", "--session", "verify-sender", "--json"], "")["messages"][0]["text"], "Cap is five");

    wire.call("everett_subscribe", json!({"target": "verify-worker"}));
    wire.call("everett_event", json!({"kind": "done", "message": "Upload tests passed", "session_id": "verify-worker"}));
    let updates = wire.call("everett_inbox", json!({}))["messages"].clone();
    assert!(updates.as_array().unwrap().iter().any(|m| m["kind"] == "event" && m["text"].as_str().unwrap().contains("Upload tests passed")), "{updates}");
    let sessions = wire.call("everett_ls", json!({}))["sessions"].clone();
    let worker = sessions.as_array().unwrap().iter().find(|s| s["id"] == "verify-worker").unwrap();
    assert_eq!(worker["state_kind"], "done");
    assert_eq!(home.json(&["events", "--json"], "").as_array().unwrap().last().unwrap()["kind"], "done");

    let fact = "Use bounded retries for uploads";
    wire.call("everett_learn", json!({"fact": fact, "scope": "global"}));
    home.run(&["learn", "Retry caps are five", "--scope", "project", "--project", "verification"], "");
    assert_eq!(wire.call("everett_core", json!({"project": "verification"}))["pending_learnings"], 2);
    home.run(&["trunk", "merge", "--llm", "none"], "");
    let shared = wire.call("everett_core", json!({"project": "verification"}));
    let core = shared["core"].as_str().unwrap();
    assert!(core.contains(fact) && core.contains("Retry caps are five") && shared["pending_learnings"] == 0, "{shared}");
    assert!(root.join(".everett/core/core.md").is_file() && root.join(".everett/core/projects/verification.md").is_file());

    // A child that reads stdin must see EOF while the MCP pipe stays open.
    home.stub("codex", "#!/bin/sh\nin=$(cat)\nprintf '{\"stdin\":\"%s\",\"thread_id\":\"verify-spawned\"}\\n' \"$in\"\n");
    let peer = root.join(".codex/sessions/2026/01/01/verify-codex.jsonl");
    std::fs::create_dir_all(peer.parent().unwrap()).unwrap();
    let codex_task = "Repair sqlite backup restoration";
    std::fs::write(&peer, format!("{}\n{}\n",
        json!({"type": "session_meta", "payload": {"id": "verify-codex", "cwd": worker_dir, "source": "cli"}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": codex_task}]}}))).unwrap();
    age(&peer, 96 * 3600);
    std::fs::write(root.join(".codex/session_index.jsonl"), format!("{}\n", json!({"id": "verify-codex", "thread_name": "Backup restoration"}))).unwrap();
    assert_eq!(wire.call("everett_ls", json!({"harness": "codex"}))["sessions"], json!([]));
    assert_eq!(wire.call("everett_route", json!({"text": codex_task, "router": "local", "hours": 168}))["session"]["id"], "verify-codex");
    let resumed = wire.call("everett_send", json!({"text": "Check stdin", "to": "Backup restoration", "hours": 168, "mode": "resume"}));
    let reply: Value = serde_json::from_str(resumed["reply"].as_str().unwrap()).unwrap();
    assert_eq!(reply["stdin"], "");
    let spawned = wire.call("everett_send", json!({"text": "Calibrate neutrino spectrometer", "spawn": true, "harness": "codex", "dir": worker_dir}));
    assert_eq!(spawned["spawned"], true, "{spawned}");
    let reply: Value = serde_json::from_str(spawned["reply"].as_str().unwrap()).unwrap();
    assert_eq!(reply["stdin"], "");
    assert_eq!(wire.request("ping", None), json!({}));
    assert_eq!(wire.close(), TOOLS.iter().map(|t| t.to_string()).collect());
}
