use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::{crypto::Envelope, queue::read_record, Outgoing, State};

const SCAN_LIMIT: usize = 256;

fn scan<T: DeserializeOwned>(directory: &Path, mut inspect: impl FnMut(&Path, T) -> bool) -> Value {
    let mut summary = json!({"state":"missing","scanned":0,"readable":0,"unreadable":0,"truncated":false,"limit":SCAN_LIMIT});
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return summary,
        Ok(metadata) if metadata.is_dir() => {}
        _ => {
            summary["state"] = json!("unreadable");
            return summary;
        }
    }
    let Ok(entries) = fs::read_dir(directory) else {
        summary["state"] = json!("unreadable");
        return summary;
    };
    summary["state"] = json!("cached");
    let (mut scanned, mut readable, mut unreadable) = (0, 0, 0);
    for (index, entry) in entries.take(SCAN_LIMIT + 1).enumerate() {
        if index == SCAN_LIMIT {
            summary["truncated"] = json!(true);
            break;
        }
        scanned += 1;
        let Ok(entry) = entry else {
            unreadable += 1;
            continue;
        };
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        if read_record(&path).is_some_and(|record| inspect(&path, record)) {
            readable += 1;
        } else {
            unreadable += 1;
        }
    }
    summary["scanned"] = json!(scanned);
    summary["readable"] = json!(readable);
    summary["unreadable"] = json!(unreadable);
    summary
}

fn file_state(path: &Path) -> &'static str {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing",
        Ok(metadata) if metadata.is_file() && metadata.permissions().mode() & 0o077 == 0 => {
            "present"
        }
        _ => "unreadable",
    }
}

pub(super) fn snapshot() -> Value {
    let root = super::root();
    let credential = read_record::<crate::cloudflare_auth::Credential>(
        &crate::cloudflare_auth::credential_path(),
    );
    let selected = crate::cloudflare_auth::require_provider().is_ok();
    let identity = credential
        .as_ref()
        .filter(|_| selected)
        .map(|c| (c.owner.as_str(), c.agent_id.as_str()));
    let credential_state = match credential.as_ref() {
        Some(_) if !selected => "provider_mismatch",
        Some(c) if c.expired() => "expired",
        Some(_) => "cached",
        None if file_state(&crate::cloudflare_auth::credential_path()) == "missing" => "missing",
        None => "unreadable",
    };
    let keys = identity
        .map(|(owner, agent)| file_state(&super::key_path(owner, agent)))
        .unwrap_or("not_checked");
    let mut receiver = super::receiver::snapshot(&root, identity)
        .unwrap_or_else(|_| json!({"state":"unreadable","next":"Check private receiver status files on this host."}));
    if let Some(capabilities) = receiver
        .get_mut("capabilities")
        .and_then(Value::as_array_mut)
    {
        capabilities.retain(|capability| capability == "same-owner-webhook-v1");
    }
    let now = super::milliseconds();
    let mut states = json!({"unconfirmed":0,"expired":0,"queued":0,"delivered":0,"replied":0});
    let mut outgoing = scan::<Outgoing>(&root.join("outgoing"), |_, out| {
        let state = match out.state {
            State::Replied => "replied",
            State::Delivered => "delivered",
            State::Queued if out.envelope.expires_at <= now => "expired",
            State::Queued if out.posted => "queued",
            State::Queued => "unconfirmed",
        };
        states[state] = json!(states[state].as_u64().unwrap_or(0) + 1);
        true
    });
    outgoing["states"] = states;
    let mut expired = 0;
    let mut ingress = scan::<Envelope>(&root.join("ingress"), |_, envelope| {
        expired += usize::from(envelope.expires_at <= now);
        true
    });
    ingress["expired"] = json!(expired);
    let external = scan::<crate::adapters::external::Registration>(
        &crate::session::home().join(".everett/external"),
        |path, record| {
            crate::adapters::external::valid_id(&record.id)
                && path.file_stem().and_then(|stem| stem.to_str()) == Some(record.id.as_str())
                && crate::adapters::external::is_external(&record.harness)
                && record.updated.is_finite()
                && record.updated >= 0.0
                && record
                    .device
                    .as_deref()
                    .is_none_or(|target| crate::peer::target(target).is_ok())
        },
    );
    json!({
        "schema":1,"offline":true,
        "receiver":receiver,
        "registration":{"credential":credential_state,"local_keys":keys,"remote":"not_checked"},
        "external":external,
        "webhook":crate::receiver_webhook::diagnostics(&root),
        "outgoing":outgoing,"ingress":ingress,
        "note":"Local snapshot only. Cache counts cover all stored accounts and are not delivery, connectivity, authorization or task-completion proof. No message bodies, identifiers, webhook URLs or bearer files are returned."
    })
}
