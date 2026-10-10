//! Account sign-in: generic OIDC device authorization grant (RFC 8628).
//! Tokens live in ~/.everett/auth.json (0600, tmp+rename). No token value is
//! ever printed or logged.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::config;
use crate::error::{EverettError, Result};
use crate::session::{home, now};

pub const AUTH_DEFAULT_ISSUER: &str = "";
pub const AUTH_DEFAULT_CLIENT_ID: &str = "";

const SCOPES: &str = "openid email profile offline_access";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const REFRESH_WINDOW_SECONDS: f64 = 60.0;
const TIMEOUT: Duration = Duration::from_secs(10);

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build();
    config.into()
}

#[derive(Debug, Clone)]
pub struct Discovery {
    pub device_authorization_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: String,
    pub revocation_endpoint: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Stored {
    pub issuer: String,
    pub client_id: String,
    pub sub: String,
    pub email: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub device_name: String,
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    pub expires_at: f64,
    #[serde(default)]
    pub scope: String,
}

pub fn issuer() -> String {
    config::get("auth.issuer", Some("EVERETT_AUTH_ISSUER"), AUTH_DEFAULT_ISSUER)
}

pub fn client_id() -> String {
    config::get("auth.client_id", Some("EVERETT_AUTH_CLIENT_ID"), AUTH_DEFAULT_CLIENT_ID)
}

pub fn configured() -> bool {
    !issuer().is_empty() && !client_id().is_empty()
}

pub fn auth_path() -> std::path::PathBuf {
    home().join(".everett").join("auth.json")
}

pub fn load() -> Option<Stored> {
    let text = fs::read_to_string(auth_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(stored: &Stored) -> io::Result<()> {
    let path = auth_path();
    fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(stored).unwrap())?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn clear() {
    let _ = fs::remove_file(auth_path());
}

pub fn hostname() -> String {
    let mut buf = vec![0u8; 256];
    // SAFETY: buf is a valid writable buffer; gethostname writes a NUL-terminated name.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut _, buf.len()) };
    if rc == 0 {
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        let name = String::from_utf8_lossy(&buf[..end]).to_string();
        if !name.is_empty() {
            return name;
        }
    }
    Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this device".to_string())
}

fn http_err(context: &str, e: ureq::Error) -> EverettError {
    EverettError::new(6, format!("{context}: provider unreachable ({e})"))
}

fn get_json(url: &str, bearer: Option<&str>, context: &str) -> Result<Value> {
    let mut req = agent().get(url);
    if let Some(token) = bearer {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }
    let mut resp = req.call().map_err(|e| http_err(context, e))?;
    if !resp.status().is_success() {
        return Err(EverettError::new(
            6,
            format!("{context}: provider returned {}", resp.status()),
        ));
    }
    resp.body_mut()
        .read_json()
        .map_err(|e| EverettError::new(6, format!("{context}: invalid JSON response ({e})")))
}

fn post_form(url: &str, fields: &[(&str, &str)], context: &str) -> Result<(u16, Value)> {
    let mut resp = agent()
        .post(url)
        .send_form(fields.iter().copied())
        .map_err(|e| http_err(context, e))?;
    let status = resp.status().as_u16();
    let body: Value = resp
        .body_mut()
        .read_json()
        .map_err(|e| EverettError::new(6, format!("{context}: invalid JSON response ({e})")))?;
    Ok((status, body))
}

fn str_field(v: &Value, name: &str, context: &str) -> Result<String> {
    v.get(name)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| EverettError::new(6, format!("{context}: provider response missing `{name}`")))
}

pub fn discover(issuer: &str) -> Result<Discovery> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let body = get_json(&url, None, "discovery")?;
    Ok(Discovery {
        device_authorization_endpoint: str_field(&body, "device_authorization_endpoint", "discovery")?,
        token_endpoint: str_field(&body, "token_endpoint", "discovery")?,
        userinfo_endpoint: str_field(&body, "userinfo_endpoint", "discovery")?,
        revocation_endpoint: body
            .get("revocation_endpoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

#[derive(Debug)]
pub struct DeviceGrant {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub interval: f64,
    pub expires_in: f64,
}

pub fn start_device_flow(disc: &Discovery, client_id: &str) -> Result<DeviceGrant> {
    let (status, body) = post_form(
        &disc.device_authorization_endpoint,
        &[("client_id", client_id), ("scope", SCOPES)],
        "device authorization",
    )?;
    if !(200..300).contains(&status) {
        let detail = body.get("error_description").or_else(|| body.get("error")).and_then(|v| v.as_str()).unwrap_or("unknown error");
        return Err(EverettError::new(6, format!("device authorization failed: {detail}")));
    }
    Ok(DeviceGrant {
        device_code: str_field(&body, "device_code", "device authorization")?,
        user_code: str_field(&body, "user_code", "device authorization")?,
        verification_uri: str_field(&body, "verification_uri", "device authorization")?,
        verification_uri_complete: body
            .get("verification_uri_complete")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        interval: body.get("interval").and_then(|v| v.as_f64()).unwrap_or(5.0),
        expires_in: body.get("expires_in").and_then(|v| v.as_f64()).unwrap_or(900.0),
    })
}

/// Poll the token endpoint until the device grant resolves.
/// Error code 5 = denied/expired/deadline; 6 = other provider errors.
pub fn poll_token(disc: &Discovery, client_id: &str, grant: &DeviceGrant) -> Result<Value> {
    let deadline = now() + grant.expires_in.max(0.0);
    let mut interval = grant.interval.max(0.0);
    loop {
        if now() >= deadline {
            return Err(EverettError::new(5, "sign-in timed out before approval"));
        }
        sleep(Duration::from_secs_f64(interval));
        let (status, body) = post_form(
            &disc.token_endpoint,
            &[
                ("grant_type", DEVICE_GRANT),
                ("client_id", client_id),
                ("device_code", grant.device_code.as_str()),
            ],
            "token",
        )?;
        if (200..300).contains(&status) {
            return Ok(body);
        }
        match body.get("error").and_then(|v| v.as_str()).unwrap_or("") {
            "authorization_pending" => {}
            "slow_down" => interval += 5.0,
            "access_denied" => return Err(EverettError::new(5, "sign-in was denied")),
            "expired_token" => return Err(EverettError::new(5, "the device code expired")),
            other => {
                return Err(EverettError::new(
                    6,
                    format!("token endpoint returned an error: {other}"),
                ))
            }
        }
    }
}

pub fn userinfo(disc: &Discovery, access_token: &str) -> Result<(String, String, String)> {
    let body = get_json(&disc.userinfo_endpoint, Some(access_token), "userinfo")?;
    let sub = str_field(&body, "sub", "userinfo")?;
    let email = body
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok((sub, email, name))
}

pub enum Refresh {
    Rotated { access_token: String, refresh_token: String, expires_in: f64 },
    Revoked,
}

pub fn refresh(disc: &Discovery, client_id: &str, refresh_token: &str) -> Result<Refresh> {
    let (status, body) = post_form(
        &disc.token_endpoint,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
        ],
        "token refresh",
    )?;
    if (200..300).contains(&status) {
        let access = str_field(&body, "access_token", "token refresh")?;
        let rotated = body
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .unwrap_or(refresh_token)
            .to_string();
        return Ok(Refresh::Rotated {
            access_token: access,
            refresh_token: rotated,
            expires_in: body.get("expires_in").and_then(|v| v.as_f64()).unwrap_or(3600.0),
        });
    }
    if body.get("error").and_then(|v| v.as_str()) == Some("invalid_grant") {
        return Ok(Refresh::Revoked);
    }
    Err(EverettError::new(6, "the provider refused the token refresh"))
}

/// Best-effort RFC 7009 revocation of the refresh token.
/// Returns true when the provider acknowledged (or there was nothing to revoke).
pub fn revoke(stored: &Stored) -> bool {
    if stored.refresh_token.is_empty() {
        return true;
    }
    let Ok(disc) = discover(&stored.issuer) else {
        return false;
    };
    let Some(endpoint) = disc.revocation_endpoint else {
        return true;
    };
    match post_form(
        &endpoint,
        &[
            ("client_id", stored.client_id.as_str()),
            ("token", stored.refresh_token.as_str()),
            ("token_type_hint", "refresh_token"),
        ],
        "revocation",
    ) {
        Ok((status, _)) => (200..300).contains(&status),
        Err(_) => false,
    }
}

/// Refresh the stored session when the access token is within the window of
/// expiry. Persists rotated tokens. Returns the (possibly updated) record, or
/// an error when the refresh token was revoked (`invalid_grant`).
pub fn ensure_fresh(stored: &Stored) -> Result<Stored> {
    if stored.expires_at - now() > REFRESH_WINDOW_SECONDS || stored.refresh_token.is_empty() {
        return Ok(stored.clone());
    }
    let disc = discover(&stored.issuer)?;
    match refresh(&disc, &stored.client_id, &stored.refresh_token)? {
        Refresh::Rotated { access_token, refresh_token, expires_in } => {
            let mut updated = stored.clone();
            updated.access_token = access_token;
            updated.refresh_token = refresh_token;
            updated.expires_at = now() + expires_in;
            save(&updated).map_err(|e| {
                EverettError::new(6, format!("could not write {}: {e}", auth_path().display()))
            })?;
            Ok(updated)
        }
        Refresh::Revoked => {
            clear();
            Err(EverettError::new(
                1,
                "this device's sign-in was revoked; run `everett login` to sign in again",
            ))
        }
    }
}

pub fn public_json(stored: &Stored) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("signed_in".to_string(), json!(true));
    out.insert("issuer".to_string(), json!(stored.issuer));
    out.insert("email".to_string(), json!(stored.email));
    out.insert("sub".to_string(), json!(stored.sub));
    out.insert("name".to_string(), json!(stored.name));
    out.insert("device_name".to_string(), json!(stored.device_name));
    out.insert("expires_at".to_string(), json!(stored.expires_at));
    out
}
