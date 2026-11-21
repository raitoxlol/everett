//! The T3 launch tool preserves the effective selection in its receipt.
use std::{
    io::{BufRead, BufReader, Read},
    time::Duration,
};

use serde_json::{json, Value};
use url::{Host, Url};

use super::{valid_receipt_id, DispatchDiagnostic, Policy, Selection, MAX_RESULT};

const UNAVAILABLE: &str = "t3_mcp_unavailable";
const EFFORT_OPTION: &str = "reasoningEffort";

pub(super) enum LaunchError {
    BeforeLaunch(&'static str),
    Unconfirmed(DispatchDiagnostic),
}

impl From<&'static str> for LaunchError {
    fn from(code: &'static str) -> Self {
        Self::BeforeLaunch(code)
    }
}

pub(super) fn loopback_endpoint(input: &str) -> bool {
    let Ok(url) = Url::parse(input) else {
        return false;
    };
    url.scheme() == "http"
        && match url.host() {
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        }
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/mcp"
        && url.query().is_none()
        && url.fragment().is_none()
}

pub(super) fn launch(
    policy: &Policy,
    selection: Selection<'_>,
    message: &str,
    request_id: &str,
    timeout: Duration,
) -> Result<Value, LaunchError> {
    let endpoint = policy.mcp_endpoint.as_deref().ok_or(UNAVAILABLE)?;
    let bearer = std::env::var("EVERETT_T3_MCP_TOKEN").map_err(|_| UNAVAILABLE)?;
    if bearer.is_empty() || bearer.len() > 4096 || !bearer.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(UNAVAILABLE.into());
    }
    let effort = json!(selection.effort.ok_or(UNAVAILABLE)?);
    let mut session = Session {
        agent: ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .timeout_global(Some(timeout / 5))
            .build()
            .into(),
        endpoint,
        auth: format!("Bearer {bearer}"),
        protocol: "2025-06-18".into(),
        id: None,
    };
    let init = session.post(
        1,
        "initialize",
        json!({
            "protocolVersion": session.protocol,
            "capabilities": {},
            "clientInfo": {"name":"everett-gateway","version":env!("CARGO_PKG_VERSION")}
        }),
    )?;
    session.protocol = match init["protocolVersion"].as_str() {
        Some(version @ ("2025-03-26" | "2025-06-18" | "2025-11-25")) => version.into(),
        _ => return Err(UNAVAILABLE.into()),
    };
    session.post(0, "notifications/initialized", Value::Null)?;

    let catalog = session.call(
        2,
        "orchestrator_capabilities",
        json!({"search":selection.model}),
    )?;
    let offered = entries(&catalog["providers"]).any(|provider| {
        provider["providerInstanceId"] == policy.provider
            && entries(&provider["models"]).any(|model| {
                model["id"] == selection.model
                    && entries(&model["options"]).any(|option| {
                        option["id"] == EFFORT_OPTION
                            && entries(&option["values"]).any(|value| *value == effort)
                    })
            })
    });
    if !offered {
        return Err("t3_model_or_effort_unavailable".into());
    }

    let receipt = session
        .call(
            3,
            "t3_thread_launch",
            json!({
                "projectId":policy.project_id, "title":"Everett owner request", "message":message,
                "clientRequestId":request_id,
                "modelSelection":{"instanceId":policy.provider,"model":selection.model,
                    "options":[{"id":EFFORT_OPTION,"value":effort}]}
            }),
        )
        .map_err(|_| LaunchError::Unconfirmed(DispatchDiagnostic::T3McpLaunchUnconfirmed))?;
    if receipt["projectId"] != policy.project_id
        || ["threadId", "runId"]
            .iter()
            .any(|k| !receipt[*k].as_str().is_some_and(valid_receipt_id))
        || !matches!(
            receipt["status"].as_str(),
            Some("preparing" | "queued" | "starting" | "running" | "waiting" | "completed")
        )
    {
        return Err(LaunchError::Unconfirmed(
            DispatchDiagnostic::DestinationMismatchOrMissingRun,
        ));
    }
    let effective = &receipt["modelSelection"];
    let verified_effort = {
        let mut options =
            entries(&effective["options"]).filter(|option| option["id"] == EFFORT_OPTION);
        options
            .next()
            .is_some_and(|option| option["value"] == effort)
            && options.next().is_none()
    };
    if effective["instanceId"] != policy.provider
        || effective["model"] != selection.model
        || !verified_effort
    {
        return Err(LaunchError::Unconfirmed(DispatchDiagnostic::T3EffectiveSelectionMismatch));
    }
    Ok(receipt)
}

fn entries(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

struct Session<'a> {
    agent: ureq::Agent,
    endpoint: &'a str,
    auth: String,
    protocol: String,
    id: Option<String>,
}

impl Session<'_> {
    fn call(&mut self, id: u64, name: &str, args: Value) -> Result<Value, &'static str> {
        let result = self.post(id, "tools/call", json!({"name":name,"arguments":args}))?;
        if result
            .get("isError")
            .is_some_and(|flag| flag.as_bool() != Some(false))
            || !result["structuredContent"].is_object()
        {
            return Err(UNAVAILABLE);
        }
        Ok(result["structuredContent"].clone())
    }

    fn post(&mut self, id: u64, method: &str, params: Value) -> Result<Value, &'static str> {
        let mut body = json!({"jsonrpc":"2.0","method":method});
        if id != 0 {
            body["id"] = json!(id);
            body["params"] = params;
        }
        let mut request = self
            .agent
            .post(self.endpoint)
            .header("accept", "application/json, text/event-stream")
            .header("authorization", &self.auth)
            .header("mcp-protocol-version", &self.protocol);
        if let Some(session_id) = &self.id {
            request = request.header("mcp-session-id", session_id);
        }
        let mut response = request.send_json(body).map_err(|_| UNAVAILABLE)?;
        if !response.status().is_success() {
            return Err(UNAVAILABLE);
        }
        if let Some(session_id) = response.headers().get("mcp-session-id") {
            self.id = Some(
                session_id
                    .to_str()
                    .ok()
                    .filter(|id| valid_receipt_id(id))
                    .ok_or(UNAVAILABLE)?
                    .into(),
            );
        }
        if id == 0 {
            return Ok(Value::Null);
        }
        let sse = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|kind| kind.starts_with("text/event-stream"));
        let reader = response.body_mut().as_reader().take(MAX_RESULT as u64);
        let envelope: Value = if sse {
            event_result(BufReader::new(reader), id)?
        } else {
            serde_json::from_reader(reader).map_err(|_| UNAVAILABLE)?
        };
        if envelope["id"] != id || envelope["jsonrpc"] != "2.0" || envelope.get("error").is_some() {
            return Err(UNAVAILABLE);
        }
        envelope
            .get("result")
            .filter(|value| value.is_object())
            .cloned()
            .ok_or(UNAVAILABLE)
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        if let Some(id) = &self.id {
            let _ = self
                .agent
                .delete(self.endpoint)
                .header("authorization", &self.auth)
                .header("mcp-session-id", id)
                .header("mcp-protocol-version", &self.protocol)
                .call();
        }
    }
}

fn event_result(mut reader: impl BufRead, id: u64) -> Result<Value, &'static str> {
    let mut data = String::new();
    let mut line = String::new();
    while reader.read_line(&mut line).map_err(|_| UNAVAILABLE)? > 0 {
        let value = line.trim_end_matches(['\r', '\n']);
        if value.is_empty() {
            if let Ok(message) = serde_json::from_str::<Value>(&data) {
                if message["id"] == id {
                    return Ok(message);
                }
            }
            data.clear();
        } else if let Some(value) = value.strip_prefix("data:") {
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
            data.push('\n');
        }
        line.clear();
    }
    Err(UNAVAILABLE)
}
