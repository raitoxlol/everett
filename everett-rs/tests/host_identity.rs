//! Host binding for harnesses whose MCP launch has no session id (Pi, OMP, Devin, Hermes).
mod common;

use common::{fixture, lib_home, tool_json, Fixture};
use everett::host::{bind, find_host_in, harness_of, parse_etime, Host, Proc};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn procs(rows: &[(i32, i32, &str)]) -> HashMap<i32, Proc> {
    rows.iter()
        .map(|(pid, ppid, args)| (*pid, Proc { pid: *pid, ppid: *ppid, age: 60.0, args: args.to_string() }))
        .collect()
}

#[test]
fn harness_of_reads_direct_and_interpreted_command_lines() {
    assert_eq!(harness_of("/usr/local/bin/pi"), Some("pi"));
    assert_eq!(harness_of("node /opt/homebrew/bin/omp --resume"), Some("omp"));
    assert_eq!(harness_of("python3.11 /home/u/.local/bin/hermes chat"), Some("hermes"));
    assert_eq!(harness_of("python3 -m hermes_cli.main"), Some("hermes"));
    assert_eq!(harness_of("devin"), Some("devin"));
    assert_eq!(harness_of("node /srv/app.js"), None);
    assert_eq!(parse_etime("01:02"), Some(62.0));
    assert_eq!(parse_etime("2-03:00:01"), Some(2.0 * 86400.0 + 3.0 * 3600.0 + 1.0));
}

#[test]
fn find_host_walks_up_and_fails_closed_on_the_wrong_harness() {
    let table = procs(&[(10, 1, "node /usr/bin/omp"), (20, 10, "sh -c everett mcp"), (30, 20, "everett mcp")]);
    let host = find_host_in(&table, 20, Some("omp")).unwrap().unwrap();
    assert_eq!((host.harness.as_str(), host.pid), ("omp", 10));
    let err = find_host_in(&table, 20, Some("pi")).unwrap_err();
    assert!(err.contains("--harness pi") && err.contains("omp"), "{err}");
    let none = procs(&[(20, 1, "bash"), (30, 20, "everett mcp")]);
    assert!(find_host_in(&none, 30, Some("devin")).unwrap_err().contains("no devin process"));
    assert!(find_host_in(&none, 30, None).unwrap().is_none());
    let claude = procs(&[(10, 1, "claude"), (30, 10, "everett mcp")]);
    assert!(find_host_in(&claude, 30, None).unwrap().is_none(), "claude binds through its own env");
}

#[test]
fn bind_prefers_the_live_record_for_this_host_pid() {
    let home = lib_home();
    let live = home.path().join(".everett").join("inbox").join("live");
    std::fs::create_dir_all(&live).unwrap();
    let me = std::process::id() as i32;
    let now = everett::session::now();
    std::fs::write(live.join("old-sess.json"), json!({"pid": me, "ts": now - 30.0}).to_string()).unwrap();
    std::fs::write(live.join("new-sess.json"), json!({"pid": me, "ts": now - 1.0}).to_string()).unwrap();
    std::fs::write(live.join("other.json"), json!({"pid": me + 1, "ts": now}).to_string()).unwrap();
    let host = Host { harness: "omp".into(), pid: me, started: now - 60.0, cwd: home.path().to_string_lossy().to_string() };
    let (sid, how) = bind(&host, &HashMap::new()).unwrap();
    assert_eq!(sid, "new-sess");
    assert!(how.contains("live record"));
    let stranger = Host { pid: me + 2, ..host };
    let err = bind(&stranger, &HashMap::new()).unwrap_err();
    assert!(err.contains("no omp session"), "{err}");
}

/// initialize + tools/call over stdio for any launcher, returning each raw response.
fn mcp_via(fx: &Fixture, program: &std::path::Path, args: &[&str], calls: &[(&str, Value)]) -> Vec<Value> {
    let mut child = spawn_retrying(fx, program, args);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut send = |msg: Value| {
        stdin.write_all(format!("{}\n", msg).as_bytes()).unwrap();
        stdin.flush().unwrap();
    };
    send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}));
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let mut out = Vec::new();
    for (i, (name, arguments)) in calls.iter().enumerate() {
        send(json!({"jsonrpc":"2.0","id":i + 1,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        out.push(serde_json::from_str(&line).unwrap());
    }
    drop(stdin);
    let _ = child.wait();
    out
}

/// Spawning a script another test thread just wrote can hit ETXTBSY while a fork holds it open.
fn spawn_retrying(fx: &Fixture, program: &std::path::Path, args: &[&str]) -> std::process::Child {
    for _ in 0..100 {
        match Command::new(program)
            .args(args)
            .env_clear()
            .env("HOME", fx.home())
            .env("EVERETT_HOME", fx.home())
            .env("EVERETT_NOTIFY", "none")
            .env("PATH", "/usr/bin:/bin")
            .env("EVERETT_BIN", env!("CARGO_BIN_EXE_everett"))
            .current_dir(fx.home())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Err(e) if e.raw_os_error() == Some(26) => std::thread::sleep(std::time::Duration::from_millis(20)),
            other => return other.unwrap(),
        }
    }
    panic!("{} stayed busy", program.display());
}

/// A shebang script named after a harness: `ps` shows `/bin/sh .../<name>`, like a node-launched
/// CLI, so it is the nearest host above the server whatever runs the test suite.
fn fake_host(fx: &Fixture, name: &str, body: &str) -> std::path::PathBuf {
    let path = fx.write(&format!("bin/{name}"), &format!("#!/bin/sh\n{body}"));
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn fake_omp(fx: &Fixture, bind_id: Option<&str>) -> std::path::PathBuf {
    let bind = bind_id.map(|id| format!("\"$EVERETT_BIN\" hook omp_bind {}\n", id)).unwrap_or_default();
    fake_host(fx, "omp", &format!("{}\"$EVERETT_BIN\" mcp --harness omp\n", bind))
}

#[test]
fn mcp_harness_binds_through_the_omp_session_start_hook() {
    let fx = fixture();
    let omp = fake_omp(&fx, Some("omp-sess-1"));
    let out = mcp_via(&fx, &omp, &[], &[
        ("everett_whoami", json!({})),
        ("everett_card", json!({"what": "binding test", "state": "working", "next": "none"})),
        ("everett_card", json!({"session_id": "someone-else", "what": "x", "state": "y", "next": "z"})),
    ]);
    let who = tool_json(&out[0]["result"]);
    assert_eq!(who["session_id"], "omp-sess-1", "{who}");
    assert_eq!(who["harness"], "omp");
    assert!(who["source"].as_str().unwrap().contains("live record"), "{who}");
    assert_eq!(out[1]["result"]["isError"], false, "{}", out[1]);
    assert!(fx.home().join(".everett/cards/omp-sess-1.md").exists());
    assert_eq!(out[2]["result"]["isError"], true, "spoofed id must not be accepted: {}", out[2]);
    assert!(out[2]["result"]["content"][0]["text"].as_str().unwrap().contains("conflicts"));
}

#[test]
fn unbound_callers_get_readable_tool_errors_not_protocol_errors() {
    let fx = fixture();
    let omp = fake_omp(&fx, None);
    let out = mcp_via(&fx, &omp, &[], &[
        ("everett_whoami", json!({})),
        ("everett_card", json!({"what": "w", "state": "s", "next": "n"})),
        ("everett_inbox", json!({"session_id": "claimed-id"})),
    ]);
    let who = tool_json(&out[0]["result"]);
    assert_eq!(who["session_id"], "");
    assert!(who["note"].as_str().unwrap().contains("install-hooks --omp"), "{who}");
    for resp in &out[1..] {
        assert!(resp.get("error").is_none(), "identity failure must not be a JSON-RPC error: {resp}");
        assert_eq!(resp["result"]["isError"], true, "{resp}");
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Cannot tell which session you are"), "{text}");
    }
    let pi = fake_host(&fx, "pi", "\"$EVERETT_BIN\" mcp --harness devin\n");
    let wrong = mcp_via(&fx, &pi, &[], &[("everett_card", json!({"what": "w", "state": "s", "next": "n"}))]);
    assert!(wrong[0].get("error").is_none(), "{}", wrong[0]);
    assert_eq!(wrong[0]["result"]["isError"], true, "{}", wrong[0]);
    let text = wrong[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("--harness devin") && text.contains("pi"), "{text}");
}

#[test]
fn schemas_no_longer_advertise_session_id() {
    let fx = fixture();
    let out = fx.run_stdin(
        &["mcp"],
        &format!("{}\n{}\n", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}), json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})),
        &[],
    );
    let text = fx.stdout(&out);
    let list: Value = serde_json::from_str(text.lines().nth(1).unwrap()).unwrap();
    for tool in list["result"]["tools"].as_array().unwrap() {
        assert!(tool["inputSchema"]["properties"].get("session_id").is_none(), "{}", tool["name"]);
    }
}

#[test]
fn install_writes_host_binding_launchers_and_hooks() {
    let fx = fixture();
    let path = format!("{}:/usr/bin:/bin", fx.home().join("bin").display());
    let run = |args: &[&str]| {
        let out = fx.run_stdin(args, "", &[("PATH", &path)]);
        assert!(out.status.success(), "{:?}: {}", args, String::from_utf8_lossy(&out.stderr));
        fx.stdout(&out)
    };
    run(&["install-hooks", "--apply"]);
    assert!(!fx.home().join(".omp").exists(), "OMP files written without OMP installed");
    assert!(!fx.home().join(".pi").exists());

    run(&["install-hooks", "--pi", "--apply"]);
    let ext = std::fs::read_to_string(fx.home().join(".pi/agent/extensions/everett.ts")).unwrap();
    assert!(ext.contains("registerMcpServer") && ext.contains("EVERETT_SESSION_ID") && ext.contains("pi_bind"));

    run(&["install-hooks", "--devin", "--apply"]);
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(fx.home().join(".config/devin/config.json")).unwrap()).unwrap();
    assert!(cfg["hooks"]["SessionStart"].to_string().contains("devin_bind"), "{cfg}");

    run(&["install-mcp", "--omp", "--apply"]);
    let mcp: Value = serde_json::from_str(&std::fs::read_to_string(fx.home().join(".omp/agent/mcp.json")).unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["everett"]["args"], json!(["mcp", "--harness", "omp"]));
    assert!(run(&["install-mcp", "--omp", "--apply"]).contains("already registered"));

    let hermes = run(&["install-mcp", "--hermes"]);
    assert!(hermes.contains("mcp_servers:") && hermes.contains("\"--harness\", \"hermes\""), "{hermes}");

    let bound = fx.run_stdin(&["hook", "devin_bind"], &json!({"session_id": "devin-sess"}).to_string(), &[]);
    assert!(bound.status.success());
    assert!(fx.home().join(".everett/inbox/live/devin-sess.json").exists());
}

#[test]
fn install_hooks_writes_omp_files_only_when_omp_is_on_path_or_set_up() {
    let fx = fixture();
    let path = format!("{}:/usr/bin:/bin", fx.home().join("bin").display());
    let install = || {
        let out = fx.run_stdin(&["install-hooks", "--apply"], "", &[("PATH", &path)]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        fx.stdout(&out)
    };
    assert!(!install().contains("omp:"));
    assert!(!fx.home().join(".omp").exists());
    fake_host(&fx, "omp", "exit 0\n");
    assert!(install().contains("omp: wrote"));
    assert!(fx.home().join(".omp/agent/extensions/everett.ts").exists());
    assert!(!fx.home().join(".pi").exists());
}
