// Each integration binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub struct Fixture {
    pub dir: tempfile::TempDir,
}

impl Fixture {
    pub fn home(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    /// Run the everett binary with HOME+EVERETT_HOME pointed at the fixture.
    pub fn run(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_everett"));
        cmd.args(args)
            .env("EVERETT_HOME", self.home())
            .env("HOME", self.home())
            .env("EVERETT_NOTIFY", "none")
            .env_remove("TYPESAFE_API_KEY")
            .env_remove("EVERETT_SEND");
        cmd.output().unwrap()
    }

    /// Same, with stdin attached and extra env.
    pub fn run_stdin(&self, args: &[&str], input: &str, extra_env: &[(&str, &str)]) -> Output {
        use std::io::Write;
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_everett"));
        cmd.args(args)
            .env("EVERETT_HOME", self.home())
            .env("HOME", self.home())
            .env("EVERETT_NOTIFY", "none")
            .env_remove("TYPESAFE_API_KEY")
            .env_remove("EVERETT_SEND")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    pub fn stdout(&self, out: &Output) -> String {
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    pub fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.home().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    /// `everett ls --json` parsed.
    pub fn ls_json(&self) -> Vec<serde_json::Value> {
        let out = self.run(&["ls", "--json"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "ls --json failed: {}",
            self.stdout(&out)
        );
        serde_json::from_str(&self.stdout(&out)).unwrap()
    }
}

pub fn fixture() -> Fixture {
    Fixture {
        dir: tempfile::tempdir().unwrap(),
    }
}

/// One JSON object per line.
pub fn jsonl(rows: &[serde_json::Value]) -> String {
    rows.iter().map(|r| r.to_string() + "\n").collect()
}

/// Copy a fixture transcript into a harness store path under the fixture home.
pub fn plant(fixture: &Fixture, name: &str, rel: &str) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fixture.write(rel, &std::fs::read_to_string(src).unwrap())
}

/// Stand-in for the Jev endpoint: answers every POST with `status` and `body`.
/// Returns the URL to pass as EVERETT_JEV_URL.
pub fn jev_stub(status: u16, body: &'static str) -> String {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut sink = vec![0u8; length];
            let _ = reader.read_exact(&mut sink);
            let _ = write!(
                stream,
                "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status,
                body.len(),
                body
            );
        }
    });
    url
}

/// Run `everett mcp` with an isolated env, send initialize plus each tools/call, return the call results.
pub fn mcp_calls(
    fixture: &Fixture,
    args: &[&str],
    env: &[(&str, &str)],
    calls: &[(&str, serde_json::Value)],
) -> Vec<serde_json::Value> {
    use std::io::{BufRead, BufReader, Write};
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_everett"));
    cmd.arg("mcp")
        .args(args)
        .env_clear()
        .env("HOME", fixture.home())
        .env("EVERETT_HOME", fixture.home())
        .env("EVERETT_NOTIFY", "none")
        .env("PATH", "/usr/bin:/bin")
        .current_dir(fixture.home())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut send = |msg: serde_json::Value| {
        stdin.write_all(format!("{}\n", msg).as_bytes()).unwrap();
        stdin.flush().unwrap();
    };
    send(serde_json::json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}));
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let mut results = Vec::new();
    for (i, (name, arguments)) in calls.iter().enumerate() {
        send(serde_json::json!({"jsonrpc":"2.0","id":i + 1,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        results.push(if response.get("result").is_some() { response["result"].clone() } else { response });
    }
    drop(stdin);
    let _ = child.wait();
    results
}

/// Text of an MCP tool result, parsed as JSON when it is JSON.
pub fn tool_json(result: &serde_json::Value) -> serde_json::Value {
    let text = result["content"][0]["text"].as_str().unwrap_or("");
    serde_json::from_str(text).unwrap_or_else(|_| serde_json::Value::String(text.to_string()))
}

/// Library-level tests share process env: hold this guard while HOME points at a fresh temp dir.
pub struct LibHome {
    pub dir: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl LibHome {
    pub fn path(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }
}

const SANDBOX_UNSET: &[&str] = &[
    "TYPESAFE_API_KEY",
    "EVERETT_VAULT",
    "EVERETT_VAULT_DIR",
    "EVERETT_ROUTER",
    "EVERETT_HARNESS",
    "EVERETT_SEND",
    "EVERETT_HOPS",
    "EVERETT_SESSION_ID",
    "EVERETT_MERGE_LLM",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDECODE",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_SESSION_ID",
    "CODEX_THREAD_ID",
    "HERMES_SESSION_ID",
    "PI_SESSION_FILE",
    "GROK_SESSION_ID",
    "EVERETT_NOTIFY_COMMAND",
    "EVERETT_ESCALATE_MINUTES",
];

pub fn lib_home() -> LibHome {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", dir.path());
    std::env::set_var("EVERETT_HOME", dir.path());
    std::env::set_var("EVERETT_NOTIFY", "none");
    for name in SANDBOX_UNSET {
        std::env::remove_var(name);
    }
    LibHome { dir, _guard: guard }
}
