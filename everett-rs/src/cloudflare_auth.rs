//! Native Cloudflare enrollment. OAuth gateway grants stay separate.
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
    #[serde(default)]
    pub base_url: String,
    pub agent_id: String,
    pub owner: String,
    pub device_name: String,
    token: String,
    expires_at: u64,
}

impl Credential {
    pub(crate) fn expired(&self) -> bool {
        self.expires_at <= (session::now() * 1000.0) as u64
    }
}

#[derive(Serialize, Deserialize)]
struct Pending {
    issuer: String,
    #[serde(default)]
    base_url: String,
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

pub fn require_provider() -> Result<()> {
    if crate::config::get("auth.provider", Some("EVERETT_AUTH_PROVIDER"), "cloudflare") != "cloudflare" {
        return Err(fail(2, "auth.provider must be cloudflare; run everett login --provider cloudflare"));
    }
    Ok(())
}

pub const PUBLIC_ISSUER: &str = "https://everett.raitox.tech";
pub const PUBLIC_RELAY: &str = "https://relay.raitox.tech";

fn canonical_identity(value: &str) -> &str {
    if value == PUBLIC_RELAY { PUBLIC_ISSUER } else { value }
}

fn pending_base(pending: &Pending) -> &str {
    if pending.base_url.is_empty() { pending.issuer.as_str() } else { pending.base_url.as_str() }
}

fn cloudflare_config_active(stored_or_pending: bool) -> bool {
    let provider = crate::config::get("auth.provider", Some("EVERETT_AUTH_PROVIDER"), "cloudflare");
    provider == "cloudflare"
        || std::env::var_os("EVERETT_AUTH_ISSUER").is_some()
        || std::env::var_os("EVERETT_AUTH_BASE_URL").is_some()
        || !stored_or_pending
}

fn resolve_identity(args: &Args, stored: Option<&Credential>, pending: Option<&Pending>) -> Result<String> {
    if let Some(issuer) = args.auth_issuer.as_deref().filter(|s| !s.is_empty()) {
        return origin(canonical_identity(issuer));
    }
    if cloudflare_config_active(stored.is_some() || pending.is_some()) {
        let configured = auth::issuer();
        if !configured.is_empty() {
            return origin(canonical_identity(&configured));
        }
    }
    if let Some(stored) = stored {
        return origin(&stored.issuer);
    }
    if let Some(pending) = pending {
        return origin(&pending.issuer);
    }
    origin(PUBLIC_ISSUER)
}

fn resolve_base(args: &Args, stored: Option<&Credential>, pending: Option<&Pending>) -> Result<String> {
    if let Some(base) = args.auth_base_url.as_deref().filter(|s| !s.is_empty()) {
        return origin(base);
    }
    if let Some(issuer) = args.auth_issuer.as_deref().filter(|s| !s.is_empty()) {
        return origin(issuer);
    }
    if cloudflare_config_active(stored.is_some() || pending.is_some()) {
        let configured_base = auth::base_url();
        if !configured_base.is_empty() {
            return origin(&configured_base);
        }
        let configured_issuer = auth::issuer();
        if !configured_issuer.is_empty() {
            return origin(&configured_issuer);
        }
    }
    if let Some(pending) = pending {
        return origin(pending_base(pending));
    }
    if let Some(stored) = stored {
        return origin(&stored.connect_origin());
    }
    origin(PUBLIC_RELAY)
}

pub fn onboarding_issuer() -> Option<String> {
    crate::config::values().ok()?;
    let provider = crate::config::get("auth.provider", Some("EVERETT_AUTH_PROVIDER"), "");
    if !provider.is_empty() && provider != "cloudflare"
    {
        return None;
    }
    if credential_path().exists() || pending_path().exists() {
        return Some(String::new());
    }
    Some(auth::issuer())
}

pub fn authenticated() -> bool {
    load().ok().flatten().is_some_and(|stored| stored.expires_at > (session::now() * 1000.0) as u64)
}

pub fn onboard_sign_in(open: bool) -> Result<Value> {
    let issuer = onboarding_issuer().ok_or_else(|| fail(2, "existing account configuration requires its selected sign-in provider"))?;
    let identity = sign_in(&Args {
        open,
        auth_provider: Some("cloudflare".into()),
        auth_issuer: (!issuer.is_empty()).then_some(issuer),
        ..Default::default()
    })?;
    if identity["verified"] == true {
        crate::config::set_value("auth.provider", "cloudflare")
            .map_err(|_| fail(6, "identity verified, but could not save the selected account provider; retry sign-in"))?;
    }
    Ok(identity)
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
        Url::parse(value).map_err(|_| fail(2, "Cloudflare origin must be an HTTPS origin"))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(fail(2, "Cloudflare origin must be an HTTPS origin without credentials, path, query or fragment (HTTP is allowed only on loopback)"));
    }
    Ok(url.origin().ascii_serialization())
}

fn request(issuer: &str, path: &str, value: &Value, token: Option<&str>) -> Result<(u16, Value)> {
    request_limited(issuer, path, value, token, 64 * 1024)
}

enum RequestFailure {
    Transport,
    Terminal(EverettError),
}

impl RequestFailure {
    fn from_send(error: ureq::Error) -> Self {
        match error {
            ureq::Error::Timeout(_) | ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => {
                Self::Transport
            }
            ureq::Error::Io(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::NotConnected
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::HostUnreachable
                        | std::io::ErrorKind::NetworkUnreachable
                ) => Self::Transport,
            _ => Self::Terminal(fail(6, "Cloudflare is unreachable; stored credentials were kept")),
        }
    }

    fn into_error(self) -> EverettError {
        match self {
            Self::Transport => fail(6, "Cloudflare is unreachable; stored credentials were kept"),
            Self::Terminal(error) => error,
        }
    }
}

fn request_limited(issuer: &str, path: &str, value: &Value, token: Option<&str>, limit: u64) -> Result<(u16, Value)> {
    request_with_timeout(issuer, path, value, token, limit, Duration::from_secs(10))
        .map_err(RequestFailure::into_error)
}

fn request_with_timeout(
    issuer: &str,
    path: &str,
    value: &Value,
    token: Option<&str>,
    limit: u64,
    timeout: Duration,
) -> std::result::Result<(u16, Value), RequestFailure> {
    let issuer = origin(issuer).map_err(RequestFailure::Terminal)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
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
        .map_err(RequestFailure::from_send)?;
    let status = response.status().as_u16();
    let invalid_response = || {
        RequestFailure::Terminal(fail(
            6,
            "Cloudflare returned an invalid response; stored credentials were kept",
        ))
    };
    let body = response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|error| match RequestFailure::from_send(error) {
            RequestFailure::Transport if status == 200 => RequestFailure::Transport,
            _ => invalid_response(),
        })?;
    let body = serde_json::from_slice(&body).map_err(|_| invalid_response())?;
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

fn start(identity: &str, base: &str, name: &str) -> Result<Pending> {
    if name.trim().is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(fail(2, "device name must have 1–100 visible characters"));
    }
    let (status, body) = request(base, "/v2/auth/device", &json!({"name":name}), None)?;
    if status != 200 {
        return Err(status_error(status));
    }
    let pending = Pending {
        issuer: origin(identity)?,
        base_url: origin(base)?,
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
        || pending.verification_uri != format!("{}/enroll", pending.base_url)
        || pending.verification_uri_complete
            != format!("{}/enroll?code={}", pending.base_url, pending.user_code)
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
        pending_base(pending),
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

fn verify_identity(stored: &Credential) -> Result<Option<Value>> {
    if stored.expires_at <= (session::now() * 1000.0) as u64 {
        return Err(fail(
            1,
            "Cloudflare credential expired; sign in again (native credentials cannot be refreshed)",
        ));
    }
    let (status, body) = request(
        &stored.connect_origin(),
        "/v2/relay/whoami",
        &json!({}),
        Some(&stored.token),
    )?;
    if status == 401 {
        return Ok(None);
    }
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
    Ok(Some(
        json!({"signed_in":true,"verified":true,"provider":"cloudflare","issuer":stored.issuer,
        "owner":stored.owner,"agent_id":stored.agent_id,"device_name":stored.device_name,
        "role":identity["role"],"expires_at":stored.expires_at}),
    ))
}

pub fn whoami(stored: &Credential) -> Result<Value> {
    verify_identity(stored)?.ok_or_else(|| status_error(401))
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
        let remaining = pending.expires_at - session::now();
        if remaining <= 0.0 {
            continue;
        }
        let response = request_with_timeout(
            pending_base(pending),
            "/v2/auth/poll",
            &json!({"device_code":pending.device_code}),
            None,
            64 * 1024,
            Duration::from_secs_f64(remaining.min(10.0)),
        );
        if INTERRUPTED.load(Ordering::Relaxed) {
            // An approved response must be saved even when Ctrl-C races approval delivery.
            if !matches!(&response, Ok((200, body)) if body["state"] == "approved") {
                return Err(cancel(pending)?.error());
            }
        }
        let (status, body) = match response {
            Ok(response) => response,
            Err(RequestFailure::Transport) => {
                interval = interval.saturating_mul(2).min(60);
                eprintln!("Connection interrupted. Retrying approval…");
                continue;
            }
            Err(RequestFailure::Terminal(error)) => return Err(error),
        };
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
                    base_url: pending_base(pending).to_string(),
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

fn open_browser(pending: &Pending) -> Result<()> {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let mut child = match std::process::Command::new(opener)
        .arg(&pending.verification_uri_complete)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            eprintln!("Could not open the browser; open the link above on any device.");
            return Ok(());
        }
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(cancel(pending)?.error());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    eprintln!("Could not open the browser; open the link above on any device.");
                }
                return Ok(());
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                eprintln!("Browser opener did not finish; open the link above on any device.");
                return Ok(());
            }
        }
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
    let issuer = resolve_identity(args, stored.as_ref(), pending.as_ref())?;
    let base = resolve_base(args, stored.as_ref(), pending.as_ref())?;
    if !args.force && pending.is_none() {
        if let Some(stored) = &stored {
            if stored.issuer != issuer {
                return Err(fail(2, "stored credential belongs to a different issuer; use --force for a new approval"));
            }
            if stored.expires_at > (session::now() * 1000.0) as u64 {
                if let Some(identity) = verify_identity(stored)? {
                    return Ok(identity);
                }
            }
        }
    }
    let _signal = InterruptGuard::install()?;
    if let Some(old) = &pending {
        if old.issuer != issuer {
            return Err(fail(
                2,
                "pending login belongs to a different issuer; cancel it before switching",
            ));
        }
        if pending_base(old) != base {
            return Err(fail(
                2,
                "pending login belongs to a different relay URL; cancel it before switching",
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
        None => start(&issuer, &base, &args.device.clone().unwrap_or_else(auth::hostname))?,
    };
    eprintln!(
        "Open {}\nCompare code {} and device name {}. Approve in your browser.",
        pending.verification_uri_complete, pending.user_code, pending.device_name
    );
    if INTERRUPTED.load(Ordering::Relaxed) {
        return Err(cancel(&pending)?.error());
    }
    if args.open {
        open_browser(&pending)?;
    }
    if INTERRUPTED.load(Ordering::Relaxed) {
        return Err(cancel(&pending)?.error());
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
                        &stored.connect_origin(),
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

impl Credential {
    fn connect_origin(&self) -> String {
        let configured = crate::config::get("auth.base_url", Some("EVERETT_AUTH_BASE_URL"), "");
        if !configured.is_empty() {
            return configured;
        }
        if !self.base_url.is_empty() {
            return self.base_url.clone();
        }
        if self.issuer == PUBLIC_ISSUER && auth::issuer().is_empty() {
            return PUBLIC_RELAY.into();
        }
        self.issuer.clone()
    }

    pub(crate) fn relay_origin(&self) -> Result<String> {
        if self.expires_at <= (session::now() * 1000.0) as u64 {
            return Err(fail(
                1,
                "Cloudflare relay denied: credential expired; sign in again",
            ));
        }
        crate::relay::valid(&self.agent_id)?;
        origin(&self.connect_origin())
    }

    pub(crate) fn relay_request(&self, name: &str, value: &Value) -> Result<Value> {
        let operation = match name {
            "relay:whoami" => "whoami", "relay:register" => "register", "relay:device" => "device",
            "relay:devices" => "devices", "relay:credentials" => "credentials", "relay:memberships" => "memberships",
            "relay:revokeAgent" => "revokeAgent", "relay:revokeMember" => "revokeMember", "relay:rotate" => "rotate",
            "relay:enqueue" => "enqueue", "relay:pending" => "pending", "relay:accept" => "accept",
            "relay:observe" => "observe", "relay:status" => "status",
            "relay:deviceLabels" => "deviceLabels", "relay:renameDevice" => "renameDevice",
            "relay:exposeSession" => "exposeSession", "relay:sessionDirectory" => "sessionDirectory",
            _ => return Err(fail(2, "this operation is not supported by the Cloudflare relay; SSH publication and cross-owner grants are unavailable")),
        };
        if crate::config::get("auth.provider", Some("EVERETT_AUTH_PROVIDER"), "cloudflare")
            != "cloudflare"
        {
            return Err(fail(
                1,
                "Cloudflare relay denied: provider changed; restart the relay",
            ));
        }
        let issuer = self.relay_origin()?;
        let current = load()?.ok_or_else(|| {
            fail(
                1,
                "Cloudflare relay denied: signed out; restart after signing in",
            )
        })?;
        if current.issuer != self.issuer
            || current.owner != self.owner
            || current.agent_id != self.agent_id
            || current.token != self.token
        {
            return Err(fail(
                1,
                "Cloudflare relay denied: signed-in identity changed; restart the relay",
            ));
        }
        let (status, body) = request_limited(
            &issuer,
            &format!("/v2/relay/{operation}"),
            value,
            Some(&self.token),
            8 * 1024 * 1024,
        )?;
        if status != 200 {
            if (400..500).contains(&status) && status != 429 {
                if body.get("error").and_then(Value::as_str) == Some("device_unavailable") {
                    return Err(fail(1, &format!("Cloudflare relay denied {operation} (HTTP {status}, device_unavailable): a device has no active registered key bundle. Verify the account and device registration; this is separate from account sign-in.")));
                }
                return Err(fail(1, &format!("Cloudflare relay denied {operation} (HTTP {status}); check approval, current keys, message expiry and peer pins")));
            }
            return Err(fail(6, &format!("Cloudflare relay {operation} unavailable (HTTP {status}); delivery is unconfirmed; retry with the same idempotency key")));
        }
        body.get("result").cloned().ok_or_else(|| {
            fail(
                6,
                "Cloudflare relay returned an invalid result; delivery is unconfirmed",
            )
        })
    }
}
