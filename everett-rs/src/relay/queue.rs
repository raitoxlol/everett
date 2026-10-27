//! Bounded, read-only views of cached receipts. No ciphertext is decrypted and
//! cached delivery observations are never treated as evidence of an agent wake.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

const SCAN_LIMIT: usize = 256;
const MESSAGE_LIMIT: usize = 20;
const FILE_LIMIT: u64 = 256 * 1024;

#[derive(Deserialize)]
struct Receipt {
    peer: String,
    posted: bool,
    state: super::State,
    envelope: Header,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    owner: String,
    from_device: String,
    to_device: String,
    message_id: String,
    created_at: i64,
    expires_at: i64,
}

pub(super) fn read_record<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.mode() & 0o077 != 0 || metadata.len() > FILE_LIMIT {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > FILE_LIMIT {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn read_receipt(path: &Path) -> Option<Receipt> {
    let receipt: Receipt = read_record(path)?;
    if receipt.peer.trim().is_empty()
        || receipt.peer.len() > 640
        || receipt.peer.chars().any(char::is_control)
        || super::valid(&receipt.envelope.message_id).is_err()
        || super::valid(&receipt.envelope.to_device).is_err()
        || receipt.envelope.created_at <= 0
        || receipt.envelope.expires_at <= receipt.envelope.created_at
    {
        return None;
    }
    Some(receipt)
}

pub(super) fn summary(root: &Path, identity: Option<(&str, &str)>, now: i64) -> Value {
    let Some((owner, device)) = identity else {
        return json!({"state":"identity-unavailable","messages":[],"note":"Select an account to inspect its cached outgoing receipts."});
    };
    let mut counts: BTreeMap<&str, usize> =
        ["queued", "unposted", "expired", "delivered", "replied"]
            .into_iter()
            .map(|state| (state, 0))
            .collect();
    let mut messages = Vec::new();
    let mut scanned = 0;
    let mut unreadable = 0;
    let mut ignored = 0;
    let mut complete = true;
    let entries = match fs::read_dir(root.join("outgoing")) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => {
            return json!({"state":"unavailable","messages":[],"note":"Outgoing receipt directory could not be read."})
        }
    };
    if let Some(entries) = entries {
        for (index, entry) in entries.enumerate() {
            if index >= SCAN_LIMIT {
                complete = false;
                break;
            }
            let Ok(entry) = entry else {
                unreadable += 1;
                continue;
            };
            if entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("json")
            {
                continue;
            }
            scanned += 1;
            let Some(receipt) = read_receipt(&entry.path()) else {
                unreadable += 1;
                continue;
            };
            if receipt.envelope.owner != owner || receipt.envelope.from_device != device {
                ignored += 1;
                continue;
            }
            let state = match receipt.state {
                super::State::Queued if receipt.envelope.expires_at <= now => "expired",
                super::State::Queued if !receipt.posted => "unposted",
                super::State::Queued => "queued",
                super::State::Delivered => "delivered",
                super::State::Replied => "replied",
            };
            *counts.get_mut(state).unwrap() += 1;
            messages.push(json!({
                "message_id":receipt.envelope.message_id,
                "peer":receipt.peer,
                "recipient_device":receipt.envelope.to_device,
                "state":state,
                "posted":receipt.posted,
                "created_at":receipt.envelope.created_at,
                "expires_at":receipt.envelope.expires_at
            }));
        }
    }
    messages.sort_by_key(|message| std::cmp::Reverse(message["created_at"].as_i64().unwrap_or(0)));
    let matched = messages.len();
    messages.truncate(MESSAGE_LIMIT);
    json!({
        "state":"readable",
        "source":"local-observations",
        "counts":counts,
        "scanned":scanned,
        "matched":matched,
        "unreadable":unreadable,
        "ignored_other_identities":ignored,
        "complete":complete,
        "messages":messages,
        "messages_limit":MESSAGE_LIMIT,
        "scan_limit":SCAN_LIMIT,
        "note":"Cached observations only. Expired queued requests are not pending. Delivered/replied do not prove automatic wake. Use peer status for current signed server observations."
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn receipt(root: &Path, name: &str, state: &str, posted: bool, expiry: i64) -> Value {
        fs::create_dir_all(root.join("outgoing")).unwrap();
        let value = json!({
            "peer":"Hugo",
            "posted":posted,
            "state":state,
            "envelope":{
                "owner":"owner",
                "fromDevice":"sender",
                "toDevice":"hugo-device",
                "messageId":name,
                "createdAt":1,
                "expiresAt":expiry,
                "ciphertext":"ciphertext-secret-canary"
            },
            "text":"plaintext-secret-canary",
            "session":"sender-session-not-for-output"
        });
        write(root, name, &value);
        value
    }

    fn write(root: &Path, name: &str, value: &Value) {
        let path = root.join("outgoing").join(format!("{name}.json"));
        fs::write(&path, value.to_string()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn cached_queue_separates_unposted_expired_and_observed_delivery() {
        let dir = tempfile::tempdir().unwrap();
        receipt(dir.path(), "queued", "queued", true, 200);
        receipt(dir.path(), "unposted", "queued", false, 200);
        receipt(dir.path(), "expired", "queued", true, 50);
        receipt(dir.path(), "delivered", "delivered", true, 50);
        receipt(dir.path(), "replied", "replied", true, 50);
        let report = summary(dir.path(), Some(("owner", "sender")), 100);
        assert_eq!(report["source"], "local-observations");
        for state in ["queued", "unposted", "expired", "delivered", "replied"] {
            assert_eq!(report["counts"][state], 1, "{state}");
        }
        let text = report.to_string();
        for secret in [
            "ciphertext-secret-canary",
            "plaintext-secret-canary",
            "sender-session-not-for-output",
        ] {
            assert!(!text.contains(secret));
        }
        assert_eq!(report["complete"], true);
    }

    #[test]
    fn other_accounts_and_other_local_devices_are_not_in_the_selected_queue() {
        let dir = tempfile::tempdir().unwrap();
        let mut value = receipt(dir.path(), "foreign", "queued", true, 200);
        value["envelope"]["owner"] = json!("other-owner");
        write(dir.path(), "foreign", &value);
        let mut value = receipt(dir.path(), "other-device", "queued", true, 200);
        value["envelope"]["fromDevice"] = json!("another-device");
        write(dir.path(), "other-device", &value);
        let report = summary(dir.path(), Some(("owner", "sender")), 100);
        assert_eq!(report["ignored_other_identities"], 2);
        assert_eq!(report["messages"], json!([]));
        assert_eq!(report["counts"]["queued"], 0);
    }

    #[test]
    fn malformed_public_oversized_and_symlink_rows_are_not_read_or_echoed() {
        let dir = tempfile::tempdir().unwrap();
        receipt(dir.path(), "bad", "queued", true, 200);
        fs::write(dir.path().join("outgoing/bad.json"), "{malformed-secret").unwrap();
        receipt(dir.path(), "public", "queued", true, 200);
        fs::set_permissions(
            dir.path().join("outgoing/public.json"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        receipt(dir.path(), "large", "queued", true, 200);
        fs::write(
            dir.path().join("outgoing/large.json"),
            vec![b'x'; FILE_LIMIT as usize + 1],
        )
        .unwrap();
        let secret = dir.path().join("outside");
        fs::write(&secret, "outside-secret").unwrap();
        symlink(&secret, dir.path().join("outgoing/link.json")).unwrap();
        let report = summary(dir.path(), Some(("owner", "sender")), 100);
        assert_eq!(report["unreadable"], 4);
        assert_eq!(report["messages"], json!([]));
        assert!(!report.to_string().contains("secret"));
        assert_eq!(fs::read_to_string(secret).unwrap(), "outside-secret");
    }

    #[test]
    fn scan_and_return_limits_are_explicit_and_diagnostics_do_not_create_directories() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let report = summary(&missing, Some(("owner", "sender")), 100);
        assert_eq!(report["scanned"], 0);
        assert!(!missing.exists());
        assert_eq!(
            summary(&missing, None, 100)["state"],
            "identity-unavailable"
        );
        for index in 0..SCAN_LIMIT + 1 {
            receipt(dir.path(), &format!("message-{index}"), "queued", true, 200);
        }
        let report = summary(dir.path(), Some(("owner", "sender")), 100);
        assert_eq!(report["complete"], false);
        assert_eq!(report["scanned"], SCAN_LIMIT);
        assert_eq!(report["messages"].as_array().unwrap().len(), MESSAGE_LIMIT);
    }
}
