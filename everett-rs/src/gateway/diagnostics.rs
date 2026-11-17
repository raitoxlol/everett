//! Read-only CLI observations. Existing authorization and delivery paths are untouched.
use super::{identities, strong_secret, Binding, OAuthVerifier};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{io::Read, path::Path, sync::Arc};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    access_token: String,
    identity_handle: String,
}

fn report(mode: &str, account: &str, mapping: &str, reason: &str, next_step: &str) -> Value {
    json!({"mode":mode,"account":account,"mapping":mapping,"reason":reason,
        "session_id":null,"label":null,"availability":"unknown",
        "dispatch":"not_observed","pickup":"not_observed","reply":"not_observed",
        "authorization":"unchanged","next_step":next_step})
}

fn native() -> (Value, i32) {
    let unmapped = |account, reason, next| report("native", account, "unmapped", reason, next);
    if crate::cloudflare_auth::require_provider().is_err() {
        return (
            unmapped(
                "unverified",
                "native_provider_unavailable",
                "Inspect the existing account provider configuration.",
            ),
            1,
        );
    }
    let credential = match crate::cloudflare_auth::load() {
        Ok(Some(credential)) => credential,
        Ok(None) => return (unmapped("not_signed_in", "native_not_signed_in", "No native credential is stored. Existing local session routing is unchanged."), 0),
        Err(_) => return (unmapped("unverified", "native_credential_unreadable", "The native credential could not be read; inspect account setup without sharing its contents."), 2),
    };
    if crate::cloudflare_auth::whoami(&credential).is_err() {
        return (unmapped("unverified", "native_credential_unverified", "Verification failed or the issuer is unavailable. Check account status with everett whoami."), 1);
    }
    (unmapped("verified", "native_ownership_provenance_unavailable", "The account is verified, but no supported native session ownership receipt exists. Use existing local session routing."), 0)
}

async fn gateway(path: &Path, credentials: Credentials) -> (Value, i32) {
    let failure =
        |account, mapping, reason, next| report("gateway", account, mapping, reason, next);
    let binding = match Binding::load(path) {
        Ok(binding) => binding,
        Err(_) => {
            return (
                failure(
                    "unverified",
                    "unmapped",
                    "gateway_config_unavailable",
                    "Check the existing gateway configuration; its values are not printed.",
                ),
                2,
            )
        }
    };
    let Some(oauth) = binding.oauth.clone() else {
        return (failure("unverified", "unmapped", "legacy_binding_unmapped", "This fixed binding has no verified account mapping. Its existing grants remain unchanged."), 0);
    };
    let secret = match std::env::var("EVERETT_OAUTH_INTROSPECTION_SECRET").ok().filter(|s| strong_secret(s)) {
        Some(secret) => secret,
        None => return (failure("unverified", "unmapped", "gateway_verifier_unavailable", "Run in the existing gateway environment with its introspection secret; do not paste secrets into command arguments."), 2),
    };
    let verifier = match OAuthVerifier::new(oauth, secret) {
        Ok(verifier) => Arc::new(verifier),
        Err(_) => {
            return (
                failure(
                    "unverified",
                    "unmapped",
                    "gateway_verifier_unavailable",
                    "Check the existing gateway verifier configuration.",
                ),
                2,
            )
        }
    };
    let agent = match verifier.verify(credentials.access_token.clone()).await {
        Ok(agent) => agent,
        Err(_) => return (failure("unverified", "unresolved", "gateway_credential_unverified", "The credential is invalid, expired, revoked, or could not be verified. Check the existing connection."), 1),
    };
    if agent.account.is_none() || agent.client_id.is_empty() {
        return (failure("unverified", "unmapped", "gateway_account_provenance_unavailable", "The verified credential lacks account/client provenance; existing legacy behavior remains unchanged."), 0);
    }
    let selected = binding.for_oauth(&agent);
    let id = match identities::resolve(&selected, &credentials.identity_handle) {
        Ok(id) => id,
        Err(_) => return (failure("verified", "unresolved", "identity_unresolved", "The handle is unknown, revoked, foreign, or its store is unavailable. Use this conversation's existing private handle."), 1),
    };
    let listing = identities::list(&selected).ok();
    let registered = listing
        .as_ref()
        .and_then(|v| v["identities"].as_array())
        .is_some_and(|rows| {
            rows.iter()
                .any(|row| row["session_id"] == id && row["state"] == "registered")
        });
    if !registered || id == credentials.identity_handle || id == credentials.access_token {
        return (failure("verified", "unavailable", "registration_unavailable", "The identity resolves, but its local registration is missing or conflicting. Inspect the registration before routing."), 1);
    }
    let mut output = report("gateway", "verified", "mapped", "registered_handle_resolved", "Use this exact session ID through the existing authorized messaging path. This diagnostic grants no access.");
    output["label"] = json!(format!("Registered conversation [{id}]"));
    output["session_id"] = json!(id);
    (output, 0)
}

pub fn run(gateway_config: Option<&Path>, as_json: bool) -> i32 {
    let (value, code) = if let Some(path) = gateway_config {
        let mut bytes = Vec::new();
        let input = std::io::stdin().take(4097).read_to_end(&mut bytes);
        let credentials = if input.is_ok() && bytes.len() <= 4096 {
            serde_json::from_slice::<Credentials>(&bytes).ok()
        } else {
            None
        };
        match credentials {
            None => (report("gateway", "unverified", "unresolved", "invalid_input", "Supply only access_token and identity_handle as bounded JSON on stdin; never supply account or session claims."), 2),
            Some(credentials) => match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
                Ok(runtime) => runtime.block_on(gateway(path, credentials)),
                Err(_) => (report("gateway", "unverified", "unresolved", "runtime_unavailable", "The verification runtime could not start; retry after checking system resources."), 2),
            },
        }
    } else {
        native()
    };
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("diagnostic report is serializable")
        );
    } else {
        println!(
            "Identity mapping: {} ({})\nAccount: {}\n{}\n{}",
            value["mapping"].as_str().unwrap(),
            value["reason"].as_str().unwrap(),
            value["account"].as_str().unwrap(),
            value["label"].as_str().unwrap_or("No mapped session"),
            value["next_step"].as_str().unwrap()
        );
    }
    code
}
