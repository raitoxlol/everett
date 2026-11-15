//! Supervise OpenAI `tunnel-client` for an Everett dot.
//!
//! `tunnel-client` v0.0.16 logs `poll timed out; backing off` with
//! `controlplane poll transport failed (timeout)` when a control-plane long-poll
//! hits the client deadline. That is a failed poll, not the normal empty `204`.
//! The poller backs off and retries, and a later success logs
//! `poller recovered; polling operational`. `/readyz` is only a startup gate, so
//! it can stay `200 ready` while every later poll times out and ChatGPT calls
//! sit unforwarded. A new process polls the queued work immediately.
//!
//! This supervisor restarts on exit or repeated timeout warnings, and renews
//! connections on a bounded schedule even without diagnostic logs. Rotation
//! is recovery insurance, not evidence that the upstream route is healthy.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;

use regex::Regex;
use wait_timeout::ChildExt;

const PASS_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "XDG_CONFIG_HOME",
    "XDG_RUNTIME_DIR",
    "XDG_DATA_HOME",
    "TUNNEL_CLIENT_PROFILE_DIR",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "ALL_PROXY",
    "all_proxy",
    "EVERETT_HOME",
];

const FILE_KEYS: &[&str] = &[
    "CONTROL_PLANE_API_KEY",
    "CONTROL_PLANE_BASE_URL",
    "CONTROL_PLANE_TUNNEL_ID",
    "CONTROL_PLANE_POLL_TIMEOUT",
    "CONTROL_PLANE_INITIAL_POLL_TIMEOUT",
    "CONTROL_PLANE_HTTP_PROXY",
    "CONTROL_PLANE_CLIENT_CERT",
    "CONTROL_PLANE_CLIENT_KEY",
    "CONTROL_PLANE_URL_PATH",
];

const MAX_ENV_FILE: usize = 64 * 1024;
const MAX_ENV_VALUE: usize = 4096;

static STOP: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
pub struct WatchConfig {
    pub profile: String,
    pub tunnel_client: PathBuf,
    pub env_file: PathBuf,
    pub timeout_streak: u32,
    pub restart_delay: Duration,
    pub max_restarts: u32,
    pub max_connection_age: Duration,
    pub expected_resource: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollSignal {
    None,
    Timeout,
    Recovered,
}

#[derive(Debug)]
pub struct PollWatch {
    consecutive: u32,
    threshold: u32,
}

impl PollWatch {
    pub fn new(threshold: u32) -> Self {
        Self {
            consecutive: 0,
            threshold: threshold.max(1),
        }
    }

    /// True when `line` completes a run of poll timeouts with no recovery.
    pub fn observe(&mut self, line: &str) -> bool {
        match classify_poll_line(line) {
            PollSignal::Recovered => {
                self.consecutive = 0;
                false
            }
            PollSignal::Timeout => {
                self.consecutive = self.consecutive.saturating_add(1);
                self.consecutive >= self.threshold
            }
            PollSignal::None => false,
        }
    }
}

pub fn classify_poll_line(line: &str) -> PollSignal {
    let lower = line.to_ascii_lowercase();
    if lower.contains("poller recovered") {
        return PollSignal::Recovered;
    }
    if lower.contains("poll timed out")
        || (lower.contains("poll transport failed") && lower.contains("timeout"))
    {
        return PollSignal::Timeout;
    }
    PollSignal::None
}

pub fn redact_log_line(line: &str) -> String {
    let headers = credential_re().replace_all(line, "[redacted]");
    let assigned = assignment_re().replace_all(&headers, "$1=[redacted]");
    token_re()
        .replace_all(&assigned, "sk-[redacted]")
        .into_owned()
}

fn credential_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bauthorization\s*[=:].*|\bbearer\s+[^\s,;]+")
            .expect("credential redaction pattern")
    })
}

fn assignment_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(CONTROL_PLANE_API_KEY|OPENAI_ADMIN_KEY|OPENAI_API_KEY|authorization|api[_-]?key|bearer)\s*[=:]\s*\S+",
        )
        .expect("assignment redaction pattern")
    })
}

fn token_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"sk-[A-Za-z0-9_-]{8,}").expect("token redaction pattern"))
}

pub fn run(config: WatchConfig) -> i32 {
    if let Err(error) = validate_config(&config) {
        eprintln!("everett tunnel: {error}");
        return 2;
    }
    let parsed = match load_env_file(&config.env_file) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("everett tunnel: {error}");
            return 2;
        }
    };
    if let Some(resource) = &config.expected_resource {
        if let Err(error) = validate_resource(resource, parsed.get("CONTROL_PLANE_TUNNEL_ID").map(String::as_str)) {
            eprintln!("everett tunnel: {error}");
            return 2;
        }
    }
    let env = child_env_from(parsed, &inherited_allowlist());
    install_stop_handler();
    let mut restarts = 0u32;
    loop {
        if stop_requested() {
            return 0;
        }
        let mut supervised = match spawn_client(&config, &env) {
            Ok(supervised) => supervised,
            Err(error) => {
                eprintln!("everett tunnel: {error}");
                if !restart_budget(restarts, config.max_restarts) { return 4; }
                restarts += 1;
                if !sleep_interruptible(config.restart_delay) { return 0; }
                continue;
            }
        };
        eprintln!(
            "everett tunnel: started tunnel-client profile {}",
            config.profile
        );
        let mut watch = PollWatch::new(config.timeout_streak);
        let action = supervise(&mut supervised, &mut watch, config.max_connection_age);
        let _ = ensure_stopped(&mut supervised);
        match action {
            StopReason::Signal => return 0,
            StopReason::ConnectionAge => {
                if !restart_budget(restarts, config.max_restarts) { return 0; }
                restarts += 1;
                eprintln!("everett tunnel: renewing control-plane connection after {}s; forwarding health is unconfirmed", config.max_connection_age.as_secs());
            }
            StopReason::PollTimeouts => {
                if !restart_budget(restarts, config.max_restarts) {
                    eprintln!("everett tunnel: stopping after {restarts} restart(s)");
                    return 0;
                }
                restarts += 1;
                eprintln!(
                    "everett tunnel: restarting tunnel-client after {} consecutive poll timeouts",
                    config.timeout_streak
                );
            }
            StopReason::Exited(code) => {
                if !restart_budget(restarts, config.max_restarts) {
                    eprintln!("everett tunnel: stopping after {restarts} restart(s)");
                    return 0;
                }
                restarts += 1;
                eprintln!("everett tunnel: tunnel-client exited ({code}); restarting");
            }
        }
        if !sleep_interruptible(config.restart_delay) {
            return 0;
        }
    }
}

fn restart_budget(restarts: u32, max_restarts: u32) -> bool {
    max_restarts == 0 || restarts < max_restarts
}

fn validate_resource(resource: &str, tunnel_id: Option<&str>) -> Result<(), &'static str> {
    let url = url::Url::parse(resource).map_err(|_| "expected resource must be an exact HTTPS MCP tunnel URL")?;
    let id = url.path().strip_prefix("/v1/mcp/").filter(|id|
        id.strip_prefix("tunnel_").is_some_and(|suffix| suffix.len() == 32 && suffix.bytes().all(|b| b.is_ascii_hexdigit())))
        .ok_or("expected resource must contain one full tunnel ID")?;
    if url.scheme() != "https" || url.host_str() != Some("tunnel-service.gateway.unified-0.internal.api.openai.org")
        || url.port().is_some() || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err("expected resource must be an exact HTTPS MCP tunnel URL");
    }
    if tunnel_id != Some(id) { return Err("runtime CONTROL_PLANE_TUNNEL_ID differs from expected resource or is missing; reconcile the gateway, runtime and connector before starting"); }
    Ok(())
}

enum StopReason {
    Signal,
    PollTimeouts,
    ConnectionAge,
    Exited(i32),
}

struct Supervised {
    child: Child,
    rx: Receiver<String>,
    readers: Vec<JoinHandle<()>>,
    reaped: bool,
}

fn supervise(supervised: &mut Supervised, watch: &mut PollWatch, max_age: Duration) -> StopReason {
    let started = Instant::now();
    loop {
        if started.elapsed() >= max_age { return StopReason::ConnectionAge; }
        if let Some(code) = reap_if_exited(supervised) { return StopReason::Exited(code); }
        if stop_requested() {
            return StopReason::Signal;
        }
        match supervised.rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                eprintln!("tunnel-client: {}", redact_log_line(&line));
                if watch.observe(&line) {
                    return StopReason::PollTimeouts;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(code) = reap_if_exited(supervised) {
                    return StopReason::Exited(code);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Closed log pipes do not imply that the process exited.
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn reap_if_exited(supervised: &mut Supervised) -> Option<i32> {
    match supervised.child.try_wait() {
        Ok(Some(status)) => {
            supervised.reaped = true;
            Some(exit_code(status))
        }
        _ => None,
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}

fn spawn_client(config: &WatchConfig, env: &HashMap<String, String>) -> Result<Supervised, String> {
    let mut cmd = Command::new(&config.tunnel_client);
    cmd.args(["run", "--profile", &config.profile])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .envs(env);
    if config.expected_resource.is_some() {
        // The guard already matched this ID to the expected resource. Pin the
        // documented CLI option as well, rather than relying on profile/env precedence.
        cmd.args(["--control-plane.tunnel-id", &env["CONTROL_PLANE_TUNNEL_ID"]]);
    }
    // SAFETY: runs in the forked child before exec, where no other threads exist.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            let mut signals = std::mem::zeroed::<libc::sigset_t>();
            if libc::sigemptyset(&mut signals) == -1
                || libc::sigaddset(&mut signals, libc::SIGINT) == -1
                || libc::sigaddset(&mut signals, libc::SIGTERM) == -1
                || libc::sigprocmask(libc::SIG_UNBLOCK, &signals, std::ptr::null_mut()) == -1
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|error| {
        format!(
            "cannot start tunnel-client ({}): {error}",
            config.tunnel_client.display()
        )
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or("tunnel-client stdout is missing")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("tunnel-client stderr is missing")?;
    let (tx, rx) = mpsc::sync_channel(256);
    let readers = vec![pump(stdout, tx.clone()), pump(stderr, tx)];
    Ok(Supervised {
        child,
        rx,
        readers,
        reaped: false,
    })
}

fn pump<R>(reader: R, tx: mpsc::SyncSender<String>) -> JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut lines = BufReader::new(reader);
        let mut oversized = false;
        loop {
            // Never echo fragments of an oversized line: a later fragment
            // could contain credential bytes without their redaction prefix.
            let mut bytes = Vec::new();
            match lines.by_ref().take(8192).read_until(b'\n', &mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let ended = bytes.ends_with(b"\n");
                    if oversized || (bytes.len() == 8192 && !ended) {
                        oversized = !ended;
                        continue;
                    }
                    let line = String::from_utf8_lossy(&bytes).trim_end().to_owned();
                    if matches!(tx.try_send(line), Err(mpsc::TrySendError::Disconnected(_))) { break; }
                }
            }
        }
    })
}

fn ensure_stopped(supervised: &mut Supervised) -> i32 {
    let code = if supervised.reaped {
        0
    } else {
        match supervised.child.try_wait() {
            Ok(Some(status)) => {
                supervised.reaped = true;
                exit_code(status)
            }
            _ => signal_and_wait(supervised),
        }
    };
    signal_group(supervised.child.id(), libc::SIGKILL);
    for handle in supervised.readers.drain(..) {
        let _ = handle.join();
    }
    code
}

fn signal_and_wait(supervised: &mut Supervised) -> i32 {
    signal_group(supervised.child.id(), libc::SIGTERM);
    match supervised.child.wait_timeout(Duration::from_secs(2)) {
        Ok(Some(status)) => {
            supervised.reaped = true;
            return exit_code(status);
        }
        Ok(None) => {}
        Err(_) => {
            supervised.reaped = true;
            return -1;
        }
    }
    signal_group(supervised.child.id(), libc::SIGKILL);
    let _ = supervised.child.kill();
    let code = supervised.child.wait().map(exit_code).unwrap_or(-1);
    supervised.reaped = true;
    code
}

fn signal_group(pid: u32, signal: i32) {
    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    // SAFETY: the child called setsid, so its pid is the process-group id.
    unsafe {
        libc::kill(-pid, signal);
    }
}

fn validate_config(config: &WatchConfig) -> Result<(), String> {
    if !valid_profile(&config.profile) {
        return Err("profile must be 1-64 letters, digits, '.', '_' or '-'".into());
    }
    if config.timeout_streak == 0 || config.timeout_streak > 100 {
        return Err("timeout streak must be 1..=100".into());
    }
    if config.max_connection_age.is_zero() || config.max_connection_age > Duration::from_secs(240) {
        return Err("max connection age must be 1..=240 seconds".into());
    }
    if config.restart_delay > Duration::from_secs(300) {
        return Err("restart delay must be at most 300 seconds".into());
    }
    let name = config.tunnel_client.to_string_lossy();
    if name.is_empty() || name.starts_with('-') || name.contains('\0') {
        return Err("tunnel-client path is invalid".into());
    }
    Ok(())
}

fn valid_profile(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub fn load_env_file(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let meta =
        fs::symlink_metadata(path).map_err(|_| "cannot read env file metadata".to_string())?;
    if meta.file_type().is_symlink() {
        return Err("env file must be a regular file, not a symlink".into());
    }
    if !meta.file_type().is_file() {
        return Err("env file must be a regular file".into());
    }
    let mode = meta.permissions().mode();
    if mode & 0o077 != 0 {
        return Err("env file must be owner-only (chmod 0600)".into());
    }
    if meta.len() > MAX_ENV_FILE as u64 {
        return Err("env file is too large".into());
    }
    let mut raw = String::new();
    File::open(path)
        .and_then(|mut file| file.read_to_string(&mut raw))
        .map_err(|_| "cannot read env file".to_string())?;
    if raw.len() > MAX_ENV_FILE {
        return Err("env file is too large".into());
    }
    parse_env_file(&raw)
}

pub fn parse_env_file(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut parsed = BTreeMap::new();
    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed).trim();
        let Some((key, value)) = trimmed.split_once('=') else {
            return Err(format!("env file line {line_no} is not KEY=VALUE"));
        };
        let key = key.trim();
        if key == "OPENAI_ADMIN_KEY" {
            return Err("OPENAI_ADMIN_KEY must not be given to the running tunnel client".into());
        }
        if !FILE_KEYS.contains(&key) {
            return Err(format!("env file line {line_no} uses an unsupported name"));
        }
        if parsed.contains_key(key) {
            return Err(format!("env file line {line_no} repeats {key}"));
        }
        let value =
            unquote(value.trim()).map_err(|error| format!("env file line {line_no}: {error}"))?;
        if value.is_empty() || value.len() > MAX_ENV_VALUE || value.chars().any(|c| c.is_control())
        {
            return Err(format!("env file line {line_no} has an invalid value"));
        }
        parsed.insert(key.to_string(), value);
    }
    match parsed.get("CONTROL_PLANE_API_KEY") {
        Some(value) if !value.is_empty() => {}
        _ => {
            return Err(
                "env file must set CONTROL_PLANE_API_KEY for the running tunnel client".into(),
            )
        }
    }
    Ok(parsed)
}

fn unquote(value: &str) -> Result<String, String> {
    if let Some(inner) = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        if inner.contains('"') {
            return Err("quoted value is invalid".into());
        }
        return Ok(inner.to_string());
    }
    if value.contains('"') || value.contains('\'') {
        return Err("value quoting is invalid".into());
    }
    Ok(value.to_string())
}

pub fn child_env_from(
    parsed: BTreeMap<String, String>,
    inherited: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for key in PASS_ENV {
        if let Some(value) = inherited.get(*key) {
            if !value.is_empty() && !value.contains('\0') {
                env.insert((*key).to_string(), value.clone());
            }
        }
    }
    env.remove("OPENAI_ADMIN_KEY");
    env.remove("OPENAI_API_KEY");
    for (key, value) in parsed {
        if key != "OPENAI_ADMIN_KEY" {
            env.insert(key, value);
        }
    }
    env.remove("OPENAI_ADMIN_KEY");
    env
}

fn inherited_allowlist() -> HashMap<String, String> {
    let mut inherited = HashMap::new();
    for key in PASS_ENV {
        if let Ok(value) = std::env::var(key) {
            inherited.insert((*key).to_string(), value);
        }
    }
    inherited
}

fn install_stop_handler() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: the mask is installed before this function starts other threads.
        // sigwait, not an async handler, consumes the signals.
        unsafe {
            let mut set = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGTERM);
            libc::sigaddset(&mut set, libc::SIGINT);
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        }
        thread::spawn(|| {
            // SAFETY: this thread is the only sigwait caller for the blocked set.
            unsafe {
                let mut set = std::mem::zeroed::<libc::sigset_t>();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGTERM);
                libc::sigaddset(&mut set, libc::SIGINT);
                let mut received = 0;
                if libc::sigwait(&set, &mut received) == 0 {
                    STOP.store(true, Ordering::SeqCst);
                }
            }
        });
    });
}

fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

fn sleep_interruptible(total: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < total {
        if stop_requested() {
            return false;
        }
        let left = total.saturating_sub(start.elapsed());
        thread::sleep(left.min(Duration::from_millis(50)));
    }
    !stop_requested()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_guard_requires_the_exact_runtime_tunnel() {
        let id = "tunnel_11111111111111111111111111111111";
        let resource = format!("https://tunnel-service.gateway.unified-0.internal.api.openai.org/v1/mcp/{id}");
        assert!(validate_resource(&resource, Some(id)).is_ok());
        for runtime in [None, Some("tunnel_22222222222222222222222222222222")] {
            assert!(validate_resource(&resource, runtime).is_err());
        }
        for invalid in [resource.replace("https:","http:"), resource.replace(".api.openai.org",".evil.example"),
            format!("{resource}?token=synthetic-private"), format!("{resource}#fragment"), format!("{resource}/"), resource.replace("https://","https://user:password@") ] {
            let error=validate_resource(&invalid,Some(id)).unwrap_err();
            assert!(!error.contains("synthetic-private") && !error.contains("password"));
        }
    }

    #[test]
    fn oversized_and_multibyte_logs_do_not_leak_fragments_or_panic() {
        let input = format!("{}CONTROL_PLANE_API_KEY=private-test-value\nordinary\n", "界".repeat(9000));
        let (tx, rx) = mpsc::sync_channel(256);
        pump(std::io::Cursor::new(input.into_bytes()), tx).join().unwrap();
        assert_eq!(rx.into_iter().collect::<Vec<_>>(), vec!["ordinary"]);
    }

    use std::os::unix::fs::OpenOptionsExt;

    #[test]
    fn streak_restarts_only_after_repeated_timeouts_without_recovery() {
        let mut watch = PollWatch::new(3);
        let timeout =
            "poll timed out; backing off error=\"controlplane poll transport failed (timeout)\"";
        assert!(!watch.observe(timeout));
        assert!(!watch.observe(timeout));
        assert!(!watch.observe("poller recovered; polling operational"));
        assert!(!watch.observe(timeout));
        assert!(!watch.observe(timeout));
        assert!(watch.observe(timeout));
        assert_eq!(
            classify_poll_line("poll failed; backing off status_code=401"),
            PollSignal::None
        );
    }

    #[test]
    fn one_log_line_counts_as_one_timeout() {
        let mut watch = PollWatch::new(2);
        let line = "msg=\"poll timed out; backing off\" error=\"controlplane poll transport failed (timeout)\"";
        assert!(!watch.observe(line));
        assert!(watch.observe(line));
    }

    #[test]
    fn redaction_hides_runtime_and_admin_material() {
        let line = redact_log_line(
            "CONTROL_PLANE_API_KEY=test-runtime-key OPENAI_ADMIN_KEY=admin-test-secret sk-proj-abcdefghijklmnop",
        );
        assert!(!line.contains("test-runtime-key"));
        assert!(!line.contains("admin-test-secret"));
        assert!(!line.contains("abcdefghijklmnop"));
        assert!(
            line.contains("poll timed out") || line.contains("CONTROL_PLANE_API_KEY=[redacted]")
        );
        assert_eq!(
            redact_log_line("poll timed out; backing off"),
            "poll timed out; backing off"
        );
    }

    #[test]
    fn redaction_hides_bearer_header_and_standalone_tokens() {
        for line in [
            "Authorization: Bearer synthetic-opaque-token",
            "authorization=Bearer synthetic-opaque-token",
            "Authorization: Basic synthetic-opaque-token",
            "Bearer synthetic-opaque-token",
        ] {
            assert_eq!(redact_log_line(line), "[redacted]");
        }
    }

    #[test]
    fn env_file_rejects_admin_key_loose_mode_and_unknown_names() {
        let dir = tempfile::tempdir().unwrap();
        let loose = dir.path().join("loose.env");
        fs::write(&loose, "CONTROL_PLANE_API_KEY=test-runtime-key\n").unwrap();
        fs::set_permissions(&loose, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_env_file(&loose).unwrap_err().contains("chmod 0600"));

        let link = dir.path().join("link.env");
        std::os::unix::fs::symlink(&loose, &link).unwrap();
        assert!(load_env_file(&link).unwrap_err().contains("symlink"));

        assert!(parse_env_file("OPENAI_ADMIN_KEY=admin-test-secret\n")
            .unwrap_err()
            .contains("OPENAI_ADMIN_KEY"));
        assert!(
            parse_env_file("CONTROL_PLANE_API_KEY=test-runtime-key\nOTHER=1\n")
                .unwrap_err()
                .contains("unsupported")
        );
        assert!(parse_env_file("# comment\n")
            .unwrap_err()
            .contains("CONTROL_PLANE_API_KEY"));

        let tight = dir.path().join("tunnel.env");
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tight)
            .unwrap();
        fs::write(
            &tight,
            "# runtime key only\nexport CONTROL_PLANE_API_KEY=\"test-runtime-key\"\n",
        )
        .unwrap();
        fs::set_permissions(&tight, fs::Permissions::from_mode(0o600)).unwrap();
        let parsed = load_env_file(&tight).unwrap();
        assert_eq!(
            parsed.get("CONTROL_PLANE_API_KEY").map(String::as_str),
            Some("test-runtime-key")
        );

        let mut inherited = HashMap::new();
        inherited.insert("HOME".into(), "/home/owner".into());
        inherited.insert("PATH".into(), "/usr/bin".into());
        inherited.insert("OPENAI_ADMIN_KEY".into(), "admin-test-secret".into());
        inherited.insert("UNRELATED".into(), "kept-out".into());
        let env = child_env_from(parsed, &inherited);
        assert_eq!(
            env.get("CONTROL_PLANE_API_KEY").map(String::as_str),
            Some("test-runtime-key")
        );
        assert_eq!(env.get("HOME").map(String::as_str), Some("/home/owner"));
        assert!(!env.contains_key("OPENAI_ADMIN_KEY"));
        assert!(!env.contains_key("UNRELATED"));
        assert!(!format!("{env:?}").contains("admin-test-secret"));
    }
}
