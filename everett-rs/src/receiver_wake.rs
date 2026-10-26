use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{EverettError, Result};

const POLICY_FILE: &str = "receiver-wake.json";
const JOBS_DIR: &str = "receiver-wake/jobs";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub version: u8,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allow: Vec<Rule>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub session_id: String,
    pub cwd: String,
    #[serde(default = "any_sender")]
    pub sender_device_id: String,
}

fn any_sender() -> String {
    "*".into()
}

impl Rule {
    pub fn allows_sender(&self, device_id: &str) -> bool {
        self.sender_device_id == "*" || self.sender_device_id == device_id
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Imported {
    pub local_id: String,
    pub digest: String,
    pub relay_message_id: String,
    pub reply_to: Option<String>,
    pub sender_session: String,
    pub sender_harness: String,
    pub relay_parent_owner: Option<String>,
    pub relay_parent_message_id: Option<String>,
    pub owner: String,
    pub sender_owner: String,
    pub recipient_owner: String,
    pub sender_device_id: String,
    pub recipient_device_id: String,
    pub grant_id: Option<String>,
    pub session_id: String,
    pub harness: String,
    pub cwd: String,
    pub expires_at: i64,
    pub hops: i64,
    pub external: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum State {
    Deferred {
        reason: String,
        checked_at: i64,
    },
    Attempting {
        started_at: i64,
    },
    PromptSent {
        at: i64,
        client_pid: u32,
        target_pid: u32,
    },
    Uncertain {
        at: i64,
        reason: String,
    },
    Cancelled {
        at: i64,
    },
    Skipped {
        at: i64,
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub imported: Imported,
    pub state: State,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttemptOutcome {
    PromptSent { client_pid: u32, target_pid: u32 },
    Deferred(String),
    Uncertain(String),
    Cancelled,
}

fn bad(message: impl ToString) -> EverettError {
    EverettError::new(2, format!("receiver wake: {}", message.to_string()))
}

fn policy_path(root: &Path) -> PathBuf {
    root.join(POLICY_FILE)
}

fn jobs_path(root: &Path) -> PathBuf {
    root.join(JOBS_DIR)
}

fn validate_rule(rule: &Rule) -> Result<()> {
    let id = Uuid::parse_str(&rule.session_id)
        .map_err(|_| bad("session_id must be a canonical UUID"))?;
    if id.hyphenated().to_string() != rule.session_id {
        return Err(bad("session_id must be a canonical UUID"));
    }
    let cwd = Path::new(&rule.cwd);
    if !cwd.is_absolute() || fs::canonicalize(cwd).map_err(bad)?.to_string_lossy() != rule.cwd {
        return Err(bad("cwd must be an existing canonical absolute path"));
    }
    if rule.sender_device_id != "*"
        && (rule.sender_device_id.is_empty()
            || rule.sender_device_id.len() > 160
            || !rule
                .sender_device_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)))
    {
        return Err(bad("sender_device_id is invalid"));
    }
    Ok(())
}

pub fn load_policy(root: &Path) -> Result<Policy> {
    let path = policy_path(root);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Policy {
                version: 1,
                enabled: false,
                allow: Vec::new(),
            });
        }
        Err(error) => return Err(bad(error)),
    };
    if !metadata.file_type().is_file() || metadata.mode() & 0o077 != 0 {
        return Err(bad(
            "policy must be a regular file with mode 0600 or stricter",
        ));
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return Err(bad(error)),
    };
    let policy: Policy = serde_json::from_slice(&bytes).map_err(bad)?;
    if policy.version != 1 {
        return Err(bad("unsupported policy version"));
    }
    let mut unique = HashSet::new();
    for rule in &policy.allow {
        validate_rule(rule)?;
        if !unique.insert(rule) {
            return Err(bad("duplicate allow rule"));
        }
    }
    Ok(policy)
}

pub fn allowed(policy: &Policy, imported: &Imported, owner: &str, device_id: &str) -> bool {
    policy.enabled
        && imported.owner == owner
        && imported.sender_owner == owner
        && imported.recipient_owner == owner
        && imported.recipient_device_id == device_id
        && imported.grant_id.is_none()
        && imported.reply_to.is_none()
        && !imported.external
        && imported.harness == "claude"
        && imported.hops > 0
        && imported.hops <= 3
        && policy.allow.iter().any(|rule| {
            rule.session_id == imported.session_id
                && rule.cwd == imported.cwd
                && rule.allows_sender(&imported.sender_device_id)
        })
}

pub fn schedule(root: &Path, imported: Imported, now: i64) -> Result<bool> {
    let policy = load_policy(root)?;
    if !allowed(
        &policy,
        &imported,
        &imported.owner,
        &imported.recipient_device_id,
    ) {
        return Ok(false);
    }
    if imported.local_id.len() != 70
        || !imported.local_id.starts_with("relay-")
        || !imported.local_id[6..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || imported.digest.len() != 64
        || !imported
            .digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(bad("import has an invalid relay id or signed digest"));
    }
    let dir = jobs_path(root);
    crate::peer::private_dir(&dir)?;
    let _guard = crate::peer::lock(&dir.join("ledger.lock"))?;
    let path = dir.join(format!("{}.json", imported.local_id));
    if path.exists() {
        let existing: Job = read(&path)?;
        if existing.imported.digest != imported.digest || existing.imported != imported {
            return Err(bad("duplicate relay id has changed import metadata"));
        }
        return Ok(false);
    }
    write(
        &path,
        &Job {
            imported,
            state: State::Deferred {
                reason: "new verified import".into(),
                checked_at: now,
            },
        },
    )?;
    sync_dir(&dir)?;
    Ok(true)
}

pub fn pending(root: &Path, now: i64) -> Result<Vec<Job>> {
    let dir = jobs_path(root);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut jobs = Vec::new();
    for path in crate::paths::glob(&dir, "relay-*.json") {
        let mut job: Job = read(&path)?;
        if let State::Attempting { started_at } = job.state {
            job.state = State::Uncertain {
                at: started_at,
                reason: "relay stopped during attach; automatic retry suppressed".into(),
            };
            write(&path, &job)?;
        }
        if job.imported.expires_at > now && matches!(job.state, State::Deferred { .. }) {
            jobs.push(job);
        }
    }
    Ok(jobs)
}

pub fn defer(root: &Path, local_id: &str, reason: &str, now: i64) -> Result<()> {
    update(root, local_id, |job| match job.state {
        State::Deferred { .. } | State::Attempting { .. } => {
            job.state = State::Deferred {
                reason: reason.into(),
                checked_at: now,
            };
            Ok(())
        }
        _ => Ok(()),
    })
}

pub fn begin_attempt(root: &Path, local_id: &str, now: i64) -> Result<bool> {
    let mut claimed = false;
    update(root, local_id, |job| match job.state {
        State::Deferred { .. } if job.imported.expires_at > now => {
            job.state = State::Attempting { started_at: now };
            claimed = true;
            Ok(())
        }
        _ => Ok(()),
    })?;
    Ok(claimed)
}

pub fn finish(root: &Path, local_id: &str, state: State) -> Result<()> {
    if !matches!(
        state,
        State::PromptSent { .. } | State::Uncertain { .. } | State::Cancelled { .. }
    ) {
        return Err(bad("invalid terminal state"));
    }
    update(root, local_id, |job| match job.state {
        State::Attempting { .. } => {
            job.state = state;
            Ok(())
        }
        _ => Ok(()),
    })
}

pub fn skip(root: &Path, local_id: &str, reason: &str, now: i64) -> Result<()> {
    update(root, local_id, |job| match job.state {
        State::Deferred { .. } => {
            job.state = State::Skipped { at: now, reason: reason.into() };
            Ok(())
        }
        _ => Ok(()),
    })
}

pub fn attempt<P>(
    root: &Path,
    local_id: &str,
    now: i64,
    prepare: impl FnOnce() -> std::result::Result<P, String>,
    attach: impl FnOnce(P, &str) -> AttemptOutcome,
) -> Result<Option<AttemptOutcome>> {
    let prepared = match prepare() {
        Ok(prepared) => prepared,
        Err(reason) => {
            defer(root, local_id, &reason, now)?;
            return Ok(None);
        }
    };
    if !begin_attempt(root, local_id, now)? {
        return Ok(None);
    }
    let outcome = attach(prepared, local_id);
    match &outcome {
        AttemptOutcome::PromptSent { client_pid, target_pid } => finish(
            root,
            local_id,
            State::PromptSent { at: now, client_pid: *client_pid, target_pid: *target_pid },
        )?,
        AttemptOutcome::Deferred(reason) => defer(root, local_id, reason, now)?,
        AttemptOutcome::Uncertain(reason) => finish(
            root,
            local_id,
            State::Uncertain { at: now, reason: reason.clone() },
        )?,
        AttemptOutcome::Cancelled => finish(root, local_id, State::Cancelled { at: now })?,
    }
    Ok(Some(outcome))
}

fn update(root: &Path, local_id: &str, change: impl FnOnce(&mut Job) -> Result<()>) -> Result<()> {
    let dir = jobs_path(root);
    let _guard = crate::peer::lock(&dir.join("ledger.lock"))?;
    let path = dir.join(format!("{local_id}.json"));
    let mut job: Job = read(&path)?;
    change(&mut job)?;
    write(&path, &job)
}

fn read(path: &Path) -> Result<Job> {
    serde_json::from_slice(&fs::read(path).map_err(bad)?).map_err(bad)
}

fn write(path: &Path, job: &Job) -> Result<()> {
    crate::peer::save(path, &serde_json::to_vec_pretty(job).map_err(bad)?)?;
    sync_dir(
        path.parent()
            .ok_or_else(|| bad("missing ledger directory"))?,
    )
}

fn sync_dir(path: &Path) -> Result<()> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(bad)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

    fn write_policy(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn fixture(root: &Path) -> Imported {
        fs::create_dir_all(root.join("project")).unwrap();
        let cwd = fs::canonicalize(root.join("project"))
            .unwrap()
            .to_string_lossy()
            .to_string();
        let id = "38cab978-79c0-4f42-9aee-5c30d8aa6e4c";
        write_policy(&root.join(POLICY_FILE), &serde_json::to_vec(&json!({"version":1,"enabled":true,"allow":[{"session_id":id,"cwd":cwd,"sender_device_id":"mac-device"}]})).unwrap());
        Imported {
            local_id: format!("relay-{}", "a".repeat(64)),
            digest: "b".repeat(64),
            relay_message_id: "mRemote123".into(),
            reply_to: None,
            sender_session: "mac-session".into(),
            sender_harness: "claude".into(),
            relay_parent_owner: None,
            relay_parent_message_id: None,
            owner: "owner".into(),
            sender_owner: "owner".into(),
            recipient_owner: "owner".into(),
            sender_device_id: "mac-device".into(),
            recipient_device_id: "nuc-device".into(),
            grant_id: None,
            session_id: id.into(),
            harness: "claude".into(),
            cwd,
            expires_at: 500,
            hops: 1,
            external: false,
        }
    }

    #[test]
    fn only_exact_same_owner_paired_claude_import_is_allowed() {
        let temp = tempfile::tempdir().unwrap();
        let imported = fixture(temp.path());
        let policy = load_policy(temp.path()).unwrap();
        assert!(allowed(&policy, &imported, "owner", "nuc-device"));
        let cases = [
            ("other owner", {
                let mut x = imported.clone();
                x.sender_owner = "other".into();
                x
            }),
            ("external", {
                let mut x = imported.clone();
                x.external = true;
                x
            }),
            ("grant", {
                let mut x = imported.clone();
                x.grant_id = Some("grant".into());
                x
            }),
            ("reply", {
                let mut x = imported.clone();
                x.reply_to = Some("mOriginal123".into());
                x
            }),
            ("wrong recipient", {
                let mut x = imported.clone();
                x.recipient_device_id = "other".into();
                x
            }),
            ("wrong sender", {
                let mut x = imported.clone();
                x.sender_device_id = "unpaired".into();
                x
            }),
            ("T3", {
                let mut x = imported.clone();
                x.harness = "t3code".into();
                x
            }),
            ("expired hops", {
                let mut x = imported.clone();
                x.hops = 4;
                x
            }),
        ];
        for (name, changed) in cases {
            assert!(
                !allowed(&policy, &changed, "owner", "nuc-device"),
                "accepted {name}"
            );
        }
    }

    #[test]
    fn disabled_unknown_wildcard_and_noncanonical_rules_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let imported = fixture(temp.path());
        let policy = load_policy(temp.path()).unwrap();
        let mut disabled = policy.clone();
        disabled.enabled = false;
        assert!(!allowed(&disabled, &imported, "owner", "nuc-device"));
        write_policy(
            &policy_path(temp.path()),
            br#"{"version":2,"enabled":true,"allow":[]}"#,
        );
        assert!(load_policy(temp.path()).is_err());
        write_policy(
            &policy_path(temp.path()),
            br#"{"version":1,"enabled":true,"wildcard":"*","allow":[]}"#,
        );
        assert!(load_policy(temp.path()).is_err());
        let mut bad_rule = policy.clone();
        bad_rule.allow[0].session_id = "38cab978".into();
        write_policy(
            &policy_path(temp.path()),
            &serde_json::to_vec(&bad_rule).unwrap(),
        );
        assert!(load_policy(temp.path()).is_err());
    }

    #[test]
    fn persisted_job_defers_then_allows_one_attempt_and_restart_suppresses_retry() {
        let temp = tempfile::tempdir().unwrap();
        let imported = fixture(temp.path());
        assert!(schedule(temp.path(), imported.clone(), 10).unwrap());
        assert!(!schedule(temp.path(), imported.clone(), 11).unwrap());
        assert!(pending(temp.path(), 11).unwrap().len() == 1);
        defer(temp.path(), &imported.local_id, "Claude session busy", 12).unwrap();
        assert!(begin_attempt(temp.path(), &imported.local_id, 13).unwrap());
        assert!(!begin_attempt(temp.path(), &imported.local_id, 14).unwrap());
        assert!(pending(temp.path(), 15).unwrap().is_empty());
        assert!(matches!(
            read(&jobs_path(temp.path()).join(format!("{}.json", imported.local_id)))
                .unwrap()
                .state,
            State::Uncertain { .. }
        ));
        assert!(!begin_attempt(temp.path(), &imported.local_id, 16).unwrap());
    }

    #[test]
    fn expired_job_cannot_be_claimed() {
        let temp = tempfile::tempdir().unwrap();
        let mut imported = fixture(temp.path());
        imported.expires_at = 10;
        schedule(temp.path(), imported.clone(), 1).unwrap();
        assert!(!begin_attempt(temp.path(), &imported.local_id, 10).unwrap());
    }

    #[test]
    fn fake_claude_defers_busy_then_attaches_once_to_local_id() {
        let temp = tempfile::tempdir().unwrap();
        let imported = fixture(temp.path());
        schedule(temp.path(), imported.clone(), 10).unwrap();
        let mut attaches = 0;
        let busy: Option<AttemptOutcome> = attempt(
            temp.path(),
            &imported.local_id,
            11,
            || Err::<(), String>("Claude background session is active".into()),
            |_, _| panic!("busy fake Claude must not attach"),
        )
        .unwrap();
        assert_eq!(busy, None);
        assert!(matches!(pending(temp.path(), 11).unwrap()[0].state, State::Deferred { .. }));
        let attached = attempt(
            temp.path(),
            &imported.local_id,
            12,
            || Ok(()),
            |(), message_id| {
                attaches += 1;
                assert_eq!(message_id, imported.local_id);
                AttemptOutcome::PromptSent { client_pid: 20, target_pid: 10 }
            },
        )
        .unwrap();
        assert_eq!(attached, Some(AttemptOutcome::PromptSent { client_pid: 20, target_pid: 10 }));
        assert_eq!(attaches, 1);
        let duplicate = attempt(
            temp.path(),
            &imported.local_id,
            13,
            || Ok(()),
            |(), _| { attaches += 1; AttemptOutcome::PromptSent { client_pid: 21, target_pid: 10 } },
        )
        .unwrap();
        assert_eq!(duplicate, None);
        assert_eq!(attaches, 1);
    }
    #[test]
    fn omitted_sender_filter_allows_same_owner_but_preserves_recipient_and_grant_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let mut imported = fixture(temp.path());
        let path = temp.path().join(POLICY_FILE);
        let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["allow"][0].as_object_mut().unwrap().remove("sender_device_id");
        write_policy(&path, &serde_json::to_vec(&value).unwrap());
        let policy = load_policy(temp.path()).unwrap();
        imported.sender_device_id = "new-same-owner-device".into();
        assert!(allowed(&policy, &imported, "owner", "nuc-device"));
        imported.sender_owner = "other-owner".into();
        assert!(!allowed(&policy, &imported, "owner", "nuc-device"));
        imported.sender_owner = "owner".into();
        imported.recipient_device_id = "different-device".into();
        assert!(!allowed(&policy, &imported, "owner", "nuc-device"));
        imported.recipient_device_id = "nuc-device".into();
        imported.grant_id = Some("grant".into());
        assert!(!allowed(&policy, &imported, "owner", "nuc-device"));
    }

}
