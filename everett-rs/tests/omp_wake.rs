mod common;

use common::{fixture, jsonl, lib_home, mcp_calls, tool_json, Fixture};
use everett::{inbox, omp_wake, send, session::{now, Session}};
use serde_json::{json, Value};

const SID: &str = "omp-wake-target";

fn lease() -> Value {
    json!({"version":1,"session_id":SID,"harness":"omp","pid":std::process::id(),"ts":now(),
        "process_identity":everett::proc::process_identity(std::process::id() as i32).unwrap()})
}

fn live(f: &Fixture) {
    f.write(&format!(".everett/inbox/live/{SID}.json"),
        &lease().to_string());
}

fn target() -> Session {
    Session::new("omp", SID, "/tmp", "", "", now())
}

#[test]
fn capability_requires_fresh_versioned_lease_bound_to_live_omp_pid() {
    let f = lib_home();
    let root = f.path().join(".everett/inbox");
    std::fs::create_dir_all(root.join("omp-wake")).unwrap();
    std::fs::create_dir_all(root.join("live")).unwrap();
    let path = root.join("omp-wake").join(format!("{SID}.json"));
    assert!(!omp_wake::available(&target()));
    std::fs::write(root.join("live").join(format!("{SID}.json")),
        lease().to_string()).unwrap();
    std::fs::write(&path, lease().to_string()).unwrap();
    assert!(omp_wake::available(&target()));
    for (key, value) in [
        ("version", json!(2)), ("session_id", json!("another")), ("harness", json!("pi")),
        ("pid", json!(0)), ("pid", json!(i32::MAX)), ("process_identity", json!("previous-incarnation")),
        ("ts", json!(now() - omp_wake::LEASE_SECONDS - 1.0)), ("ts", json!(now() + 60.0)),
    ] {
        let mut invalid = lease();
        invalid[key] = value;
        std::fs::write(&path, invalid.to_string()).unwrap();
        assert!(!omp_wake::available(&target()), "accepted {invalid}");
    }
    std::fs::write(&path, lease().to_string()).unwrap();
    let mut wrong_target = target();
    wrong_target.source = "t3code".into();
    assert!(!omp_wake::available(&wrong_target));
    wrong_target = target();
    wrong_target.harness = "pi".into();
    assert!(!omp_wake::available(&wrong_target));
    std::fs::remove_file(root.join("live").join(format!("{SID}.json"))).unwrap();
    assert!(!omp_wake::available(&target()));
}

fn cli_fixture() -> Fixture {
    let f = fixture();
    f.write(".omp/agent/sessions/wake/session.jsonl", &jsonl(&[
        json!({"type":"session","id":SID,"cwd":f.home(),"timestamp":chrono::Utc::now().to_rfc3339()}),
        json!({"type":"message","message":{"role":"user","content":"watch inbox"}}),
    ]));
    live(&f);
    f
}

#[test]
fn cli_and_mcp_ping_and_auto_use_live_extension_transport() {
    for mode in ["ping", "auto"] {
        for mcp in [false, true] {
            let f = cli_fixture();
            f.write(&format!(".everett/inbox/omp-wake/{SID}.json"), &lease().to_string());
            let result = if mcp {
                let results = mcp_calls(&f, &[], &[("EVERETT_SESSION_ID", "wake-sender")], &[("everett_send",
                    json!({"to":SID,"text":"private sender text","mode":mode}))]);
                assert_ne!(results[0]["isError"], true, "{}", results[0]);
                tool_json(&results[0])
            } else {
                let out = f.run(&["send", "private sender text", "--to", SID, "--mode", mode, "--json"]);
                assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
                serde_json::from_slice(&out.stdout).unwrap()
            };
            assert_eq!(result["mode"], "ping");
            assert_eq!(result["transport"], "omp-extension");
            assert_eq!(result["wake"]["state"], "extension_pending");
            assert_eq!(result["pinged"], false);
            assert_eq!(result["queued"], true);
            assert_eq!(result["delivered"], false);
            let pending = f.run(&["hook", "omp_wake_pending", SID]);
            let ids: Vec<String> = serde_json::from_slice(&pending.stdout).unwrap();
            assert_eq!(ids, vec![result["message_id"].as_str().unwrap()]);
            assert!(!f.stdout(&pending).contains("private sender text"));
            assert!(!f.home().join(format!(".everett/inbox/{SID}.done")).exists());
        }
    }
}

#[test]
fn attached_omp_without_supported_live_lease_is_refused_without_queuing() {
    for mode in ["ping", "auto"] {
        for marker in [None, Some(json!({})), Some({ let mut l = lease(); l["ts"] = json!(now() - 60.0); l })] {
            let f = cli_fixture();
            if let Some(marker) = marker {
                f.write(&format!(".everett/inbox/omp-wake/{SID}.json"), &marker.to_string());
            }
            let out = f.run(&["send", "request", "--to", SID, "--mode", mode, "--json"]);
            assert_eq!(out.status.code(), Some(4), "{}", f.stdout(&out));
            assert!(String::from_utf8_lossy(&out.stderr).contains("Nothing was queued"));
            assert!(!f.home().join(format!(".everett/inbox/{SID}.jsonl")).exists());
        }
    }
}

#[test]
fn remote_derived_caller_cannot_ping_live_extension() {
    for mode in ["ping", "auto"] {
        let f = cli_fixture();
        f.write(&format!(".everett/inbox/omp-wake/{SID}.json"), &lease().to_string());
        f.write(".everett/inbox/relay-context/wake-sender.json", "{\"relay_message_id\":\"remote\"}");
        let out = f.run_stdin(&["send", "remote request", "--to", SID, "--mode", mode], "",
            &[("EVERETT_SESSION_ID", "wake-sender")]);
        assert!(!out.status.success());
        assert!(!f.home().join(format!(".everett/inbox/{SID}.jsonl")).exists());
    }
}

#[test]
fn wake_candidates_exclude_external_remote_consumed_expired_and_nudged_ids() {
    let f = lib_home();
    let post = |sender: &str, extra| inbox::post(SID, "never in nudge", sender, "message", "", 1, "codex", "", extra).unwrap();
    let local = post("local", None);
    for extra in [json!({"external":true}), json!({"untrusted":true}), json!({"trust":"external-untrusted"}),
        json!({"from_harness":"grok-bot"}), json!({"relay_message_id":"remote"}), json!({"relay_lineage":{}}),
        json!({"ts":now() - inbox::TTL - 1.0}), json!({"to":"another-session"})] {
        post("local", Some(extra.as_object().unwrap().clone()));
    }
    let done = post("local", None);
    inbox::mark_done(SID, &[done["id"].as_str().unwrap()]);
    let nudged = post("local", None);
    std::fs::write(f.path().join(format!(".everett/inbox/{SID}.omp-nudged")), nudged["id"].as_str().unwrap()).unwrap();
    let claimed = post("local", None);
    let claims = f.path().join(format!(".everett/inbox/omp-wake/{SID}.nudged"));
    std::fs::create_dir_all(&claims).unwrap();
    std::fs::write(claims.join(claimed["id"].as_str().unwrap()), "").unwrap();
    let context = f.path().join(".everett/inbox/relay-context");
    std::fs::create_dir_all(&context).unwrap();
    std::fs::write(context.join("remote-sender.json"), "{}").unwrap();
    post("remote-sender", None);
    assert_eq!(omp_wake::pending_ids(SID), vec![local["id"].as_str().unwrap()]);
    assert!(!inbox::done_ids(SID).contains(nudged["id"].as_str().unwrap()));
    assert!(!inbox::done_ids(SID).contains(claimed["id"].as_str().unwrap()));
}

#[test]
fn nudged_messages_skip_passive_hook_but_explicit_read_records_receipt() {
    let f = cli_fixture();
    let message = json!({"id":"m123abc","to":SID,"from":"sender","text":"private sender text","ts":now(),"hops":1});
    f.write(&format!(".everett/inbox/{SID}.jsonl"), &jsonl(&[message]));
    f.write(&format!(".everett/inbox/{SID}.omp-nudged"), "m123abc\n");
    let out = f.run(&["hook", "omp_inbox", SID]);
    assert!(out.stdout.is_empty());
    let done = f.home().join(format!(".everett/inbox/{SID}.done"));
    assert!(!done.exists());
    let out = f.run(&["inbox", "--session", SID, "--message", "m123abc"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(f.stdout(&out).contains("private sender text"));
    assert!(std::fs::read_to_string(done).unwrap().contains("m123abc"));
}

#[test]
fn external_caller_cannot_use_omp_ping_transport() {
    let _f = lib_home();
    let root = inbox::inbox_dir();
    std::fs::create_dir_all(root.join("live")).unwrap();
    std::fs::create_dir_all(root.join("omp-wake")).unwrap();
    std::fs::write(root.join("live").join(format!("{SID}.json")), lease().to_string()).unwrap();
    std::fs::write(root.join("omp-wake").join(format!("{SID}.json")), lease().to_string()).unwrap();
    assert!(send::ping_as(&target(), "request", 1.0, Some("external-sender"), Some("grok-bot")).is_err());
    assert!(inbox::read(SID).is_empty());
}

#[test]
fn heartbeat_refreshes_expired_binding_without_activity_consumption_or_rebinding() {
    let f = cli_fixture();
    let path = format!(".everett/inbox/live/{SID}.json");
    let mut old = lease();
    old["ts"] = json!(now() - 90_000.0);
    old["state"] = json!("idle");
    old["hops"] = json!(3);
    old["hops_ts"] = json!(now() - 120.0);
    f.write(&path, &old.to_string());
    f.write(&format!(".everett/inbox/{SID}.jsonl"), &jsonl(&[
        json!({"id":"m123","to":SID,"text":"unread","ts":now(),"hops":1})]));
    let result = f.run_stdin(&["hook", "omp_wake_heartbeat", SID], "", &[("EVERETT_AUTH_PROVIDER", "custom-provider")]);
    assert!(result.status.success());
    let heartbeat: Value = serde_json::from_slice(&result.stdout).expect("heartbeat must return verified command settings");
    assert_eq!(heartbeat["process_identity"], old["process_identity"]);
    assert_eq!(heartbeat["pid"], old["pid"]);
    assert_eq!(heartbeat["everett_home"], json!(f.home()));
    assert_eq!(heartbeat["auth_provider"], "custom-provider");
    assert_eq!(heartbeat["executable"], json!(std::path::Path::new(env!("CARGO_BIN_EXE_everett")).canonicalize().unwrap()));
    let refreshed: Value = serde_json::from_str(&std::fs::read_to_string(f.home().join(&path)).unwrap()).unwrap();
    assert!(refreshed["ts"].as_f64().unwrap() > old["ts"].as_f64().unwrap());
    for key in ["pid", "process_identity", "state", "hops", "hops_ts"] { assert_eq!(refreshed[key], old[key]); }
    assert!(!f.home().join(format!(".everett/inbox/{SID}.done")).exists());
    f.write(&format!(".everett/inbox/omp-wake/{SID}.json"), &lease().to_string());
    let sent = f.run(&["send", "request", "--to", SID, "--mode", "ping", "--json"]);
    assert!(sent.status.success(), "{}", String::from_utf8_lossy(&sent.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&sent.stdout).unwrap()["transport"], "omp-extension");
    let wrong = f.run(&["hook", "omp_wake_heartbeat", "wrong-session"]);
    assert!(wrong.stdout.is_empty());
    assert!(!f.home().join(".everett/inbox/live/wrong-session.json").exists());
    old["pid"] = json!(i32::MAX);
    f.write(&path, &old.to_string());
    assert!(f.run(&["hook", "omp_wake_heartbeat", SID]).stdout.is_empty());
    old["pid"] = json!(std::process::id());
    old["process_identity"] = json!("previous-incarnation");
    f.write(&path, &old.to_string());
    assert!(f.run(&["hook", "omp_wake_heartbeat", SID]).stdout.is_empty());
}

#[test]
fn heartbeat_resolves_relative_profile_and_shared_command_works_from_another_cwd() {
    let f = cli_fixture();
    let profile = "profile 'quoted'";
    let provider = "custom 'provider'";
    f.write(&format!("{profile}/.everett/inbox/live/{SID}.json"), &lease().to_string());
    f.write(&format!("{profile}/.everett/config.toml"), "[auth]\nprovider = \"custom 'provider'\"\n");
    f.write(&format!("{profile}/.everett/inbox/{SID}.jsonl"), &jsonl(&[
        json!({"id":"m123","to":SID,"from":"sender","text":"shared command pickup","ts":now(),"hops":1})]));
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(["hook", "omp_wake_heartbeat", SID])
        .current_dir(f.home()).env("HOME", f.home()).env("EVERETT_HOME", profile)
        .env_remove("EVERETT_AUTH_PROVIDER").output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let heartbeat: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(heartbeat["everett_home"], json!(f.home().canonicalize().unwrap().join(profile)));
    assert_eq!(heartbeat["auth_provider"], provider);
    let elsewhere = tempfile::tempdir().unwrap();
    let pickup = std::process::Command::new("/bin/sh")
        .args(["-c", &format!("{} inbox --session {SID} --message m123", heartbeat["command_prefix"].as_str().unwrap())])
        .current_dir(elsewhere.path()).env("HOME", elsewhere.path())
        .env("EVERETT_HOME", elsewhere.path()).env("EVERETT_AUTH_PROVIDER", "wrong-provider")
        .output().unwrap();
    assert!(pickup.status.success(), "{}", String::from_utf8_lossy(&pickup.stderr));
    assert!(String::from_utf8_lossy(&pickup.stdout).contains("shared command pickup"));
    assert!(f.home().join(format!("{profile}/.everett/inbox/{SID}.done")).exists());
    assert!(!elsewhere.path().join(".everett/inbox").exists());
}

#[test]
fn matching_live_and_lease_records_require_the_current_process_incarnation() {
    let _f = lib_home();
    let root = inbox::inbox_dir();
    std::fs::create_dir_all(root.join("live")).unwrap();
    std::fs::create_dir_all(root.join("omp-wake")).unwrap();
    let publish = |record: &Value| {
        std::fs::write(inbox::live_path(SID), record.to_string()).unwrap();
        std::fs::write(root.join("omp-wake").join(format!("{SID}.json")), record.to_string()).unwrap();
    };
    let mut record = lease();
    publish(&record);
    assert!(omp_wake::available(&target()));
    record["process_identity"] = json!("previous-incarnation");
    publish(&record);
    assert!(!omp_wake::available(&target()), "matching PID records must reject another incarnation");
    record.as_object_mut().unwrap().remove("process_identity");
    publish(&record);
    assert!(!omp_wake::available(&target()), "missing identity must fail closed");
    let mut child = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
    record = lease();
    record["pid"] = json!(child.id());
    record["process_identity"] = json!(everett::proc::process_identity(child.id() as i32).unwrap());
    publish(&record);
    assert!(omp_wake::available(&target()));
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(!omp_wake::available(&target()), "matching records backed by a dead process must be rejected");
}

fn assert_recent_binding_is_repaired(identity: Option<&str>, harness: &str) {
    let f = cli_fixture();
    let path = format!(".everett/inbox/live/{SID}.json");
    let mut record = lease();
    record["state"] = json!("turn");
    record["harness"] = json!(harness);
    match identity {
        Some(identity) => { record["process_identity"] = json!(identity); }
        None => { record.as_object_mut().unwrap().remove("process_identity"); }
    }
    f.write(&path, &record.to_string());
    let bound = f.run(&["hook", "omp_bind", SID]);
    assert!(bound.status.success());
    let heartbeat = f.run(&["hook", "omp_wake_heartbeat", SID]);
    assert!(heartbeat.status.success());
    let settings: Value = serde_json::from_slice(&heartbeat.stdout)
        .expect("a recent binding must support heartbeat immediately after the real bind hook");
    let repaired: Value = serde_json::from_str(&std::fs::read_to_string(f.home().join(path)).unwrap()).unwrap();
    assert_eq!(settings["pid"], record["pid"]);
    assert_eq!(settings["process_identity"], lease()["process_identity"]);
    assert_eq!(repaired["process_identity"], settings["process_identity"]);
    assert_eq!(repaired["harness"], "omp");
}

#[test]
fn recent_legacy_binding_without_identity_is_repaired_before_heartbeat() {
    assert_recent_binding_is_repaired(None, "omp");
}

#[test]
fn recent_binding_with_stale_incarnation_is_repaired_before_heartbeat() {
    assert_recent_binding_is_repaired(Some("previous-incarnation"), "omp");
}

#[test]
fn recent_binding_with_wrong_harness_is_repaired_before_heartbeat() {
    let identity = everett::proc::process_identity(std::process::id() as i32).unwrap();
    assert_recent_binding_is_repaired(Some(&identity), "pi");
}

#[test]
fn overlapping_bind_hooks_leave_one_current_binding_and_working_heartbeat() {
    use fs2::FileExt;
    use std::fs::{self, OpenOptions};
    use std::process::{Child, Command, Output, Stdio};
    use std::time::{Duration, Instant};

    fn wait_until(description: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(Instant::now() < deadline, "timed out waiting for {description}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    struct Hook(Option<Child>);
    impl Hook {
        fn finish(mut self) -> Output {
            wait_until("hook completion", || self.0.as_mut().unwrap().try_wait().unwrap().is_some());
            let output = self.0.take().unwrap().wait_with_output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            output
        }
    }
    impl Drop for Hook {
        fn drop(&mut self) {
            if let Some(child) = &mut self.0 {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    let f = fixture();
    let spawn = |hook: &str, sid: &str| Hook(Some(Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(["hook", hook, sid]).env_clear()
        .env("HOME", f.home()).env("EVERETT_HOME", f.home())
        .env("EVERETT_NOTIFY", "none").env("PATH", common::HERMETIC_PATH)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap()));
    for sid in ["00-barrier", "race-a", "race-b"] {
        f.write(&format!(".everett/inbox/live/{sid}.json"), "{\"pid\":0}");
    }
    let root = f.home().join(".everett/inbox");
    let barrier = OpenOptions::new().create(true).write(true).truncate(false)
        .open(root.join("live/00-barrier.lock")).unwrap();
    barrier.lock_exclusive().unwrap();
    let read = |sid: &str| fs::read(root.join(format!("live/{sid}.json"))).ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
    let published = |sid: &str| read(sid).is_some_and(|record| record["pid"] == std::process::id());
    let mut first = spawn("omp_bind", "race-a");
    wait_until("first binding publication", || published("race-a"));

    // The old implementation can publish both claims before either cleanup proceeds.
    // A serialized binder instead holds the PID lock while waiting on the barrier.
    let binding_path = root.join(format!("bindings/{}.lock", std::process::id()));
    fs::create_dir_all(binding_path.parent().unwrap()).unwrap();
    let binding_lock = OpenOptions::new().create(true).write(true).truncate(false)
        .open(binding_path).unwrap();
    let serialized = match binding_lock.try_lock_exclusive() {
        Ok(()) => { FileExt::unlock(&binding_lock).unwrap(); false }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => true,
        Err(error) => panic!("cannot probe binding lock: {error}"),
    };
    let mut second = spawn("omp_bind", "race-b");
    if !serialized {
        wait_until("second binding publication before cleanup", || published("race-b"));
    }
    assert!(first.0.as_mut().unwrap().try_wait().unwrap().is_none());
    assert!(second.0.as_mut().unwrap().try_wait().unwrap().is_none());
    // Cleanup must release its own session lock before waiting on another session.
    let heartbeat = spawn("omp_wake_heartbeat", "race-a").finish();
    let settings: Value = serde_json::from_slice(&heartbeat.stdout).unwrap();
    assert_eq!(settings["pid"], std::process::id());
    drop(barrier);
    first.finish();
    second.finish();

    let survivors: Vec<_> = ["race-a", "race-b"].into_iter().filter(|sid| published(sid)).collect();
    assert_eq!(survivors.len(), 1, "both successful bind hooks must leave one current binding: {survivors:?}");
    let sid = survivors[0];
    let record = read(sid).unwrap();
    assert_eq!(record["harness"], "omp");
    assert_eq!(record["process_identity"], lease()["process_identity"]);
    let heartbeat = spawn("omp_wake_heartbeat", sid).finish();
    let settings: Value = serde_json::from_slice(&heartbeat.stdout)
        .expect("the surviving binding must support heartbeat after both hooks finish");
    assert_eq!(settings["pid"], record["pid"]);
    assert_eq!(settings["process_identity"], record["process_identity"]);
}
