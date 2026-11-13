use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::adapters::{external, grok as grok_adapter, hermes};
use crate::error::{EverettError, Result};
use crate::proc::{ps_commands, run_capture, shlex_join, which, RunOutput};
use crate::session::{file_mtime, now, Session};
use crate::{cards, inbox, paths};
use crate::execution::{child_env, HeadlessCommand};
use crate::host::{caller_identity, caller_session_id};

pub struct SendResult {
    pub command: Vec<String>,
    pub reply: String,
}

/// Build the non-interactive resume command for a supported harness.
pub fn command_for(session: &Session, text: &str) -> Result<Vec<String>> {
    if session.id.is_empty() {
        return Err(EverettError::new(2, "The selected session has no session id."));
    }
    if text.trim().is_empty() {
        return Err(EverettError::new(2, "The request must not be empty."));
    }
    if external::is_external(&session.harness) {
        return Err(EverettError::new(
            2,
            "Registered external sessions are inbox-only; use --mode inbox. \
             No provider CLI resume or wake-up is available.",
        ));
    }
    if session.source == "t3code" {
        return Err(EverettError::new(
            2,
            "T3 Code owns this thread's resume point, so a CLI resume would fork it. \
             Use --mode ping to send an inbox nudge through the T3 CLI, or --mode inbox \
             to queue without waking the thread.",
        ));
    }
    match session.harness.as_str() {
        "claude" => Ok(vec!["claude".into(), "--resume".into(), session.id.clone(), "--print".into(), text.into()]),
        "codex" => Ok(vec!["codex".into(), "exec".into(), "resume".into(), session.id.clone(), text.into()]),
        "omp" => Ok(vec!["omp".into(), "-r".into(), session.id.clone(), "-p".into(), text.into()]),
        "pi" => {
            if session.path.is_empty() {
                return Err(EverettError::new(2, "This Pi session has no session file to resume from."));
            }
            Ok(vec!["pi".into(), "--session".into(), session.path.clone(), "-p".into(), text.into()])
        }
        "hermes" => {
            if !session.source.is_empty() && !hermes::INTERACTIVE.contains(&session.source.as_str()) {
                return Err(EverettError::new(
                    2,
                    format!(
                        "Hermes {} sessions are list/route only: a CLI resume would not reach that \
                         chat. Talk to it on its own platform.",
                        session.source
                    ),
                ));
            }
            let profile = if session.profile.is_empty() { "default" } else { &session.profile };
            Ok(vec![
                "hermes".into(),
                "-p".into(),
                profile.into(),
                "chat".into(),
                "--resume".into(),
                session.id.clone(),
                "-Q".into(),
                "-q".into(),
                text.into(),
            ])
        }
        "grok" => Ok(vec!["grok".into(), "--resume".into(), session.id.clone(), "-p".into(), text.into()]),
        "devin" => Ok(vec!["devin".into(), "--resume".into(), session.id.clone(), "--print".into(), text.into()]),
        other => Err(EverettError::new(2, format!("Unsupported harness: {}", other))),
    }
}

pub const IDLE_QUIET: f64 = 60.0;
pub const IDLE_WAIT: f64 = 120.0;
pub const POLL: f64 = 5.0;

/// Busy = its id is in a live process, or its session file was written in the last IDLE_QUIET s.
pub fn is_busy(session: &Session, ps_out: &str, now: f64) -> bool {
    if !session.id.is_empty()
        && ps_out
            .lines()
            .any(|line| line.contains(&session.id) && crate::harness::binary_pattern().is_match(line))
    {
        return true;
    }
    let mtime = if session.harness == "hermes" || (session.harness == "devin" && session.path.ends_with(".db")) {
        session.last_active
    } else if session.harness == "grok" {
        let t = grok_adapter::activity(Path::new(&session.path));
        if t > 0.0 {
            t
        } else {
            session.last_active
        }
    } else {
        let t = file_mtime(Path::new(&session.path));
        if t > 0.0 {
            t
        } else {
            session.last_active
        }
    };
    now - mtime < IDLE_QUIET
}

pub fn wait_idle(session: &Session, wait: f64, poll: f64) -> bool {
    wait_idle_with(session, wait, poll, ps_commands, now, |d| {
        std::thread::sleep(std::time::Duration::from_secs_f64(d))
    })
}

pub fn wait_idle_with(
    session: &Session,
    wait: f64,
    poll: f64,
    ps: impl Fn() -> String,
    clock: impl Fn() -> f64,
    sleep: impl Fn(f64),
) -> bool {
    let deadline = clock() + wait;
    loop {
        if !is_busy(session, &ps(), clock()) {
            return true;
        }
        if clock() >= deadline {
            return false;
        }
        sleep(poll);
    }
}

pub fn format_command(command: &[String]) -> String {
    shlex_join(command)
}

pub fn require_harness(harness: &str, env: Option<&HashMap<String, String>>) -> Result<()> {
    let path = env
        .and_then(|e| e.get("PATH").cloned())
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_else(|| ":/bin:/usr/bin".to_string());
    if which(harness, &path).is_none() {
        return Err(EverettError::new(
            5,
            format!("`{}` is not on PATH. Install its CLI, then run `everett doctor`.", harness),
        ));
    }
    Ok(())
}

type Runner = dyn Fn(&[String], Option<&str>, &HashMap<String, String>, f64) -> std::result::Result<RunOutput, String>;

/// Wait for the session to go idle (up to `wait` s), resume it, capture its reply.
pub fn send(
    session: &Session,
    text: &str,
    timeout: f64,
    wait: f64,
    env: Option<&HashMap<String, String>>,
) -> Result<SendResult> {
    send_with(session, text, timeout, wait, env, &|cmd, cwd, env, t| run_capture(cmd, cwd, env, t))
}

pub fn send_with(
    session: &Session,
    text: &str,
    timeout: f64,
    wait: f64,
    env: Option<&HashMap<String, String>>,
    runner: &Runner,
) -> Result<SendResult> {
    send_with_for(session, text, timeout, wait, env, runner, None)
}

pub fn send_for(
    session: &Session,
    text: &str,
    timeout: f64,
    wait: f64,
    env: Option<&HashMap<String, String>>,
    caller: Option<&str>,
) -> Result<SendResult> {
    send_with_for(
        session,
        text,
        timeout,
        wait,
        env,
        &|cmd, cwd, env, t| run_capture(cmd, cwd, env, t),
        caller,
    )
}

fn send_with_for(
    session: &Session,
    text: &str,
    timeout: f64,
    wait: f64,
    env: Option<&HashMap<String, String>>,
    runner: &Runner,
    caller: Option<&str>,
) -> Result<SendResult> {
    require_local_prompt(caller)?;
    let command = command_for(session, text)?;
    if !timeout.is_finite() || timeout <= 0.0 {
        return Err(EverettError::new(2, "--timeout must be a finite number greater than zero."));
    }
    require_harness(&session.harness, env)?;
    if !wait_idle(session, wait, POLL) {
        return Err(EverettError::new(
            4,
            format!(
                "Target session stayed busy for {}s; nothing was sent. \
                 Use `everett route` for a manual resume command.",
                crate::fmt::g(wait)
            ),
        ));
    }
    let child = env.cloned().unwrap_or_else(|| child_env(&HashMap::new()));
    let result = runner(&command, Some(&session.cwd), &child, timeout)
        .map_err(|e| EverettError::new(5, format!("Could not start {}: {}", session.harness, e)))?;
    if result.timed_out {
        return Err(EverettError::new(
            5,
            format!("{} did not reply within {}s.", session.harness, crate::fmt::g(timeout)),
        ));
    }
    if result.code != 0 {
        let detail = result.stderr.trim().to_string();
        if !detail.is_empty() {
            let detail: String = detail.chars().rev().take(2000).collect::<String>().chars().rev().collect();
            let mut detail = detail;
            if session.harness == "codex"
                && (detail.to_lowercase().contains("interrupted system call")
                    || detail.to_lowercase().contains("os error 4"))
            {
                detail.push_str(
                    ". Delivery is unconfirmed; check the target session before retrying. \
                     For an open session with Everett hooks, use inbox delivery.",
                );
            }
            return Err(EverettError::new(
                6,
                format!("{} exited {}: {}", session.harness, result.code, detail),
            ));
        }
        return Err(EverettError::new(
            6,
            format!("{} exited with status {}.", session.harness, result.code),
        ));
    }
    Ok(SendResult { command, reply: result.stdout.trim().to_string() })
}


pub struct SpawnResult {
    pub command: Vec<String>,
    pub reply: String,
    pub session_id: String,
    pub harness: String,
    pub cwd: String,
}

/// Environment for forwarding, subject to the inbox hop policy.
pub fn hop_env() -> Result<HashMap<String, String>> {
    hop_env_for(None)
}

pub fn hop_env_for(caller: Option<&str>) -> Result<HashMap<String, String>> {
    let hops = caller_hops_for(caller);
    if let Some(limit) = inbox::hop_limit().filter(|l| hops >= *l) {
        return Err(EverettError::new(
            7,
            format!(
                "Hop limit reached ({}/{}): this request was already forwarded {} times between \
                 sessions. Answer it here instead of forwarding again.",
                hops, limit, hops
            ),
        ));
    }
    let mut extra = HashMap::new();
    extra.insert("EVERETT_HOPS".into(), (hops + 1).to_string());
    Ok(child_env(&extra))
}

pub fn current_hops() -> i64 {
    std::env::var("EVERETT_HOPS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0)
}

/// EVERETT_HOPS plus the highest hop take() delivered to the calling live session,
/// so a live session can't forward forever just because its env hop count is 0.
pub fn caller_hops() -> i64 {
    caller_hops_for(None)
}

pub fn caller_hops_for(caller: Option<&str>) -> i64 {
    let mut hops = current_hops();
    let sid = caller.map(str::to_string).unwrap_or_else(caller_session_id);
    if !sid.is_empty() {
        hops = hops.max(inbox::session_hops(&sid));
        hops = hops.max(
            inbox::relay_context(&sid)
                .as_ref()
                .and_then(|m| m.get("hops").and_then(Value::as_i64))
                .unwrap_or(0),
        );
    }
    hops
}

/// Never promote remote agent data into a harness's owner prompt, including
/// local intermediate deliveries and callers whose transient hop memory expired.
pub fn require_local_prompt(caller: Option<&str>) -> Result<()> {
    let sid = caller.map(str::to_string).unwrap_or_else(caller_session_id);
    if inbox::relay_context(&sid).is_some() {
        return Err(EverettError::new(2,
            "Remote-derived callers are inbox-only; resume/spawn would promote agent data to an owner prompt."));
    }
    Ok(())
}

pub fn guard_delivery(mode: &str, spawn: bool, caller: Option<&str>) -> Result<()> {
    if spawn || mode == "resume" {
        require_local_prompt(caller)?;
    }
    Ok(())
}

pub fn refuse_self(session: &Session, caller: Option<&str>) -> Result<()> {
    let caller = caller.map(|c| c.to_string()).unwrap_or_else(caller_session_id);
    if !caller.is_empty()
        && !session.id.is_empty()
        && (session.id == caller || session.id.starts_with(&caller) || caller.starts_with(&session.id))
    {
        return Err(EverettError::new(2, "Refusing to send to the calling session itself."));
    }
    Ok(())
}

/// Find the session a spawn just created: parse the harness output, else rescan its store.
fn new_session_id(harness: &str, cwd: &str, since: f64, output: &str) -> String {
    static RE1: OnceLock<Regex> = OnceLock::new();
    static RE2: OnceLock<Regex> = OnceLock::new();
    let re1 = RE1.get_or_init(|| Regex::new(r"session id:\s*([0-9a-fA-F-]{36})").unwrap());
    let re2 = RE2.get_or_init(|| Regex::new(r#""thread_id"\s*:\s*"([^"]+)""#).unwrap());
    if let Some(m) = re1.captures(output).or_else(|| re2.captures(output)) {
        return m[1].to_string();
    }
    let scan = crate::adapters::scan(harness, 1.0);
    let mut fresh: Vec<Session> = scan
        .into_iter()
        .filter(|s| {
            s.last_active >= since - 1.0
                && (cwd.is_empty()
                    || paths::realpath(Path::new(if s.cwd.is_empty() { "/" } else { &s.cwd }))
                        == paths::realpath(Path::new(cwd)))
        })
        .collect();
    fresh.sort_by(|a, b| b.last_active.partial_cmp(&a.last_active).unwrap_or(std::cmp::Ordering::Equal));
    fresh.first().map(|s| s.id.clone()).unwrap_or_default()
}

/// Remember sessions Everett started, so ls/route show them although they ran headless.
fn record_spawn(harness: &str, session_id: &str, cwd: &str, text: &str) {
    let path = crate::session::home().join(".everett").join("spawned.jsonl");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let text: String = text.chars().take(200).collect();
        let line = serde_json::to_string(&json!({
            "ts": now(),
            "harness": harness,
            "id": session_id,
            "cwd": cwd,
            "text": text,
        }))
        .unwrap();
        let _ = f.write_all((line + "\n").as_bytes());
    }
}

/// Start a NEW headless session in `cwd`, wait for its reply, return it with the new session id.
pub fn spawn(
    harness: &str,
    text: &str,
    cwd: &str,
    timeout: f64,
    env: Option<&HashMap<String, String>>,
) -> Result<SpawnResult> {
    spawn_for(harness, text, cwd, timeout, env, None)
}

pub fn spawn_for(
    harness: &str,
    text: &str,
    cwd: &str,
    timeout: f64,
    env: Option<&HashMap<String, String>>,
    caller: Option<&str>,
) -> Result<SpawnResult> {
    require_local_prompt(caller)?;
    if !Path::new(cwd).is_dir() {
        return Err(EverettError::new(2, format!("Directory does not exist: {}", cwd)));
    }
    if !timeout.is_finite() || timeout <= 0.0 {
        return Err(EverettError::new(2, "--timeout must be a finite number greater than zero."));
    }
    let mut session_id = if harness == "claude" || harness == "grok" {
        uuid::Uuid::new_v4().to_string()
    } else {
        String::new()
    };
    let out_file = std::env::temp_dir()
        .join(format!("everett-codex-{}.txt", uuid::Uuid::new_v4().simple()))
        .to_string_lossy().to_string();
    let prepared = HeadlessCommand::new(harness, text, &session_id, &out_file)?;
    require_harness(harness, env)?;
    let command = prepared.argv.clone();
    let started = now();
    let child = env.cloned().unwrap_or_else(|| child_env(&HashMap::new()));
    let result = run_capture(&command, Some(cwd), &child, timeout)
        .map_err(|e| EverettError::new(5, format!("Could not start {}: {}", harness, e)))?;
    let outcome = if result.timed_out {
        Err(EverettError::new(5, format!("{} did not reply within {}s.", harness, crate::fmt::g(timeout))))
    } else if result.code != 0 {
        let detail: String = result
            .stderr
            .trim()
            .chars()
            .rev()
            .take(2000)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if !detail.is_empty() {
            Err(EverettError::new(6, format!("{} exited {}: {}", harness, result.code, detail)))
        } else {
            Err(EverettError::new(6, format!("{} exited {}.", harness, result.code)))
        }
    } else {
        let reply = prepared.output().map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty()).unwrap_or_else(|| result.stdout.trim().to_string());
        Ok(reply)
    };
    drop(prepared);
    let reply = outcome?;
    if session_id.is_empty() {
        session_id = new_session_id(harness, cwd, started, &format!("{}{}", result.stdout, result.stderr));
    }
    if !session_id.is_empty() {
        record_spawn(harness, &session_id, cwd, text);
    }
    Ok(SpawnResult {
        command,
        reply,
        session_id,
        harness: harness.to_string(),
        cwd: cwd.to_string(),
    })
}


pub const MODES: &[&str] = &["ping", "auto", "resume", "inbox"];
pub const INBOX_HARNESSES: &[&str] = &["claude", "codex", "grok", "omp"];

/// A harness process is attached to this session right now (its TUI is open or it is mid-run).
pub fn attached(session: &Session, ps_out: Option<&str>) -> bool {
    if inbox::live(&session.id).is_some() {
        return true;
    }
    let ps = ps_out.map(|p| p.to_string()).unwrap_or_else(ps_commands);
    !session.id.is_empty()
        && ps
            .lines()
            .any(|line| line.contains(&session.id) && crate::harness::binary_pattern().is_match(line))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PingTransport {
    InboxOnly,
    T3,
    OmpExtension,
    Attached,
    Headless,
}

pub fn ping_transport(session: &Session, ps_out: Option<&str>) -> PingTransport {
    if external::is_external(&session.harness) {
        PingTransport::InboxOnly
    } else if session.source == "t3code" {
        PingTransport::T3
    } else if crate::omp_wake::available(session) {
        PingTransport::OmpExtension
    } else if attached(session, ps_out) {
        PingTransport::Attached
    } else {
        PingTransport::Headless
    }
}

impl PingTransport {
    pub fn name(self) -> &'static str {
        match self {
            Self::InboxOnly => "inbox",
            Self::T3 => "t3",
            Self::OmpExtension => "omp-extension",
            Self::Attached => "claude-bg",
            Self::Headless => "resume",
        }
    }
}

pub fn delivery_mode(session: &Session, mode: &str, ps_out: Option<&str>) -> Result<String> {
    delivery_mode_for(session, mode, ps_out, None)
}

pub fn delivery_mode_for(
    session: &Session,
    mode: &str,
    ps_out: Option<&str>,
    caller: Option<&str>,
) -> Result<String> {
    if !MODES.contains(&mode) {
        return Err(EverettError::new(2, format!("--mode must be one of {}.", MODES.join(", "))));
    }
    guard_delivery(mode, false, caller)?;
    let sid = caller.map(str::to_string).unwrap_or_else(caller_session_id);
    let remote_derived = inbox::relay_context(&sid).is_some();
    let transport = ping_transport(session, ps_out);
    if transport == PingTransport::T3 && matches!(mode, "ping" | "auto")
        && !remote_derived
    {
        require_local_prompt(caller)?;
        crate::t3_wake::validate_target(session)?;
        return Ok("ping".into());
    }
    if mode == "ping" {
        require_local_prompt(caller)?;
        if matches!(transport, PingTransport::InboxOnly | PingTransport::T3)
            || (session.harness != "claude" && transport == PingTransport::Attached) {
            return Err(EverettError::new(4, "No immediate ping transport for this session. Nothing was queued. Use --mode inbox only for legacy hooks/polling."));
        }
        return Ok("ping".into());
    }
    if external::is_external(&session.harness) && mode == "resume" {
        return Err(EverettError::new(
            2,
            "Registered external sessions are inbox-only; use --mode inbox. \
             No provider CLI resume or wake-up is available.",
        ));
    }
    if mode != "auto" {
        return Ok(if remote_derived { "inbox" } else { mode }.to_string());
    }
    if transport == PingTransport::OmpExtension && !remote_derived {
        return Ok("ping".into());
    }
    if remote_derived || transport != PingTransport::Headless {
        return Err(EverettError::new(
            4,
            format!(
                "Automatic delivery cannot ping or wake [{}] {}. Nothing was queued. \
                 Use --to {} --mode inbox (MCP to={:?}, mode=\"inbox\") to explicitly queue for hooks or polling. \
                 A local terminal user can use --wake for one exact verified Claude background session.",
                session.harness, session.id, session.id, session.id,
            ),
        ));
    }
    Ok("resume".to_string())
}

pub fn ping_as(
    session: &Session,
    text: &str,
    timeout: f64,
    caller: Option<&str>,
    caller_harness: Option<&str>,
) -> Result<Map<String, Value>> {
    require_local_prompt(caller)?;
    refuse_self(session, caller)?;
    if text.trim().is_empty() || text.len() > crate::peer::BODY_CAP || !timeout.is_finite() || timeout <= 0.0 {
        return Err(EverettError::new(2, "Ping requires non-empty text within 64 KiB and a positive finite timeout"));
    }
    delivery_mode_for(session, "ping", None, caller)?;
    let transport = ping_transport(session, None);
    if transport == PingTransport::T3 {
        let thread = crate::t3_wake::validate_target(session)?;
        let mut out = send_inbox_as(session, text, 0.0, caller, caller_harness, 1.0, None)?;
        let mid = out.get("message_id").and_then(Value::as_str).unwrap_or("");
        let receipt = crate::t3_wake::wake(&thread, session, mid).map_err(|error| {
            EverettError::new(4, format!(
                "Immediate T3 ping not confirmed: {}. Inbox message {mid} remains recorded and may have been read. Do not blindly retry.",
                error.message,
            ))
        })?;
        out.insert("wake".into(), json!({"state":"prompt_sent","transport":"t3","thread_id":thread,"receipt":receipt}));
        out.insert("mode".into(), json!("ping"));
        out.insert("pinged".into(), json!(true));
        out.insert("queued".into(), json!(true));
        out.remove("pickup");
        out.remove("poll_session_id");
        out.remove("pickup_note");
        out.insert("note".into(), json!("T3 accepted the inbox nudge for a new or queued turn. The durable inbox is retained; consumption and reply are not confirmed."));
        return Ok(out);
    }
    if transport == PingTransport::OmpExtension {
        if caller_harness.is_some_and(external::is_external) {
            return Err(EverettError::new(2, "External callers are inbox-only."));
        }
        let mut out = send_inbox_as(session, text, 0.0, caller, caller_harness, 1.0, None)?;
        out.insert("mode".into(), json!("ping"));
        out.insert("transport".into(), json!("omp-extension"));
        out.insert("pinged".into(), json!(false));
        out.insert("wake".into(), json!({"state":"extension_pending"}));
        out.insert("note".into(), json!("Accepted by the live OMP watcher transport. Nudge, consumption, and reply are not confirmed."));
        return Ok(out);
    }
    if transport == PingTransport::Attached {
        crate::claude_wake::validate_target(session).map_err(|error| EverettError::new(4,
            format!("Ping unavailable for {}: {}. Nothing was queued. Use --mode inbox for legacy delivery.", session.id, error.message)))?;
        let mut out = send_inbox_as(session, text, 0.0, caller, caller_harness, 1.0, None)?;
        let mid = out.get("message_id").and_then(Value::as_str).unwrap_or("");
        let prepared = crate::claude_wake::prepare(session).map_err(|error| EverettError::new(4,
            format!("Immediate ping unavailable for {}: {}. Inbox message {mid} remains recorded; no native nudge was sent. Do not blindly retry.", session.id, error.message)))?;
        match crate::claude_wake::wake(&prepared, mid) {
            crate::claude_wake::WakeOutcome::PromptSent { client_pid, target_pid } => {
                out.insert("wake".into(), json!({"state":"prompt_sent","client_pid":client_pid,"target_pid":target_pid}));
                out.insert("mode".into(), json!("ping"));
                out.insert("pinged".into(), json!(true));
                out.insert("note".into(), json!("Immediate native nudge sent. The durable inbox is retained; consumption and reply are not confirmed."));
                Ok(out)
            }
            outcome => Err(EverettError::new(
                if matches!(outcome, crate::claude_wake::WakeOutcome::Cancelled { .. }) { 130 } else { 4 },
                format!("Immediate ping not confirmed ({outcome:?}); inbox message {mid} remains recorded and may have been read. Do not blindly retry."),
            )),
        }
    } else {
        let env = hop_env_for(caller)?;
        let result = send_for(session, text, timeout, 0.0, Some(&env), caller)?;
        Ok(serde_json::from_value(json!({"mode":"ping","pinged":true,"queued":false,
            "delivered":true,"command":result.command,"reply":result.reply})).unwrap())
    }
}

/// Who is sending: the calling session (with its card), else the human at a terminal.
pub fn sender_info(caller: Option<&str>) -> Map<String, Value> {
    sender_info_as(caller, None)
}

pub fn sender_info_as(caller: Option<&str>, caller_harness: Option<&str>) -> Map<String, Value> {
    let ident = caller_identity();
    let sid = caller
        .map(|c| c.to_string())
        .unwrap_or_else(|| ident.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string());
    let card = if !sid.is_empty() {
        cards::read_card(&sid).map(|(body, _)| body).unwrap_or_default()
    } else {
        String::new()
    };
    let mut out = Map::new();
    out.insert("sender".into(), json!(if sid.is_empty() { inbox::HUMAN.to_string() } else { sid.clone() }));
    out.insert(
        "from_harness".into(),
        json!(if sid.is_empty() {
            String::new()
        } else {
            caller_harness
                .filter(|h| !h.is_empty())
                .or_else(|| ident.get("harness").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string()
        }),
    );
    out.insert("from_card".into(), json!(card));
    out
}

/// Queue `text` in a running session's inbox; optionally wait for its reply.
pub fn send_inbox(
    session: &Session,
    text: &str,
    wait: f64,
    caller: Option<&str>,
    poll: f64,
    key: Option<&str>,
) -> Result<Map<String, Value>> {
    send_inbox_as(session, text, wait, caller, None, poll, key)
}

pub fn send_inbox_as(
    session: &Session,
    text: &str,
    wait: f64,
    caller: Option<&str>,
    caller_harness: Option<&str>,
    poll: f64,
    key: Option<&str>,
) -> Result<Map<String, Value>> {
    if !wait.is_finite() || wait < 0.0 {
        return Err(EverettError::new(2, "--wait must be a finite number of seconds, 0 or more."));
    }
    if external::is_external(&session.harness) {
        if let Some(target) = external::device_target(&session.id) {
            let out = crate::peer::send_as(&target, text, key, caller, caller_harness)?;
            let reply_inbox = sender_info_as(caller, caller_harness)
                .get("sender")
                .and_then(|v| v.as_str())
                .unwrap_or(inbox::HUMAN)
                .to_string();
            let mut result = Map::new();
            result.insert("mode".into(), json!("inbox"));
            result.insert("queued".into(), json!(true));
            result.insert("delivered".into(), json!(false));
            result.insert("external".into(), json!(true));
            result.insert("hooked".into(), json!(false));
            result.insert("wake".into(), json!(false));
            result.insert("transport".into(), json!("relay"));
            result.insert("peer".into(), out.get("peer").cloned().unwrap_or(Value::Null));
            result.insert("relay_state".into(), out.get("state").cloned().unwrap_or(json!("queued")));
            result.insert("message_id".into(), out.get("message_id").cloned().unwrap_or(Value::Null));
            result.insert("idempotency_key".into(), out.get("idempotency_key").cloned().unwrap_or(Value::Null));
            result.insert("reply_inbox".into(), json!(reply_inbox));
            result.insert("reply".into(), Value::Null);
            result.insert("pickup_note".into(), json!(
                "Queued on the relay; the remote device picks it up and replies arrive via the relay listener. Poll: everett peer status <peer> <message id>."
            ));
            return Ok(result);
        }
    }
    let who = sender_info_as(caller, caller_harness);
    let sender = who.get("sender").and_then(|v| v.as_str()).unwrap_or(inbox::HUMAN);
    let hops = caller_hops_for(Some(sender));
    if let Some(limit) = inbox::hop_limit().filter(|l| hops >= *l) {
        return Err(EverettError::new(
            7,
            format!("Hop limit reached ({}/{}): answer here instead of forwarding again.", hops, limit),
        ));
    }
    let message = inbox::post(
        &session.id,
        text,
        sender,
        "message",
        "",
        hops + 1,
        who.get("from_harness").and_then(|v| v.as_str()).unwrap_or(""),
        who.get("from_card").and_then(|v| v.as_str()).unwrap_or(""),
        inbox::relay_context(sender)
            .as_ref()
            .and_then(inbox::forward_provenance),
    )?;
    let webhook = if external::is_external(&session.harness) {
        match crate::receiver_webhook::after_local_queue(&session.id, &session.harness, &message) {
            Ok(value) => value,
            Err(error) => Some(json!({"state": "error", "reason": error.message})),
        }
    } else {
        None
    };
    let mut result = Map::new();
    result.insert("mode".into(), json!("inbox"));
    result.insert("message_id".into(), message.get("id").cloned().unwrap_or(Value::Null));
    result.insert("reply_inbox".into(), json!(sender));
    result.insert("reply".into(), Value::Null);
    result.insert("hooked".into(), json!(INBOX_HARNESSES.contains(&session.harness.as_str())));
    result.insert("wake".into(), json!(false));
    if let Some(webhook) = webhook {
        result.insert("webhook".into(), webhook);
    }
    if session.source == "t3code" {
        result.insert("hooked".into(), json!(false));
        result.insert("pickup".into(), json!("poll"));
        result.insert("poll_session_id".into(), json!(session.id));
        result.insert("pickup_note".into(), json!(
            "Queued only for this explicit inbox send; no T3 turn was requested. Use mode=ping to send an inbox nudge through the T3 CLI. For manual pickup, bind EVERETT_SESSION_ID to this provider session in the MCP launch environment, call everett_inbox, and reply with everett_send(reply_to=<message id>, text=...)."
        ));
    }
    if wait > 0.0 {
        let mid = message.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(reply) = inbox::wait_reply(sender, mid, wait, poll) {
            result.insert("reply".into(), json!(inbox::consumption_text(&reply)));
            if inbox::external(&reply) {
                result.insert("external".into(), json!(true));
                result.insert("untrusted".into(), json!(true));
                result.insert("trust".into(), json!("external-untrusted"));
            }
            result.insert(
                "reply_from".into(),
                reply.get("from").cloned().unwrap_or(Value::Null),
            );
        }
    }
    let picked_up = inbox::done_ids(&session.id)
        .contains(message.get("id").and_then(Value::as_str).unwrap_or(""));
    result.insert("delivered".into(), json!(picked_up));
    result.insert("queued".into(), json!(!picked_up));
    if picked_up {
        result.remove("pickup_note");
    }
    Ok(result)
}

/// Answer an inbox message: the reply goes to the original sender's inbox.
/// A session caller resolves the message inside its own inbox only, so a
/// colliding id in another inbox can never reroute the reply.
pub fn reply(message_id: &str, text: &str, caller: Option<&str>) -> Result<Map<String, Value>> {
    reply_as(message_id, text, caller, None)
}

pub fn reply_as(
    message_id: &str,
    text: &str,
    caller: Option<&str>,
    caller_harness: Option<&str>,
) -> Result<Map<String, Value>> {
    let caller_id = caller.map(str::trim).unwrap_or("");
    let message_id = message_id.trim();
    let original = if caller_id.is_empty() {
        inbox::find(message_id)
    } else {
        inbox::find_in(caller_id, message_id)
    };
    let Some(original) = original else {
        return Err(EverettError::new(
            2,
            if caller_id.is_empty() {
                format!("No message with id {:?} in any Everett inbox.", message_id)
            } else {
                format!(
                    "No message with id {:?} addressed to this session in its Everett inbox.",
                    message_id
                )
            },
        ));
    };
    let who = sender_info_as(caller, caller_harness);
    let env_hops = caller_hops_for(Some(who["sender"].as_str().unwrap_or(inbox::HUMAN)));
    if original.contains_key("relay_device") {
        let hops = inbox::remote_reply_hops(&original, env_hops)?;
        return crate::relay::retain_reply(&original, text, &who, hops);
    }
    if original.contains_key("peer_client") {
        let hops = inbox::remote_reply_hops(&original, env_hops)?;
        return crate::peer::retain_reply(&original, text, &who, hops);
    }
    let context = inbox::relay_context(who["sender"].as_str().unwrap_or(inbox::HUMAN));
    let (mut hops, depth) = inbox::local_reply_hops(&original, env_hops)?;
    if context.as_ref().is_some_and(inbox::external) {
        // Remote-derived answers get no free hop: untrusted lineage keeps spending the budget.
        hops = inbox::reply_hops(&original, env_hops)?;
    }
    let to = original.get("from").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or(inbox::HUMAN);
    let mut provenance = inbox::forward_provenance(&original)
        .or_else(|| context.as_ref().and_then(inbox::forward_provenance));
    if context.as_ref().is_some_and(inbox::external) {
        provenance.get_or_insert_with(Map::new).extend(serde_json::from_value::<Map<String,Value>>(json!({"external":true,"untrusted":true,"trust":"external-untrusted","instruction_source":"agent-data"})).unwrap());
    }
    provenance.get_or_insert_with(Map::new).insert("reply_depth".into(), json!(depth));
    let message = inbox::post(
        to,
        text,
        who.get("sender").and_then(|v| v.as_str()).unwrap_or(""),
        "reply",
        original.get("id").and_then(|v| v.as_str()).unwrap_or(""),
        hops,
        who.get("from_harness").and_then(|v| v.as_str()).unwrap_or(""),
        who.get("from_card").and_then(|v| v.as_str()).unwrap_or(""),
        provenance,
    )?;
    if !caller_id.is_empty() {
        inbox::mark_done(caller_id, &[message_id]);
    }
    let mut out = Map::new();
    out.insert("mode".into(), json!("reply"));
    out.insert("message_id".into(), message.get("id").cloned().unwrap_or(Value::Null));
    out.insert("reply_to".into(), original.get("id").cloned().unwrap_or(Value::Null));
    out.insert("to".into(), message.get("to").cloned().unwrap_or(Value::Null));
    Ok(out)
}
