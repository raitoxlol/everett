//! Owner-local control over agent sessions: list, start, send, read and stop.
//!
//! T3 Code threads are driven through the `t3` CLI (`t3 thread list|start|send|show`).
//! Other harnesses reuse Everett's existing headless spawn and read their own
//! transcript files. Every action refuses remote-derived callers, because sending
//! or starting promotes text to an owner prompt and reading discloses transcripts.
//! `stop` is reported per harness; no harness exposes a safe stop to Everett yet.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use crate::error::{EverettError, Result};
use crate::proc::run_capture;
use crate::session::Session;

/// Upper bound for a `--wait` T3 turn or a headless start.
pub const MAX_WAIT: f64 = 1800.0;
const QUICK: f64 = 30.0;
const MAX_MESSAGES: usize = 50;

fn t3_bin() -> Result<String> {
    crate::t3_wake::binary().ok_or_else(|| {
        EverettError::new(4, "T3 Code CLI (t3) is unavailable. Set EVERETT_T3_BIN or install t3 on PATH or in ~/.t3/bin.")
    })
}

/// Run `t3 <args> --json` and parse its stdout. T3 writes diagnostics to stderr.
fn t3_json(args: &[&str], timeout: f64) -> Result<Value> {
    let mut argv = vec![t3_bin()?];
    argv.extend(args.iter().map(|a| a.to_string()));
    let env: HashMap<String, String> = std::env::vars().collect();
    let out = run_capture(&argv, None, &env, timeout)
        .map_err(|e| EverettError::new(4, format!("t3 failed to start: {e}")))?;
    if out.timed_out {
        return Err(EverettError::new(4, format!("t3 {} timed out after {timeout}s", args.join(" "))));
    }
    let stdout = out.stdout.trim();
    if out.code != 0 && out.code != 2 {
        let detail = if stdout.is_empty() { out.stderr.trim() } else { stdout };
        let detail: String = detail.chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(EverettError::new(4, format!("t3 {} exited {}: {}", args.first().unwrap_or(&""), out.code, detail)));
    }
    let mut value = parse_json_output(stdout).unwrap_or_else(|| json!({"output": stdout}));
    if out.code == 2 {
        if let Some(map) = value.as_object_mut() {
            map.insert("waiting_for_answer".into(), json!(true));
        }
    }
    Ok(value)
}

/// The JSON document in `t3` stdout. Some builds print log lines (such as a
/// database migration warning) to stdout first, so take the first line that
/// starts a JSON value at column 0 and parses through the end.
fn parse_json_output(stdout: &str) -> Option<Value> {
    let mut offset = 0;
    for line in stdout.split_inclusive('\n') {
        if line.starts_with('{') || line.starts_with('[') {
            if let Ok(v) = serde_json::from_str(stdout[offset..].trim()) {
                return Some(v);
            }
        }
        offset += line.len();
    }
    None
}

fn clamp_timeout(timeout: Option<f64>, wait: bool) -> Result<f64> {
    let t = timeout.unwrap_or(if wait { 600.0 } else { QUICK });
    if !t.is_finite() || t <= 0.0 {
        return Err(EverettError::new(2, "timeout must be a finite number greater than zero."));
    }
    Ok(t.min(MAX_WAIT))
}

/// An Everett session id or prefix, resolved over the last week of sessions.
pub fn resolve_session(target: &str) -> Result<Session> {
    let sessions = crate::registry::scan(168.0, true, None, "");
    crate::registry::find(target, &sessions)
}

/// The T3 thread for a control target: an explicit thread id, or a session T3 drives.
fn thread_for(thread: &str, session: Option<&Session>) -> Option<String> {
    if !thread.is_empty() {
        return Some(thread.to_string());
    }
    session.and_then(crate::t3_wake::thread_id)
}

pub fn threads(project: &str) -> Result<Value> {
    let mut args = vec!["thread", "list", "--json"];
    if !project.is_empty() {
        args.extend(["--project", project]);
    }
    t3_json(&args, QUICK)
}

pub struct StartRequest<'a> {
    pub text: &'a str,
    pub harness: &'a str,
    pub project: &'a str,
    pub cwd: &'a str,
    pub title: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub worktree: &'a str,
    pub branch: &'a str,
    pub wait: bool,
    pub timeout: Option<f64>,
}

pub fn start(req: &StartRequest, caller: Option<&str>) -> Result<Value> {
    crate::send::require_local_prompt(caller)?;
    if req.text.trim().is_empty() {
        return Err(EverettError::new(2, "text must not be empty."));
    }
    if req.harness == "t3" || (req.harness.is_empty() && !req.project.is_empty()) {
        let project = if req.project.is_empty() { req.cwd } else { req.project };
        if project.is_empty() {
            return Err(EverettError::new(2, "A T3 start needs project (a T3 project id or a path inside it)."));
        }
        if !req.provider.is_empty() && req.model.is_empty() {
            return Err(EverettError::new(2, "provider requires model."));
        }
        let mut args: Vec<&str> = vec!["thread", "start", "--json"];
        for (flag, value) in [("--title", req.title), ("--provider", req.provider), ("--model", req.model),
                              ("--worktree", req.worktree), ("--branch", req.branch)] {
            if !value.is_empty() {
                args.extend([flag, value]);
            }
        }
        if req.wait {
            args.push("--wait");
        }
        args.extend([project, req.text]);
        let result = t3_json(&args, clamp_timeout(req.timeout, req.wait)?)?;
        return Ok(json!({"via": "t3", "started": true, "waited": req.wait, "result": result}));
    }
    if req.harness.is_empty() {
        return Err(EverettError::new(2, "start needs harness (claude, codex, omp, pi, hermes, grok, devin or t3) or a T3 project."));
    }
    let cwd = if req.cwd.is_empty() { "." } else { req.cwd };
    let timeout = clamp_timeout(req.timeout, true)?;
    let r = crate::send::spawn_for(req.harness, req.text, cwd, timeout, None, caller)?;
    Ok(json!({"via": "headless", "started": true, "waited": true, "harness": r.harness,
              "session_id": r.session_id, "cwd": r.cwd, "reply": r.reply}))
}

/// Send text into a T3 thread as its next turn. Non-T3 sessions go through
/// `everett_send`, which owns ping/resume delivery and its receipts.
pub fn send_t3(thread: &str, session: Option<&Session>, text: &str, wait: bool, timeout: Option<f64>,
               caller: Option<&str>) -> Result<Value> {
    crate::send::require_local_prompt(caller)?;
    if text.trim().is_empty() {
        return Err(EverettError::new(2, "text must not be empty."));
    }
    let thread = thread_for(thread, session)
        .ok_or_else(|| EverettError::new(4, "No T3 thread resolves to this target."))?;
    let mut args = vec!["thread", "send", "--json"];
    if wait {
        args.push("--wait");
    }
    args.extend([thread.as_str(), text]);
    let result = t3_json(&args, clamp_timeout(timeout, wait)?)?;
    Ok(json!({"via": "t3", "thread": thread, "sent": true, "waited": wait, "result": result}))
}

pub fn read(thread: &str, session: Option<&Session>, messages: usize, caller: Option<&str>) -> Result<Value> {
    crate::send::require_local_prompt(caller)?;
    let messages = messages.clamp(1, MAX_MESSAGES);
    if let Some(thread) = thread_for(thread, session) {
        let n = messages.to_string();
        let result = t3_json(&["thread", "show", "--json", "--messages", &n, &thread], QUICK)?;
        return Ok(json!({"via": "t3", "thread": thread, "result": result}));
    }
    let session = session.ok_or_else(|| EverettError::new(2, "read needs session or thread."))?;
    let text = std::fs::read_to_string(&session.path).map_err(|e| {
        EverettError::new(4, format!("Cannot read the {} transcript: {e}", session.harness))
    })?;
    Ok(json!({
        "via": "transcript",
        "session_id": session.id,
        "harness": session.harness,
        "title": session.title,
        "cwd": session.cwd,
        "running": session.running,
        "state": session.state,
        "messages": transcript_tail(&text, messages),
    }))
}

/// Explicit per-harness answer; Everett never kills a user's interactive session.
pub fn stop(thread: &str, session: Option<&Session>) -> Result<Value> {
    let target = if !thread.is_empty() || session.and_then(crate::t3_wake::thread_id).is_some() {
        "t3"
    } else {
        session.map(|s| s.harness.as_str()).unwrap_or("")
    };
    let reason = match target {
        "t3" => "The t3 CLI exposes no interrupt. Stop the turn from T3 Code itself.",
        "" => "stop needs session or thread.",
        _ => "Everett starts headless runs synchronously and does not own interactive sessions, so it has no process it may stop. Stop it in its own terminal or app.",
    };
    Err(EverettError::new(4, format!("stop is not supported for {}: {reason}", if target.is_empty() { "this target" } else { target })))
}

/// Last `limit` user/assistant text messages from a JSONL transcript of any
/// supported harness. Finds `{role, content}` objects at any depth, so Claude
/// (`message`), Codex (`payload`) and plain chat rows all parse.
pub fn transcript_tail(text: &str, limit: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        if let Some((role, body)) = find_message(&row, 0) {
            if !body.trim().is_empty() {
                let body: String = body.chars().take(4000).collect();
                out.push(json!({"role": role, "text": body}));
            }
        }
    }
    let skip = out.len().saturating_sub(limit);
    out.split_off(skip)
}

fn find_message(v: &Value, depth: usize) -> Option<(String, String)> {
    let obj = v.as_object()?;
    if let (Some(role), Some(content)) = (obj.get("role").and_then(Value::as_str), obj.get("content")) {
        if matches!(role, "user" | "assistant") {
            return Some((role.to_string(), content_text(content)));
        }
    }
    if depth >= 3 {
        return None;
    }
    obj.values().find_map(|child| find_message(child, depth + 1))
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter()
            .filter(|p| matches!(p.get("type").and_then(Value::as_str), Some("text" | "input_text" | "output_text")))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub fn as_map(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        other => {
            let mut m = Map::new();
            m.insert("result".into(), other);
            m
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_reads_claude_codex_and_plain_rows() {
        let text = [
            r#"{"type":"user","message":{"role":"user","content":"hi claude"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello"},{"type":"tool_use","name":"x"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"codex ask"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"hidden"}]}}"#,
            r#"{"role":"assistant","content":"plain"}"#,
            "not json",
        ].join("\n");
        let all = transcript_tail(&text, 10);
        let texts: Vec<_> = all.iter().map(|m| m["text"].as_str().unwrap()).collect();
        assert_eq!(texts, ["hi claude", "hello", "codex ask", "plain"]);
        assert_eq!(transcript_tail(&text, 2).len(), 2);
        assert_eq!(transcript_tail(&text, 2)[1]["text"], "plain");
    }

    #[test]
    fn json_output_skips_leading_log_noise() {
        let noisy = "[19:46] WARN (#20): migration diverges {\n  divergent: [\n    '73:X'\n  ]\n}\n[\n  {\"id\": \"t1\"}\n]\n";
        assert_eq!(parse_json_output(noisy), Some(json!([{"id": "t1"}])));
        assert_eq!(parse_json_output("{\"ok\":true}"), Some(json!({"ok": true})));
        assert_eq!(parse_json_output("plain text"), None);
    }

    #[test]
    fn timeouts_are_bounded() {
        assert_eq!(clamp_timeout(None, false).unwrap(), QUICK);
        assert_eq!(clamp_timeout(Some(1e9), true).unwrap(), MAX_WAIT);
        assert!(clamp_timeout(Some(f64::NAN), true).is_err());
    }
}
