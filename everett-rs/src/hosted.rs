//! Owner-approved enrollment and per-agent Convex Auth credentials.
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::auth::{self, Stored};
use crate::error::{EverettError, Result};

pub const SCOPE: &str = "everett-hosted-agent";
pub fn is_session(stored: &Stored) -> bool {
    stored.scope.starts_with(&format!("{SCOPE} "))
}
fn scope(token: &str) -> Result<String> {
    Ok(format!(
        "{SCOPE} {} {}",
        hex::encode(Sha256::digest(token.as_bytes())),
        deployment()?
    ))
}
fn bad(message: &str) -> EverettError {
    EverettError::new(6, message)
}
fn configured_url() -> String {
    crate::config::get("hosted.url", Some("EVERETT_HOSTED_URL"), "")
}
pub fn configured() -> bool {
    !configured_url().is_empty()
}
pub fn deployment() -> Result<String> {
    let value = configured_url();
    let url = url::Url::parse(&value).map_err(|_| {
        bad("set EVERETT_HOSTED_URL or [hosted] url to the HTTPS Convex cloud origin")
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(bad("Hosted deployment must be an HTTPS origin"));
    }
    Ok(value.trim_end_matches('/').to_string())
}
fn path(name: &str) -> PathBuf {
    crate::session::home().join(".everett").join(name)
}
fn save_private<T: Serialize>(name: &str, value: &T) -> Result<()> {
    let path = path(name);
    let parent = path.parent().unwrap();
    crate::peer::private_dir_io(parent)
        .map_err(|_| bad("Cannot secure hosted credential directory"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| bad("Cannot create hosted credential file"))?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|_| bad("Cannot secure hosted credential file"))?;
    serde_json::to_writer(&mut file, value).map_err(|_| bad("Cannot save hosted credentials"))?;
    file.as_file()
        .sync_all()
        .map_err(|_| bad("Cannot sync hosted credentials"))?;
    file.persist(&path)
        .map_err(|_| bad("Cannot persist hosted credentials"))?;
    fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|_| bad("Cannot sync hosted credential directory"))
}
fn read_private(path: &Path) -> Result<String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| bad("Cannot read private hosted credential file"))?;
    let meta = file
        .metadata()
        .map_err(|_| bad("Cannot inspect hosted credential file"))?;
    if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 || meta.len() > 16_384 {
        return Err(bad(
            "Hosted credential must be a small, mode-0600 regular file",
        ));
    }
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|_| bad("Cannot read hosted credential file"))?;
    Ok(text)
}
#[derive(Serialize, Deserialize)]
struct Credential {
    deployment: String,
    token: String,
}
#[derive(Serialize, Deserialize)]
struct Pending {
    deployment: String,
    token: String,
    device_code: String,
    agent_id: String,
    name: String,
    expires_at: f64,
}

/// A private-CA hosted origin (the local dev stack) trusts only the explicit relay CA file.
fn private_ca() -> Result<Option<ureq::tls::RootCerts>> {
    let file = crate::config::get("relay.ca_file", Some("EVERETT_RELAY_CA_FILE"), "");
    if file.is_empty() {
        return Ok(None);
    }
    let invalid = || bad("EVERETT_RELAY_CA_FILE / [relay] ca_file must be a readable PEM certificate file");
    let pem = fs::read(&file).map_err(|_| invalid())?;
    let certs = ureq::tls::parse_pem(&pem)
        .filter_map(|item| match item {
            Ok(ureq::tls::PemItem::Certificate(cert)) => Some(Ok(cert)),
            Ok(_) => None,
            Err(_) => Some(Err(invalid())),
        })
        .collect::<Result<Vec<_>>>()?;
    if certs.is_empty() {
        return Err(invalid());
    }
    Ok(Some(ureq::tls::RootCerts::new_with_certs(&certs)))
}

fn call(kind: &str, function: &str, args: Value, bearer: Option<&str>) -> Result<Value> {
    let endpoint = format!("{}/api/{kind}", deployment()?);
    let mut config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .max_redirects(0)
        .http_status_as_error(false);
    if let Some(roots) = private_ca()? {
        config = config.tls_config(ureq::tls::TlsConfig::builder().root_certs(roots).build());
    }
    let agent: ureq::Agent = config.build().into();
    let mut request = agent.post(&endpoint);
    if let Some(token) = bearer {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request
        .send_json(json!({"path":function,"args":args,"format":"json"}))
        .map_err(|_| bad("Hosted Everett request failed"))?;
    let value: Value = response
        .body_mut()
        .read_json()
        .map_err(|_| bad("Invalid hosted Everett response"))?;
    if !response.status().is_success() || value["status"] != "success" {
        return Err(bad("Hosted Everett denied the request; check approval, credential revocation and deployment configuration"));
    }
    value
        .get("value")
        .cloned()
        .ok_or_else(|| bad("Missing hosted Everett result"))
}
fn field(value: &Value, key: &str) -> Result<String> {
    value[key]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| bad("Invalid hosted Everett response"))
}
fn token() -> Result<Option<String>> {
    if let Ok(value) = std::env::var("EVERETT_AGENT_TOKEN") {
        return Ok(Some(value));
    }
    if let Some(file) = std::env::var_os("EVERETT_AGENT_TOKEN_FILE") {
        return read_private(Path::new(&file)).map(|s| Some(s.trim().into()));
    }
    if !path("agent-token.json").exists() {
        return Ok(None);
    }
    let stored: Credential = serde_json::from_str(&read_private(&path("agent-token.json"))?)
        .map_err(|_| bad("Invalid hosted credential file"))?;
    if stored.deployment != deployment()? {
        return Err(bad(
            "Hosted token belongs to another deployment; enroll there separately",
        ));
    }
    Ok(Some(stored.token))
}
fn decode_jwt(jwt: &str) -> Result<Value> {
    let pieces: Vec<_> = jwt.split('.').collect();
    if pieces.len() != 3 {
        return Err(bad("Invalid hosted agent JWT"));
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(pieces[1])
        .map_err(|_| bad("Invalid hosted agent JWT"))?;
    serde_json::from_slice(&bytes).map_err(|_| bad("Invalid hosted agent JWT"))
}
fn secret_parts(secret: &str) -> Result<(&str, Option<&str>)> {
    let (bearer, seeds) = secret
        .split_once('.')
        .map_or((secret, None), |(a, b)| (a, Some(b)));
    let hex = |s: &str| {
        s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    };
    if bearer.len() != 68
        || !bearer.starts_with("evt_")
        || !hex(&bearer[4..])
        || seeds.is_some_and(|s| s.len() != 128 || !hex(s))
    {
        return Err(bad("Invalid agent token format"));
    }
    Ok((bearer, seeds))
}
pub(crate) fn key_material() -> Result<Option<([u8; 32], [u8; 32])>> {
    if !configured() {
        return Ok(None);
    }
    let secret = token()?.ok_or_else(|| bad("Missing hosted agent token"))?;
    let (_, seeds) = secret_parts(&secret)?;
    seeds
        .map(|s| {
            let signing = crate::relay::crypto::decode(&s[..64])?;
            let encryption = crate::relay::crypto::decode(&s[64..])?;
            Ok((signing, encryption))
        })
        .transpose()
}
fn exchange(token: &str, name: &str) -> Result<Stored> {
    let (bearer, _) = secret_parts(token)?;
    let response = call(
        "action",
        "auth:signIn",
        json!({"provider":"agent","params":{"token":bearer}}),
        None,
    )?;
    let jwt = field(&response["tokens"], "token")?;
    let claim = decode_jwt(&jwt)?;
    if claim["everett_agent"].as_str().is_none()
        || claim["everett_org"].as_str().is_none()
        || claim["aud"] != "convex"
    {
        return Err(bad("Hosted token is not an agent identity"));
    }
    let stored = Stored {
        issuer: field(&claim, "iss")?,
        client_id: "convex".into(),
        sub: field(&claim, "sub")?,
        email: String::new(),
        name: field(&claim, "everett_agent")?,
        device_name: name.into(),
        access_token: jwt.clone(),
        id_token: jwt,
        refresh_token: String::new(),
        expires_at: claim["exp"]
            .as_f64()
            .ok_or_else(|| bad("Hosted JWT has no expiration"))?,
        scope: scope(token)?,
    };
    auth::save(&stored).map_err(|_| bad("Cannot save hosted sign-in"))?;
    Ok(stored)
}
pub fn session(force: bool) -> Result<Stored> {
    let secret = token()?.ok_or_else(|| {
        bad("Run everett login for owner approval, or inject EVERETT_AGENT_TOKEN")
    })?;
    secret_parts(&secret)?;
    let scope = scope(&secret)?;
    if !force {
        if let Some(stored) =
            auth::load().filter(|s| s.scope == scope && s.expires_at - crate::session::now() > 60.0)
        {
            return Ok(stored);
        }
    }
    exchange(&secret, &auth::hostname())
}
pub fn begin(name: &str) -> Result<Value> {
    let mut bytes = [0u8; 96];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let bearer = format!("evt_{}", hex::encode(&bytes[..32]));
    let token = format!("{bearer}.{}", hex::encode(&bytes[32..]));
    let result = call(
        "action",
        "hosted:begin",
        json!({"tokenHash":hex::encode(Sha256::digest(bearer.as_bytes())),"name":name}),
        None,
    )?;
    let pending = Pending {
        deployment: deployment()?,
        token,
        device_code: field(&result, "deviceCode")?,
        agent_id: field(&result, "agentId")?,
        name: name.into(),
        expires_at: result["expiresAt"]
            .as_f64()
            .ok_or_else(|| bad("Invalid enrollment expiration"))?
            / 1000.0,
    };
    save_private("enrollment.json", &pending)?;
    Ok(
        json!({"state":"pending","agent_id":pending.agent_id,"user_code":result["userCode"],"verification_uri":result["verificationUri"],"expires_at":pending.expires_at,"interval":5}),
    )
}
pub fn finish() -> Result<Value> {
    let pending: Pending = serde_json::from_str(&read_private(&path("enrollment.json"))?)
        .map_err(|_| bad("No valid pending enrollment"))?;
    if pending.deployment != deployment()? {
        return Err(bad("Enrollment belongs to another deployment"));
    }
    if pending.expires_at <= crate::session::now() {
        let _ = fs::remove_file(path("enrollment.json"));
        return Err(bad("Enrollment expired; run everett login again"));
    }
    let result = call(
        "mutation",
        "hosted:poll",
        json!({"deviceCode":pending.device_code}),
        None,
    )?;
    match result["state"].as_str() {
        Some("pending") => Ok(json!({"state":"pending"})),
        Some("approved") => {
            save_private(
                "agent-token.json",
                &Credential {
                    deployment: pending.deployment,
                    token: pending.token.clone(),
                },
            )?;
            let stored = exchange(&pending.token, &pending.name)?;
            if stored.name != pending.agent_id {
                return Err(bad("Approved agent identity mismatch"));
            }
            let _ = fs::remove_file(path("enrollment.json"));
            Ok(Value::Object(auth::public_json(&stored)))
        }
        _ => {
            let _ = fs::remove_file(path("enrollment.json"));
            Err(bad("Enrollment denied or expired"))
        }
    }
}
pub fn login(name: &str, open: bool, force: bool) -> Result<Value> {
    if token()?.is_some() {
        return session(force).map(|s| Value::Object(auth::public_json(&s)));
    }
    let enrollment = begin(name)?;
    let uri = field(&enrollment, "verification_uri")?;
    eprintln!(
        "Open {uri} and approve code {} for {name}",
        field(&enrollment, "user_code")?
    );
    if open {
        let _ = std::process::Command::new(if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        })
        .arg(uri)
        .status();
    }
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let value = finish()?;
        if value["state"] != "pending" {
            return Ok(value);
        }
    }
}
pub fn export(out: &Path) -> Result<Value> {
    let stored = session(false)?;
    let secret = token()?.ok_or_else(|| bad("No hosted agent token"))?;
    let (bearer, seeds) = secret_parts(&secret)?;
    let seeds = crate::relay::bootstrap_seeds(&stored)?
        .or_else(|| seeds.map(str::to_string))
        .ok_or_else(|| bad("Register this agent locally before exporting its relay keys"))?;
    let secret = format!("{bearer}.{seeds}");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(out)
        .map_err(|_| bad("Cannot create token output; choose a new file in a private directory"))?;
    file.write_all(secret.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| bad("Cannot write agent token"))?;
    Ok(
        json!({"agent_id":stored.name,"written":out,"mode":"0600","note":"This is the current agent's credential, not a new agent. Enroll separately for each agent."}),
    )
}
pub fn logout() -> Result<Value> {
    let stored = session(false)?;
    call(
        "mutation",
        "hosted:revokeSelf",
        json!({}),
        Some(&stored.id_token),
    )?;
    clear();
    Ok(
        json!({"signed_out":true,"revoked":true,"note":"Remove any injected agent secret too; it is now revoked."}),
    )
}
pub fn clear() {
    auth::clear();
    let _ = fs::remove_file(path("agent-token.json"));
    let _ = fs::remove_file(path("enrollment.json"));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auth_component_never_includes_independent_relay_keys() {
        let bearer = format!("evt_{}", "a".repeat(64));
        let seeds = format!("{}{}", "b".repeat(64), "c".repeat(64));
        let bundle = format!("{bearer}.{seeds}");
        let (auth, client) = secret_parts(&bundle).unwrap();
        assert_eq!(auth, bearer);
        assert_eq!(client, Some(seeds.as_str()));
        assert!(!auth.contains(&seeds));
        let restored = secret_parts(&bundle).unwrap().1.unwrap();
        let signing = crate::relay::crypto::decode(&restored[..64]).unwrap();
        let encryption = crate::relay::crypto::decode(&restored[64..]).unwrap();
        let a =
            crate::relay::crypto::Keys::new("owner".into(), "agent".into(), signing, encryption);
        let b =
            crate::relay::crypto::Keys::new("owner".into(), "agent".into(), signing, encryption);
        assert_eq!(a.device, b.device);
        assert_ne!(signing, encryption);
        for invalid in [
            format!("{bearer}."),
            format!("{bearer}.{seeds}.extra"),
            "evt_é".into(),
        ] {
            assert!(secret_parts(&invalid).is_err());
        }
    }
}
