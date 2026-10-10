use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::adapters::{external, grok as grok_adapter, hermes};
use crate::error::{EverettError, Result};
use crate::proc::{ps_commands, run_capture, shlex_join, which, RunOutput};
use crate::session::{file_mtime, now, Session};
use crate::{cards, inbox, paths, registry};

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
             Continue it in T3 Code, or queue inbox delivery and explicitly poll everett_inbox \
             with the provider session_id.",
        ));
    }
    match session.harness.as_str() {
        "claude" => Ok(vec!["claude".into(), "--resume".into(), session.id.clone(), "--print".into(), text.into()]),
        "codex" => Ok(vec!["codex".into(), "exec".into(), "resume".into(), session.id.clone(), text.into()]),
        "omp" => Ok(vec!["omp".into(), "-r".into(), session.id.clone(), "-p".into(), text.into()]),
        "pi" => Ok(vec!["pi".into(), "--session".into(), session.path.clone(), "-p".into(), text.into()]),
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

fn harness_bin() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(^|/)(claude|codex|omp|pi|hermes|grok|devin)(\s|$)").unwrap())
}

pub const IDLE_QUIET: f64 = 60.0;
pub const IDLE_WAIT: f64 = 120.0;
pub const POLL: f64 = 5.0;

/// Busy = its id is in a live process, or its session file was written in the last IDLE_QUIET s.
pub fn is_busy(session: &Session, ps_out: &str, now: f64) -> bool {
    if !session.id.is_empty()
        && ps_out
            .lines()
            .any(|line| line.contains(&session.id) && harness_bin().is_match(line))
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
    wait_idle_with(session, wait, poll, ps_commands, || now(), |d| {
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

/// Environment for a headless harness run: card hooks stay quiet, hop count carries over.
pub fn child_env(extra: &HashMap<String, String>) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("EVERETT_SEND".into(), "1".into());
    for (k, v) in extra {
        env.insert(k.clone(), v.clone());
    }
    env
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

// ---- spawning a NEW session ----

pub const SPAWNABLE: &[&str] = &["claude", "codex", "omp", "pi", "hermes", "grok", "devin"];

pub struct SpawnResult {
    pub command: Vec<String>,
    pub reply: String,
    pub session_id: String,
    pub harness: String,
    pub cwd: String,
}

/// Headless command that starts a NEW session with this request.
pub fn spawn_command(harness: &str, text: &str, session_id: &str, out_file: &str) -> Result<Vec<String>> {
    if external::is_external(harness) {
        return Err(EverettError::new(
            2,
            "External agents must be registered explicitly and are inbox-only; Everett cannot spawn them.",
        ));
    }
    if text.trim().is_empty() {
        return Err(EverettError::new(2, "The request must not be empty."));
    }
    match harness {
        "claude" => Ok(if session_id.is_empty() {
            vec!["claude".into(), "--print".into(), text.into()]
        } else {
            vec!["claude".into(), "--session-id".into(), session_id.into(), "--print".into(), text.into()]
        }),
        "codex" => {
            let mut cmd = vec!["codex".into(), "exec".into(), "--skip-git-repo-check".into()];
            if !out_file.is_empty() {
                cmd.push("-o".into());
                cmd.push(out_file.into());
            }
            cmd.push(text.into());
            Ok(cmd)
        }
        "omp" => Ok(vec!["omp".into(), "-p".into(), text.into()]),
        "pi" => Ok(vec!["pi".into(), "-p".into(), text.into()]),
        "hermes" => Ok(vec!["hermes".into(), "chat".into(), "-Q".into(), "-q".into(), text.into()]),
        "grok" => Ok(if session_id.is_empty() {
            vec!["grok".into(), "-p".into(), text.into()]
        } else {
            vec!["grok".into(), "--session-id".into(), session_id.into(), "-p".into(), text.into()]
        }),
        "devin" => Ok(vec!["devin".into(), "-p".into(), text.into()]),
        other => Err(EverettError::new(
            2,
            format!("Cannot spawn harness {:?}; choose one of {}.", other, SPAWNABLE.join(", ")),
        )),
    }
}

pub const MAX_HOPS: i64 = 3;

/// Env for a delivery: EVERETT_HOPS counts agent-to-agent forwards; refuse past MAX_HOPS.
pub fn hop_env() -> Result<HashMap<String, String>> {
    let hops = caller_hops();
    if hops >= MAX_HOPS {
        return Err(EverettError::new(
            7,
            format!(
                "Hop limit reached ({}/{}): this request was already forwarded {} times between \
                 sessions. Answer it here instead of forwarding again.",
                hops, MAX_HOPS, hops
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
    let mut hops = current_hops();
    let sid = caller_identity()
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if !sid.is_empty() {
        hops = hops.max(inbox::session_hops(&sid));
    }
    hops
}

const IDENTITY_ENV: &[(&str, &str)] = &[
    ("EVERETT_SESSION_ID", ""),
    ("CLAUDE_CODE_SESSION_ID", "claude"),
    ("CLAUDE_SESSION_ID", "claude"),
    ("CODEX_THREAD_ID", "codex"),
    ("HERMES_SESSION_ID", "hermes"),
    ("PI_SESSION_FILE", "pi"),
    ("GROK_SESSION_ID", "grok"),
];

/// {session_id, harness, source} of the session running this process, if detectable.
pub fn caller_identity() -> Map<String, Value> {
    for (name, harness) in IDENTITY_ENV {
        let Ok(value) = std::env::var(name) else { continue };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let id = if *name == "PI_SESSION_FILE" {
            Path::new(value)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .and_then(|stem| stem.rsplit('_').next().map(|s| s.to_string()))
                .unwrap_or_default()
        } else {
            value.to_string()
        };
        let harness = if harness.is_empty() {
            std::env::var("EVERETT_HARNESS_NAME").unwrap_or_default()
        } else {
            harness.to_string()
        };
        let mut out = Map::new();
        out.insert("session_id".into(), json!(id));
        out.insert("harness".into(), json!(harness));
        out.insert("source".into(), json!(format!("env {}", name)));
        return out;
    }
    let mut out = Map::new();
    out.insert("session_id".into(), json!(""));
    out.insert("harness".into(), json!(""));
    out.insert("source".into(), json!("none"));
    out
}

pub fn caller_session_id() -> String {
    caller_identity()
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
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
    if !registry::ADAPTERS.contains(&harness) {
        return String::new();
    }
    let scan = match harness {
        "claude" => crate::adapters::claude::scan(1.0),
        "codex" => crate::adapters::codex::scan(1.0),
        "omp" => crate::adapters::omp::scan(1.0),
        "pi" => crate::adapters::pi::scan(1.0),
        "hermes" => crate::adapters::hermes::scan(1.0),
        "grok" => crate::adapters::grok::scan(1.0),
        "devin" => crate::adapters::devin::scan(1.0),
        _ => Vec::new(),
    };
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
    let mut command = spawn_command(harness, text, &session_id, "")?;
    require_harness(harness, env)?;
    let out_file = if harness == "codex" {
        std::env::temp_dir()
            .join(format!("everett-codex-{}.txt", uuid::Uuid::new_v4().simple()))
            .to_string_lossy()
            .to_string()
    } else {
        String::new()
    };
    if !out_file.is_empty() {
        command = spawn_command(harness, text, &session_id, &out_file)?;
    }
    let started = now();
    let child = env.cloned().unwrap_or_else(|| child_env(&HashMap::new()));
    let result = run_capture(&command, Some(cwd), &child, timeout)
        .map_err(|e| EverettError::new(5, format!("Could not start {}: {}", harness, e)));
    let result = match result {
        Ok(r) => r,
        Err(e) => {
            if !out_file.is_empty() {
                let _ = std::fs::remove_file(&out_file);
            }
            return Err(e);
        }
    };
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
        let reply = if !out_file.is_empty() {
            std::fs::read_to_string(&out_file)
                .ok()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| result.stdout.trim().to_string())
        } else {
            result.stdout.trim().to_string()
        };
        Ok(reply)
    };
    if !out_file.is_empty() {
        let _ = std::fs::remove_file(&out_file);
    }
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

// ---- live delivery: running sessions get an inbox message instead of a resume ----

pub const MODES: &[&str] = &["auto", "resume", "inbox"];
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
            .any(|line| line.contains(&session.id) && harness_bin().is_match(line))
}

pub fn delivery_mode(session: &Session, mode: &str, ps_out: Option<&str>) -> Result<String> {
    if !MODES.contains(&mode) {
        return Err(EverettError::new(2, format!("--mode must be one of {}.", MODES.join(", "))));
    }
    if external::is_external(&session.harness) {
        if mode == "resume" {
            return Err(EverettError::new(
                2,
                "Registered external sessions are inbox-only; use --mode inbox. \
                 No provider CLI resume or wake-up is available.",
            ));
        }
        return Ok("inbox".to_string());
    }
    if mode != "auto" {
        return Ok(mode.to_string());
    }
    if session.source == "t3code" {
        return Ok("inbox".to_string());
    }
    Ok(if attached(session, ps_out) { "inbox" } else { "resume" }.to_string())
}

/// Who is sending: the calling session (with its card), else the human at a terminal.
pub fn sender_info(caller: Option<&str>) -> Map<String, Value> {
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
            ident.get("harness").and_then(|v| v.as_str()).unwrap_or("").to_string()
        }),
    );
    out.insert("from_card".into(), json!(card));
    out
}

/// Queue `text` in a running session's inbox; optionally wait for its reply.
pub fn send_inbox(session: &Session, text: &str, wait: f64, caller: Option<&str>, poll: f64) -> Result<Map<String, Value>> {
    if !wait.is_finite() || wait < 0.0 {
        return Err(EverettError::new(2, "--wait must be a finite number of seconds, 0 or more."));
    }
    let who = sender_info(caller);
    let sender = who.get("sender").and_then(|v| v.as_str()).unwrap_or(inbox::HUMAN);
    let mut hops = current_hops();
    if sender != inbox::HUMAN {
        hops = hops.max(inbox::session_hops(sender));
    }
    if hops >= MAX_HOPS {
        return Err(EverettError::new(
            7,
            format!("Hop limit reached ({}/{}): answer here instead of forwarding again.", hops, MAX_HOPS),
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
        None,
    )?;
    let mut result = Map::new();
    result.insert("mode".into(), json!("inbox"));
    result.insert("message_id".into(), message.get("id").cloned().unwrap_or(Value::Null));
    result.insert("reply_inbox".into(), json!(sender));
    result.insert("reply".into(), Value::Null);
    result.insert("hooked".into(), json!(INBOX_HARNESSES.contains(&session.harness.as_str())));
    if external::is_external(&session.harness) {
        result.insert("queued".into(), json!(true));
    }
    if session.source == "t3code" {
        result.insert("hooked".into(), json!(false));
        result.insert("pickup".into(), json!("poll"));
        result.insert("poll_session_id".into(), json!(session.id));
        result.insert("pickup_note".into(), json!(
            "Queued only. In T3 Code, call everett_inbox with this provider session_id; reply with everett_send(reply_to=<message id>, text=..., session_id=...)."
        ));
    }
    if wait > 0.0 {
        let mid = message.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(reply) = inbox::wait_reply(sender, mid, wait, poll) {
            result.insert("reply".into(), reply.get("text").cloned().unwrap_or(Value::Null));
            result.insert("reply_from".into(), reply.get("from").cloned().unwrap_or(Value::Null));
        }
    }
    Ok(result)
}

/// Answer an inbox message: the reply goes to the original sender's inbox.
/// A session caller resolves the message inside its own inbox only, so a
/// colliding id in another inbox can never reroute the reply.
pub fn reply(message_id: &str, text: &str, caller: Option<&str>) -> Result<Map<String, Value>> {
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
    let hops = inbox::reply_hops(&original, current_hops())?;
    let who = sender_info(caller);
    let to = original.get("from").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or(inbox::HUMAN);
    let message = inbox::post(
        to,
        text,
        who.get("sender").and_then(|v| v.as_str()).unwrap_or(""),
        "reply",
        original.get("id").and_then(|v| v.as_str()).unwrap_or(""),
        hops,
        who.get("from_harness").and_then(|v| v.as_str()).unwrap_or(""),
        who.get("from_card").and_then(|v| v.as_str()).unwrap_or(""),
        None,
    )?;
    let mut out = Map::new();
    out.insert("mode".into(), json!("reply"));
    out.insert("message_id".into(), message.get("id").cloned().unwrap_or(Value::Null));
    out.insert("reply_to".into(), original.get("id").cloned().unwrap_or(Value::Null));
    out.insert("to".into(), message.get("to").cloned().unwrap_or(Value::Null));
    Ok(out)
}
