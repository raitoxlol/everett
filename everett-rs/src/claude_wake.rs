use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde_json::Value;

use crate::error::{EverettError, Result};
use crate::proc::{run_capture, which, RunOutput};
use crate::session::Session;

pub const MIN_VERSION: (u64, u64, u64) = (2, 1, 285);
pub const ATTACH_TIMEOUT: f64 = 30.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WakeOutcome {
    PromptSent { client_pid: u32, target_pid: u32 },
    NotAttached { detail: String },
    Failed { detail: String },
    TimedOut { client_pid: u32, target_pid: u32 },
    Cancelled { client_pid: u32, target_pid: u32 },
}

pub trait WakeRunner {
    fn capture(
        &self,
        argv: &[String],
        cwd: &str,
        timeout: f64,
    ) -> std::result::Result<RunOutput, String>;
    fn attach(
        &self,
        argv: &[String],
        cwd: &str,
        timeout: f64,
        receipt: &str,
        target_pid: u32,
    ) -> std::result::Result<PtyOutput, String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PtyOutput {
    pub client_pid: u32,
    pub code: i32,
    pub output: String,
    pub timed_out: bool,
    pub cancelled: bool,
}

static CANCEL_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn wake_signal_handler(_: libc::c_int) {
    CANCEL_REQUESTED.store(true, Ordering::Relaxed);
}

struct ScopedSignals {
    old_int: libc::sigaction,
    old_term: libc::sigaction,
}

impl ScopedSignals {
    fn install() -> std::io::Result<Self> {
        CANCEL_REQUESTED.store(false, Ordering::Relaxed);
        // SAFETY: zeroed sigaction values are initialized below before installation.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = wake_signal_handler as *const () as usize;
        action.sa_flags = 0;
        unsafe { libc::sigemptyset(&mut action.sa_mask) };
        // SAFETY: sigaction writes the previous handlers to the initialized structs.
        let mut old_int: libc::sigaction = unsafe { std::mem::zeroed() };
        let mut old_term: libc::sigaction = unsafe { std::mem::zeroed() };
        if unsafe { libc::sigaction(libc::SIGINT, &action, &mut old_int) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if unsafe { libc::sigaction(libc::SIGTERM, &action, &mut old_term) } != 0 {
            unsafe { libc::sigaction(libc::SIGINT, &old_int, std::ptr::null_mut()) };
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self { old_int, old_term })
    }
}

impl Drop for ScopedSignals {
    fn drop(&mut self) {
        // SAFETY: these are the exact handlers replaced by install().
        unsafe {
            libc::sigaction(libc::SIGINT, &self.old_int, std::ptr::null_mut());
            libc::sigaction(libc::SIGTERM, &self.old_term, std::ptr::null_mut());
        }
        CANCEL_REQUESTED.store(false, Ordering::Relaxed);
    }
}

pub struct PreparedWake {
    binary: String,
    session_id: String,
    cwd: String,
    target_pid: u32,
    _lock: File,
}

struct SystemWakeRunner;

impl WakeRunner for SystemWakeRunner {
    fn capture(
        &self,
        argv: &[String],
        cwd: &str,
        timeout: f64,
    ) -> std::result::Result<RunOutput, String> {
        let env = std::env::vars().collect();
        run_capture(argv, Some(cwd), &env, timeout).map_err(|e| e.to_string())
    }

    fn attach(
        &self,
        argv: &[String],
        cwd: &str,
        timeout: f64,
        receipt: &str,
        target_pid: u32,
    ) -> std::result::Result<PtyOutput, String> {
        run_pty(argv, cwd, timeout, receipt, target_pid)
    }
}

pub fn validate_target(session: &Session) -> Result<()> {
    if session.harness != "claude" {
        return Err(reject(
            "--wake is supported only for local Claude Code sessions.",
        ));
    }
    if session.source == "t3code" || session.source == "external" {
        return Err(reject(
            "Claude background --wake is unavailable for T3 Code or registered external sessions. Use --mode ping for T3 Code, or --mode inbox for external sessions.",
        ));
    }
    if crate::adapters::external::is_external(&session.harness) {
        return Err(reject("Registered external sessions are inbox-only."));
    }
    let parsed = uuid::Uuid::parse_str(&session.id)
        .map_err(|_| reject("--wake requires a full Claude session UUID."))?;
    if parsed.hyphenated().to_string() != session.id {
        return Err(reject(
            "--wake requires the canonical full Claude session UUID.",
        ));
    }
    let cwd = Path::new(&session.cwd);
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err(reject("Claude wake requires the existing trusted project directory recorded for this session."));
    }
    Ok(())
}

pub fn prepare(session: &Session) -> Result<PreparedWake> {
    prepare_with(session, &SystemWakeRunner)
}

pub fn prepare_with(session: &Session, runner: &dyn WakeRunner) -> Result<PreparedWake> {
    validate_target(session)?;
    let lock = wake_lock(&session.id)?;
    let path = std::env::var("PATH").unwrap_or_default();
    let binary = which("claude", &path).ok_or_else(|| reject("Claude Code is not on PATH."))?;
    let cwd = fs::canonicalize(&session.cwd)
        .map_err(|e| reject(format!("cannot resolve trusted Claude cwd: {e}")))?;
    let cwd = cwd.to_string_lossy().to_string();

    let version = checked_capture(runner, &[binary.clone(), "--version".into()], &cwd)?;
    let version_text = version.stdout.trim();
    if version.code != 0 || !version_at_least(version_text, MIN_VERSION) {
        return Err(reject(format!(
            "Claude Code must be version 2.1.285 or newer; found {:?}.",
            version_text
        )));
    }
    let agents = checked_capture(
        runner,
        &[binary.clone(), "agents".into(), "--json".into()],
        &cwd,
    )?;
    if agents.code != 0 {
        return Err(reject(
            "`claude agents --json` failed; background session state is unknown.",
        ));
    }
    let expected_cwd = fs::canonicalize(&session.cwd)
        .map_err(|e| reject(format!("cannot resolve trusted Claude cwd: {e}")))?;
    let target_pid = matching_background_pid(&agents.stdout, &session.id, &expected_cwd)?;
    if !pid_alive(target_pid) {
        return Err(reject(
            "Claude reported a background session PID that is not alive.",
        ));
    }
    Ok(PreparedWake {
        binary,
        session_id: session.id.clone(),
        cwd,
        target_pid,
        _lock: lock,
    })
}

fn wake_lock(session_id: &str) -> Result<File> {
    let home = std::env::var_os("HOME")
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| reject("Claude HOME is unavailable; cannot lock this session safely."))?;
    wake_lock_at(&home, session_id)
}

fn wake_lock_at(home: &Path, session_id: &str) -> Result<File> {
    let lock_dir = home.join(".everett").join("wake-locks");
    fs::create_dir_all(&lock_dir).map_err(|e| reject(format!("cannot prepare wake lock: {e}")))?;
    fs::set_permissions(&lock_dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| reject(format!("cannot secure Claude wake lock directory: {e}")))?;
    let lock_path = lock_dir.join(format!("{session_id}.lock"));
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(lock_path)
        .map_err(|e| reject(format!("cannot prepare wake lock: {e}")))?;
    lock.try_lock_exclusive()
        .map_err(|_| reject("a wake is already in progress for this Claude session."))?;
    Ok(lock)
}

pub fn wake(prepared: &PreparedWake, message_id: &str) -> WakeOutcome {
    wake_with(prepared, message_id, &SystemWakeRunner)
}

pub fn wake_with(
    prepared: &PreparedWake,
    message_id: &str,
    runner: &dyn WakeRunner,
) -> WakeOutcome {
    if !valid_message_id(message_id) {
        return WakeOutcome::NotAttached {
            detail: "the queued Everett message has an invalid ID.".into(),
        };
    }
    let nudge = match fixed_nudge(&prepared.session_id, message_id) {
        Ok(nudge) => nudge,
        Err(error) => return WakeOutcome::NotAttached { detail: error.message },
    };
    let argv = vec![
        prepared.binary.clone(),
        "--resume".into(),
        prepared.session_id.clone(),
        nudge,
    ];
    let agents = match runner.capture(
        &[prepared.binary.clone(), "agents".into(), "--json".into()],
        &prepared.cwd,
        5.0,
    ) {
        Ok(output) if output.code == 0 => output.stdout,
        _ => {
            return WakeOutcome::NotAttached {
                detail: "could not recheck Claude background session state before attach.".into(),
            }
        }
    };
    let expected_cwd = match fs::canonicalize(&prepared.cwd) {
        Ok(path) => path,
        Err(error) => {
            return WakeOutcome::NotAttached {
                detail: format!("trusted Claude cwd disappeared: {error}"),
            }
        }
    };
    match matching_background_pid(&agents, &prepared.session_id, &expected_cwd) {
        Ok(pid) if pid == prepared.target_pid => {}
        Ok(_) => {
            return WakeOutcome::NotAttached {
                detail: "Claude background session PID changed after preflight.".into(),
            }
        }
        Err(error) => {
            return WakeOutcome::NotAttached {
                detail: error.message,
            }
        }
    }
    if !pid_alive(prepared.target_pid) {
        return WakeOutcome::NotAttached {
            detail: "Claude background session PID is no longer alive.".into(),
        };
    }
    let short = &prepared.session_id[..8];
    let receipt = format!("Sent your prompt to the background session ({short})");
    match runner.attach(
        &argv,
        &prepared.cwd,
        ATTACH_TIMEOUT,
        &receipt,
        prepared.target_pid,
    ) {
        Ok(result) if result.cancelled => WakeOutcome::Cancelled {
            client_pid: result.client_pid,
            target_pid: prepared.target_pid,
        },
        Ok(result) if result.timed_out => WakeOutcome::TimedOut {
            client_pid: result.client_pid,
            target_pid: prepared.target_pid,
        },
        Ok(result)
            if result.output.contains(&receipt) && result.client_pid != prepared.target_pid =>
        {
            WakeOutcome::PromptSent {
                client_pid: result.client_pid,
                target_pid: prepared.target_pid,
            }
        }
        Ok(result) => {
            let detail = if result.output.contains("Your prompt was not sent") {
                "Claude reported that the prompt was not sent."
            } else if result.client_pid == prepared.target_pid {
                "Claude attach resolved to the background session PID; refusing to terminate that process."
            } else {
                "Claude did not emit the expected background-session receipt."
            };
            WakeOutcome::Failed {
                detail: detail.into(),
            }
        }
        Err(error) => WakeOutcome::Failed {
            detail: format!("could not attach to Claude through a PTY: {error}"),
        },
    }
}

fn checked_capture(runner: &dyn WakeRunner, argv: &[String], cwd: &str) -> Result<RunOutput> {
    runner
        .capture(argv, cwd, 5.0)
        .map_err(|e| reject(format!("could not run Claude preflight: {e}")))
}

fn matching_background_pid(output: &str, session_id: &str, cwd: &Path) -> Result<u32> {
    let value: Value = serde_json::from_str(output)
        .map_err(|_| reject("`claude agents --json` returned invalid JSON."))?;
    let rows = value
        .as_array()
        .or_else(|| value.get("agents").and_then(Value::as_array))
        .ok_or_else(|| reject("`claude agents --json` did not return an agents array."))?;
    let matches: Vec<&Value> = rows
        .iter()
        .filter(|row| row.get("sessionId").and_then(Value::as_str) == Some(session_id))
        .collect();
    if matches.len() != 1 {
        return Err(reject(
            "Claude must report exactly one matching background session.",
        ));
    }
    let row = matches[0];
    if row.get("kind").and_then(Value::as_str) != Some("background") {
        return Err(reject(
            "The matching Claude session is not confirmed as a background session.",
        ));
    }
    // Claude's observed idle-open row is state=done, status=busy. State is the
    // work predicate; status=busy alone is not evidence of active generation.
    if row.get("state").and_then(Value::as_str) != Some("done")
        || !matches!(
            row.get("status").and_then(Value::as_str),
            Some("busy" | "idle")
        )
        || row
            .get("waitingFor")
            .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
    {
        return Err(reject(
            "The Claude background session is active or waiting for user input.",
        ));
    }
    let pid = row
        .get("pid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0 && *pid <= u32::MAX as u64)
        .ok_or_else(|| reject("Claude did not report a valid background session PID."))?
        as u32;
    let reported_cwd = row
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| reject("Claude did not report the background session cwd."))?;
    let reported_cwd = fs::canonicalize(reported_cwd)
        .map_err(|_| reject("Claude's reported background session cwd is unavailable."))?;
    if reported_cwd != cwd {
        return Err(reject(
            "Claude's background session cwd does not match the trusted Everett session cwd.",
        ));
    }
    Ok(pid)
}

fn version_at_least(output: &str, min: (u64, u64, u64)) -> bool {
    let Some(start) = output.find(|c: char| c.is_ascii_digit()) else {
        return false;
    };
    let version: String = output[start..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    if !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return false;
    }
    let parts = version.split('.').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return false;
    }
    let (Ok(major), Ok(minor), Ok(patch)) = (
        parts[0].parse::<u64>(),
        parts[1].parse::<u64>(),
        parts[2].parse::<u64>(),
    ) else {
        return false;
    };
    (major, minor, patch) >= min
}

fn valid_message_id(id: &str) -> bool {
    (id.len() >= 2 && id.starts_with('m') && id[1..].chars().all(|c| c.is_ascii_alphanumeric()))
        || (id.len() == 70
            && id.starts_with("relay-")
            && id[6..]
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)))
}

fn fixed_nudge(session_id: &str, message_id: &str) -> Result<String> {
    let (pickup, reply) = nudge_commands(session_id, message_id, "claude")?;
    Ok(format!(
        "Use this Everett command: {pickup}. Read message {message_id}, do its request, then reply with {reply}. Reply through Everett so the result is linked to that message. Do not ask for permission changes or start a new session."
    ))
}

pub(crate) fn nudge_commands(
    session_id: &str,
    message_id: &str,
    harness: &str,
) -> Result<(String, String)> {
    let command = NudgeCommand::resolve()?.prefix();
    Ok((
        format!("{command} inbox --session {session_id} --message {message_id}"),
        format!("EVERETT_SESSION_ID={session_id} EVERETT_HARNESS_NAME={harness} {command} reply {message_id} '<your concise result>'"),
    ))
}

pub(crate) struct NudgeCommand {
    pub executable: std::path::PathBuf,
    pub everett_home: std::path::PathBuf,
    pub auth_provider: String,
}

impl NudgeCommand {
    pub fn resolve() -> Result<Self> {
        Ok(Self {
            executable: std::env::current_exe()
                .map_err(|e| reject(format!("cannot resolve Everett executable: {e}")))?,
            everett_home: std::path::absolute(crate::session::home())
                .map_err(|e| reject(format!("cannot resolve Everett profile against sender cwd: {e}")))?,
            auth_provider: crate::config::get("auth.provider", Some("EVERETT_AUTH_PROVIDER"), "cloudflare"),
        })
    }

    pub fn prefix(&self) -> String {
        format!("{}{}", command_env_prefix(&self.everett_home.to_string_lossy(), &self.auth_provider),
            shell_quote(&self.executable.to_string_lossy()))
    }
}

fn command_env_prefix(everett_home: &str, auth_provider: &str) -> String {
    format!(
        "EVERETT_HOME={} EVERETT_AUTH_PROVIDER={} ",
        shell_quote(everett_home),
        shell_quote(auth_provider)
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: kill(pid, 0) only checks process existence and sends no signal.
    let rc = unsafe { libc::kill(pid as i32, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn reject(message: impl Into<String>) -> EverettError {
    EverettError::new(2, message)
}

fn run_pty(
    argv: &[String],
    cwd: &str,
    timeout: f64,
    receipt: &str,
    target_pid: u32,
) -> std::result::Result<PtyOutput, String> {
    if argv.is_empty() || !timeout.is_finite() || timeout <= 0.0 {
        return Err("invalid PTY command or timeout".into());
    }
    let mut master_fd = -1;
    let mut slave_fd = -1;
    let mut size = libc::winsize {
        ws_row: 24,
        ws_col: 100,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty writes two valid file descriptors to these pointers.
    if unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(size),
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: each descriptor was returned by openpty and is owned exactly once here.
    let mut master = unsafe { File::from_raw_fd(master_fd) };
    let slave = unsafe { File::from_raw_fd(slave_fd) };
    let stdin = slave.try_clone().map_err(|e| e.to_string())?;
    let stdout = slave.try_clone().map_err(|e| e.to_string())?;
    let stderr = slave;
    let _signals = ScopedSignals::install()
        .map_err(|e| format!("cannot install scoped cancel handlers: {e}"))?;
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    // Provide a controlling terminal as well as tty-backed stdio.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child: Child = command.spawn().map_err(|e| e.to_string())?;
    let client_pid = child.id();
    if client_pid == target_pid {
        return Err("connector PID matched target daemon PID; stopped before attach".into());
    }
    let mut child = ChildGuard(Some(child));
    // SAFETY: set nonblocking on the owned PTY master so output drains cannot hang.
    let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut output = String::new();
    let deadline = Instant::now() + Duration::from_secs_f64(timeout.min(300.0));
    let mut buffer = [0u8; 4096];
    loop {
        match child.0.as_mut().expect("live connector").try_wait() {
            Ok(Some(status)) => {
                drain_master(&mut master, &mut output, &mut buffer);
                child.0.take();
                return Ok(PtyOutput {
                    client_pid,
                    code: status.code().unwrap_or(-1),
                    output,
                    timed_out: false,
                    cancelled: false,
                });
            }
            Ok(None) => {}
            Err(error) => return Err(error.to_string()),
        }
        if CANCEL_REQUESTED.load(Ordering::Relaxed) {
            terminate_connector(&mut child);
            return Ok(PtyOutput {
                client_pid,
                code: -1,
                output,
                timed_out: false,
                cancelled: true,
            });
        }
        if Instant::now() >= deadline {
            terminate_connector(&mut child);
            return Ok(PtyOutput {
                client_pid,
                code: -1,
                output,
                timed_out: true,
                cancelled: false,
            });
        }
        let mut poll_fd = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll receives one initialized descriptor record.
        let ready = unsafe { libc::poll(&mut poll_fd, 1, 100) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.to_string());
        }
        if ready == 0 {
            continue;
        }
        match master.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => {
                if output.len() < 131_072 {
                    let remaining = 131_072 - output.len();
                    output.push_str(&String::from_utf8_lossy(&buffer[..count.min(remaining)]));
                }
                if output.contains(receipt) {
                    // Match the documented interactive detach first, with a
                    // bounded direct-client fallback if Claude ignores Ctrl-Z.
                    let _ = std::io::Write::write_all(&mut master, b"\x1a");
                    let grace = Instant::now() + Duration::from_secs(2);
                    let mut status = None;
                    while Instant::now() < grace {
                        if CANCEL_REQUESTED.load(Ordering::Relaxed) {
                            terminate_connector(&mut child);
                            return Ok(PtyOutput {
                                client_pid,
                                code: -1,
                                output,
                                timed_out: false,
                                cancelled: true,
                            });
                        }
                        if let Ok(Some(done)) = child.0.as_mut().expect("live connector").try_wait()
                        {
                            status = Some(done);
                            break;
                        }
                        let _ = drain_master(&mut master, &mut output, &mut buffer);
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    let code = if let Some(status) = status {
                        status.code().unwrap_or(-1)
                    } else {
                        terminate_connector(&mut child);
                        0
                    };
                    child.0.take();
                    return Ok(PtyOutput {
                        client_pid,
                        code,
                        output,
                        timed_out: false,
                        cancelled: false,
                    });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
}

struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill(); // Direct connector PID only; never its process group.
            let _ = child.wait();
        }
    }
}

fn terminate_connector(child: &mut ChildGuard) {
    if let Some(process) = child.0.as_mut() {
        let _ = process.kill();
        let _ = process.wait();
    }
    child.0.take();
}

fn drain_master(master: &mut File, output: &mut String, buffer: &mut [u8]) -> bool {
    loop {
        match master.read(buffer) {
            Ok(0) => return true,
            Ok(count) => {
                if output.len() < 131_072 {
                    let remaining = 131_072 - output.len();
                    output.push_str(&String::from_utf8_lossy(&buffer[..count.min(remaining)]));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return true,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_ENV: Mutex<()> = Mutex::new(());

    fn row(sid: &str, pid: u32, cwd: &str, state: &str, status: &str, kind: &str) -> String {
        serde_json::json!([{"sessionId":sid,"pid":pid,"cwd":cwd,"state":state,"status":status,"kind":kind}]).to_string()
    }

    #[test]
    fn observed_background_idle_row_accepts_busy_status() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            matching_background_pid(
                &row(
                    "1f57ab77-c669-4bb6-9ac8-f9a9073de8ea",
                    std::process::id(),
                    cwd.to_str().unwrap(),
                    "done",
                    "busy",
                    "background"
                ),
                "1f57ab77-c669-4bb6-9ac8-f9a9073de8ea",
                &cwd
            )
            .unwrap(),
            std::process::id()
        );
    }

    #[test]
    fn discovery_rejects_active_waiting_wrong_kind_duplicate_and_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let sid = "1f57ab77-c669-4bb6-9ac8-f9a9073de8ea";
        for output in [
            row(sid, 19, cwd.to_str().unwrap(), "running", "busy", "background"),
            row(sid, 19, cwd.to_str().unwrap(), "done", "busy", "interactive"),
            row(sid, 19, "/tmp", "done", "busy", "background"),
            serde_json::json!([{"sessionId":sid,"pid":19,"cwd":cwd,"state":"done","status":"busy","kind":"background","waitingFor":"question"}]).to_string(),
            format!("[{},{}]", row(sid, 19, cwd.to_str().unwrap(), "done", "busy", "background").trim_matches(['[',']']), row(sid, 20, cwd.to_str().unwrap(), "done", "busy", "background").trim_matches(['[',']'])),
        ] {
            assert!(matching_background_pid(&output, sid, &cwd).is_err(), "accepted {output}");
        }
    }

    #[test]
    fn version_gate_and_message_id_gate_are_strict() {
        assert!(version_at_least("2.1.285 (Claude Code)", MIN_VERSION));
        assert!(version_at_least("Claude Code 2.2.0", MIN_VERSION));
        assert!(!version_at_least("2.1.284", MIN_VERSION));
        assert!(!version_at_least("2.1.285-beta.1", MIN_VERSION));
        assert!(!version_at_least("2.1..285", MIN_VERSION));
        assert!(!version_at_least("2.1", MIN_VERSION));
        assert!(!version_at_least("unknown", MIN_VERSION));
        assert!(valid_message_id("m0123456789"));
        assert!(!valid_message_id("/commands"));
    }

    #[test]
    fn target_gate_rejects_non_claude_t3_and_non_uuid_targets() {
        let cwd = std::env::current_dir().unwrap();
        let mut s = session(&cwd);
        s.id = "prefix".into();
        assert!(validate_target(&s).is_err());
        s.id = "1f57ab77-c669-4bb6-9ac8-f9a9073de8ea".into();
        s.source = "t3code".into();
        assert!(validate_target(&s).is_err());
        s.source.clear();
        s.harness = "codex".into();
        assert!(validate_target(&s).is_err());
    }

    struct FakeRunner {
        agents: String,
        attached: PtyOutput,
        seen: Mutex<Vec<Vec<String>>>,
    }
    impl WakeRunner for FakeRunner {
        fn capture(
            &self,
            argv: &[String],
            _: &str,
            _: f64,
        ) -> std::result::Result<RunOutput, String> {
            self.seen.lock().unwrap().push(argv.to_vec());
            if argv.last().is_some_and(|arg| arg == "--version") {
                Ok(RunOutput {
                    code: 0,
                    stdout: "2.1.285 (Claude Code)".into(),
                    stderr: String::new(),
                    timed_out: false,
                })
            } else {
                Ok(RunOutput {
                    code: 0,
                    stdout: self.agents.clone(),
                    stderr: String::new(),
                    timed_out: false,
                })
            }
        }
        fn attach(
            &self,
            argv: &[String],
            _: &str,
            _: f64,
            _: &str,
            _: u32,
        ) -> std::result::Result<PtyOutput, String> {
            self.seen.lock().unwrap().push(argv.to_vec());
            Ok(self.attached.clone())
        }
    }

    fn session(cwd: &Path) -> Session {
        Session::new(
            "claude",
            "1f57ab77-c669-4bb6-9ac8-f9a9073de8ea",
            cwd.to_str().unwrap(),
            "/tmp/transcript.jsonl",
            "",
            0.0,
        )
    }

    #[test]
    fn attach_uses_only_fixed_nudge_and_exact_native_argv() {
        let _env = TEST_ENV.lock().unwrap();
        let cwd = std::env::current_dir().unwrap();
        let session = session(&cwd);
        let fake = FakeRunner {
            agents: row(
                &session.id,
                std::process::id(),
                cwd.to_str().unwrap(),
                "done",
                "busy",
                "background",
            ),
            attached: PtyOutput {
                client_pid: std::process::id() + 1,
                code: 0,
                output: format!(
                    "Sent your prompt to the background session ({}); opening it…",
                    &session.id[..8]
                ),
                timed_out: false,
                cancelled: false,
            },
            seen: Mutex::new(Vec::new()),
        };
        let prepared = PreparedWake {
            binary: "/usr/bin/claude".into(),
            session_id: session.id.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
            target_pid: std::process::id(),
            _lock: OpenOptions::new().read(true).open("/dev/null").unwrap(),
        };
        assert_eq!(
            wake_with(&prepared, "m0123456789", &fake),
            WakeOutcome::PromptSent {
                client_pid: std::process::id() + 1,
                target_pid: std::process::id()
            }
        );
        let seen = fake.seen.lock().unwrap();
        let attach = seen.last().unwrap();
        assert_eq!(
            attach,
            &vec![
                "/usr/bin/claude",
                "--resume",
                &session.id,
                &fixed_nudge(&session.id, "m0123456789").unwrap()
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
        );
        assert!(!attach.iter().any(|arg| [
            "--bg",
            "--print",
            "--fork-session",
            "--dangerously-skip-permissions"
        ]
        .contains(&arg.as_str())));
    }

    #[test]
    fn wrong_receipt_and_timeout_are_distinct_and_do_not_claim_sent() {
        let cwd = std::env::current_dir().unwrap();
        let id = "1f57ab77-c669-4bb6-9ac8-f9a9073de8ea";
        let prepared = PreparedWake {
            binary: "/usr/bin/claude".into(),
            session_id: id.into(),
            cwd: cwd.to_string_lossy().into_owned(),
            target_pid: std::process::id(),
            _lock: OpenOptions::new().read(true).open("/dev/null").unwrap(),
        };
        let target_pid = std::process::id();
        let base = row(
            id,
            target_pid,
            cwd.to_str().unwrap(),
            "done",
            "busy",
            "background",
        );
        let failed = FakeRunner {
            agents: base.clone(),
            attached: PtyOutput {
                client_pid: target_pid + 1,
                code: 1,
                output: "Your prompt was not sent".into(),
                timed_out: false,
                cancelled: false,
            },
            seen: Mutex::new(Vec::new()),
        };
        assert!(matches!(
            wake_with(&prepared, "m0123456789", &failed),
            WakeOutcome::Failed { .. }
        ));
        let timeout = FakeRunner {
            agents: base,
            attached: PtyOutput {
                client_pid: target_pid + 1,
                code: -1,
                output: String::new(),
                timed_out: true,
                cancelled: false,
            },
            seen: Mutex::new(Vec::new()),
        };
        assert_eq!(
            wake_with(&prepared, "m0123456789", &timeout),
            WakeOutcome::TimedOut {
                client_pid: target_pid + 1,
                target_pid
            }
        );
    }

    #[test]
    fn pty_timeout_kills_only_connector_pid_and_is_bounded() {
        let argv = vec!["/bin/sh".into(), "-c".into(), "exec sleep 2".into()];
        let started = Instant::now();
        let target_pid = std::process::id();
        let out = run_pty(&argv, "/", 0.2, "never emitted", target_pid).unwrap();
        assert!(out.timed_out);
        assert_ne!(out.client_pid, target_pid);
        assert!(pid_alive(target_pid));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn pty_cancel_helper() {
        if std::env::var_os("EVERETT_TEST_PTY_CANCEL_CHILD").is_none() {
            return;
        }
        let pid = std::process::id();
        let signal = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            // SAFETY: target the isolated helper process, never the test runner.
            unsafe { libc::kill(pid as i32, libc::SIGINT) };
        });
        let out = run_pty(
            &["/bin/sh".into(), "-c".into(), "exec sleep 10".into()],
            "/",
            10.0,
            "never emitted",
            pid,
        )
        .unwrap();
        signal.join().unwrap();
        assert!(out.cancelled);
        assert!(pid_alive(pid));
    }

    #[test]
    fn pty_cancellation_cleans_connector_in_isolated_process() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "claude_wake::tests::pty_cancel_helper",
                "--nocapture",
            ])
            .env("EVERETT_TEST_PTY_CANCEL_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn nudge_uses_explicit_binary_and_only_generated_message_id() {
        let nudge = fixed_nudge("1f57ab77-c669-4bb6-9ac8-f9a9073de8ea", "m0123456789").unwrap();
        assert!(nudge.contains(" inbox --session 1f57ab77-c669-4bb6-9ac8-f9a9073de8ea --message m0123456789"));
        assert!(nudge.contains(" reply m0123456789"));
        assert!(nudge.contains("EVERETT_SESSION_ID=1f57ab77-c669-4bb6-9ac8-f9a9073de8ea EVERETT_HARNESS_NAME=claude "));
        assert!(!nudge.contains("raw request"));
        let relay_id = format!("relay-{}", "a".repeat(64));
        assert!(valid_message_id(&relay_id));
        assert!(fixed_nudge("1f57ab77-c669-4bb6-9ac8-f9a9073de8ea", &relay_id).unwrap().contains(&relay_id));
        assert!(!valid_message_id("relay-ABC"));
    }

    #[test]
    fn nudge_commands_propagate_quoted_profile_and_provider_without_changing_home() {
        let prefix = command_env_prefix("/tmp/Everett profile/'quoted'", "custom 'provider'");
        assert_eq!(
            prefix,
            "EVERETT_HOME='/tmp/Everett profile/'\\''quoted'\\''' EVERETT_AUTH_PROVIDER='custom '\\''provider'\\''' "
        );
        assert!(!prefix.contains(" HOME="));
        assert!(!prefix.contains("TOKEN"));
    }

    #[test]
    fn real_claude_home_lock_is_shared_across_everett_profiles() {
        let home = tempfile::tempdir().unwrap();
        let first = wake_lock_at(home.path(), "1f57ab77-c669-4bb6-9ac8-f9a9073de8ea").unwrap();
        assert!(wake_lock_at(home.path(), "1f57ab77-c669-4bb6-9ac8-f9a9073de8ea").is_err());
        drop(first);
    }
}
