use axum::{
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::{identities, permissions, strong_secret, Binding, HttpBoundary, OAuthAgent};

pub(super) const HANDLE_HEADER: &str = "everett-identity-handle";
pub(super) const ADMIN_HEADER: &str = "everett-identity-admin";
pub(super) const LIFECYCLE_PATH: &str = "/mcp/identity";

pub(super) fn is_tool(name: &str) -> bool {
    matches!(
        name,
        "everett_whoami" | "everett_inbox" | "everett_card" | "everett_send" | "everett_directory"
    )
}

pub(super) fn grant(binding: &Binding) -> Result<Option<permissions::Grant>, String> {
    binding
        .agent_tool_permissions
        .as_deref()
        .map(permissions::Grant::load)
        .transpose()
}

pub(super) fn authorize_tool(binding: &Binding, name: &str) -> Result<(), String> {
    if !is_tool(name) || grant(binding)?.is_some_and(|grant| !grant.permits(name)) {
        return Err("tool is not granted to this conversation".into());
    }
    Ok(())
}

pub(super) fn admin_digest() -> Result<[u8; 32], String> {
    let token = std::env::var("EVERETT_GATEWAY_IDENTITY_ADMIN_TOKEN")
        .ok()
        .filter(|token| strong_secret(token))
        .ok_or("conversation_identity requires EVERETT_GATEWAY_IDENTITY_ADMIN_TOKEN of 32..512 printable ASCII characters without whitespace")?;
    Ok(Sha256::digest(token.as_bytes()).into())
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Create {
        registration_key: String,
        name: String,
        kind: String,
    },
    Revoke {
        identity_handle: Option<String>,
        session_id: Option<String>,
    },
}

pub(super) async fn manage(
    state: &HttpBoundary,
    method: &Method,
    headers: &HeaderMap,
    bytes: &[u8],
    agent: Option<&OAuthAgent>,
) -> Response {
    let Some(admin) = &state.identity_admin else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if *method != Method::POST {
        return (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response();
    }
    let authorized = headers
        .get(ADMIN_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| {
            strong_secret(token) && bool::from(admin.ct_eq(&Sha256::digest(token.as_bytes())))
        });
    if !authorized {
        return StatusCode::FORBIDDEN.into_response();
    }
    let json_content = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
    if !json_content {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let Ok(operation) = serde_json::from_slice::<Operation>(bytes) else {
        return response(
            StatusCode::BAD_REQUEST,
            json!({"error":"invalid conversation identity request"}),
        );
    };

    let binding = &state.gateway.binding;
    let binding = agent.map_or_else(|| (**binding).clone(), |agent| binding.for_oauth(agent));
    let result = tokio::task::spawn_blocking(move || match operation {
        Operation::Create {
            registration_key,
            name,
            kind,
        } => identities::create(&binding, &registration_key, &name, &kind),
        Operation::Revoke {
            identity_handle,
            session_id,
        } => match (identity_handle, session_id) {
            (Some(handle), None) => identities::revoke(&binding, &handle),
            (None, Some(id)) => identities::revoke_session(&binding, &id),
            _ => Err("provide exactly one identity_handle or session_id to revoke".into()),
        },
    })
    .await;
    match result {
        Ok(Ok(data)) => response(StatusCode::OK, data),
        Ok(Err(error)) => response(StatusCode::BAD_REQUEST, json!({"error":error})),
        Err(_) => response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"identity operation unavailable"}),
        ),
    }
}

fn response(status: StatusCode, data: Value) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_TYPE, "application/json"),
        ],
        data.to_string(),
    )
        .into_response()
}
