use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct KillRecorded {
    marker: PathBuf,
    everett: Option<u32>,
}

impl Drop for KillRecorded {
    fn drop(&mut self) {
        if let Some(pid) = self.everett {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        if let Ok(text) = fs::read_to_string(&self.marker) {
            for line in text.lines() {
                if let Some(pid) = line.strip_prefix("pid ") {
                    let _ = Command::new("kill")
                        .args(["-KILL", pid])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            }
        }
    }
}

fn write_exec(path: &std::path::Path, body: &str) {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o755)
        .open(path)
        .unwrap();
    file.write_all(body.as_bytes()).unwrap();
}

fn write_secret(path: &std::path::Path, body: &str, mode: u32) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn loose_env_file_is_refused_before_start() {
    let dir = tempfile::tempdir().unwrap();
    let env_file = dir.path().join("tunnel.env");
    write_secret(&env_file, "CONTROL_PLANE_API_KEY=test-runtime-key\n", 0o644);
    let output = Command::new(env!("CARGO_BIN_EXE_everett"))
        .args([
            "tunnel",
            "watch",
            "--env-file",
            env_file.to_str().unwrap(),
            "--tunnel-client",
            "tunnel-client",
            "--max-restarts",
            "0",
        ])
        .env("OPENAI_ADMIN_KEY", "admin-test-secret")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("chmod 0600"), "{stderr}");
    assert!(!stderr.contains("test-runtime-key"), "{stderr}");
    assert!(!stderr.contains("admin-test-secret"), "{stderr}");
}

#[test]
fn watch_restarts_after_poll_timeouts_and_hides_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");
    let env_file = dir.path().join("tunnel.env");
    let client = dir.path().join("fake-tunnel");
    write_secret(&env_file, "CONTROL_PLANE_API_KEY=test-runtime-key\n", 0o600);
    write_exec(
        &client,
        &format!(
            r#"#!/bin/sh
marker={marker:?}
printf 'pid %s\n' "$$" >> "$marker"
printf 'start\n' >> "$marker"
if [ -n "${{OPENAI_ADMIN_KEY:-}}" ]; then
  printf 'admin\n' >> "$marker"
fi
if [ "${{CONTROL_PLANE_API_KEY:-}}" = "test-runtime-key" ]; then
  printf 'runtime\n' >> "$marker"
else
  printf 'runtime-missing\n' >> "$marker"
fi
echo "CONTROL_PLANE_API_KEY=${{CONTROL_PLANE_API_KEY:-}}"
echo "OPENAI_ADMIN_KEY=${{OPENAI_ADMIN_KEY:-missing}}"
starts=$(grep -c '^start$' "$marker" || true)
if [ "$starts" -ge 2 ]; then
  exit 0
fi
echo 'poll timed out; backing off error="controlplane poll transport failed (timeout)"'
echo 'poll timed out; backing off error="controlplane poll transport failed (timeout)"'
echo 'poll timed out; backing off error="controlplane poll transport failed (timeout)"'
sleep 30
"#,
            marker = marker.display()
        ),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_everett"))
        .args([
            "tunnel",
            "watch",
            "--profile",
            "everett-dot",
            "--tunnel-client",
            client.to_str().unwrap(),
            "--env-file",
            env_file.to_str().unwrap(),
            "--timeout-streak",
            "3",
            "--restart-delay",
            "0",
            "--max-restarts",
            "1",
        ])
        .env("OPENAI_ADMIN_KEY", "admin-test-secret")
        .env_remove("CONTROL_PLANE_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut guard = KillRecorded {
        marker: marker.clone(),
        everett: Some(child.id()),
    };
    let start = Instant::now();
    let finished = loop {
        if start.elapsed() > Duration::from_secs(8) {
            let _ = child.kill();
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(error) => panic!("{error}"),
        }
    };
    let output = child.wait_with_output().expect("reap everett");
    guard.everett = None;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let status = match finished {
        Some(status) => status,
        None => panic!("watch did not finish\n{stderr}"),
    };
    assert!(status.success(), "{stderr}");
    assert!(
        stderr.contains("restarting tunnel-client after 3 consecutive poll timeouts"),
        "{stderr}"
    );
    assert!(!stderr.contains("test-runtime-key"), "{stderr}");
    assert!(!stderr.contains("admin-test-secret"), "{stderr}");
    assert!(
        stderr.contains("CONTROL_PLANE_API_KEY=[redacted]"),
        "{stderr}"
    );
    let record = fs::read_to_string(&marker).unwrap();
    assert_eq!(record.matches("start").count(), 2, "{record}");
    assert_eq!(record.matches("runtime").count(), 2, "{record}");
    assert!(!record.contains("admin"), "{record}");
    assert!(!record.contains("runtime-missing"), "{record}");
}

#[test]
fn exited_client_descendants_do_not_block_restart() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");
    let env_file = dir.path().join("tunnel.env");
    let client = dir.path().join("fake-tunnel");
    write_secret(&env_file, "CONTROL_PLANE_API_KEY=test-runtime-key\n", 0o600);
    write_exec(
        &client,
        &format!(
            r#"#!/bin/sh
printf 'pid %s\n' "$$" >> {marker:?}
sleep 30 &
printf 'pid %s\n' "$!" >> {marker:?}
exit 1
"#,
            marker = marker.display()
        ),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_everett"))
        .args([
            "tunnel",
            "watch",
            "--env-file",
            env_file.to_str().unwrap(),
            "--tunnel-client",
            client.to_str().unwrap(),
            "--restart-delay",
            "0",
            "--max-restarts",
            "1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut guard = KillRecorded {
        marker: marker.clone(),
        everett: Some(child.id()),
    };
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            guard.everett = None;
            assert!(status.success());
            assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 4);
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(6),
            "descendant held watcher pipes open"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[test]
fn watch_forwards_termination_to_client() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");
    let env_file = dir.path().join("tunnel.env");
    let client = dir.path().join("fake-tunnel");
    write_secret(&env_file, "CONTROL_PLANE_API_KEY=test-runtime-key\n", 0o600);
    write_exec(
        &client,
        &format!(
            r#"#!/bin/sh
marker={marker:?}
printf 'pid %s\n' "$$" >> "$marker"
printf 'started\n' >> "$marker"
trap 'printf "graceful\n" >> "$marker"; exit 0' TERM INT
while :; do sleep 1; done
"#,
            marker = marker.display()
        ),
    );
    let mut watcher = Command::new(env!("CARGO_BIN_EXE_everett"))
        .args([
            "tunnel",
            "watch",
            "--env-file",
            env_file.to_str().unwrap(),
            "--tunnel-client",
            client.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut guard = KillRecorded {
        marker: marker.clone(),
        everett: Some(watcher.id()),
    };
    let start = Instant::now();
    while !fs::read_to_string(&marker)
        .unwrap_or_default()
        .contains("started")
    {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "client did not start"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let status = Command::new("kill")
        .args(["-TERM", &watcher.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let status = watcher.wait().unwrap();
    guard.everett = None;
    assert!(status.success());
    assert!(
        fs::read_to_string(&marker).unwrap().contains("graceful"),
        "client did not handle SIGTERM gracefully"
    );
}
