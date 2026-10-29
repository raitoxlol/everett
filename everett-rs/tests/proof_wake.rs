mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::io::{Read, Seek};
use std::os::unix::process::CommandExt;
use std::sync::mpsc;
use wait_timeout::ChildExt;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use common::{fixture, Fixture};
use rusqlite::Connection;
use serde_json::{json, Value};

const TARGET: &str = "proof-provider-session";
const SENDER: &str = "proof-sender";

fn command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_everett"));
    command.env_clear()
        .env("HOME", home)
        .env("EVERETT_HOME", home)
        .env("EVERETT_NOTIFY", "none")
        .env("EVERETT_SESSION_ID", SENDER)
        .env("EVERETT_T3_BIN", home.join("fake-t3"))
        .env("PATH", format!("{}:{}", home.join("bin").display(), common::HERMETIC_PATH))
        .current_dir(home);
    command
}

trait Watchdog {
    fn checked_output(&mut self) -> Output;
}

impl Watchdog for Command {
    fn checked_output(&mut self) -> Output {
        let mut stdout = tempfile::tempfile().unwrap();
        let mut stderr = tempfile::tempfile().unwrap();
        let mut child = self.process_group(0).stdin(Stdio::null())
            .stdout(stdout.try_clone().unwrap()).stderr(stderr.try_clone().unwrap()).spawn().unwrap();
        let status = child.wait_timeout(Duration::from_secs(12)).unwrap();
        if status.is_none() {
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL); }
            child.kill().ok();
            child.wait().unwrap();
            panic!("subprocess exceeded independent 12s watchdog");
        }
        stdout.rewind().unwrap();
        stderr.rewind().unwrap();
        let mut out = Output { status: status.unwrap(), stdout: Vec::new(), stderr: Vec::new() };
        stdout.read_to_end(&mut out.stdout).unwrap();
        stderr.read_to_end(&mut out.stderr).unwrap();
        out
    }
}

fn executable(f: &Fixture, name: &str, body: &str) {
    let body = body.replacen("#!/bin/sh\n", "#!/bin/sh\nif [ \"$1\" = --fixture-ready ]; then printf 'READY\\n'; exit 0; fi\n", 1);
    let path = f.write(name, &body);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let ready = Command::new(path).arg("--fixture-ready").env_clear()
        .env("HOME", f.home()).env("PATH", common::HERMETIC_PATH).checked_output();
    assert!(ready.status.success(), "{ready:?}");
    assert_eq!(ready.stdout, b"READY\n");
}

fn t3_fixture(exit: i32) -> Fixture {
    let f = fixture();
    let path = f.home().join(".t3/userdata/statev2.sqlite");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let con = Connection::open(path).unwrap();
    con.execute_batch(include_str!("../../tests/fixtures/t3_v2.sql")).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    con.execute("INSERT INTO projection_projects VALUES ('p','Proof',?1,NULL,'[]',?2,?2,NULL)",
        [f.home().to_str().unwrap(), &now]).unwrap();
    con.execute("INSERT INTO orchestration_v2_projection_threads \
        (thread_id,project_id,title,default_provider,runtime_mode,interaction_mode,active_provider_thread_id,created_at,updated_at,payload_json) \
        VALUES ('proof-thread','p','Proof target','codex','full-access','default','provider',?1,?1,'{}')", [&now]).unwrap();
    con.execute("INSERT INTO orchestration_v2_projection_provider_threads \
        (provider_thread_id,thread_id,provider,driver,status,updated_at,payload_json) \
        VALUES ('provider','proof-thread','codex','codex','idle',?1,?2)",
        [&now, &json!({"driver":"codex","nativeThreadRef":{"driver":"codex","nativeId":TARGET,"strength":"strong"}}).to_string()]).unwrap();
    executable(&f, "fake-t3", &format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/t3-argv\"\nprintf '%s\\n' '{{\"accepted\":true}}'\nexit {exit}\n"
    ));
    f
}

fn target_id(f: &Fixture) -> String {
    fs::read_to_string(f.home().join("target-id")).unwrap_or_else(|_| TARGET.into())
}

fn proof(f: &Fixture, timeout: &str, extra: &[&str]) -> (Output, Value) {
    let target = target_id(f);
    let out = command(&f.home()).args(["proof", "wake", "--to", &target, "--timeout", timeout, "--json"])
        .args(extra).checked_output();
    let report = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{out:?}"));
    (out, report)
}

#[derive(Clone, Copy)]
enum Answer {
    Exact,
    ExactUnmarked,
    Wrong,
    None,
    SpoofedSender,
    WrongCorrelation,
}

fn answer(f: &Fixture, picked_up: bool, reply: Answer) -> JoinHandle<Value> {
    let home = f.home();
    let target = target_id(f);
    let (ready, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        ready.send(()).unwrap();
        let started = Instant::now();
        let canary = loop {
            let message = fs::read_to_string(home.join(format!(".everett/inbox/{target}.jsonl")))
                .ok().and_then(|s| s.lines().next().and_then(|l| serde_json::from_str::<Value>(l).ok()));
            if let Some(message) = message {
                if home.join("t3-argv").exists() || home.join("resume-argv").exists() {
                    break message;
                }
            }
            assert!(started.elapsed() < Duration::from_secs(3), "No dispatch seen within responder deadline: target={target} inbox={:?} argv={:?}", fs::read_to_string(home.join(format!(".everett/inbox/{target}.jsonl"))), fs::read_to_string(home.join("resume-argv")));
            thread::sleep(Duration::from_millis(10));
        };
        let mid = canary["id"].as_str().unwrap();
        let expected = canary["text"].as_str().unwrap().strip_prefix("Reply through Everett with exactly: ").unwrap();
        let nonce = expected.strip_prefix("WAKE-OK ").unwrap();
        assert_eq!(nonce.len(), 32);
        assert!(nonce.chars().all(|c| c.is_ascii_hexdigit()));
        if picked_up {
            let out = command(&home).env("EVERETT_SESSION_ID", &target)
                .args(["inbox", "--session", &target, "--message", mid, "--json"]).checked_output();
            assert!(out.status.success(), "{out:?}");
        }
        match reply {
            Answer::Exact | Answer::Wrong => {
                let text = if matches!(reply, Answer::Exact) { expected } else { "WAKE-OK wrong-nonce" };
                let out = command(&home).env("EVERETT_SESSION_ID", &target)
                    .args(["reply", mid, text]).checked_output();
                assert!(out.status.success(), "{out:?}");
            }
            Answer::ExactUnmarked | Answer::SpoofedSender | Answer::WrongCorrelation => {
                let path = home.join(format!(".everett/inbox/{SENDER}.jsonl"));
                use std::io::Write;
                let mut file = fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
                writeln!(file, "{}", json!({"id":"mspoof", "from":if matches!(reply, Answer::SpoofedSender) {"other-session"} else {target.as_str()},
                    "to":SENDER, "reply_to":if matches!(reply, Answer::WrongCorrelation) {"other-message"} else {mid},
                    "text":expected, "ts":everett::session::now()})).unwrap();
            }
            Answer::None => {}
        }
        canary
    });
    receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    handle
}

#[test]
fn success_reports_stages_and_preserves_unrelated_sender_inbox() {
    let f = t3_fixture(0);
    f.write(&format!(".everett/inbox/{SENDER}.jsonl"), &format!("{}\n", json!({
        "id":"munrelated", "from":"another", "to":SENDER, "text":"Leave this pending", "ts":everett::session::now()
    })));
    let responder = answer(&f, true, Answer::Exact);
    let (out, report) = proof(&f, "6", &[]);
    let canary = responder.join().unwrap();
    assert_eq!(out.status.code(), Some(0), "{report}");
    assert_eq!(report["status"], "passed");
    assert_eq!(report["target_resolved"]["harness"], "codex");
    assert_eq!(report["target_resolved"]["transport"], "t3");
    assert_eq!(report["dispatched"]["receipt"]["wake"]["receipt"], "{\"accepted\":true}");
    for stage in ["target_resolved", "dispatched", "picked_up", "replied"] {
        chrono::DateTime::parse_from_rfc3339(report[stage]["at"].as_str().unwrap()).unwrap();
    }
    assert_eq!(report["dispatched"]["message_id"], canary["id"]);
    assert_eq!(report["replied"]["reply_to"], canary["id"]);
    assert!(report["total_latency_ms"].as_f64().unwrap() > 0.0);
    assert!(!f.home().join(format!(".everett/inbox/{SENDER}.done")).exists());
    let argv = fs::read_to_string(f.home().join("t3-argv")).unwrap();
    assert!(!argv.contains("WAKE-OK"));
    assert!(argv.contains(canary["id"].as_str().unwrap()));
}

#[test]
fn matching_reply_without_done_passes_without_claiming_pickup() {
    let f = t3_fixture(0);
    let responder = answer(&f, false, Answer::ExactUnmarked);
    let (out, report) = proof(&f, "6", &[]);
    responder.join().unwrap();
    assert!(out.status.success(), "{report}");
    assert!(report["picked_up"].is_null());
    assert!(report["replied"].is_object());
}

#[test]
fn wrong_nonce_does_not_pass() {
    let f = t3_fixture(0);
    let responder = answer(&f, true, Answer::Wrong);
    let (out, report) = proof(&f, "6", &[]);
    responder.join().unwrap();
    assert_eq!(out.status.code(), Some(6), "{report}");
    assert!(report["picked_up"].is_object());
    assert!(report["incorrect_reply"].is_object());
    assert!(report["replied"].is_null());
}

#[test]
fn pickup_without_reply_times_out_separately() {
    let f = t3_fixture(0);
    let responder = answer(&f, true, Answer::None);
    let (out, report) = proof(&f, "6", &[]);
    responder.join().unwrap();
    assert_eq!(out.status.code(), Some(6), "{report}");
    assert_eq!(report["status"], "reply_timeout");
    assert!(report["incorrect_reply"].is_null());
}

#[test]
fn no_pickup_times_out_with_exit_five() {
    let f = t3_fixture(0);
    let (out, report) = proof(&f, "3", &[]);
    assert_eq!(out.status.code(), Some(5), "{report}");
    assert_eq!(report["status"], "pickup_timeout");
    assert!(report["dispatched"].is_object());
    assert!(report["picked_up"].is_null());
}

#[test]
fn unrelated_or_wrong_sender_reply_cannot_pass() {
    for reply in [Answer::SpoofedSender, Answer::WrongCorrelation] {
        let f = t3_fixture(0);
        let responder = answer(&f, true, reply);
        let (out, report) = proof(&f, "6", &[]);
        responder.join().unwrap();
        assert_eq!(out.status.code(), Some(6), "{report}");
        assert!(report["replied"].is_null());
        assert!(report["incorrect_reply"].is_null());
        assert!(!f.home().join(format!(".everett/inbox/{SENDER}.done")).exists());
    }
}

#[test]
fn no_transport_refuses_without_queueing() {
    let f = t3_fixture(0);
    fs::remove_file(f.home().join("fake-t3")).unwrap();
    let (out, report) = proof(&f, "1", &[]);
    assert_eq!(out.status.code(), Some(3), "{report}");
    assert_eq!(report["target_resolved"]["transport"], "none");
    assert!(report["dispatched"].is_null());
    assert!(!f.home().join(format!(".everett/inbox/{TARGET}.jsonl")).exists());
}

#[test]
fn dispatch_failure_reports_retained_canary() {
    let f = t3_fixture(1);
    let (out, report) = proof(&f, "1", &[]);
    assert_eq!(out.status.code(), Some(4), "{report}");
    assert_eq!(report["status"], "dispatch_failed");
    assert!(report["dispatched"].is_null());
    let message: Value = serde_json::from_str(fs::read_to_string(f.home().join(format!(".everett/inbox/{TARGET}.jsonl"))).unwrap().trim()).unwrap();
    assert!(report["detail"].as_str().unwrap().contains(message["id"].as_str().unwrap()));
}

#[test]
fn stalled_dispatch_is_bounded_by_timeout() {
    let f = t3_fixture(0);
    executable(&f, "fake-t3", "#!/bin/sh\ntouch \"$HOME/t3-started\"\nexec sleep 30\n");
    let (out, report) = proof(&f, "2", &[]);
    assert_eq!(out.status.code(), Some(4), "{report}");
    assert!(report["total_latency_ms"].as_f64().unwrap() < 3000.0, "{report}");
    assert!(f.home().join("t3-started").exists(), "dispatch must actually start");
}

#[test]
fn dry_run_reports_transport_and_sends_nothing() {
    let f = t3_fixture(0);
    let (out, report) = proof(&f, "1", &["--dry-run"]);
    assert!(out.status.success(), "{report}");
    assert_eq!(report["status"], "dry_run");
    assert_eq!(report["target_resolved"]["transport"], "t3");
    assert!(report["dispatched"].is_null());
    assert!(!f.home().join("t3-argv").exists());
    assert!(!f.home().join(format!(".everett/inbox/{TARGET}.jsonl")).exists());
    let out = command(&f.home()).args(["proof", "wake", "--to", TARGET, "--dry-run"]).checked_output();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("target resolved:") && text.contains("t3") && text.contains("Nothing sent"), "{text}");
}

#[test]
fn remote_derived_caller_is_refused_even_for_dry_run() {
    let f = t3_fixture(0);
    f.write(&format!(".everett/inbox/relay-context/{SENDER}.json"), "{\"relay_message_id\":\"remote\"}");
    for extra in [vec![], vec!["--dry-run"]] {
        let (out, report) = proof(&f, "1", &extra);
        assert_eq!(out.status.code(), Some(2), "{report}");
        assert_eq!(report["status"], "refused");
        assert!(report["detail"].as_str().unwrap().contains("Remote-derived"));
        assert!(!f.home().join("t3-argv").exists());
        assert!(!f.home().join(format!(".everett/inbox/{TARGET}.jsonl")).exists());
    }
}

#[test]
fn invalid_inputs_never_send_or_auto_select() {
    let f = t3_fixture(0);
    for args in [
        vec!["proof", "wake"],
        vec!["proof", "wake", "--to", "missing", "--json"],
        vec!["proof", "wake", "--to", TARGET, "--timeout", "NaN", "--json"],
        vec!["proof", "wake", "--to", TARGET, "--timeout", "0", "--json"],
        vec!["proof", "wake", "--to", TARGET, "--timeout", "1e300", "--json"],
    ] {
        let out = command(&f.home()).args(args).checked_output();
        assert_eq!(out.status.code(), Some(2), "{out:?}");
    }
    let out = command(&f.home()).env("EVERETT_SESSION_ID", TARGET)
        .args(["proof", "wake", "--to", TARGET, "--json"]).checked_output();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(!f.home().join("t3-argv").exists());
    assert!(!f.home().join(format!(".everett/inbox/{TARGET}.jsonl")).exists());
}

fn resume_fixture() -> Fixture {
    let f = fixture();
    let target = format!("proof-{}", uuid::Uuid::new_v4().simple());
    f.write("target-id", &target);
    let path = f.write(".codex/sessions/2026/10/29/proof.jsonl", &common::jsonl(&[
        json!({"type":"session_meta", "payload":{"id":target, "cwd":f.home(), "timestamp":chrono::Utc::now().to_rfc3339()}}),
        json!({"type":"response_item", "payload":{"type":"message", "role":"user", "content":[{"type":"input_text", "text":"Proof target"}]}}),
    ]));
    fs::File::open(path).unwrap().set_times(fs::FileTimes::new().set_modified(
        std::time::SystemTime::now() - Duration::from_secs(120)
    )).unwrap();
    executable(&f, "bin/codex", r#"#!/bin/sh
printf '%s\n' "$$" > "$HOME/resume-pid"
printf '%s\n' "$@" > "$HOME/resume-argv"
IFS= read -r line < "$HOME/.everett/inbox/$3.jsonl"
token=${line#*Reply through Everett with exactly: }
token=${token%%\"*}
printf '%s\n' "$token" > "$HOME/stdout-token"
printf '%s\n' "$token"
exec sleep 30
"#);
    f
}

fn assert_resume_cleaned(f: &Fixture) {
    let pid: i32 = fs::read_to_string(f.home().join("resume-pid")).unwrap().trim().parse().unwrap();
    assert!(!everett::proc::pid_alive(pid), "proof must reap its connector");
}

#[test]
fn resume_reply_passes_while_client_lingers() {
    let f = resume_fixture();
    let responder = answer(&f, true, Answer::Exact);
    let (out, report) = proof(&f, "6", &[]);
    responder.join().unwrap();
    assert!(out.status.success(), "{report}");
    assert_eq!(report["target_resolved"]["transport"], "resume");
    assert!(report["picked_up"].is_object());
    assert!(report["replied"].is_object());
    let argv = fs::read_to_string(f.home().join("resume-argv")).unwrap();
    assert!(!argv.contains("WAKE-OK"));
    assert_resume_cleaned(&f);
}

#[test]
fn resume_pickup_without_reply_while_client_lingers() {
    let f = resume_fixture();
    let responder = answer(&f, true, Answer::None);
    let (out, report) = proof(&f, "6", &[]);
    responder.join().unwrap();
    assert_eq!(out.status.code(), Some(6), "{report}");
    assert!(report["picked_up"].is_object());
    assert!(report["dispatched"].is_object());
    assert_resume_cleaned(&f);
}

#[test]
fn resume_stdout_without_inbox_reply_cannot_pass() {
    let f = resume_fixture();
    let (out, report) = proof(&f, "3", &[]);
    assert_eq!(out.status.code(), Some(5), "{report}");
    assert!(report["dispatched"].is_object());
    assert!(report["replied"].is_null());
    assert!(fs::read_to_string(f.home().join("stdout-token")).unwrap().starts_with("WAKE-OK "));
    assert_resume_cleaned(&f);
}

#[test]
fn omp_dry_run_requires_live_capability_and_never_queues() {
    let f = fixture();
    f.write(".omp/agent/sessions/proof/session.jsonl", &common::jsonl(&[
        json!({"type":"session", "id":TARGET, "cwd":f.home(), "timestamp":chrono::Utc::now().to_rfc3339()}),
        json!({"type":"message", "message":{"role":"user", "content":"Proof target"}}),
    ]));
    let lease = json!({"version":1, "session_id":TARGET, "harness":"omp", "pid":std::process::id(),
        "ts":everett::session::now(), "process_identity":everett::proc::process_identity(std::process::id() as i32).unwrap()});
    f.write(&format!(".everett/inbox/live/{TARGET}.json"), &lease.to_string());
    let (out, report) = proof(&f, "1", &["--dry-run"]);
    assert_eq!(out.status.code(), Some(3), "{report}");
    f.write(&format!(".everett/inbox/omp-wake/{TARGET}.json"), &lease.to_string());
    let (out, report) = proof(&f, "1", &["--dry-run"]);
    assert!(out.status.success(), "{report}");
    assert_eq!(report["target_resolved"]["transport"], "omp");
    assert!(!f.home().join(format!(".everett/inbox/{TARGET}.jsonl")).exists());
}

#[test]
fn canary_lock_is_inside_budget_and_never_appends_late() {
    use fs2::FileExt;
    let f = t3_fixture(0);
    let lock_path = f.write(&format!(".everett/inbox/{TARGET}.lock"), "");
    let (ready, receiver) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let holder = thread::spawn(move || {
        let lock = fs::OpenOptions::new().write(true).open(lock_path).unwrap();
        lock.lock_exclusive().unwrap();
        ready.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(15)).unwrap();
    });
    receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    let (out, report) = proof(&f, "1", &[]);
    release.send(()).unwrap();
    holder.join().unwrap();
    assert_eq!(out.status.code(), Some(4), "{report}");
    assert!(report["total_latency_ms"].as_f64().unwrap() < 2000.0, "{report}");
    assert!(!f.home().join(format!(".everett/inbox/{TARGET}.jsonl")).exists());
    assert!(!f.home().join("t3-argv").exists());
}

#[test]
fn ordinary_t3_ping_retains_twenty_second_dispatch_timeout() {
    let f = t3_fixture(0);
    executable(&f, "fake-t3", "#!/bin/sh\ntouch \"$HOME/t3-started\"\nsleep 0.5\nprintf '{\"accepted\":true}\\n'\n");
    let out = command(&f.home()).args(["send", "--to", TARGET, "--mode", "ping", "--timeout", "0.1", "ordinary ping"]).checked_output();
    assert!(out.status.success(), "{out:?}");
    assert!(f.home().join("t3-started").exists());
}

const CLAUDE_TARGET: &str = "1f57ab77-c669-4bb6-9ac8-f9a9073de8ea";

fn claude_fixture(foreground: bool) -> Fixture {
    let f = fixture();
    f.write(&format!(".claude/projects/proof/{CLAUDE_TARGET}.jsonl"), &common::jsonl(&[
        json!({"type":"user", "sessionId":CLAUDE_TARGET, "cwd":f.home(), "timestamp":chrono::Utc::now().to_rfc3339(),
            "message":{"role":"user", "content":"Proof target"}}),
    ]));
    f.write(&format!(".everett/inbox/live/{CLAUDE_TARGET}.json"), &json!({"session_id":CLAUDE_TARGET,
        "pid":std::process::id(), "ts":everett::session::now()}).to_string());
    if foreground {
        let row = json!([{"sessionId":CLAUDE_TARGET, "pid":std::process::id(), "cwd":f.home(),
            "kind":"foreground", "state":"done", "status":"idle"}]);
        executable(&f, "bin/claude", &format!("#!/bin/sh\ncase \"$1\" in\n--version) echo '2.1.285';;\nagents) echo '{}';;\n*) touch \"$HOME/unexpected-attach\"; exit 88;;\nesac\n", row));
    }
    f
}

#[test]
fn missing_claude_cli_is_no_transport_for_preview_and_real_proof() {
    assert_claude_unavailable(false);
}

#[test]
fn foreground_claude_is_no_transport_for_preview_and_real_proof() {
    assert_claude_unavailable(true);
}

fn assert_claude_unavailable(foreground: bool) {
    let f = claude_fixture(foreground);
    for dry in [true, false] {
        let mut cmd = command(&f.home());
        cmd.args(["proof", "wake", "--to", CLAUDE_TARGET, "--timeout", "2", "--json"]);
        if dry { cmd.arg("--dry-run"); }
        let out = cmd.checked_output();
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(out.status.code(), Some(3), "{report}");
        assert_eq!(report["target_resolved"]["transport"], "none");
        assert!(!f.home().join(format!(".everett/inbox/{CLAUDE_TARGET}.jsonl")).exists());
        assert!(!f.home().join("unexpected-attach").exists());
    }
}

#[test]
fn fixture_identity_helper() {
    if std::env::var_os("EVERETT_FIXTURE_IDENTITY_HELPER").is_none() { return; }
    let f = fixture();
    f.write(".everett/inbox/x-1.jsonl", &common::jsonl(&[json!({
        "id":"mfixture", "to":"x-1", "from":"origin", "text":"fixture reply", "ts":everett::session::now()
    })]));
    let out = f.run(&["reply", "mfixture", "unbound"]);
    assert!(out.status.success(), "run leaked parent identity: {out:?}");
    let out = f.run_stdin(&["reply", "mfixture", "unbound stdin"], "", &[]);
    assert!(out.status.success(), "run_stdin leaked parent identity: {out:?}");
    let out = f.run_stdin(&["reply", "mfixture", "intentional override"], "", &[("EVERETT_SESSION_ID", "x-1")]);
    assert!(out.status.success(), "{out:?}");
    let replies = fs::read_to_string(f.home().join(".everett/inbox/origin.jsonl")).unwrap();
    let last: Value = serde_json::from_str(replies.lines().last().unwrap()).unwrap();
    assert_eq!(last["from"], "x-1");
}

#[test]
fn fixture_scrubs_every_parent_identity_before_overrides() {
    for identity in ["", "EVERETT_SESSION_ID", "CLAUDE_CODE_SESSION_ID", "CLAUDE_SESSION_ID",
        "CODEX_THREAD_ID", "HERMES_SESSION_ID", "PI_SESSION_FILE", "GROK_SESSION_ID"] {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.env_clear().env("PATH", common::HERMETIC_PATH).env("EVERETT_FIXTURE_IDENTITY_HELPER", "1")
            .args(["--exact", "fixture_identity_helper", "--nocapture"]);
        if !identity.is_empty() { cmd.env(identity, "inherited-session"); }
        let out = cmd.checked_output();
        assert!(out.status.success(), "identity {identity}: {out:?}");
    }
}

#[test]
fn claude_preflight_obeys_budget_before_queueing() {
    let f = claude_fixture(true);
    executable(&f, "bin/claude", "#!/bin/sh\ntouch \"$HOME/preflight-started\"\nexec sleep 30\n");
    for dry in [true, false] {
        let mut cmd = command(&f.home());
        cmd.args(["proof", "wake", "--to", CLAUDE_TARGET, "--timeout", "1", "--json"]);
        if dry { cmd.arg("--dry-run"); }
        let out = cmd.checked_output();
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(out.status.code(), Some(3), "{report}");
        assert_eq!(report["target_resolved"]["transport"], "none");
        assert!(report["total_latency_ms"].as_f64().unwrap() < 2000.0, "{report}");
        assert!(f.home().join("preflight-started").exists());
        assert!(!f.home().join(format!(".everett/inbox/{CLAUDE_TARGET}.jsonl")).exists());
    }
}

#[test]
fn delayed_mcp_ping_helper() {
    if std::env::var_os("EVERETT_TEST_MCP_TIMEOUT_CHILD").is_none() { return; }
    let f = t3_fixture(0);
    executable(&f, "fake-t3", "#!/bin/sh\nsleep 1.5\nprintf '{\"accepted\":true}\\n'\n");
    let binary = f.home().join("fake-t3");
    let results = common::mcp_calls(&f, &[], &[("EVERETT_T3_BIN", binary.to_str().unwrap()),
        ("EVERETT_SESSION_ID", SENDER)], &[("everett_send", json!({"to":TARGET,"text":"ordinary MCP ping","mode":"ping","timeout":1.0}))]);
    let result = common::tool_json(&results[0]);
    assert_eq!(result["pinged"], true, "results={results:?}");
    assert_eq!(result["wake"]["receipt"], "{\"accepted\":true}");
}

#[test]
fn ordinary_mcp_ping_retains_twenty_second_dispatch_timeout() {
    let out = Command::new(std::env::current_exe().unwrap()).env_clear()
        .env("PATH", common::HERMETIC_PATH).env("EVERETT_TEST_MCP_TIMEOUT_CHILD", "1")
        .args(["--exact", "delayed_mcp_ping_helper", "--nocapture"]).checked_output();
    assert!(out.status.success(), "{out:?}");
}
