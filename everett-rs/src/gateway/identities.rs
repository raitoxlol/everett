use std::{
    fs,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    time::Duration,
};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::Binding;

const STORE_ERROR: &str = "chat identity store unavailable";
const INVALID: &str = "identity handle is invalid, revoked, or belongs to another connection";

fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
fn scope(binding: &Binding) -> String {
    let oauth = binding.oauth.as_ref().map(|o| (&o.issuer, &o.resource));
    hash(
        &serde_json::to_string(&(
            binding.identity_owner.as_ref().unwrap_or(&binding.agent_id),
            oauth,
        ))
        .expect("identity scope is serializable"),
    )
}
fn path(binding: &Binding) -> PathBuf {
    binding.home.join(".everett/gateway-identities.sqlite")
}
fn open(binding: &Binding, write: bool) -> Result<Connection, String> {
    let flags = if write {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let metadata = fs::symlink_metadata(path(binding)).map_err(|_| STORE_ERROR)?;
    if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err("chat identity store must be a private regular file".into());
    }
    let con = Connection::open_with_flags(path(binding), flags).map_err(|_| STORE_ERROR)?;
    con.busy_timeout(Duration::from_secs(2))
        .map_err(|_| STORE_ERROR)?;
    Ok(con)
}
fn valid_secret(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(super) fn create(
    binding: &Binding,
    key: &str,
    name: &str,
    kind: &str,
) -> Result<Value, String> {
    if !(32..=128).contains(&key.len())
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("registration_key must be a fresh private random string of 32 to 128 URL-safe characters".into());
    }
    if name.trim().is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err("name must have 1 to 100 characters without control characters".into());
    }
    if !["chat", "dot", "codex"].contains(&kind) {
        return Err("kind must be chat, dot or codex".into());
    }
    let folder = binding.home.join(".everett");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
        .map_err(|_| STORE_ERROR)?;
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path(binding))
    {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(STORE_ERROR.into()),
    }
    let mut con = open(binding, true)?;
    con.execute_batch(
        "CREATE TABLE IF NOT EXISTS identities (
        scope TEXT NOT NULL, retry_key TEXT NOT NULL, handle TEXT NOT NULL UNIQUE,
        session_id TEXT NOT NULL UNIQUE, name TEXT NOT NULL, kind TEXT NOT NULL,
        created REAL NOT NULL, revoked INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (scope, retry_key))",
    )
    .map_err(|_| STORE_ERROR)?;
    let tx = con
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| STORE_ERROR)?;
    let scope = scope(binding);
    let retry_key = hash(key);
    let existing = tx.query_row("SELECT handle,session_id,name,kind,revoked FROM identities WHERE scope=?1 AND retry_key=?2",
        params![scope,retry_key], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,bool>(4)?)))
        .optional().map_err(|_| STORE_ERROR)?;
    let (handle, id) = if let Some((handle, id, old_name, old_kind, revoked)) = existing {
        if revoked {
            return Err(INVALID.into());
        }
        if old_name != name || old_kind != kind {
            return Err("registration_key already belongs to another identity request".into());
        }
        (handle, id)
    } else {
        let count: i64 = tx
            .query_row(
                "SELECT count(*) FROM identities WHERE scope=?1",
                [&scope],
                |r| r.get(0),
            )
            .map_err(|_| STORE_ERROR)?;
        if count >= 256 {
            return Err("connection identity limit reached".into());
        }
        let handle = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let id = format!("chat-{}", uuid::Uuid::new_v4().simple());
        tx.execute("INSERT INTO identities(scope,retry_key,handle,session_id,name,kind,created) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![scope,retry_key,handle,id,name,kind,crate::session::now()]).map_err(|_| STORE_ERROR)?;
        (handle, id)
    };
    tx.commit().map_err(|_| STORE_ERROR)?;
    // A retry repairs registration after a crash between these two durable writes.
    let tx = con
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| STORE_ERROR)?;
    let revoked: bool = tx
        .query_row(
            "SELECT revoked FROM identities WHERE handle=?1",
            [&handle],
            |r| r.get(0),
        )
        .map_err(|_| STORE_ERROR)?;
    if revoked {
        return Err(INVALID.into());
    }
    let registrations = crate::adapters::external::Store::new(&binding.home);
    if !registrations.exists(&id).map_err(|_| STORE_ERROR)? {
        registrations
            .register(
                &id,
                &binding.harness,
                name,
                &binding.cwd.to_string_lossy(),
                None,
                false,
            )
            .map_err(|_| {
                "identity saved but local registration failed; retry with the same registration_key"
            })?;
    }
    if !registration_matches(binding, &id, name) {
        return Err("identity saved but local registration conflicts or is invalid; repair it before retrying with the same registration_key".into());
    }
    tx.commit().map_err(|_| STORE_ERROR)?;
    Ok(json!({"session_id":id,"identity_handle":handle,"name":name,"kind":kind}))
}

// A file's existence alone does not establish a discoverable local inbox.
fn registration_matches(binding: &Binding, id: &str, name: &str) -> bool {
    crate::adapters::external::Store::new(&binding.home)
        .get(id)
        .ok()
        .flatten()
        .is_some_and(|record| {
            record.harness == binding.harness
                && record.title == name
                && record.cwd == binding.cwd.to_string_lossy()
                && record.device.is_none()
        })
}

pub(super) fn resolve(binding: &Binding, handle: &str) -> Result<String, String> {
    if !valid_secret(handle) || !path(binding).is_file() {
        return Err(INVALID.into());
    }
    open(binding, false)?
        .query_row(
            "SELECT session_id FROM identities WHERE scope=?1 AND handle=?2 AND revoked=0",
            params![scope(binding), handle],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| STORE_ERROR)?
        .ok_or_else(|| INVALID.into())
}

pub(super) fn list(binding: &Binding) -> Result<Value, String> {
    if !path(binding).is_file() {
        return Ok(json!({"identities":[]}));
    }
    let con = open(binding, false)?;
    let mut stmt = con.prepare("SELECT session_id,name,kind,created,revoked FROM identities WHERE scope=?1 ORDER BY created,session_id").map_err(|_| STORE_ERROR)?;
    let rows = stmt.query_map([scope(binding)], |r| Ok(json!({"session_id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,
        "kind":r.get::<_,String>(2)?,"created_at":r.get::<_,f64>(3)?,"state":if r.get::<_,bool>(4)? { "revoked" } else { "registered" }}))).map_err(|_| STORE_ERROR)?;
    let mut identities = rows
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| STORE_ERROR)?;
    for identity in &mut identities {
        if identity["state"] == "registered"
            && !registration_matches(binding, identity["session_id"].as_str().unwrap(), identity["name"].as_str().unwrap())
        {
            identity["state"] = json!("registration_unavailable");
        }
        // Registration is durable metadata, not evidence of a connected receiver.
        identity["availability"] = json!("unknown");
    }
    Ok(json!({"identities":identities}))
}

pub(super) fn revoke(binding: &Binding, handle: &str) -> Result<Value, String> {
    if !valid_secret(handle) || !path(binding).is_file() {
        return Err(INVALID.into());
    }
    let con = open(binding, true)?;
    let id: String = con
        .query_row(
            "SELECT session_id FROM identities WHERE scope=?1 AND handle=?2",
            params![scope(binding), handle],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| STORE_ERROR)?
        .ok_or(INVALID)?;
    con.execute(
        "UPDATE identities SET revoked=1 WHERE scope=?1 AND handle=?2",
        params![scope(binding), handle],
    )
    .map_err(|_| STORE_ERROR)?;
    crate::adapters::external::Store::new(&binding.home)
        .remove_if_exists(&id)
        .map_err(|_| "identity revoked but local registration cleanup failed")?;
    Ok(json!({"session_id":id,"revoked":true}))
}
