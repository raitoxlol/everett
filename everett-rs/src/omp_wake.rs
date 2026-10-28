use std::collections::HashSet;
use std::fs;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{inbox, session::Session};

pub const LEASE_SECONDS: f64 = 20.0;

#[derive(Deserialize)]
struct Lease {
    version: u32,
    session_id: String,
    harness: String,
    pid: i32,
    ts: f64,
    process_identity: String,
}

/// Only a running watcher advertises this capability, not an installed file.
pub fn available(session: &Session) -> bool {
    if session.harness != "omp" || session.source == "t3code" || !inbox::valid_id(&session.id) {
        return false;
    }
    let path = inbox::inbox_dir().join("omp-wake").join(format!("{}.json", session.id));
    let Some(lease) = fs::read(path).ok().and_then(|raw| serde_json::from_slice::<Lease>(&raw).ok()) else {
        return false;
    };
    let age = crate::session::now() - lease.ts;
    lease.version == 1
        && lease.session_id == session.id
        && lease.harness == "omp"
        && lease.pid > 0
        && age.is_finite()
        && (0.0..=LEASE_SECONDS).contains(&age)
        && crate::proc::process_identity(lease.pid).as_deref() == Some(lease.process_identity.as_str())
        && inbox::live(&session.id).is_some_and(|live| {
            live.get("pid").and_then(Value::as_i64) == Some(i64::from(lease.pid))
                && live.get("harness").and_then(Value::as_str) == Some("omp")
                && live.get("process_identity").and_then(Value::as_str) == Some(lease.process_identity.as_str())
        })
}

/// Refresh only the caller's existing binding, preserving idle state and message receipts.
pub fn heartbeat(session_id: &str) -> Option<Value> {
    heartbeat_after_read(session_id, || {})
}

fn heartbeat_after_read(session_id: &str, after_read: impl FnOnce()) -> Option<Value> {
    if !inbox::valid_id(session_id) {
        return None;
    }
    let pid = inbox::harness_pid();
    let identity = crate::proc::process_identity(pid)?;
    let _guard = inbox::lock_live(session_id).ok()?;
    let path = inbox::live_path(session_id);
    let mut live: Map<String, Value> = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    after_read();
    if live.get("pid").and_then(Value::as_i64) != Some(i64::from(pid))
        || live.get("harness").and_then(Value::as_str) != Some("omp")
        || live.get("process_identity").and_then(Value::as_str) != Some(identity.as_str())
    {
        return None;
    }
    let command = crate::claude_wake::NudgeCommand::resolve().ok()?;
    live.insert("ts".into(), json!(crate::session::now()));
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, serde_json::to_vec(&live).ok()?).ok()?;
    if fs::rename(&temporary, path).is_err() {
        let _ = fs::remove_file(temporary);
        return None;
    }
    Some(json!({"pid":pid, "process_identity":identity, "executable":command.executable,
        "everett_home":command.everett_home, "auth_provider":command.auth_provider,
        "command_prefix":command.prefix()}))
}

/// Nudge attempts are separate from .done: only reading the inbox consumes data.
pub fn nudged_ids(session_id: &str) -> HashSet<String> {
    if !inbox::valid_id(session_id) {
        return HashSet::new();
    }
    let mut ids: HashSet<String> = fs::read_to_string(inbox::inbox_dir().join(format!("{session_id}.omp-nudged")))
        .map(|text| text.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    if let Ok(claims) = fs::read_dir(inbox::inbox_dir().join("omp-wake").join(format!("{session_id}.nudged"))) {
        ids.extend(claims.flatten().filter_map(|claim| claim.file_name().into_string().ok()));
    }
    ids
}

/// Return IDs only. Remote-derived and external messages retain passive pickup.
pub fn pending_ids(session_id: &str) -> Vec<String> {
    let nudged = nudged_ids(session_id);
    inbox::pending(session_id, None)
        .into_iter()
        .filter(|m| {
            !inbox::external(m)
                && !m.contains_key("relay_message_id")
                && !m.contains_key("relay_lineage")
                && m.get("from").and_then(Value::as_str).and_then(inbox::relay_context).is_none()
                && m.get("to").and_then(Value::as_str) == Some(session_id)
        })
        .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_owned))
        .filter(|id| !nudged.contains(id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs2::FileExt;
    use std::fs::OpenOptions;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn heartbeat_preserves_concurrent_hop_and_activity_updates() {
        if !crate::test_support::isolated("omp_wake::tests::heartbeat_preserves_concurrent_hop_and_activity_updates") {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("EVERETT_HOME", home.path());
        let sid = "heartbeat-interleaving";
        let path = inbox::live_path(sid);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for activity in [false, true] {
            let pid = inbox::harness_pid();
            let identity = crate::proc::process_identity(pid).unwrap();
            fs::write(&path, json!({"pid":pid,"process_identity":identity,"harness":"omp",
                "state":"turn","ts":crate::session::now(),"hops":0,"hops_ts":0}).to_string()).unwrap();
            let (read_tx, read_rx) = mpsc::channel();
            let (resume_tx, resume_rx) = mpsc::channel();
            let heartbeat = std::thread::spawn(move || heartbeat_after_read(sid, || {
                read_tx.send(()).unwrap();
                resume_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            }));
            read_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            let lock = OpenOptions::new().create(true).write(true).truncate(false)
                .open(path.with_extension("lock")).unwrap();
            let serialized = match lock.try_lock_exclusive() {
                Ok(()) => { FileExt::unlock(&lock).unwrap(); false }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => true,
                Err(error) => panic!("cannot probe live-record lock: {error}"),
            };
            let write = move || {
                inbox::record_hops(sid, 3);
                if activity { inbox::touch_live(sid, "omp", "idle"); }
            };
            // Without serialization, complete the writer while heartbeat holds its old snapshot.
            // With serialization, the writer must wait until heartbeat releases the record.
            let writer = if serialized {
                Some(std::thread::spawn(write))
            } else {
                write();
                None
            };
            resume_tx.send(()).unwrap();
            assert!(heartbeat.join().unwrap().is_some());
            if let Some(writer) = writer { writer.join().unwrap(); }
            let live: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(live["hops"], 3, "heartbeat must not restore its old hop snapshot");
            assert!(live["hops_ts"].as_f64().unwrap() > 0.0);
            assert_eq!(live["state"], if activity { "idle" } else { "turn" });
            assert_eq!(inbox::session_hops(sid), 3);
            assert!(serialized, "heartbeat must hold the shared live-record lock after reading");
        }
    }
}
