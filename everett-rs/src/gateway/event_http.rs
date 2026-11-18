use super::*;
use crate::gateway_events::{Events, Failure, Principal};

fn principal(
    state: &HttpBoundary,
    agent: Option<&OAuthAgent>,
    token: Option<String>,
    handle: Option<&str>,
) -> Result<Principal, String> {
    let binding = agent.map_or_else(
        || (*state.gateway.binding).clone(),
        |a| state.gateway.binding.for_oauth(a),
    );
    let owner = serde_json::to_string(&(
        &state.event_scope,
        binding.identity_owner.as_ref().unwrap_or(&binding.agent_id),
    ))
    .unwrap();
    let selected = match handle {
        Some(handle) => identities::resolve(&binding, handle)?,
        None => binding.agent_id.clone(),
    };
    Ok(Principal {
        owner,
        agent: selected,
        token: Some(
            json!({"access":if agent.is_some() { token } else { None },"handle":handle})
                .to_string(),
        ),
    })
}

pub(super) async fn advertise(response: Response) -> Response {
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_RESULT).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    if value["result"]["capabilities"].is_object() {
        value["result"]["capabilities"]["events"] = json!({});
        parts.headers.remove("content-length");
        return Response::from_parts(parts, Body::from(value.to_string()));
    }
    Response::from_parts(parts, Body::from(bytes))
}

pub(super) async fn dispatch(
    state: &HttpBoundary,
    method: &Method,
    uri: &axum::http::Uri,
    headers: &axum::http::HeaderMap,
    bytes: &[u8],
    agent: Option<&OAuthAgent>,
    token: Option<String>,
) -> Option<Response> {
    let events = state.events.as_ref()?;
    if method != Method::POST || uri.path() != "/mcp" {
        return None;
    }
    let rpc: Value = serde_json::from_slice(bytes).ok()?;
    let method = rpc["method"].as_str()?;
    if !method.starts_with("events/") {
        return None;
    }
    let permit = match state.event_requests.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return Some(StatusCode::SERVICE_UNAVAILABLE.into_response()),
    };
    if !headers
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| {
            s.split(';')
                .next()
                .is_some_and(|s| s.trim() == "application/json")
        })
    {
        return Some(StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response());
    }
    if rpc["jsonrpc"] != "2.0"
        || !(rpc["id"].is_string() || rpc["id"].is_i64() || rpc["id"].is_u64())
    {
        return Some(StatusCode::BAD_REQUEST.into_response());
    }
    let mut args = rpc.get("params").cloned().unwrap_or(json!({}));
    let handle = args
        .get_mut("arguments")
        .and_then(Value::as_object_mut)
        .and_then(|a| a.remove("identity_handle"));
    let invalid_handle = Failure {
        code: -32602,
        reason: "invalid_identity_handle",
    };
    let selected = match handle.as_ref() {
        None if state.gateway.binding.require_identity_handle
            && matches!(method, "events/subscribe" | "events/unsubscribe") =>
        {
            Err(Failure {
                code: -32602,
                reason: "identity_handle_required",
            })
        }
        Some(value) if !value.is_string() => Err(invalid_handle),
        _ => principal(state, agent, token, handle.as_ref().and_then(Value::as_str))
            .map_err(|_| invalid_handle),
    };
    let result = match selected {
        Err(error) => Err(error),
        Ok(p) if method == "events/list" => {
            let mut catalog = Events::catalog(&p.agent);
            let schema = &mut catalog["events"][0]["inputSchema"];
            schema["properties"]["identity_handle"] = json!({"type":"string","description":"Private handle selecting your registered inbox. Omit session_id when using a handle."});
            if state.gateway.binding.require_identity_handle {
                schema["required"] = json!(["identity_handle"]);
            } else {
                schema.as_object_mut().unwrap().remove("required");
            }
            catalog["events"][0]["inputSchema"]["properties"]["session_id"] = json!({"type":"string","description":"Must match the selected inbox; may be omitted."});
            Ok(catalog)
        }
        Ok(p) => {
            if args["arguments"].is_object() && args["arguments"].get("session_id").is_none() {
                args["arguments"]["session_id"] = json!(p.agent);
            }
            let expires_at =
                agent.map(|a| a.expires_at.saturating_mul(1000).min(i64::MAX as u64) as i64);
            let events = events.clone();
            let method = method.to_owned();
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                match method.as_str() {
                    "events/subscribe" => events.subscribe_until(&p, &args, expires_at),
                    "events/unsubscribe" => events.unsubscribe(&p, &args),
                    _ => Err(crate::gateway_events::Failure {
                        code: -32601,
                        reason: "method not found",
                    }),
                }
            })
            .await
            .unwrap_or(Err(crate::gateway_events::Failure {
                code: -32603,
                reason: "event worker unavailable",
            }))
        }
    };
    let body = match result {
        Ok(value) => json!({"jsonrpc":"2.0","id":rpc["id"],"result":value}),
        Err(e) => {
            json!({"jsonrpc":"2.0","id":rpc["id"],"error":{"code":e.code,"message":e.reason,"data":{"reason":e.reason}}})
        }
    };
    Some(
        (
            [
                ("content-type", "application/json"),
                ("cache-control", "no-store"),
            ],
            body.to_string(),
        )
            .into_response(),
    )
}

pub(super) async fn pump(state: HttpBoundary) {
    let Some(events) = &state.events else { return };
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let store = events.clone();
        let Ok(Ok(leases)) = tokio::task::spawn_blocking(move || store.leases()).await else {
            continue;
        };
        use futures_util::StreamExt;
        futures_util::stream::iter(leases)
            .for_each_concurrent(state.gateway.binding.max_workers, |lease| {
                let state = &state;
                async move {
                    let credentials = lease
                        .principal
                        .token
                        .as_deref()
                        .and_then(|s| serde_json::from_str::<Value>(s).ok())
                        .unwrap_or(Value::Null);
                    let handle = credentials["handle"].as_str();
                    if state.gateway.binding.require_identity_handle && handle.is_none() {
                        return;
                    }
                    let access = credentials["access"].as_str().map(str::to_owned);
                    let agent = match &state.auth {
                        HttpAuth::Bearer(_) => None,
                        HttpAuth::OAuth(verifier) => {
                            let Some(token) = access.clone() else {
                                events.revoke(&lease);
                                return;
                            };
                            match verifier.verify(token).await {
                                Ok(a)
                                    if !state
                                        .gateway
                                        .binding
                                        .destinations
                                        .contains(&a.agent_id) =>
                                {
                                    Some(a)
                                }
                                Err((StatusCode::SERVICE_UNAVAILABLE, _)) => return,
                                _ => {
                                    events.revoke(&lease);
                                    return;
                                }
                            }
                        }
                    };
                    let expected = principal(
                        state,
                        agent.as_ref(),
                        access,
                        handle,
                    );
                    let store = events.clone();
                    if !expected.is_ok_and(|p| {
                        p.owner == lease.principal.owner && p.agent == lease.principal.agent
                    }) {
                        let _ = tokio::task::spawn_blocking(move || store.revoke(&lease)).await;
                        return;
                    }
                    let _ = tokio::task::spawn_blocking(move || store.deliver(&lease)).await;
                }
            })
            .await;
    }
}
