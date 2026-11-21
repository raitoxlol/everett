use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{EverettError, Result};
use crate::session::home;

const MAX_FILE_BYTES: u64 = 64 * 1024;
const DEFAULT_MAX_HOPS: i64 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccessMode {
    Deny,
    SameAccount,
    Allowlist,
}

impl AccessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deny => "deny",
            Self::SameAccount => "same-account",
            Self::Allowlist => "allowlist",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim() {
            "deny" => Ok(Self::Deny),
            "same-account" => Ok(Self::SameAccount),
            "allowlist" => Ok(Self::Allowlist),
            _ => Err(bad("mode must be deny, same-account, or allowlist")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SenderRule {
    pub owner: String,
    pub device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockRule {
    pub owner: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessPolicy {
    pub version: u8,
    pub mode: AccessMode,
    pub allow_external_send: bool,
    pub max_hops: i64,
    #[serde(default)]
    pub allowed_senders: Vec<SenderRule>,
    #[serde(default)]
    pub blocked_senders: Vec<BlockRule>,
}

impl Default for AccessPolicy {
    fn default() -> Self {
        Self {
            version: 1,
            mode: AccessMode::Allowlist,
            allow_external_send: true,
            max_hops: DEFAULT_MAX_HOPS,
            allowed_senders: Vec::new(),
            blocked_senders: Vec::new(),
        }
    }
}

fn bad(message: impl std::fmt::Display) -> EverettError {
    EverettError::new(2, format!("access: {message}"))
}

fn valid_owner(owner: &str) -> Result<()> {
    if owner.is_empty() || owner.len() > 4096 || owner.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(bad("invalid owner identifier"));
    }
    Ok(())
}

fn valid_device(device: &str) -> Result<()> {
    if device.is_empty()
        || device.len() > 160
        || !device
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
    {
        return Err(bad("invalid device identifier"));
    }
    Ok(())
}

fn valid_note(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 160 || value.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(bad(format!("invalid {field}")));
    }
    Ok(())
}

pub fn default_path() -> PathBuf {
    home().join(".everett").join("access.json")
}

pub fn resolve_path(custom: Option<&Path>) -> Result<PathBuf> {
    match custom {
        Some(path) if path.is_absolute() => Ok(path.to_path_buf()),
        Some(_) => Err(bad("--config path must be absolute")),
        None => Ok(default_path()),
    }
}

impl AccessPolicy {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(bad("unsupported access policy version"));
        }
        if !(1..=DEFAULT_MAX_HOPS).contains(&self.max_hops) {
            return Err(bad("max_hops must be between 1 and 3"));
        }
        for rule in &self.allowed_senders {
            valid_owner(&rule.owner)?;
            valid_device(&rule.device)?;
            if let Some(address) = &rule.address {
                if !crate::relay::inbox_address(address) {
                    return Err(bad("invalid inbox address in allowed_senders"));
                }
            }
            if let Some(label) = &rule.label {
                valid_note(label, "label")?;
            }
        }
        for rule in &self.blocked_senders {
            valid_owner(&rule.owner)?;
            if let Some(device) = &rule.device {
                valid_device(device)?;
            }
            if let Some(reason) = &rule.reason {
                valid_note(reason, "reason")?;
            }
        }
        Ok(())
    }
}

pub fn load(path: &Path) -> Result<AccessPolicy> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(AccessPolicy::default()),
        Err(_) => return Err(bad("cannot open access policy")),
    };
    let metadata = file
        .metadata()
        .map_err(|_| bad("cannot inspect access policy"))?;
    if !metadata.is_file() || metadata.mode() & 0o077 != 0 {
        return Err(bad("access policy must be a private regular file (0600)"));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(bad("access policy exceeds size limit"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| bad("cannot read access policy"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(bad("access policy exceeds size limit"));
    }
    let policy: AccessPolicy =
        serde_json::from_slice(&bytes).map_err(|_| bad("invalid access policy JSON"))?;
    policy.validate()?;
    Ok(policy)
}

pub fn save(path: &Path, policy: &AccessPolicy) -> Result<()> {
    policy.validate()?;
    let mut encoded = serde_json::to_vec_pretty(policy).map_err(|_| bad("encoding failed"))?;
    encoded.push(b'\n');
    crate::storage::save(path, &encoded)
}

pub fn check_outgoing_cross_org() -> Result<()> {
    let policy = load(&default_path())?;
    if policy.mode == AccessMode::Deny || !policy.allow_external_send {
        return Err(EverettError::new(
            3,
            "cross-org sends are disabled by local access policy; run `everett access set --mode allowlist --external-send on` to allow",
        ));
    }
    Ok(())
}

pub fn check_incoming_cross_org(
    sender_owner: &str,
    sender_device: &str,
    target_address: Option<&str>,
    hop: i64,
    is_reply: bool,
) -> Result<()> {
    let policy = load(&default_path())?;
    if matches!(policy.mode, AccessMode::Deny | AccessMode::SameAccount) {
        return Err(EverettError::new(
            3,
            format!(
                "cross-org messages are disabled by local access policy (mode: {})",
                policy.mode.as_str()
            ),
        ));
    }
    if policy.blocked_senders.iter().any(|rule| {
        rule.owner == sender_owner
            && rule
                .device
                .as_deref()
                .is_none_or(|device| device == sender_device)
    }) {
        return Err(EverettError::new(
            3,
            "sender is blocked by local access policy",
        ));
    }
    if hop > policy.max_hops {
        return Err(EverettError::new(
            7,
            format!(
                "cross-org hop count {hop} exceeds local access max_hops ({})",
                policy.max_hops
            ),
        ));
    }
    if !is_reply && !policy.allowed_senders.is_empty() {
        let matched = policy.allowed_senders.iter().any(|rule| {
            rule.owner == sender_owner
                && rule.device == sender_device
                && rule
                    .address
                    .as_deref()
                    .is_none_or(|addr| target_address == Some(addr))
        });
        if !matched {
            return Err(EverettError::new(
                3,
                "sender is not in local access allowlist",
            ));
        }
    }
    Ok(())
}

pub fn show(custom: Option<&Path>) -> Result<(PathBuf, AccessPolicy)> {
    let path = resolve_path(custom)?;
    let policy = load(&path)?;
    Ok((path, policy))
}

pub fn set_settings(
    custom: Option<&Path>,
    mode: Option<&str>,
    external_send: Option<bool>,
    max_hops: Option<i64>,
) -> Result<(PathBuf, AccessPolicy)> {
    let path = resolve_path(custom)?;
    let mut policy = load(&path)?;
    if let Some(raw_mode) = mode {
        policy.mode = AccessMode::parse(raw_mode)?;
    }
    if let Some(enabled) = external_send {
        policy.allow_external_send = enabled;
    }
    if let Some(hops) = max_hops {
        policy.max_hops = hops;
    }
    save(&path, &policy)?;
    Ok((path, policy))
}

pub fn allow_sender(
    custom: Option<&Path>,
    owner: &str,
    device: &str,
    address: Option<&str>,
    label: Option<&str>,
    sync_relay: bool,
) -> Result<(PathBuf, AccessPolicy, Option<Value>)> {
    if sync_relay && address.is_none() {
        return Err(bad("--sync requires --address <local@scope>"));
    }
    let path = resolve_path(custom)?;
    let mut policy = load(&path)?;
    let rule = SenderRule {
        owner: owner.to_string(),
        device: device.to_string(),
        address: address.map(|s| s.to_string()),
        label: label.map(|s| s.to_string()),
    };
    policy
        .blocked_senders
        .retain(|b| !(b.owner == owner && b.device.as_deref() == Some(device)));
    if let Some(existing) = policy
        .allowed_senders
        .iter_mut()
        .find(|r| r.owner == owner && r.device == device && r.address == rule.address)
    {
        existing.label = rule.label;
    } else {
        policy.allowed_senders.push(rule);
    }
    policy.validate()?;
    let grant = if sync_relay {
        Some(crate::relay::grant_inbox(
            address.expect("checked above"),
            owner,
            device,
        )?)
    } else {
        None
    };
    save(&path, &policy)?;
    Ok((path, policy, grant))
}

pub fn deny_sender(
    custom: Option<&Path>,
    owner: &str,
    device: Option<&str>,
    reason: Option<&str>,
) -> Result<(PathBuf, AccessPolicy)> {
    let path = resolve_path(custom)?;
    let mut policy = load(&path)?;
    let rule = BlockRule {
        owner: owner.to_string(),
        device: device.map(|s| s.to_string()),
        reason: reason.map(|s| s.to_string()),
    };
    policy
        .allowed_senders
        .retain(|r| !(r.owner == owner && device.is_none_or(|d| r.device == d)));
    if let Some(existing) = policy
        .blocked_senders
        .iter_mut()
        .find(|b| b.owner == owner && b.device == rule.device)
    {
        existing.reason = rule.reason;
    } else {
        policy.blocked_senders.push(rule);
    }
    save(&path, &policy)?;
    Ok((path, policy))
}

pub fn remove_sender(
    custom: Option<&Path>,
    owner: &str,
    device: Option<&str>,
) -> Result<(PathBuf, AccessPolicy)> {
    valid_owner(owner)?;
    if let Some(d) = device {
        valid_device(d)?;
    }
    let path = resolve_path(custom)?;
    let mut policy = load(&path)?;
    policy
        .allowed_senders
        .retain(|r| !(r.owner == owner && device.is_none_or(|d| r.device == d)));
    policy
        .blocked_senders
        .retain(|b| !(b.owner == owner && device.is_none_or(|d| b.device.as_deref() == Some(d))));
    save(&path, &policy)?;
    Ok((path, policy))
}

pub fn reset(custom: Option<&Path>) -> Result<(PathBuf, AccessPolicy)> {
    let path = resolve_path(custom)?;
    let policy = AccessPolicy::default();
    save(&path, &policy)?;
    Ok((path, policy))
}

pub fn format_output(path: &Path, policy: &AccessPolicy, grant: Option<&Value>, as_json: bool) -> String {
    if as_json {
        let mut out = json!({
            "config": path.display().to_string(),
            "policy": policy,
        });
        if let Some(g) = grant {
            out["grant"] = g.clone();
        }
        return serde_json::to_string_pretty(&out).unwrap_or_default();
    }
    let mut lines = vec![
        format!("config: {}", path.display()),
        format!("mode: {}", policy.mode.as_str()),
        format!(
            "external_send: {}",
            if policy.allow_external_send { "on" } else { "off" }
        ),
        format!("max_hops: {}", policy.max_hops),
    ];
    if policy.allowed_senders.is_empty() {
        lines.push("allowed_senders: (relay grants authoritative)".to_string());
    } else {
        lines.push(format!("allowed_senders ({}):", policy.allowed_senders.len()));
        for r in &policy.allowed_senders {
            let mut detail = format!("  - {} / {}", r.owner, r.device);
            if let Some(addr) = &r.address {
                detail.push_str(&format!(" -> {addr}"));
            }
            if let Some(label) = &r.label {
                detail.push_str(&format!(" ({label})"));
            }
            lines.push(detail);
        }
    }
    if policy.blocked_senders.is_empty() {
        lines.push("blocked_senders: none".to_string());
    } else {
        lines.push(format!("blocked_senders ({}):", policy.blocked_senders.len()));
        for b in &policy.blocked_senders {
            let target = b.device.as_deref().unwrap_or("*");
            let mut detail = format!("  - {} / {}", b.owner, target);
            if let Some(reason) = &b.reason {
                detail.push_str(&format!(" ({reason})"));
            }
            lines.push(detail);
        }
    }
    if let Some(g) = grant {
        if let Some(id) = g["grantId"].as_str() {
            lines.push(format!("relay_grant: {id}"));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn default_policy_and_roundtrip_set_allow_deny_remove_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.json");
        let (_, initial) = show(Some(&path)).unwrap();
        assert_eq!(initial, AccessPolicy::default());

        let (_, updated) = set_settings(Some(&path), Some("same-account"), Some(false), Some(2)).unwrap();
        assert_eq!(updated.mode, AccessMode::SameAccount);
        assert!(!updated.allow_external_send);
        assert_eq!(updated.max_hops, 2);

        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o077, 0);

        let (_, allowed, grant) = allow_sender(
            Some(&path),
            r#"["https://issuer.example/","org","org-a"]"#,
            "agent-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some("claude@wmm.10462"),
            Some("Alice"),
            false,
        )
        .unwrap();
        assert!(grant.is_none());
        assert_eq!(allowed.allowed_senders.len(), 1);

        let (_, denied) = deny_sender(
            Some(&path),
            r#"["https://issuer.example/","org","org-a"]"#,
            Some("agent-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            Some("paused"),
        )
        .unwrap();
        assert!(denied.allowed_senders.is_empty());
        assert_eq!(denied.blocked_senders.len(), 1);

        let (_, removed) = remove_sender(
            Some(&path),
            r#"["https://issuer.example/","org","org-a"]"#,
            None,
        )
        .unwrap();
        assert!(removed.blocked_senders.is_empty());

        let (_, after_reset) = reset(Some(&path)).unwrap();
        assert_eq!(after_reset, AccessPolicy::default());
    }

    #[test]
    fn rejects_insecure_permissions_and_invalid_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.json");
        save(&path, &AccessPolicy::default()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load(&path).is_err());

        let other = dir.path().join("bad.json");
        assert!(set_settings(Some(&other), Some("open-world"), None, None).is_err());
        assert!(set_settings(Some(&other), None, None, Some(4)).is_err());
        assert!(resolve_path(Some(Path::new("relative/access.json"))).is_err());
    }
}
