//! Explicit owner grant to promote remote text to a fixed T3 project's prompt.
//! This does not grant general control, wake, transcript access or destinations.
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};

use rmcp::model::Tool;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    time::timeout,
};

use super::{Binding, OAuthAgent, MAX_RESULT};
use crate::adapters::t3_cli::{self, Dispatch};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Policy {
    owner_account: String,
    client_id: String,
    command: PathBuf,
    project_id: String,
    provider: String,
    model: String,
}

impl Policy {
    pub(super) fn validate(&self, binding: &Binding) -> Result<(), String> {
        let token =
            |s: &str| !s.is_empty() && s.len() <= 256 && s.bytes().all(|b| b.is_ascii_graphic());
        if binding.oauth.is_none()
            || !token(&self.owner_account)
            || !token(&self.client_id)
            || !super::valid_destination(&self.project_id)
            || self.project_id.starts_with('-')
            || !token(&self.provider)
            || !token(&self.model)
            || !self.command.is_absolute()
            || !crate::proc::is_executable(&self.command.to_string_lossy())
        {
            return Err("native_start requires OAuth, an exact owner_account/client_id, executable absolute command, fixed project_id, provider and model".into());
        }
        Ok(())
    }

    pub(super) fn permits(&self, agent: Option<&OAuthAgent>) -> bool {
        agent.is_some_and(|a| {
            a.account.as_deref() == Some(self.owner_account.as_str())
                && a.client_id == self.client_id
        })
    }
}

pub(super) fn tool() -> Tool {
    serde_json::from_value(json!({
        "name":"everett_start",
        "description":"Start a native T3 run using the owner's fixed project/provider/model grant. This promotes text to a model prompt and may consume the owner's model allowance. Use a fresh request_id once; on uncertainty reuse exactly that ID and text, never a new ID. Dispatch confirmation is not model completion, native provider identity, inbox pickup or reply. Does not grant send/read/control permissions.",
        "inputSchema":{"type":"object","additionalProperties":false,"required":["request_id","text","identity_handle"],"properties":{
            "request_id":{"type":"string","minLength":32,"maxLength":128},
            "text":{"type":"string","minLength":1,"maxLength":16000},
            "identity_handle":{"type":"string","minLength":64,"maxLength":64}
        }},
        "annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":true,"openWorldHint":true}
    })).expect("native start tool schema")
}

const STORE_ERROR: &str =
    "native start journal unavailable; reconcile existing requests before retrying";

const RECOVERY: &str = "Reuse this request_id and identical text to read its receipt. Do not launch under a new ID until the owner reconciles T3; a previous dispatch may have succeeded.";

fn cached_receipt(raw: &str, key: &str, policy: &Policy) -> Result<Value, String> {
    if raw.len() > MAX_RESULT {
        return Err(STORE_ERROR.into());
    }
    let value: Value = serde_json::from_str(raw).map_err(|_| STORE_ERROR)?;
    let object = value.as_object().ok_or(STORE_ERROR)?;
    if value["request_id"] != key
        || [
            "pickup_confirmed",
            "completion_confirmed",
            "reply_confirmed",
        ]
        .iter()
        .any(|k| value[*k] != false)
    {
        return Err(STORE_ERROR.into());
    }
    let allowed: &[&str] = match value["state"].as_str() {
        Some("dispatch_confirmed")
            if value["dispatch_confirmed"] == true
                && value["project_id"] == policy.project_id
                && value.get("native_session_id") == Some(&Value::Null)
                && ["thread_id", "run_id"]
                    .iter()
                    .all(|k| value[*k].as_str().is_some_and(super::valid_destination)) =>
        {
            &[
                "state",
                "dispatch_confirmed",
                "request_id",
                "thread_id",
                "run_id",
                "project_id",
                "native_session_id",
                "pickup_confirmed",
                "completion_confirmed",
                "reply_confirmed",
            ]
        }
        Some("dispatch_unconfirmed")
            if value["dispatch_confirmed"] == false
                && value["recovery"] == RECOVERY
                && matches!(
                    value["diagnostic"].as_str(),
                    Some(
                        "interrupted_or_pending"
                            | "cli_unavailable"
                            | "cli_io_error"
                            | "cli_exit"
                            | "cli_timeout"
                            | "cli_output_too_large"
                            | "invalid_receipt"
                            | "destination_mismatch_or_missing_run"
                    )
                ) =>
        {
            &[
                "state",
                "dispatch_confirmed",
                "diagnostic",
                "request_id",
                "pickup_confirmed",
                "completion_confirmed",
                "reply_confirmed",
                "recovery",
            ]
        }
        _ => return Err(STORE_ERROR.into()),
    };
    if object.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(STORE_ERROR.into());
    }
    Ok(value)
}

fn hash(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn journal(binding: &Binding) -> Result<Connection, String> {
    let dir = binding.home.join(".everett");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|_| STORE_ERROR)?;
    let path = dir.join("gateway-starts.sqlite");
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
    {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(STORE_ERROR.into()),
    }
    let meta = fs::symlink_metadata(&path).map_err(|_| STORE_ERROR)?;
    if !meta.file_type().is_file() || meta.permissions().mode() & 0o077 != 0 {
        return Err(STORE_ERROR.into());
    }
    let con = Connection::open(&path).map_err(|_| STORE_ERROR)?;
    con.busy_timeout(Duration::from_secs(2))
        .map_err(|_| STORE_ERROR)?;
    con.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS starts (
        scope TEXT NOT NULL, request_key TEXT NOT NULL, fingerprint TEXT NOT NULL, result TEXT NOT NULL,
        PRIMARY KEY(scope,request_key));").map_err(|_| STORE_ERROR)?;
    Ok(con)
}

pub(super) async fn start(
    binding: &Binding,
    policy: &Policy,
    args: Map<String, Value>,
) -> Result<Value, String> {
    if args
        .keys()
        .any(|k| !matches!(k.as_str(), "request_id" | "text"))
    {
        return Err("unexpected native start argument".into());
    }
    let key = args
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|s| {
            (32..=128).contains(&s.len())
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
        .ok_or("request_id must be 32..128 URL-safe characters")?;
    super::require_text(&args, "text", 16000)?;
    let text = args["text"].as_str().expect("validated text");
    let oauth = binding
        .oauth
        .as_ref()
        .ok_or("native start requires OAuth")?;
    let scope = hash(
        &serde_json::to_vec(&(
            &oauth.issuer,
            &oauth.resource,
            &policy.owner_account,
            &policy.client_id,
        ))
        .map_err(|_| STORE_ERROR)?,
    );
    let request_key =
        hash(&serde_json::to_vec(&(&binding.agent_id, key)).map_err(|_| STORE_ERROR)?);
    let fingerprint = hash(&serde_json::to_vec(&(policy, text)).map_err(|_| STORE_ERROR)?);
    let unknown = json!({"state":"dispatch_unconfirmed","dispatch_confirmed":false,"diagnostic":"interrupted_or_pending","request_id":key,
        "pickup_confirmed":false,"completion_confirmed":false,"reply_confirmed":false,
        "recovery":RECOVERY});
    let mut con = journal(binding)?;
    {
        let tx = con
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| STORE_ERROR)?;
        let prior: Option<(String, String)> = tx
            .query_row(
                "SELECT fingerprint,result FROM starts WHERE scope=?1 AND request_key=?2",
                params![scope, request_key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| STORE_ERROR)?;
        if let Some((old, result)) = prior {
            if old != fingerprint {
                return Err(
                    "request_id belongs to a different native start request or owner policy".into(),
                );
            }
            return cached_receipt(&result, key, policy);
        }
        let count: i64 = tx
            .query_row(
                "SELECT count(*) FROM starts WHERE scope=?1",
                [&scope],
                |r| r.get(0),
            )
            .map_err(|_| STORE_ERROR)?;
        if count >= 256 {
            return Err("native start journal limit reached; owner reconciliation required".into());
        }
        tx.execute(
            "INSERT INTO starts(scope,request_key,fingerprint,result) VALUES(?1,?2,?3,?4)",
            params![scope, request_key, fingerprint, unknown.to_string()],
        )
        .map_err(|_| STORE_ERROR)?;
        // Commit before dispatch. Cancellation/crash at any later point must never replay a prompt.
        tx.commit().map_err(|_| STORE_ERROR)?;
    }
    let receipt = launch(binding, policy, text).await;
    let result = match receipt {
        Ok(receipt) => {
            json!({"state":"dispatch_confirmed","dispatch_confirmed":true,"request_id":key,
            "thread_id":receipt["threadId"],"run_id":receipt["runId"],"project_id":receipt["projectId"],
            "native_session_id":null,"pickup_confirmed":false,"completion_confirmed":false,"reply_confirmed":false})
        }
        Err(code) => {
            let mut unknown = unknown;
            unknown["diagnostic"] = json!(code);
            unknown
        }
    };
    con.execute(
        "UPDATE starts SET result=?3 WHERE scope=?1 AND request_key=?2",
        params![scope, request_key, result.to_string()],
    )
    .map_err(|_| STORE_ERROR)?;
    Ok(result)
}

struct OwnedCli {
    child: Child,
    group: u32,
}
impl Drop for OwnedCli {
    fn drop(&mut self) {
        // Only this CLI's group is owned, never the independently running T3 server/model.
        if let Ok(group) = i32::try_from(self.group) {
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        let _ = self.child.start_kill();
    }
}

async fn launch(binding: &Binding, policy: &Policy, text: &str) -> Result<Value, &'static str> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(&policy.command);
    command
        .args([
            "thread",
            "start",
            "--json",
            "--title",
            "Everett owner request",
            "--provider",
            &policy.provider,
            "--model",
            &policy.model,
            &policy.project_id,
            "-",
        ])
        .current_dir(&binding.cwd)
        .env_clear()
        .env("HOME", &binding.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in ["PATH", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.as_std_mut().process_group(0);
    let child = command.spawn().map_err(|_| "cli_unavailable")?;
    let group = child.id().ok_or("cli_unavailable")?;
    let mut owned = OwnedCli { child, group };
    let mut stdin = owned.child.stdin.take().ok_or("cli_io_error")?;
    let stdout = owned.child.stdout.take().ok_or("cli_io_error")?;
    let operation = async {
        let send = async {
            stdin
                .write_all(text.as_bytes())
                .await
                .map_err(|_| "cli_io_error")?;
            stdin.shutdown().await.map_err(|_| "cli_io_error")?;
            drop(stdin);
            Ok::<(), &'static str>(())
        };
        let receive = async {
            let mut bytes = Vec::new();
            stdout
                .take((MAX_RESULT + 1) as u64)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| "cli_io_error")?;
            if bytes.len() > MAX_RESULT {
                return Err("cli_output_too_large");
            }
            Ok(bytes)
        };
        let (_, bytes) = tokio::try_join!(send, receive)?;
        let status = owned.child.wait().await.map_err(|_| "cli_io_error")?;
        if !status.success() {
            return Err("cli_exit");
        }
        let stdout = std::str::from_utf8(&bytes).map_err(|_| "invalid_receipt")?;
        let receipt = t3_cli::parse_json_output(stdout).ok_or("invalid_receipt")?;
        t3_cli::validate_dispatch(
            &receipt,
            Dispatch::Start,
            false,
            status.code().unwrap_or(-1),
        )
        .map_err(|_| "invalid_receipt")?;
        if receipt["projectId"] != policy.project_id
            || !receipt["threadId"]
                .as_str()
                .is_some_and(super::valid_destination)
            || !receipt["runId"]
                .as_str()
                .is_some_and(super::valid_destination)
        {
            return Err("destination_mismatch_or_missing_run");
        }
        Ok(receipt)
    };
    timeout(Duration::from_secs_f64(binding.timeout), operation)
        .await
        .map_err(|_| "cli_timeout")?
}
