use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_everett");
const MESSAGE: &str = "private message $(touch /nope) 'quoted'\nsecond line";

fn write(path: impl AsRef<Path>, text: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn key(path: &Path) {
    assert!(Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(path)
        .status()
        .unwrap()
        .success());
}

fn fingerprint(path: &Path) -> String {
    let out = Command::new("ssh-keygen")
        .args(["-l", "-E", "sha256", "-f"])
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .into()
}

fn checked(out: Output) -> Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

struct Pair {
    local: tempfile::TempDir,
    remote: tempfile::TempDir,
    fixture_bin: tempfile::TempDir,
    fingerprint: String,
    device_fingerprint: String,
}

impl Pair {
    fn new() -> Self {
        let local = tempfile::tempdir().unwrap();
        let remote = tempfile::tempdir().unwrap();
        let fixture_bin = tempfile::tempdir().unwrap();
        key(&remote.path().join("host_key"));
        let fingerprint = fingerprint(&remote.path().join("host_key"));
        let wrapper = fixture_bin.path().join("ssh");
        write(
            &wrapper,
            &format!(
                "#!/bin/sh\nexec {} --fake-ssh \"$@\"\n",
                everett::proc::shlex_join(&[std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into()])
            ),
        );
        fs::set_permissions(wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        write(
            remote.path().join(".ssh/authorized_keys"),
            "# unrelated comment\nssh-ed25519 foreign-key foreign\n",
        );
        let mut p = Self {
            local,
            remote,
            fixture_bin,
            fingerprint,
            device_fingerprint: String::new(),
        };
        p.device_fingerprint = checked(p.remote_run(&["peer", "identity"], ""))["fingerprint"]
            .as_str()
            .unwrap()
            .into();
        checked(p.remote_run(
            &[
                "external",
                "add",
                "--id",
                "worker",
                "--harness",
                "openai-dot",
                "--title",
                "Remote worker",
                "--cwd",
                "/remote/work",
            ],
            "",
        ));
        p
    }

    fn command(&self, remote: bool) -> Command {
        let mut cmd = Command::new(BIN);
        let home = if remote {
            self.remote.path()
        } else {
            self.local.path()
        };
        cmd.env("HOME", home)
            .env("EVERETT_HOME", home)
            .env("EVERETT_NOTIFY", "none")
            .env_remove("EVERETT_SESSION_ID")
            .env_remove("EVERETT_HARNESS_NAME")
            .env_remove("EVERETT_HOPS")
            .env_remove("EVERETT_SEND")
            .env_remove("TYPESAFE_API_KEY")
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.fixture_bin.path().display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("FAKE_REMOTE", self.remote.path())
            .env("FAKE_LOCAL", self.local.path());
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(false).args(args).output().unwrap()
    }

    fn remote_run(&mut self, args: &[&str], input: &str) -> Output {
        let mut cmd = self.command(true);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn remote_mcp(&self, input: &str) -> Output {
        let mut cmd = self.command(true);
        cmd.arg("mcp")
            .env("EVERETT_SESSION_ID", "worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn add(&self) -> Output {
        self.run(&[
            "peer",
            "add",
            "linux",
            "--ssh",
            "owner@box",
            "--binary",
            BIN,
            "--host-key",
            &self.fingerprint,
            "--device-key",
            &self.device_fingerprint,
        ])
    }

    fn send(&self, key: &str) -> Value {
        checked(self.run(&[
            "send",
            "--to",
            "linux/openai-dot/worker",
            "--json",
            "--idempotency-key",
            key,
            MESSAGE,
        ]))
    }

    fn status(&self, mid: &str) -> Value {
        checked(self.run(&["peer", "status", "linux", mid]))
    }

    fn client(&self) -> String {
        fs::read_to_string(self.local.path().join(".everett/peer/device_id")).unwrap()
    }

    fn rpc(&mut self, input: Value) -> Value {
        let client = self.client();
        checked(self.remote_run(
            &["peer", "serve", "--client", &client],
            &(input.to_string() + "\n"),
        ))
    }
}

fn fake_ssh(args: &[String]) -> i32 {
    let remote = PathBuf::from(std::env::var("FAKE_REMOTE").unwrap());
    let local = PathBuf::from(std::env::var("FAKE_LOCAL").unwrap());
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(local.join("ssh-argv.jsonl"))
        .unwrap();
    writeln!(log, "{}", json!(args)).unwrap();
    if remote.join("offline").exists() {
        eprintln!("fixture: connection refused");
        return 255;
    }
    if remote.join("hang").exists() {
        std::thread::sleep(Duration::from_secs(60));
        return 255;
    }
    let option = |prefix: &str| args.iter().find_map(|a| a.strip_prefix(prefix));
    let pin = PathBuf::from(option("UserKnownHostsFile=").unwrap());
    let host_public = fs::read_to_string(remote.join("host_key.pub")).unwrap();
    let public = host_public
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    let existing = fs::read_to_string(&pin).unwrap_or_default();
    if existing.is_empty() {
        assert_eq!(option("StrictHostKeyChecking="), Some("accept-new"));
        write(&pin, &format!("box {public}\n"));
    } else if !existing.contains(&public) {
        eprintln!("WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!");
        return 255;
    }
    if args.last().map(String::as_str) == Some("true") {
        return 0;
    }
    let mut cmd = Command::new(BIN);
    cmd.env("HOME", &remote)
        .env("EVERETT_HOME", &remote)
        .env_remove("EVERETT_SESSION_ID")
        .env_remove("EVERETT_HARNESS_NAME")
        .env_remove("EVERETT_HOPS");
    if args.last().unwrap().ends_with(" peer identity") {
        cmd.args(["peer", "identity"]);
    } else if args.last().unwrap().ends_with(" peer install") {
        assert_eq!(
            args.last().unwrap(),
            &format!("{} peer install", everett::proc::shlex_join(&[BIN.into()]))
        );
        cmd.args(["peer", "install"]);
    } else {
        if remote.join("ignore-authorized-keys").exists() {
            eprintln!("permission denied: AuthorizedKeysFile fixture");
            return 255;
        }
        assert_eq!(option("StrictHostKeyChecking="), Some("yes"));
        assert_eq!(option("IdentityAgent="), Some("none"));
        assert_eq!(option("IdentitiesOnly="), Some("yes"));
        assert_eq!(option("ClearAllForwardings="), Some("yes"));
        assert!(!args.iter().any(|a| a.contains(MESSAGE)));
        let client = fs::read_to_string(local.join(".everett/peer/device_id")).unwrap();
        let keys = fs::read_to_string(remote.join(".ssh/authorized_keys")).unwrap();
        let public = fs::read_to_string(local.join(".everett/peer/id_ed25519.pub")).unwrap();
        let line = keys
            .lines()
            .find(|l| l.ends_with(&format!("# everett-peer:{client}")));
        let Some(line) = line else {
            eprintln!("permission denied");
            return 255;
        };
        assert!(line.contains(public.split_whitespace().nth(1).unwrap()));
        assert!(line.contains("no-pty,no-port-forwarding,no-agent-forwarding,no-X11-forwarding"));
        cmd.args(["peer", "serve", "--client", &client]);
    }
    cmd.status().unwrap().code().unwrap_or(1)
}

fn pairing_pins_and_restricts_real_authorized_file() {
    let p = Pair::new();
    checked(p.add());
    checked(p.add());
    let keys = fs::read_to_string(p.remote.path().join(".ssh/authorized_keys")).unwrap();
    assert!(keys.starts_with("# unrelated comment\nssh-ed25519 foreign-key foreign\n"));
    assert_eq!(keys.matches("# everett-peer:").count(), 1);
    assert!(keys.contains(&format!(
        "command=\"{BIN} peer serve --client {}\"",
        p.client()
    )));
    for rel in [".ssh/authorized_keys", ".everett/peer/id_ed25519"] {
        assert_eq!(
            fs::metadata(p.remote.path().join(rel))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(
        fs::metadata(p.remote.path().join(".ssh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(!p
        .remote
        .path()
        .join(".everett/peer/authorized_keys")
        .exists());
    let listed = checked(p.run(&["ls", "--peer", "linux", "--json"]));
    assert_eq!(listed[0]["id"], "linux/openai-dot/worker");
    assert_eq!(listed[0]["device"], "linux");
    assert!(checked(p.run(&["peer", "card", "linux/openai-dot/worker"]))
        .get("card")
        .is_some());
    checked(p.run(&["peer", "remove", "linux"]));
    assert_eq!(
        fs::read_to_string(p.remote.path().join(".ssh/authorized_keys")).unwrap(),
        "# unrelated comment\nssh-ed25519 foreign-key foreign\n"
    );
    assert_eq!(checked(p.run(&["peer", "list"])), json!({}));
}

fn mismatches_refuse_first_pair_and_changed_host() {
    let p = Pair::new();
    let out = p.run(&[
        "peer",
        "add",
        "linux",
        "--ssh",
        "owner@box",
        "--binary",
        BIN,
        "--host-key",
        "SHA256:wrong",
        "--device-key",
        &p.device_fingerprint,
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!p.local.path().join(".everett/peer/linux.json").exists());
    assert!(
        !fs::read_to_string(p.remote.path().join(".ssh/authorized_keys"))
            .unwrap()
            .contains("everett-peer:")
    );
    let out = p.run(&[
        "peer",
        "add",
        "linux",
        "--ssh",
        "owner@box",
        "--binary",
        BIN,
        "--host-key",
        &p.fingerprint,
        "--device-key",
        "SHA256:wrong",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        !fs::read_to_string(p.remote.path().join(".ssh/authorized_keys"))
            .unwrap()
            .contains("everett-peer:")
    );
    checked(p.add());
    let old_pin = fs::read(p.local.path().join(".everett/peer/linux.known_hosts")).unwrap();
    key(&p.remote.path().join("new_host"));
    fs::copy(
        p.remote.path().join("new_host.pub"),
        p.remote.path().join("host_key.pub"),
    )
    .unwrap();
    let out = p.run(&["ls", "--peer", "linux", "--json"]);
    assert_eq!(out.status.code(), Some(7));
    assert!(String::from_utf8_lossy(&out.stderr).contains("host-key mismatch"));
    assert_eq!(
        fs::read(p.local.path().join(".everett/peer/linux.known_hosts")).unwrap(),
        old_pin
    );
    assert_eq!(p.add().status.code(), Some(7));
}

fn incomplete_pairing_and_local_revocation_fail_closed() {
    let mut p = Pair::new();
    write(p.remote.path().join("ignore-authorized-keys"), "");
    assert_eq!(p.add().status.code(), Some(7));
    assert!(!p.local.path().join(".everett/peer/linux.json").exists());
    fs::remove_file(p.remote.path().join("ignore-authorized-keys")).unwrap();
    checked(p.add());
    let client = p.client();
    checked(p.remote_run(&["peer", "revoke", &client], ""));
    assert_eq!(p.run(&["ls", "--peer", "linux"]).status.code(), Some(7));
    assert!(
        fs::read_to_string(p.remote.path().join(".ssh/authorized_keys"))
            .unwrap()
            .contains("foreign-key")
    );
}

fn convex_alias_rejects_ssh_before_identity_pin_or_remote_key_side_effects() {
    let p = Pair::new();
    write(p.local.path().join(".everett/relay/peers/linux.json"), "{}");
    let authorized = p.remote.path().join(".ssh/authorized_keys");
    let before = fs::read(&authorized).unwrap();
    let rejected = p.add();
    assert_eq!(rejected.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("alias already selects Convex"));
    for name in ["linux.json", "linux.known_hosts", "id_ed25519"] {
        assert!(!p.local.path().join(".everett/peer").join(name).exists(), "unexpected SSH side effect: {name}");
    }
    assert_eq!(fs::read(authorized).unwrap(), before);
}

fn duplicate_alias_requires_scope_and_scoped_repair_leaves_other_transport() {
    let p = Pair::new();
    checked(p.add());
    let relay = p.local.path().join(".everett/relay/peers/linux.json");
    write(&relay, "{}");
    let rejected = p.run(&["peer", "remove", "linux"]);
    assert_eq!(rejected.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("remove one binding explicitly"));
    assert!(p.local.path().join(".everett/peer/linux.json").exists());
    assert!(relay.exists());

    let repaired = checked(p.run(&["peer", "remove", "linux", "--transport", "convex"]));
    assert_eq!(repaired["transport"], "convex");
    assert!(!relay.exists());
    assert!(p.local.path().join(".everett/peer/linux.json").exists());
}

fn mcp_pickup_and_reply_use_the_same_peer_inbox() {
    let p = Pair::new();
    checked(p.add());
    let mid = p.send("mcp-request")["message_id"]
        .as_str()
        .unwrap()
        .to_string();
    let calls = [
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"everett_inbox","arguments":{"session_id":"worker"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"everett_send","arguments":{"session_id":"worker","reply_to":mid,"text":"MCP answer"}}}),
    ];
    let input = calls
        .iter()
        .map(|c| c.to_string() + "\n")
        .collect::<String>();
    let out = p.remote_mcp(&input);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["result"]["isError"], false, "{row}");
    }
    let status = p.status(&mid);
    assert_eq!(status["state"], "replied");
    assert_eq!(status["reply"]["text"], "MCP answer");
    assert_eq!(status["reply"]["hops"], 2);
}

fn queued_delivered_replied_and_deduplicated() {
    let mut p = Pair::new();
    checked(p.add());
    let first = p.send("request-1");
    let mid = first["message_id"].as_str().unwrap();
    assert_eq!(first["state"], "queued");
    let concurrent: Vec<_> = (0..2)
        .map(|_| {
            p.command(false)
                .args([
                    "send",
                    "--to",
                    "linux/openai-dot/worker",
                    "--json",
                    "--idempotency-key",
                    "request-1",
                    MESSAGE,
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in concurrent {
        assert_eq!(
            checked(child.wait_with_output().unwrap())["message_id"],
            mid
        );
    }
    assert_eq!(p.send("request-1")["message_id"], mid);
    assert_eq!(
        fs::read_to_string(p.remote.path().join(".everett/inbox/worker.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(p.status(mid)["state"], "queued");
    checked(p.remote_run(&["inbox", "--session", "worker", "--peek", "--json"], ""));
    assert_eq!(p.status(mid)["state"], "queued");
    checked(p.remote_run(&["inbox", "--session", "worker", "--json"], ""));
    assert_eq!(p.status(mid)["state"], "delivered");
    let reply = p.remote_run(&["reply", mid, "remote answer"], "");
    assert!(reply.status.success());
    assert!(String::from_utf8_lossy(&reply.stdout).contains("RETAINED"));
    assert!(p
        .remote_run(&["reply", mid, "remote answer"], "")
        .status
        .success());
    assert!(!p.local.path().join(".everett/inbox/human.jsonl").exists());
    let status = p.status(mid);
    assert_eq!(status["state"], "replied");
    assert_eq!(status["reply"]["text"], "remote answer");
    assert_eq!(p.status(mid)["local_reply_id"], status["local_reply_id"]);
    let inbox = checked(p.run(&["inbox", "--json"]));
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 1);
    assert_eq!(inbox["messages"][0]["hops"], 2);
    assert_eq!(inbox["messages"][0]["reply_to"], mid);
    assert_eq!(p.status(mid)["local_reply_id"], status["local_reply_id"]);
    let collision = p.run(&[
        "send",
        "--to",
        "linux/openai-dot/worker",
        "--idempotency-key",
        "request-1",
        "different",
    ]);
    assert_eq!(collision.status.code(), Some(2));
    let args = fs::read_to_string(p.local.path().join("ssh-argv.jsonl")).unwrap();
    assert!(!args.contains("private message"));
}

fn body_hops_authorization_and_offline_are_bounded() {
    let mut p = Pair::new();
    checked(p.add());
    let oversized = "x".repeat(everett::peer::BODY_CAP);
    assert_eq!(
        p.run(&["send", "--to", "linux/openai-dot/worker", &oversized])
            .status
            .code(),
        Some(2)
    );
    let client = p.client();
    let out = checked(p.remote_run(&["peer", "serve", "--client", &client], &(oversized + "\n")));
    assert!(out["error"]["message"].as_str().unwrap().contains("64 KiB"));
    let request = json!({"op":"send","session":"worker","harness":"openai-dot","text":"hop","from":{"session":"source","harness":"claude","card":""},"idempotency_key":"hop-4","hops":4});
    assert_eq!(p.rpc(request)["error"]["code"], 7);
    let out = p
        .command(false)
        .env("EVERETT_HOPS", "3")
        .args(["send", "--to", "linux/openai-dot/worker", "hop"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7));
    assert!(!p.remote.path().join(".everett/inbox/worker.jsonl").exists());
    let forwarded = p
        .command(false)
        .env("EVERETT_HOPS", "2")
        .args(["send", "--to", "linux/openai-dot/worker", "hop 3", "--json"])
        .output()
        .unwrap();
    let mid = checked(forwarded)["message_id"]
        .as_str()
        .unwrap()
        .to_string();
    let inbox = checked(p.remote_run(&["inbox", "--session", "worker", "--json"], ""));
    assert_eq!(inbox["messages"][0]["hops"], 3);
    assert_eq!(
        p.remote_run(&["reply", &mid, "hop 4"], "").status.code(),
        Some(7)
    );
    let live: Value = serde_json::from_str(
        &fs::read_to_string(p.remote.path().join(".everett/inbox/live/worker.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(live["hops"], 3);
    let inherited = p
        .command(true)
        .env("EVERETT_SESSION_ID", "worker")
        .env("EVERETT_HARNESS_NAME", "openai-dot")
        .args(["send", "--to", "anything/openai-dot/worker", "hop 4"])
        .output()
        .unwrap();
    assert_eq!(inherited.status.code(), Some(7));
    assert_eq!(
        p.rpc(json!({"op":"status","message_id":"not-mine"}))["error"]["code"],
        2
    );
    let out = checked(p.remote_run(
        &["peer", "serve", "--client", "stranger"],
        "{\"op\":\"hello\"}\n",
    ));
    assert!(out["error"].is_object());
    assert!(p.rpc(json!({"op":"exec","command":"cat /etc/passwd"}))["error"].is_object());
    write(p.remote.path().join("offline"), "");
    let before = Instant::now();
    let out = p.run(&["send", "--to", "linux/openai-dot/worker", "offline"]);
    assert_eq!(out.status.code(), Some(7));
    assert!(before.elapsed() < Duration::from_secs(5));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no local queue"));
    assert!(!p.local.path().join(".everett/inbox/human.jsonl").exists());
    assert_eq!(
        fs::read_dir(p.local.path().join(".everett/peer/outgoing"))
            .unwrap()
            .count(),
        1
    );
}

fn transport_and_server_time_out() {
    let p = Pair::new();
    checked(p.add());
    write(p.remote.path().join("hang"), "");
    let before = Instant::now();
    let out = p.run(&["send", "--to", "linux/openai-dot/worker", "timeout"]);
    assert_eq!(out.status.code(), Some(7));
    assert!(before.elapsed() >= Duration::from_secs(20));
    assert!(before.elapsed() < Duration::from_secs(25));
    assert!(!p.local.path().join(".everett/peer/outgoing").exists());
    let client = p.client();
    let mut child = p
        .command(true)
        .args(["peer", "serve", "--client", &client])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(b"{").unwrap();
    let before = Instant::now();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(7));
    assert!(before.elapsed() < Duration::from_secs(25));
    drop(input);
}

fn mcp_new_remote_send_is_idempotent_and_never_wakes() {
    let p = Pair::new();
    checked(p.add());
    let calls: String = (1..=2).map(|id| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"everett_send","arguments":{"to":"linux/openai-dot/worker","text":MESSAGE,"idempotency_key":"mcp-new","session_id":"local-sender"}}}).to_string() + "\n").collect();
    let mut child = p
        .command(false)
        .arg("mcp")
        .env("EVERETT_SESSION_ID", "local-sender")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(calls.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for row in &rows {
        assert_eq!(row["result"]["isError"], false, "{row}");
        let result = &row["result"]["structuredContent"];
        assert_eq!(result["idempotency_key"], "mcp-new");
        assert_eq!(result["mode"], "inbox");
        assert_eq!(result["router"], "direct-remote");
        assert_eq!(result["wake"], false);
    }
    assert_eq!(
        rows[0]["result"]["structuredContent"]["message_id"],
        rows[1]["result"]["structuredContent"]["message_id"]
    );
    let inbox = fs::read_to_string(p.remote.path().join(".everett/inbox/worker.jsonl")).unwrap();
    assert_eq!(inbox.lines().count(), 1);
    let message: Value = serde_json::from_str(inbox.trim()).unwrap();
    assert_eq!(message["peer_sender"], "local-sender");
    assert_eq!(message["text"], MESSAGE);
}

type Case = (&'static str, fn());
const CASES: &[Case] = &[
    (
        "convex_alias_rejects_ssh_before_identity_pin_or_remote_key_side_effects",
        convex_alias_rejects_ssh_before_identity_pin_or_remote_key_side_effects,
    ),
    (
        "pairing_pins_and_restricts_real_authorized_file",
        pairing_pins_and_restricts_real_authorized_file,
    ),
    (
        "mismatches_refuse_first_pair_and_changed_host",
        mismatches_refuse_first_pair_and_changed_host,
    ),
    (
        "incomplete_pairing_and_local_revocation_fail_closed",
        incomplete_pairing_and_local_revocation_fail_closed,
    ),
    (
        "duplicate_alias_requires_scope_and_scoped_repair_leaves_other_transport",
        duplicate_alias_requires_scope_and_scoped_repair_leaves_other_transport,
    ),
    (
        "mcp_pickup_and_reply_use_the_same_peer_inbox",
        mcp_pickup_and_reply_use_the_same_peer_inbox,
    ),
    (
        "mcp_new_remote_send_is_idempotent_and_never_wakes",
        mcp_new_remote_send_is_idempotent_and_never_wakes,
    ),
    (
        "queued_delivered_replied_and_deduplicated",
        queued_delivered_replied_and_deduplicated,
    ),
    (
        "body_hops_authorization_and_offline_are_bounded",
        body_hops_authorization_and_offline_are_bounded,
    ),
    (
        "transport_and_server_time_out",
        transport_and_server_time_out,
    ),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--fake-ssh") {
        std::process::exit(fake_ssh(&args[2..]));
    }
    let filters: Vec<_> = args[1..].iter().filter(|a| !a.starts_with('-')).collect();
    let selected: Vec<_> = CASES
        .iter()
        .filter(|(name, _)| filters.is_empty() || filters.iter().any(|a| name.contains(a.as_str())))
        .collect();
    let mut failures = 0;
    for (name, case) in &selected {
        let ok = std::panic::catch_unwind(*case).is_ok();
        println!("test {name} ... {}", if ok { "ok" } else { "FAILED" });
        failures += usize::from(!ok);
    }
    println!("{} passed, {failures} failed", selected.len() - failures);
    if failures != 0 {
        std::process::exit(1);
    }
}
