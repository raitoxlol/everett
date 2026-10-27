mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct Dashboard(Child);

#[test]
fn authored_cards_show_separate_identity_tags_and_escape_untrusted_metadata() {
    let card = everett::messaging::AuthorCard {
        owner: "account".into(), author: "alice<&>".into(), agent: "agent-1".into(),
        session: "session-1".into(), harness: "codex".into(),
        card: "Repair login\n<script>alert(1)</script>".into(), updated_at: 1_000.0,
        target: "id:agent-1/codex/session-1".into(),
    };
    let local = everett::session::Session::new("claude", "local-1", "", "local task", "", 0.0);
    let page = everett::dashboard::render_with_authors(&[local], &[card], "", 72.0, 10.0);
    for text in ["Author: alice&lt;&amp;&gt;", "Agent: agent-1", "Session: session-1",
        "Agent: local", "Session: local-1", "Repair login\n&lt;script&gt;alert(1)&lt;/script&gt;",
        "id:agent-1/codex/session-1", "Copy ping command", "No queue fallback"] {
        assert!(page.contains(text), "missing {text}");
    }
    assert!(!page.contains("<script>alert(1)</script>"));
    assert!(!page.contains("--mode inbox</code>"));
    let unavailable = everett::dashboard::render_with_authors(&[], &[], "Directory unavailable <error>", 72.0, 10.0);
    assert!(unavailable.contains("Directory unavailable &lt;error&gt;"));
}

impl Drop for Dashboard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn request(port: u16, method: &str, path: &str, host: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

#[test]
fn dashboard_serves_local_sessions_without_mutating_stores() {
    let fixture = common::fixture();
    let transcript = common::plant(
        &fixture,
        "claude.jsonl",
        ".claude/projects/app/claude.jsonl",
    );
    let before = std::fs::read(&transcript).unwrap();
    let card = format!(
        "API: repair the login retry path\n\n{}\nEND-OF-FULL-CARD & <safe>",
        "Long card line\n".repeat(60)
    );
    fixture.write(".everett/cards/c-1.md", &card);
    let listed = fixture.run(&["ls", "--json"]);
    assert!(listed.status.success());
    let sessions: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert!(!sessions[0]["card"]
        .as_str()
        .unwrap()
        .contains("END-OF-FULL-CARD"));
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let child = Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(["dashboard", "--no-open", "--port", &port.to_string()])
        .env("HOME", fixture.home())
        .env("EVERETT_HOME", fixture.home())
        .env("EVERETT_NOTIFY", "none")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _dashboard = Dashboard(child);
    let host = format!("127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..100 {
        if request(port, "GET", "/healthz", &host).is_ok() {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ready, "dashboard did not become ready");
    let page = request(port, "GET", "/", &host).unwrap();
    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains("API: repair the login retry path"));
    assert!(page.contains("API: repair the login retry path\n\nLong card line\n"));
    assert!(page.contains("END-OF-FULL-CARD &amp; &lt;safe&gt;"));
    assert!(page.contains("Copy route command"));
    assert!(page.contains("everett events --session c-1"));
    assert!(page.contains("Claude Code"));
    assert!(page.contains("Content-Security-Policy:"));
    assert!(request(port, "POST", "/", &host)
        .unwrap()
        .starts_with("HTTP/1.1 405"));
    assert!(request(port, "GET", "/", "untrusted.example")
        .unwrap()
        .starts_with("HTTP/1.1 403"));
    assert!(request(port, "GET", "/missing", &host)
        .unwrap()
        .starts_with("HTTP/1.1 404"));
    assert_eq!(std::fs::read(transcript).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(fixture.home().join(".everett/cards/c-1.md")).unwrap(),
        card
    );
}

#[test]
fn dashboard_rejects_invalid_parameters() {
    let fixture = common::fixture();
    for args in [
        vec!["dashboard", "--port", "0", "--no-open"],
        vec!["--hours", "NaN", "dashboard", "--no-open"],
    ] {
        let output = fixture.run(&args);
        assert_eq!(output.status.code(), Some(2));
    }
}
