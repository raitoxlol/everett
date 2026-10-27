//! Local receiver capability evidence. A held lease, not a version string or
//! stale PID file, proves that the running receiver understands the policy.

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{EverettError, Result};

const LEASE: &str = "receiver-capabilities.lock";
const RECORD: &str = "receiver-status.json";
const SAME_OWNER_WEBHOOK: &str = "same-owner-webhook-v1";
const STALE_MS: i64 = 90_000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u8,
    pid: u32,
    version: String,
    binary: String,
    owner: String,
    device: String,
    capabilities: Vec<String>,
    started_at: i64,
    checked_at: i64,
    connection: String,
}

pub(super) struct Lease {
    _lock: File,
    root: PathBuf,
    record: Record,
}

fn failure(message: &str) -> EverettError {
    EverettError::new(2, format!("relay receiver: {message}"))
}

fn private_read(path: &Path) -> Result<Option<File>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(failure("local status file is not safely readable")),
    };
    let metadata = file
        .metadata()
        .map_err(|_| failure("local status metadata is unavailable"))?;
    if !metadata.is_file() || metadata.mode() & 0o077 != 0 {
        return Err(failure("local status files must be private regular files"));
    }
    Ok(Some(file))
}

fn held(path: &Path) -> Result<bool> {
    let Some(file) = private_read(path)? else {
        return Ok(false);
    };
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(true),
        Err(_) => Err(failure("receiver lock could not be inspected")),
    }
}

impl Lease {
    pub(super) fn start(root: &Path, owner: &str, device: &str) -> Result<Self> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(root.join(LEASE))
            .map_err(|_| failure("capability lease could not be opened"))?;
        let metadata = lock
            .metadata()
            .map_err(|_| failure("capability lease metadata is unavailable"))?;
        if !metadata.is_file() || metadata.mode() & 0o077 != 0 {
            return Err(failure("capability lease must be a private regular file"));
        }
        FileExt::try_lock_exclusive(&lock)
            .map_err(|_| failure("another capability-aware receiver is already running"))?;
        let now = super::milliseconds();
        let lease = Self {
            _lock: lock,
            root: root.into(),
            record: Record {
                schema: 1,
                pid: std::process::id(),
                version: env!("CARGO_PKG_VERSION").into(),
                binary: std::env::current_exe()
                    .map_err(|_| failure("receiver executable path is unavailable"))?
                    .to_string_lossy()
                    .into(),
                owner: owner.into(),
                device: device.into(),
                capabilities: vec![SAME_OWNER_WEBHOOK.into()],
                started_at: now,
                checked_at: now,
                connection: "starting".into(),
            },
        };
        lease.save()?;
        Ok(lease)
    }

    fn save(&self) -> Result<()> {
        crate::peer::save(
            &self.root.join(RECORD),
            &serde_json::to_vec(&self.record).map_err(|_| failure("status encoding failed"))?,
        )
    }

    fn checked(&mut self, healthy: bool) -> Result<()> {
        self.record.checked_at = super::milliseconds();
        self.record.connection = if healthy { "healthy" } else { "retrying" }.into();
        self.save()
    }

    pub(super) fn observe(&mut self, healthy: bool) -> Result<()> {
        self.checked(healthy)
    }
}

pub(crate) fn snapshot(root: &Path, identity: Option<(&str, &str)>) -> Result<Value> {
    if !held(&root.join("daemon.lock"))? {
        return Ok(
            json!({"state":"stopped","note":"No receiver is holding the local daemon lock."}),
        );
    }
    if !held(&root.join(LEASE))? {
        return Ok(json!({
            "state":"unknown",
            "reason":"The running receiver does not advertise capabilities.",
            "next":"Upgrade and restart the actual receiver before changing wake policy."
        }));
    }
    let record = private_read(&root.join(RECORD))?.and_then(|file| {
        let mut bytes = Vec::new();
        file.take(16_385).read_to_end(&mut bytes).ok()?;
        (bytes.len() <= 16_384)
            .then(|| serde_json::from_slice::<Record>(&bytes).ok())
            .flatten()
    });
    let Some(record) = record.filter(|record| {
        record.schema == 1
            && record.pid > 0
            && record.started_at > 0
            && record.checked_at >= record.started_at
            && record.checked_at <= super::milliseconds().saturating_add(30_000)
            && ["starting", "healthy", "retrying"].contains(&record.connection.as_str())
    }) else {
        return Ok(
            json!({"state":"unknown","reason":"Running receiver status is missing or invalid."}),
        );
    };
    if !identity.is_some_and(|(owner, device)| owner == record.owner && device == record.device) {
        return Ok(json!({
            "state":"identity-mismatch",
            "reason":"Receiver identity does not match this CLI's selected account and device.",
            "next":"Restart the receiver using the current account and the same Everett home."
        }));
    }
    let age = super::milliseconds()
        .saturating_sub(record.checked_at)
        .max(0);
    Ok(json!({
        "state":"running",
        "connection":if age > STALE_MS { "stale" } else { record.connection.as_str() },
        "pid":record.pid,
        "version":record.version,
        "binary":record.binary,
        "capabilities":record.capabilities,
        "started_at":record.started_at,
        "checked_at":record.checked_at,
        "check_age_ms":age,
        "note":"Receiver capability and polling health do not prove agent wake or message consumption."
    }))
}

pub(crate) fn require_account_policy(root: &Path, owner: &str, device: &str) -> Result<()> {
    let status = snapshot(root, Some((owner, device)))?;
    if status["state"] == "stopped"
        || (status["state"] == "running"
            && status["capabilities"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item == SAME_OWNER_WEBHOOK)))
    {
        return Ok(());
    }
    Err(failure(
        "running receiver has not proven same-account webhook support; upgrade and restart it first. No wake policy changed.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn daemon(root: &Path) -> File {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join("daemon.lock"))
            .unwrap();
        FileExt::try_lock_exclusive(&file).unwrap();
        file
    }

    #[test]
    fn stopped_status_is_read_only_and_stale_records_are_not_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert_eq!(snapshot(&missing, None).unwrap()["state"], "stopped");
        assert!(!missing.exists());
        let lease = Lease::start(dir.path(), "owner", "device").unwrap();
        drop(lease);
        assert_eq!(
            snapshot(dir.path(), Some(("owner", "device"))).unwrap()["state"],
            "stopped"
        );
        require_account_policy(dir.path(), "owner", "device").unwrap();
    }

    #[test]
    fn a_running_legacy_receiver_blocks_migration_even_with_a_stale_capability_record() {
        let dir = tempfile::tempdir().unwrap();
        let lease = Lease::start(dir.path(), "owner", "device").unwrap();
        drop(lease);
        let _daemon = daemon(dir.path());
        let status = snapshot(dir.path(), Some(("owner", "device"))).unwrap();
        assert_eq!(status["state"], "unknown");
        assert!(require_account_policy(dir.path(), "owner", "device").is_err());
    }

    #[test]
    fn held_capability_lease_proves_support_independently_of_release_version() {
        let dir = tempfile::tempdir().unwrap();
        let _daemon = daemon(dir.path());
        let mut lease = Lease::start(dir.path(), "owner", "device").unwrap();
        let first = snapshot(dir.path(), Some(("owner", "device"))).unwrap();
        assert_eq!(first["state"], "running");
        assert_eq!(first["connection"], "starting");
        require_account_policy(dir.path(), "owner", "device").unwrap();
        lease.checked(false).unwrap();
        assert_eq!(
            snapshot(dir.path(), Some(("owner", "device"))).unwrap()["connection"],
            "retrying"
        );
        lease.checked(true).unwrap();
        assert_eq!(
            snapshot(dir.path(), Some(("owner", "device"))).unwrap()["connection"],
            "healthy"
        );
        assert!(Lease::start(dir.path(), "owner", "device").is_err());
        drop(lease);
        assert!(require_account_policy(dir.path(), "owner", "device").is_err());
    }

    #[test]
    fn changed_or_missing_cli_identity_never_proves_compatibility() {
        let dir = tempfile::tempdir().unwrap();
        let _daemon = daemon(dir.path());
        let _lease = Lease::start(dir.path(), "owner", "device").unwrap();
        for identity in [
            None,
            Some(("other-owner", "device")),
            Some(("owner", "other-device")),
        ] {
            assert_eq!(
                snapshot(dir.path(), identity).unwrap()["state"],
                "identity-mismatch"
            );
        }
        assert!(require_account_policy(dir.path(), "other-owner", "device").is_err());
        assert!(require_account_policy(dir.path(), "owner", "other-device").is_err());
    }

    #[test]
    fn stale_polling_is_distinct_from_running_and_policy_format_support() {
        let dir = tempfile::tempdir().unwrap();
        let _daemon = daemon(dir.path());
        let mut lease = Lease::start(dir.path(), "owner", "device").unwrap();
        lease.record.started_at -= STALE_MS * 2;
        lease.record.checked_at -= STALE_MS * 2;
        lease.save().unwrap();
        let status = snapshot(dir.path(), Some(("owner", "device"))).unwrap();
        assert_eq!(status["state"], "running");
        assert_eq!(status["connection"], "stale");
        require_account_policy(dir.path(), "owner", "device").unwrap();
    }

    #[test]
    fn malformed_or_oversized_records_do_not_leak_contents_or_prove_support() {
        let dir = tempfile::tempdir().unwrap();
        let _daemon = daemon(dir.path());
        let _lease = Lease::start(dir.path(), "owner", "device").unwrap();
        for bytes in [b"{secret-canary".to_vec(), vec![b'x'; 16_385]] {
            fs::write(dir.path().join(RECORD), bytes).unwrap();
            let status = snapshot(dir.path(), Some(("owner", "device"))).unwrap();
            assert_eq!(status["state"], "unknown");
            assert!(!status.to_string().contains("secret-canary"));
            assert!(require_account_policy(dir.path(), "owner", "device").is_err());
        }
    }

    #[test]
    fn symlinks_and_public_status_files_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::write(&outside, "do not touch").unwrap();
        symlink(&outside, dir.path().join(LEASE)).unwrap();
        assert!(Lease::start(dir.path(), "owner", "device").is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "do not touch");
        let _daemon = daemon(dir.path());
        assert!(snapshot(dir.path(), Some(("owner", "device"))).is_err());
        fs::remove_file(dir.path().join(LEASE)).unwrap();
        let lease = Lease::start(dir.path(), "owner", "device").unwrap();
        fs::set_permissions(dir.path().join(RECORD), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(snapshot(dir.path(), Some(("owner", "device"))).is_err());
        drop(lease);
    }
}
