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

mod t3_mcp;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Policy {
    owner_account: String,
    client_id: String,
    command: PathBuf,
    project_id: String,
    provider: String,
    model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effort: Option<Effort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mcp_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allowed_selections: Vec<AllowedSelection>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AllowedSelection {
    model: String,
    effort: Effort,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
struct Selection<'a> {
    model: &'a str,
    effort: Option<Effort>,
}

#[derive(Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Effort {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultra,
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
        match (&self.mcp_endpoint, self.effort) {
            (None, None) if self.allowed_selections.is_empty() => {}
            (Some(endpoint), Some(_)) if t3_mcp::loopback_endpoint(endpoint) => {}
            _ => return Err(
                "native_start effort requires a matching mcp_endpoint at a loopback T3 /mcp URL"
                    .into(),
            ),
        }
        if self.allowed_selections.len() > 32
            || self
                .allowed_selections
                .iter()
                .enumerate()
                .any(|(index, pair)| {
                    !token(&pair.model)
                        || (pair.model == self.model && Some(pair.effort) == self.effort)
                        || self.allowed_selections[..index].contains(pair)
                })
        {
            return Err("native_start allowed_selections must contain at most 32 distinct non-default model/effort pairs".into());
        }
        Ok(())
    }

    fn default_selection(&self) -> Selection<'_> {
        Selection {
            model: &self.model,
            effort: self.effort,
        }
    }

    fn resolve_selection<'a>(
        &'a self,
        args: &'a Map<String, Value>,
    ) -> Result<Selection<'a>, String> {
        if self.mcp_endpoint.is_none()
            && (args.contains_key("model") || args.contains_key("effort"))
        {
            return Err("explicit model or effort requires the T3 MCP transport".into());
        }
        let model = match args.get("model") {
            Some(value) => value.as_str().ok_or("model must be a string")?,
            None => &self.model,
        };
        let effort = match args.get("effort") {
            Some(value) => Some(
                serde_json::from_value::<Effort>(value.clone())
                    .map_err(|_| "effort must be a supported reasoning effort string")?,
            ),
            None => self.effort,
        };
        let selection = Selection { model, effort };
        if selection == self.default_selection()
            || self
                .allowed_selections
                .iter()
                .any(|pair| pair.model == model && Some(pair.effort) == effort)
        {
            return Ok(selection);
        }
        Err("model and effort must match an owner-authorized pair".into())
    }

    pub(super) fn permits(&self, agent: Option<&OAuthAgent>) -> bool {
        agent.is_some_and(|a| {
            a.account.as_deref() == Some(self.owner_account.as_str())
                && a.client_id == self.client_id
        })
    }
}

pub(super) fn tool(policy: &Policy) -> Tool {
    let mut schema = json!({
        "name":"everett_start",
        "description":"Start a NEW native T3 run in the owner's fixed project and provider; it cannot reach an existing thread -- for a handoff to an existing session or T3 Code thread, use everett_send with an ID from everett_directory. When advertised, choose an owner-authorized model and effort pair or omit both to use the default. This promotes text to a model prompt and may consume the owner's model allowance. Reuse the same request ID and inputs after an uncertain result. A dispatch receipt establishes only T3 dispatch.",
        "inputSchema":{"type":"object","additionalProperties":false,"required":["request_id","text","identity_handle"],"properties":{
            "request_id":{"type":"string","minLength":32,"maxLength":128},
            "text":{"type":"string","minLength":1,"maxLength":16000},
            "identity_handle":{"type":"string","minLength":64,"maxLength":64}
        }},
        "annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":true,"openWorldHint":true}
    });
    if let Some(effort) = policy.effort {
        let mut models = vec![policy.model.as_str()];
        let mut efforts = vec![effort];
        let mut choices = vec![json!({"properties":{
            "model":{"const":policy.model}, "effort":{"const":effort}
        }})];
        for pair in &policy.allowed_selections {
            let mut required = Vec::new();
            if pair.model != policy.model {
                required.push("model");
            }
            if pair.effort != effort {
                required.push("effort");
            }
            choices.push(json!({"required":required, "properties":{
                "model":{"const":pair.model}, "effort":{"const":pair.effort}
            }}));
            if !models.contains(&pair.model.as_str()) {
                models.push(&pair.model);
            }
            if !efforts.contains(&pair.effort) {
                efforts.push(pair.effort);
            }
        }
        let props = &mut schema["inputSchema"]["properties"];
        props["model"] = json!({"type":"string", "enum":models, "default":policy.model});
        props["effort"] = json!({"type":"string", "enum":efforts, "default":effort,
            "description": "Use with a matching model from the allowed pairs."});
        schema["inputSchema"]["anyOf"] = json!(choices);
    }
    serde_json::from_value(schema).expect("native start tool schema")
}

const STORE_ERROR: &str =
    "native start journal unavailable; reconcile existing requests before retrying";

const RECOVERY: &str = "Reuse this request_id and identical text to read its receipt. Do not launch under a new ID until the owner reconciles T3; a previous dispatch may have succeeded.";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum DispatchDiagnostic {
    InterruptedOrPending,
    CliUnavailable,
    CliIoError,
    CliExit,
    CliTimeout,
    CliOutputTooLarge,
    InvalidReceipt,
    DestinationMismatchOrMissingRun,
    // Retain diagnostic spellings accepted in receipts written by older builds.
    T3McpUnavailable,
    T3ModelOrEffortUnavailable,
    T3McpLaunchUnconfirmed,
    T3EffectiveSelectionMismatch,
}

// T3 thread/run IDs are opaque receipt values, never Everett inbox paths.
fn valid_receipt_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 512 && id.bytes().all(|b| b.is_ascii_graphic())
}

fn cached_receipt(
    raw: &str,
    key: &str,
    policy: &Policy,
    requested: &Value,
) -> Result<Value, String> {
    if raw.len() > MAX_RESULT {
        return Err(STORE_ERROR.into());
    }
    let mut value: Value = serde_json::from_str(raw).map_err(|_| STORE_ERROR)?;
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
                    .all(|k| value[*k].as_str().is_some_and(valid_receipt_id)) =>
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
                && value["diagnostic"].as_str().is_some_and(|diagnostic| {
                    use serde::de::{value::StrDeserializer, IntoDeserializer};
                    let value: StrDeserializer<'_, serde::de::value::Error> = diagnostic.into_deserializer();
                    DispatchDiagnostic::deserialize(value).is_ok()
                }) =>
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
    if object
        .keys()
        .any(|k| !matches!(k.as_str(), "requested" | "effective") && !allowed.contains(&k.as_str()))
    {
        return Err(STORE_ERROR.into());
    }
    let effective = if value["dispatch_confirmed"] == true && policy.mcp_endpoint.is_some() {
        requested.clone()
    } else {
        Value::Null
    };
    let legacy = policy.mcp_endpoint.is_none()
        && value.get("requested").is_none()
        && value.get("effective").is_none();
    if !legacy
        && (value.get("requested") != Some(requested) || value.get("effective") != Some(&effective))
    {
        return Err(STORE_ERROR.into());
    }
    value["requested"] = requested.clone();
    value["effective"] = effective;
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
        .any(|k| !matches!(k.as_str(), "request_id" | "text" | "model" | "effort"))
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
    let selection = policy.resolve_selection(&args)?;
    let requested = json!(selection);
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
    let fingerprint = hash(
        &if selection == policy.default_selection() {
            serde_json::to_vec(&(policy, text))
        } else {
            serde_json::to_vec(&(policy, text, selection))
        }
        .map_err(|_| STORE_ERROR)?,
    );
    let unknown = json!({"state":"dispatch_unconfirmed","dispatch_confirmed":false,"diagnostic":DispatchDiagnostic::InterruptedOrPending,"request_id":key,
        "requested":requested,"effective":null,
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
            return cached_receipt(&result, key, policy, &requested);
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
    let receipt = if policy.mcp_endpoint.is_some() {
        let request_key_for_t3 = hash(format!("{scope}/{request_key}").as_bytes());
        let policy = policy.clone();
        let model = selection.model.to_owned();
        let effort = selection.effort;
        let prompt = text.to_owned();
        let timeout = Duration::from_secs_f64(binding.timeout);
        tokio::task::spawn_blocking(move || {
            let selection = Selection {
                model: &model,
                effort,
            };
            t3_mcp::launch(&policy, selection, &prompt, &request_key_for_t3, timeout)
        })
        .await
        .unwrap_or(Err(t3_mcp::LaunchError::Unconfirmed(
            DispatchDiagnostic::T3McpLaunchUnconfirmed,
        )))
    } else {
        launch_cli(binding, policy, text)
            .await
            .map_err(t3_mcp::LaunchError::Unconfirmed)
    };
    let result = match receipt {
        Ok(receipt) => {
            json!({"state":"dispatch_confirmed","dispatch_confirmed":true,"request_id":key,
            "thread_id":receipt["threadId"],"run_id":receipt["runId"],"project_id":receipt["projectId"],
            "requested":requested,
            "effective":policy.mcp_endpoint.as_ref().map(|_| &requested),
            "native_session_id":null,"pickup_confirmed":false,"completion_confirmed":false,"reply_confirmed":false})
        }
        Err(t3_mcp::LaunchError::BeforeLaunch(code)) => {
            con.execute(
                "DELETE FROM starts WHERE scope=?1 AND request_key=?2 AND fingerprint=?3",
                params![scope, request_key, fingerprint],
            )
            .map_err(|_| STORE_ERROR)?;
            return Ok(
                json!({"state":"dispatch_not_started","dispatch_confirmed":false,
                "diagnostic":code,"request_id":key,"requested":requested,"effective":null,
                "pickup_confirmed":false,"completion_confirmed":false,"reply_confirmed":false,
                "recovery":"Resolve the T3 preflight issue and retry with the same request_id and inputs."}),
            );
        }
        Err(t3_mcp::LaunchError::Unconfirmed(code)) => {
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

async fn launch_cli(binding: &Binding, policy: &Policy, text: &str) -> Result<Value, DispatchDiagnostic> {
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
    let child = command.spawn().map_err(|_| DispatchDiagnostic::CliUnavailable)?;
    let group = child.id().ok_or(DispatchDiagnostic::CliUnavailable)?;
    let mut owned = OwnedCli { child, group };
    let mut stdin = owned.child.stdin.take().ok_or(DispatchDiagnostic::CliIoError)?;
    let stdout = owned.child.stdout.take().ok_or(DispatchDiagnostic::CliIoError)?;
    let operation = async {
        let send = async {
            stdin
                .write_all(text.as_bytes())
                .await
                .map_err(|_| DispatchDiagnostic::CliIoError)?;
            stdin.shutdown().await.map_err(|_| DispatchDiagnostic::CliIoError)?;
            drop(stdin);
            Ok::<(), DispatchDiagnostic>(())
        };
        let receive = async {
            let mut bytes = Vec::new();
            stdout
                .take((MAX_RESULT + 1) as u64)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| DispatchDiagnostic::CliIoError)?;
            if bytes.len() > MAX_RESULT {
                return Err(DispatchDiagnostic::CliOutputTooLarge);
            }
            Ok(bytes)
        };
        let (_, bytes) = tokio::try_join!(send, receive)?;
        let status = owned.child.wait().await.map_err(|_| DispatchDiagnostic::CliIoError)?;
        if !status.success() {
            return Err(DispatchDiagnostic::CliExit);
        }
        let stdout = std::str::from_utf8(&bytes).map_err(|_| DispatchDiagnostic::InvalidReceipt)?;
        let receipt = t3_cli::parse_json_output(stdout).ok_or(DispatchDiagnostic::InvalidReceipt)?;
        t3_cli::validate_dispatch(
            &receipt,
            Dispatch::Start,
            false,
            status.code().unwrap_or(-1),
        )
        .map_err(|_| DispatchDiagnostic::InvalidReceipt)?;
        if receipt["projectId"] != policy.project_id
            || !receipt["threadId"].as_str().is_some_and(valid_receipt_id)
            || !receipt["runId"].as_str().is_some_and(valid_receipt_id)
        {
            return Err(DispatchDiagnostic::DestinationMismatchOrMissingRun);
        }
        Ok(receipt)
    };
    timeout(Duration::from_secs_f64(binding.timeout), operation)
        .await
        .map_err(|_| DispatchDiagnostic::CliTimeout)?
}
