use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

#[derive(Debug)]
pub struct RunOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// `subprocess.run(capture_output=True, stdin=DEVNULL, timeout=)` — drains both pipes on
/// reader threads so a chatty child can't deadlock on a full pipe.
pub fn run_capture(
    command: &[String],
    cwd: Option<&str>,
    env: &HashMap<String, String>,
    timeout: f64,
) -> Result<RunOutput, String> {
    run_inner(command, cwd, env, timeout, None)
}

/// Same, but feeds `input` to the child's stdin (`subprocess.run(input=...)`).
pub fn run_capture_stdin(
    command: &[String],
    cwd: Option<&str>,
    env: &HashMap<String, String>,
    timeout: f64,
    input: Option<&str>,
) -> Result<RunOutput, String> {
    run_inner(command, cwd, env, timeout, input)
}

fn run_inner(
    command: &[String],
    cwd: Option<&str>,
    env: &HashMap<String, String>,
    timeout: f64,
    input: Option<&str>,
) -> Result<RunOutput, String> {
    if command.is_empty() {
        return Err("empty command".to_string());
    }
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..])
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .envs(env);
    if let Some(cwd) = cwd {
        if !cwd.is_empty() {
            cmd.current_dir(cwd);
        }
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    if let (Some(mut stdin), Some(text)) = (child.stdin.take(), input) {
        use std::io::Write;
        let _ = stdin.write_all(text.as_bytes());
    }
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let timed_out;
    let code;
    match child.wait_timeout(Duration::from_secs_f64(timeout.max(0.001))) {
        Ok(Some(status)) => {
            timed_out = false;
            code = status.code().unwrap_or(-1);
        }
        Ok(None) => {
            timed_out = true;
            code = -1;
            let _ = child.kill();
            let _ = child.wait();
        }
        Err(e) => return Err(e.to_string()),
    }
    let out = t_out.join().unwrap_or_default();
    let err = t_err.join().unwrap_or_default();
    Ok(RunOutput { code, stdout: out, stderr: err, timed_out })
}

/// `shutil.which(bin, path=...)`.
pub fn which(bin: &str, path_env: &str) -> Option<String> {
    for dir in path_env.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let candidate = format!("{}/{}", dir.trim_end_matches('/'), bin);
        if std::fs::metadata(&candidate).map(|m| m.is_file()).unwrap_or(false)
            && is_executable(&candidate)
        {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

const SHLEX_SAFE: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789@%_+=:,./-";

/// `shlex.quote`.
pub fn shlex_quote(arg: &str) -> String {
    if !arg.is_empty() && arg.chars().all(|c| SHLEX_SAFE.contains(c)) {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "'\"'\"'"))
}

/// `shlex.join`.
pub fn shlex_join(command: &[String]) -> String {
    command.iter().map(|a| shlex_quote(a)).collect::<Vec<_>>().join(" ")
}


/// `ps -axo command` output (empty on failure).
pub fn ps_commands() -> String {
    let Ok(mut child) = Command::new("ps")
        .args(["-axo", "command"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
    else {
        return String::new();
    };
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    let _ = child.wait();
    out
}

/// `os.kill(pid, 0)`: true when the process exists (EPERM still means alive).
pub fn pid_alive(pid: i32) -> bool {
    if pid <= 1 {
        return false;
    }
    unsafe { libc::kill(pid, 0) == 0 || *libc::__error() == libc::EPERM }
}

/// Parent pid and comm of a process, via `ps -o ppid=,comm= -p`.
pub fn parent_of(pid: i32) -> Option<(i32, String)> {
    let out = Command::new("ps")
        .args(["-o", "ppid=,comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.trim();
    let (ppid, comm) = line.split_once(char::is_whitespace)?;
    Some((ppid.trim().parse().ok()?, comm.trim().to_string()))
}
