mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct Dashboard(Child);

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
    fixture.write(".everett/cards/c-1.md", "API: repair the login retry path");
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
