//! Local receiver capability evidence. A held lease, not a version string or
//! stale PID file, proves that the running receiver understands the policy.

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{EverettError, Result};

const LEASE: &str = "receiver-capabilities.lock";
const RECORD: &str = "receiver-status.json";
const SAME_OWNER_WEBHOOK: &str = "same-owner-webhook-v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u8,
    owner: String,
    device: String,
    capabilities: Vec<String>,
}

pub(super) struct Lease {
    _lock: File,
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
        let record = Record {
            schema: 1,
            owner: owner.into(),
            device: device.into(),
            capabilities: vec![SAME_OWNER_WEBHOOK.into()],
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| failure("status encoding failed"))?;
        let status = root.join(RECORD);
        // The capability record is advisory. A failed write must not release the
        // lease or stop delivery, and it must not leave an older owner/device
        // looking like the live receiver.
        if let Err(error) = crate::peer::save(&status, &bytes) {
            discard_status(&status);
            eprintln!(
                "everett relay: receiver status could not be written; delivery continues: {}",
                error.message
            );
        }
        Ok(Self { _lock: lock })
    }
}

fn discard_status(path: &Path) {
    if std::fs::remove_file(path).is_ok() {
        return;
    }
    // Unlink fails when the directory is not writable. Wipe a regular file in
    // place, without following a symlink, so the previous identity is gone.
    let Ok(file) = OpenOptions::new()
        .write(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    else {
        return;
    };
    let _ = file.sync_all();
}

fn capability_record(root: &Path) -> Result<Option<Record>> {
    if !held(&root.join(LEASE))? {
        return Ok(None);
    }
    let Some(file) = private_read(&root.join(RECORD))? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(16_385)
        .read_to_end(&mut bytes)
        .map_err(|_| failure("local status file is not safely readable"))?;
    if bytes.len() > 16_384 {
        return Ok(None);
    }
    Ok(serde_json::from_slice(&bytes).ok())
}

pub(crate) fn snapshot(root: &Path, identity: Option<(&str, &str)>) -> Result<Value> {
    if !held(&root.join("daemon.lock"))? {
        return Ok(json!({"state":"stopped"}));
    }
    let Some(record) = capability_record(root)? else {
        return Ok(json!({
            "state":"unknown",
            "reason":"The running receiver does not advertise capabilities.",
            "next":"Upgrade and restart the actual receiver before changing wake policy."
        }));
    };
    if record.schema != 1 {
        return Ok(json!({"state":"unknown","reason":"Running receiver status is missing or invalid."}));
    }
    if !identity.is_some_and(|(owner, device)| owner == record.owner && device == record.device) {
        return Ok(json!({
            "state":"identity-mismatch",
            "reason":"Receiver identity does not match this CLI's selected account and device.",
            "next":"Restart the receiver using the current account and the same Everett home."
        }));
    }
    Ok(json!({
        "state":"running",
        "capabilities":record.capabilities,
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
        let lease = Lease::start(dir.path(), "owner", "device").unwrap();
        let first = snapshot(dir.path(), Some(("owner", "device"))).unwrap();
        assert_eq!(first["state"], "running");
        assert_eq!(first["capabilities"], json!(["same-owner-webhook-v1"]));
        assert!(first.get("connection").is_none());
        assert!(first.get("pid").is_none());
        assert!(first.get("binary").is_none());
        assert!(first.get("check_age_ms").is_none());
        assert!(first.get("note").is_none());
        require_account_policy(dir.path(), "owner", "device").unwrap();
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

    #[test]
    fn failed_status_write_does_not_abort_delivery_or_drop_the_lease() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("receiver");
        fs::create_dir(&root).unwrap();
        let _daemon = daemon(&root);
        fs::create_dir(root.join(RECORD)).unwrap();
        let lease = Lease::start(&root, "owner", "device").unwrap();
        assert!(
            root.join(RECORD).is_dir(),
            "failed status write must leave the existing path in place"
        );
        assert!(Lease::start(&root, "owner", "device").is_err());
        assert!(snapshot(&root, Some(("owner", "device"))).is_err());
        assert!(require_account_policy(&root, "owner", "device").is_err());
        drop(lease);
        let restarted = Lease::start(&root, "owner", "device").unwrap();
        assert!(root.join(RECORD).is_dir());
        drop(restarted);
    }

    #[test]
    fn failed_status_write_rejects_a_stale_identity_record() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("receiver");
        fs::create_dir(&root).unwrap();
        let _daemon = daemon(&root);
        let status = root.join(RECORD);
        fs::write(
            &status,
            json!({
                "schema": 1,
                "owner": "previous-owner",
                "device": "previous-device",
                "capabilities": ["same-owner-webhook-v1"]
            })
            .to_string(),
        )
        .unwrap();
        fs::set_permissions(&status, fs::Permissions::from_mode(0o600)).unwrap();
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(root.join(LEASE))
            .unwrap();
        // The lease file can still be locked, but a replacement status file
        // cannot be created. The previous record must not survive that failure.
        fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).unwrap();
        let _restore = RestoreWrite(&root);
        let lease = Lease::start(&root, "current-owner", "current-device").unwrap();
        let reported = snapshot(&root, Some(("previous-owner", "previous-device"))).unwrap();
        assert_eq!(reported["state"], "unknown");
        assert!(!reported.to_string().contains("previous-owner"));
        assert!(!reported.to_string().contains("previous-device"));
        assert!(require_account_policy(&root, "previous-owner", "previous-device").is_err());
        assert!(require_account_policy(&root, "current-owner", "current-device").is_err());
        let left = fs::read_to_string(&status).unwrap_or_default();
        assert!(!left.contains("previous-owner"));
        assert!(Lease::start(&root, "current-owner", "current-device").is_err());
        drop(lease);
    }

    struct RestoreWrite<'a>(&'a Path);

    impl Drop for RestoreWrite<'_> {
        fn drop(&mut self) {
            let _ = fs::set_permissions(self.0, fs::Permissions::from_mode(0o700));
        }
    }
}
