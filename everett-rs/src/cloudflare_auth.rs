//! Native Cloudflare enrollment. OAuth gateway grants and Convex OIDC stay separate.
use crate::cli::Args;
use crate::error::{EverettError, Result};
use crate::{auth, peer, session};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use url::Url;

#[derive(Clone, Serialize, Deserialize)]
pub struct Credential {
    pub issuer: String,
    pub agent_id: String,
    pub owner: String,
    pub device_name: String,
    token: String,
    expires_at: u64,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    issuer: String,
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: String,
    device_name: String,
    expires_at: f64,
    interval: u64,
}

fn fail(code: i32, message: &str) -> EverettError {
    EverettError::new(code, message)
}

fn root() -> PathBuf {
    session::home().join(".everett")
}

pub fn credential_path() -> PathBuf {
    root().join("cloudflare-auth.json")
}

fn pending_path() -> PathBuf {
    root().join("cloudflare-pending.json")
}

pub fn available() -> bool {
    credential_path().exists()
        || pending_path().exists()
        || (crate::config::get("auth.provider", Some("EVERETT_AUTH_PROVIDER"), "oidc")
            == "cloudflare"
            && !auth::issuer().is_empty())
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|_| {
            fail(
                6,
                "stored Cloudflare sign-in is invalid; restore or remove its private file",
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(fail(
            6,
            "could not read the private Cloudflare sign-in file",
        )),
    }
}

pub fn load() -> Result<Option<Credential>> {
    read(&credential_path())
}

fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    peer::private_dir_io(&root())
        .map_err(|_| fail(6, "could not create the private Everett directory"))?;
    let mut temp = tempfile::NamedTempFile::new_in(root())
        .map_err(|_| fail(6, "could not create a private sign-in file"))?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|_| fail(6, "could not protect the private sign-in file"))?;
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|_| fail(6, "could not encode sign-in state"))?;
    temp.write_all(&bytes)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| fail(6, "could not write the private sign-in file"))?;
    temp.persist(path).map_err(|_| fail(6, "could not save the private sign-in file; approval may already be consumed; revoke it on the owner page before retrying"))?;
    File::open(root())
        .and_then(|f| f.sync_all())
        .map_err(|_| fail(6, "could not sync the private Everett directory"))
}

fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => File::open(root())
            .and_then(|f| f.sync_all())
            .map_err(|_| fail(6, "could not sync sign-in cleanup")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(fail(6, "could not remove the private sign-in file")),
    }
}

fn lock() -> Result<File> {
    peer::private_dir_io(&root())
        .map_err(|_| fail(6, "could not create the private Everett directory"))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(root().join("cloudflare-login.lock"))
        .map_err(|_| fail(6, "could not lock Cloudflare sign-in"))?;
    file.try_lock_exclusive().map_err(|_| fail(4, "another Cloudflare login is running; cancel it there or use `everett login --provider cloudflare --cancel`"))?;
    Ok(file)
}

fn origin(value: &str) -> Result<String> {
    let url =
        Url::parse(value).map_err(|_| fail(2, "Cloudflare issuer must be an HTTPS origin"))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(fail(2, "Cloudflare issuer must be an HTTPS origin without credentials, path, query or fragment (HTTP is allowed only on loopback)"));
    }
    Ok(url.origin().ascii_serialization())
}

fn request(issuer: &str, path: &str, value: &Value, token: Option<&str>) -> Result<(u16, Value)> {
    let issuer = origin(issuer)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into();
    let mut request = agent.post(format!("{issuer}{path}"));
    if let Some(token) = token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request
        .send_json(value)
        .map_err(|_| fail(6, "Cloudflare is unreachable; stored credentials were kept"))?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_json::<Value>()
        .map_err(|_| {
            fail(
                6,
                "Cloudflare returned an invalid response; stored credentials were kept",
            )
        })?;
    Ok((status, body))
}

fn status_error(status: u16) -> EverettError {
    match status {
        401 => fail(
            1,
            "Cloudflare credential expired or was revoked; sign in again",
        ),
        403 => fail(
            1,
            "Cloudflare denied this operation; check owner approval and device registration",
        ),
        429 => fail(6, "Cloudflare is rate limiting requests; retry later"),
        503 => fail(
            6,
            "hosted sign-in is not configured on this Cloudflare deployment",
        ),
        _ => fail(
            6,
            "Cloudflare refused the request; stored credentials were kept",
        ),
    }
}

fn text(body: &Value, key: &str) -> Result<String> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(|| fail(6, "Cloudflare response is missing a valid required field"))
}

fn start(issuer: &str, name: &str) -> Result<Pending> {
    if name.trim().is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(fail(2, "device name must have 1–100 visible characters"));
    }
    let (status, body) = request(issuer, "/v2/auth/device", &json!({"name":name}), None)?;
    if status != 200 {
        return Err(status_error(status));
    }
    let pending = Pending {
        issuer: origin(issuer)?,
        device_code: text(&body, "device_code")?,
        user_code: text(&body, "user_code")?,
        verification_uri: text(&body, "verification_uri")?,
        verification_uri_complete: text(&body, "verification_uri_complete")?,
        device_name: name.into(),
        expires_at: session::now()
            + body["expires_in"]
                .as_u64()
                .filter(|n| (1..=600).contains(n))
                .ok_or_else(|| fail(6, "Cloudflare returned an invalid approval lifetime"))?
                as f64,
        interval: body["interval"]
            .as_u64()
            .filter(|n| (1..=60).contains(n))
            .ok_or_else(|| fail(6, "Cloudflare returned an invalid poll interval"))?,
    };
    if pending.device_code.len() != 64
        || !pending.device_code.bytes().all(|b| b.is_ascii_hexdigit())
        || pending.user_code.len() != 12
        || !pending
            .user_code
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
        || pending.verification_uri != format!("{}/enroll", pending.issuer)
        || pending.verification_uri_complete
            != format!("{}/enroll?code={}", pending.issuer, pending.user_code)
    {
        return Err(fail(
            6,
            "Cloudflare returned an invalid approval code or URL",
        ));
    }
    save(&pending_path(), &pending)?;
    Ok(pending)
}

enum StoppedRequest {
    Cancelled,
    Denied,
    Expired,
}
impl StoppedRequest {
    fn state(&self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Denied => "denied",
            Self::Expired => "expired",
        }
    }
    fn error(&self) -> EverettError {
        fail(
            if matches!(self, Self::Cancelled) {
                130
            } else {
                5
            },
            &format!("sign-in {}; run login again", self.state()),
        )
    }
}

fn cancel(pending: &Pending) -> Result<StoppedRequest> {
    let (status, body) = request(
        &pending.issuer,
        "/v2/auth/cancel",
        &json!({"device_code":pending.device_code}),
        None,
    )?;
    if status != 200 {
        return Err(status_error(status));
    }
    match body["state"].as_str() {
        Some(state @ ("cancelled" | "expired" | "denied")) => {
            remove(&pending_path())?;
            Ok(match state { "cancelled" => StoppedRequest::Cancelled, "expired" => StoppedRequest::Expired, _ => StoppedRequest::Denied })
        },
        Some("approved" | "consumed") => Err(fail(5, "approval already happened; resume login to collect it, or revoke the credential on the owner page")),
        _ => Err(fail(6, "Cloudflare did not confirm cancellation; pending request was kept")),
    }
}

pub fn whoami(stored: &Credential) -> Result<Value> {
    if stored.expires_at <= (session::now() * 1000.0) as u64 {
        return Err(fail(
            1,
            "Cloudflare credential expired; sign in again (native credentials cannot be refreshed)",
        ));
    }
    let (status, body) = request(
        &stored.issuer,
        "/v2/relay/whoami",
        &json!({}),
        Some(&stored.token),
    )?;
    if status != 200 {
        return Err(status_error(status));
    }
    let identity = &body["result"];
    if identity["owner"].as_str() != Some(&stored.owner)
        || identity["deviceId"].as_str() != Some(&stored.agent_id)
        || !matches!(identity["role"].as_str(), Some("member" | "admin"))
    {
        return Err(fail(
            6,
            "Cloudflare identity does not match the approved credential",
        ));
    }
    Ok(
        json!({"signed_in":true,"verified":true,"provider":"cloudflare","issuer":stored.issuer,
        "owner":stored.owner,"agent_id":stored.agent_id,"device_name":stored.device_name,
        "role":identity["role"],"expires_at":stored.expires_at}),
    )
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
extern "C" fn interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}
struct InterruptGuard(libc::sigaction);
impl InterruptGuard {
    fn install() -> Result<Self> {
        // SAFETY: both sigaction records are initialized, handler only sets an atomic flag.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            let mut previous: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = interrupt as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            INTERRUPTED.store(false, Ordering::Relaxed);
            if libc::sigaction(libc::SIGINT, &action, &mut previous) != 0 {
                return Err(fail(6, "could not enable login cancellation"));
            }
            Ok(Self(previous))
        }
    }
}
impl Drop for InterruptGuard {
    fn drop(&mut self) {
        // SAFETY: restores the initialized handler saved by install.
        unsafe {
            libc::sigaction(libc::SIGINT, &self.0, std::ptr::null_mut());
        }
    }
}

fn wait(pending: &Pending) -> Result<Credential> {
    let _signal = InterruptGuard::install()?;
    let mut interval = pending.interval;
    loop {
        let delay = Instant::now() + Duration::from_secs(interval);
        while Instant::now() < delay {
            if INTERRUPTED.load(Ordering::Relaxed) {
                return Err(cancel(pending)?.error());
            }
            if session::now() >= pending.expires_at {
                remove(&pending_path())?;
                return Err(fail(5, "approval code expired; run login again"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let response = request(
            &pending.issuer,
            "/v2/auth/poll",
            &json!({"device_code":pending.device_code}),
            None,
        );
        if INTERRUPTED.load(Ordering::Relaxed) {
            // An approved response must be saved even when Ctrl-C races approval delivery.
            if !matches!(&response, Ok((200, body)) if body["state"] == "approved") {
                return Err(cancel(pending)?.error());
            }
        }
        let (status, body) = response?;
        if status == 429 && body["error"] == "slow_down" {
            interval = (interval + 5).min(60);
            continue;
        }
        if status != 200 {
            return Err(status_error(status));
        }
        match body["state"].as_str() {
            Some("pending") => {
                interval = body["interval"]
                    .as_u64()
                    .filter(|n| (1..=60).contains(n))
                    .ok_or_else(|| fail(6, "Cloudflare returned an invalid poll interval"))?;
            }
            Some("approved") => {
                let stored = Credential {
                    issuer: pending.issuer.clone(),
                    token: text(&body, "token")?,
                    agent_id: text(&body, "agent_id")?,
                    owner: text(&body, "owner")?,
                    device_name: pending.device_name.clone(),
                    expires_at: body["expires_at"]
                        .as_u64()
                        .filter(|n| *n > (session::now() * 1000.0) as u64)
                        .ok_or_else(|| fail(6, "Cloudflare returned an expired credential"))?,
                };
                if !stored.token.starts_with("evt_")
                    || stored.token.len() != 68
                    || !stored.token[4..].bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err(fail(6, "Cloudflare returned an invalid native credential"));
                }
                save(&credential_path(), &stored)?;
                remove(&pending_path())?;
                return Ok(stored);
            }
            Some(state @ ("denied" | "cancelled" | "expired")) => {
                remove(&pending_path())?;
                return Err(fail(5, &format!("approval was {state}; run login again")));
            }
            Some("consumed") => {
                remove(&pending_path())?;
                return Err(fail(5, "approval was already collected; if the credential was lost, revoke it on the owner page and run login again"));
            }
            _ => return Err(fail(6, "Cloudflare returned an unknown approval state")),
        }
    }
}

fn show(identity: &Value, json_output: bool) {
    if json_output {
        println!("{}", serde_json::to_string_pretty(identity).unwrap());
    } else {
        println!(
            "Signed in: {} ({})",
            identity["device_name"].as_str().unwrap_or(""),
            identity["agent_id"].as_str().unwrap_or("")
        );
        println!(
            "Identity verified by {}",
            identity["issuer"].as_str().unwrap_or("")
        );
    }
}

pub fn sign_in(args: &Args) -> Result<Value> {
    if args.auth_cancel {
        let Some(pending) = read::<Pending>(&pending_path())? else {
            return Err(fail(1, "no pending Cloudflare login"));
        };
        let outcome = cancel(&pending)?;
        return Ok(json!({"provider":"cloudflare","state":outcome.state()}));
    }
    let _lock = lock()?;
    let stored = load()?;
    let mut pending = read::<Pending>(&pending_path())?;
    let configured_issuer = args.auth_issuer.clone().unwrap_or_else(|| {
        let provider = crate::config::get("auth.provider", Some("EVERETT_AUTH_PROVIDER"), "oidc");
        if provider == "cloudflare"
            || std::env::var_os("EVERETT_AUTH_ISSUER").is_some()
            || (stored.is_none() && pending.is_none())
        {
            auth::issuer()
        } else {
            String::new()
        }
    });
    let issuer = origin(if configured_issuer.is_empty() {
        stored
            .as_ref()
            .map(|s| s.issuer.as_str())
            .or_else(|| pending.as_ref().map(|p| p.issuer.as_str()))
            .unwrap_or("")
    } else {
        &configured_issuer
    })?;
    if !args.force {
        if let Some(stored) = &stored {
            if stored.issuer != issuer {
                return Err(fail(2, "stored credential belongs to a different issuer; use --force for a new approval"));
            }
            if stored.expires_at > (session::now() * 1000.0) as u64 {
                return whoami(stored);
            }
        }
    }
    if let Some(old) = &pending {
        if old.issuer != issuer {
            return Err(fail(
                2,
                "pending login belongs to a different issuer; cancel it before switching",
            ));
        }
        if args.force {
            cancel(old)?;
            pending = None;
        } else if old.expires_at <= session::now() {
            remove(&pending_path())?;
            pending = None;
        }
    }
    let pending = match pending {
        Some(p) => p,
        None => start(&issuer, &args.device.clone().unwrap_or_else(auth::hostname))?,
    };
    eprintln!(
        "Open {}\nCompare code {} and device name {}. Approve in your browser.",
        pending.verification_uri_complete, pending.user_code, pending.device_name
    );
    if args.open {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        if !std::process::Command::new(opener)
            .arg(&pending.verification_uri_complete)
            .status()
            .is_ok_and(|s| s.success())
        {
            eprintln!("Could not open the browser; open the link above on any device.");
        }
    }
    if args.auth_no_wait {
        return Ok(
            json!({"provider":"cloudflare","state":"pending","user_code":pending.user_code,
            "verification_uri":pending.verification_uri_complete,"device_name":pending.device_name,"expires_at":pending.expires_at}),
        );
    }
    eprintln!("Waiting for approval… Ctrl-C cancels. Rerun login to resume after an interruption.");
    let stored = wait(&pending)?;
    let identity = whoami(&stored).map_err(|e| fail(e.code, &format!("credential saved privately, but identity verification failed: {e}; retry `everett whoami --provider cloudflare`")))?;
    Ok(identity)
}

fn report(command: &str, result: Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("everett {command}: {e}");
            e.code
        }
    }
}

pub fn cmd_login(args: &Args) -> i32 {
    report(
        "login",
        sign_in(args).map(|identity| {
            if identity["verified"] == true {
                show(&identity, args.json);
            } else if args.json || identity["state"] == "pending" {
                println!("{identity}");
            } else {
                println!(
                    "Sign-in {}.",
                    identity["state"].as_str().unwrap_or("stopped")
                );
            }
        }),
    )
}

pub fn cmd_whoami(args: &Args) -> i32 {
    report(
        "whoami",
        (|| {
            let Some(stored) = load()? else {
                if args.json {
                    println!("{}", json!({"signed_in":false,"provider":"cloudflare"}));
                }
                return Err(fail(1, "not signed in to Cloudflare"));
            };
            show(&whoami(&stored)?, args.json);
            Ok(())
        })(),
    )
}

pub fn cmd_logout(args: &Args) -> i32 {
    report(
        "logout",
        (|| {
            let _lock = lock()?;
            let stored = load()?;
            if args.revoke_device {
                if let Some(stored) = &stored {
                    let (status, _) = request(
                        &stored.issuer,
                        "/v2/relay/revokeAgent",
                        &json!({"deviceId":stored.agent_id}),
                        Some(&stored.token),
                    )?;
                    if status != 200 {
                        return Err(fail(6, "remote revocation was not confirmed; local credential kept. An unregistered credential must be revoked on the owner approval page"));
                    }
                }
            }
            if let Some(pending) = read::<Pending>(&pending_path())? {
                cancel(&pending)?;
            }
            remove(&credential_path())?;
            if args.json {
                println!(
                    "{}",
                    json!({"provider":"cloudflare","signed_out":true,"revoked":stored.is_some() && args.revoke_device})
                );
            } else if args.revoke_device && stored.is_some() {
                println!("Credential revoked; signed out.");
            } else {
                println!(
                    "Signed out locally. The owner approval page can revoke the remote credential."
                );
            }
            Ok(())
        })(),
    )
}
