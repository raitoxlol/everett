//! Fixture-only ownership adapter. No production caller or live registration writer.
#[path = "../identity_shadow.rs"]
mod model;
use super::{identities, Binding, OAuthVerifier};
use crate::{cloudflare_auth, host, session};
use model::{AccountPrincipal, ClientDevice, SessionIdentity};
use std::sync::Arc;

struct ProvenBinding {
    client: ClientDevice,
    session: SessionIdentity,
}

struct NativeReceipt {
    account: AccountPrincipal,
    device_id: String,
    session_id: String,
    harness: String,
}

fn native_identity(host: &host::Host) -> Result<(AccountPrincipal, String, String), &'static str> {
    let credential = cloudflare_auth::load()
        .map_err(|_| "native credential unavailable")?
        .ok_or("native credential unavailable")?;
    let identity =
        cloudflare_auth::whoami(&credential).map_err(|_| "native credential unverified")?;
    let id = host::live_binding(host).ok_or("native session has no hook binding")?;
    let account = AccountPrincipal {
        issuer: identity["issuer"].as_str().ok_or("missing issuer")?.into(),
        subject: identity["owner"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("missing owner")?
            .into(),
    };
    Ok((
        account,
        identity["agent_id"]
            .as_str()
            .ok_or("missing device")?
            .into(),
        id,
    ))
}

impl NativeReceipt {
    // Explicit registration is distinct from projecting historical discovered sessions.
    fn register(host: &host::Host) -> Result<Self, &'static str> {
        let (account, device_id, session_id) = native_identity(host)?;
        Ok(Self {
            account,
            device_id,
            session_id,
            harness: host.harness.clone(),
        })
    }
}

fn native(
    host: &host::Host,
    receipt: Option<&NativeReceipt>,
) -> Result<Option<ProvenBinding>, &'static str> {
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    let (account, device, id) = native_identity(host)?;
    if account != receipt.account
        || device != receipt.device_id
        || id != receipt.session_id
        || host.harness != receipt.harness
    {
        return Err("native registration does not match current verified binding");
    }
    let client_id = serde_json::to_string(&("native", &account.issuer, &device)).unwrap();
    let record = session::Session::new(&host.harness, &id, "", "", "", 0.0);
    Ok(Some(ProvenBinding {
        client: ClientDevice {
            id: client_id.clone(),
            account,
            revoked: false,
        },
        session: SessionIdentity::from_native(&client_id, &record),
    }))
}

async fn logical(
    binding: &Binding,
    verifier: &Arc<OAuthVerifier>,
    token: String,
    handle: &str,
) -> Result<ProvenBinding, &'static str> {
    let oauth = binding.oauth.as_ref().ok_or("legacy binding unmapped")?;
    if oauth.issuer != verifier.config.issuer || oauth.resource != verifier.config.resource {
        return Err("verifier does not match gateway");
    }
    let agent = verifier
        .verify(token)
        .await
        .map_err(|_| "gateway credential unverified")?;
    let account = AccountPrincipal {
        issuer: verifier.config.issuer.clone(),
        subject: agent
            .account
            .as_ref()
            .ok_or("legacy account unmapped")?
            .clone(),
    };
    let selected = binding.for_oauth(&agent);
    let id = identities::resolve(&selected, handle).map_err(|_| "logical session unresolved")?;
    let listed = identities::list(&selected).map_err(|_| "logical registration unavailable")?;
    let row = listed["identities"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["session_id"] == id))
        .filter(|row| row["state"] == "registered")
        .ok_or("logical registration unavailable")?;
    let client_id =
        serde_json::to_string(&("oauth", &account.issuer, &oauth.resource, &agent.client_id))
            .unwrap();
    Ok(ProvenBinding {
        client: ClientDevice {
            id: client_id.clone(),
            account,
            revoked: false,
        },
        session: SessionIdentity::from_registered(&client_id, row)
            .map_err(|_| "logical registration invalid")?,
    })
}

#[cfg(test)]
mod tests;
