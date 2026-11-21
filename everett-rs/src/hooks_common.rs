//! Shared card context and stop-hook logic for Claude Code, Codex, and Grok hooks.
//! Every public entry swallows all errors — a hook must never break a harness session.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

use crate::adapters::grok;
use crate::cards::auto::{adapter_fns, auto_card};
use crate::cards::{AUTO_MARKER, INSTRUCTION};
use crate::session::read_edges;

const SKIP_ENV: &str = "EVERETT_SEND";

/// Card instruction plus the shared core for a new session ('' to stay silent).
pub fn session_start_context(session_id: &str, cwd: &str) -> String {
    if std::env::var(SKIP_ENV).map(|v| !v.is_empty()).unwrap_or(false) || session_id.is_empty() {
        return String::new();
    }
    let Some(path) = crate::cards::card_path(session_id) else {
        return String::new();
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let text = INSTRUCTION.replace("{path}", &path.to_string_lossy());
    let cwd_owned = if cwd.is_empty() {
        std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        cwd.to_string()
    };
    let shared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::core::context(&cwd_owned)
    }))
    .unwrap_or_default();
    if shared.is_empty() {
        text
    } else {
        format!("{}\n\n{}", text, shared)
    }
}

/// Hook stdin JSON -> stdout JSON with the card instruction, or "" to stay silent.
pub fn session_start_output(raw: &str) -> String {
    let data: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let Some(map) = data.as_object() else { return String::new() };
    let sid = map.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
    if sid.is_empty() {
        return String::new();
    }
    let cwd = map.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
    let context = session_start_context(sid, cwd);
    if context.is_empty() {
        return String::new();
    }
    serde_json::to_string(&json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": context,
        }
    }))
    .unwrap_or_default()
}

/// Write a deterministic fallback card, silently ignoring every hook error.
pub fn stop_hook(raw: &str, harness: &str) {
    let _ = stop_hook_inner(raw, harness);
}

fn stop_hook_inner(raw: &str, harness: &str) -> Option<()> {
    if std::env::var(SKIP_ENV).map(|v| !v.is_empty()).unwrap_or(false)
        || (harness == "claude" && std::env::var("CLAUDE_CODE_ENTRYPOINT").as_deref() == Ok("sdk-cli"))
    {
        return None;
    }
    let data: Value = serde_json::from_str(raw).ok()?;
    let map = data.as_object()?;
    let session_id = map.get("session_id").and_then(|v| v.as_str())?;
    static SID_RE: OnceLock<Regex> = OnceLock::new();
    let sid_re = SID_RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9._-]+$").unwrap());
    if !sid_re.is_match(session_id) || session_id == "." || session_id == ".." {
        return None;
    }
    let transcript_value = map.get("transcript_path").and_then(|v| v.as_str())?;
    if transcript_value.is_empty() {
        return None;
    }
    let transcript = PathBuf::from(transcript_value);
    let transcript_mtime = transcript.metadata().and_then(|m| m.modified()).ok()?;
    let target = crate::cards::card_path(session_id)?;
    if target.is_symlink() {
        return None;
    }
    let (current, current_stat, existed) = match fs::read_to_string(&target) {
        Ok(c) => match target.metadata() {
            Ok(m) => (c, Some(m), true),
            Err(_) => return None,
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), None, false),
        Err(_) => return None,
    };
    if existed
        && (current.lines().next() != Some(AUTO_MARKER)
            || current_stat
                .as_ref()
                .map(|m| m.modified().unwrap_or(std::time::UNIX_EPOCH))
                .unwrap_or(std::time::UNIX_EPOCH)
                >= transcript_mtime)
    {
        return None;
    }
    let content = auto_card(&transcript, harness, map.get("cwd").and_then(|v| v.as_str()).unwrap_or(""), session_id);
    if content.is_empty() {
        return None;
    }
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if !existed {
        // Exclusive create makes an agent card that appeared meanwhile win.
        let mut f = OpenOptions::new().write(true).create_new(true).open(&target).ok()?;
        use std::io::Write;
        f.write_all(content.as_bytes()).ok()?;
        return Some(());
    }
    // Confirm the stale auto card did not become an agent card while parsing.
    let latest = fs::read_to_string(&target).ok()?;
    let latest_stat = target.metadata().ok()?;
    if latest.lines().next() != Some(AUTO_MARKER)
        || latest_stat.modified().ok()
            != current_stat.as_ref().and_then(|m| m.modified().ok())
        || latest != current
    {
        return None;
    }
    let tmp = target.with_file_name(format!(".{}.{}.tmp", target.file_name()?.to_string_lossy(), std::process::id()));
    fs::write(&tmp, &content).ok()?;
    let still_ok = !target.is_symlink()
        && fs::read_to_string(&target).ok().as_deref() == Some(current.as_str())
        && target.metadata().ok().and_then(|m| m.modified().ok())
            == current_stat.as_ref().and_then(|m| m.modified().ok());
    if !still_ok {
        let _ = fs::remove_file(&tmp);
        return None;
    }
    fs::rename(&tmp, &target).ok()?;
    Some(())
}

fn last_assistant(data: &Value, harness: &str) -> String {
    if let Some(map) = data.as_object() {
        for key in ["last_assistant_message", "lastAssistantMessage"] {
            if let Some(value) = map.get(key).and_then(|v| v.as_str()) {
                if !value.trim().is_empty() {
                    return value.to_string();
                }
            }
        }
    }
    let Some((_, assistant_text)) = adapter_fns(harness) else {
        return String::new();
    };
    let transcript = data.get("transcript_path").and_then(|v| v.as_str()).unwrap_or("");
    if transcript.is_empty() {
        return String::new();
    }
    let (head, tail) = read_edges(Path::new(transcript));
    let rows = if !tail.is_empty() { tail } else { head };
    for row in rows.iter().rev() {
        let text = assistant_text(row, true);
        if !text.is_empty() {
            return text;
        }
    }
    String::new()
}

/// Stop hook awareness: record done / blocked / needs-input for this session (debounced),
/// mark it idle, and run the throttled escalation check. Silent on every error.
pub fn stop_event(raw: &str, harness: &str) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| stop_event_inner(raw, harness)));
}

fn stop_event_inner(raw: &str, harness: &str) {
    if std::env::var(SKIP_ENV).map(|v| !v.is_empty()).unwrap_or(false)
        || (harness == "claude" && std::env::var("CLAUDE_CODE_ENTRYPOINT").as_deref() == Ok("sdk-cli"))
    {
        return;
    }
    let data: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let Some(map) = data.as_object() else { return };
    if map.get("stop_hook_active").and_then(|v| v.as_bool()).unwrap_or(false)
        || map.get("stopHookActive").and_then(|v| v.as_bool()).unwrap_or(false)
    {
        return;
    }
    let session_id = map
        .get("session_id")
        .and_then(|v| v.as_str())
        .or_else(|| map.get("sessionId").and_then(|v| v.as_str()))
        .unwrap_or("");
    if !crate::inbox::valid_id(session_id) {
        return;
    }
    crate::inbox::touch_live(session_id, harness, "idle");
    if let Some((kind, summary)) = crate::events::classify(&last_assistant(&data, harness)) {
        let cwd = map.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
        let cwd = if cwd.is_empty() {
            std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
        } else {
            cwd.to_string()
        };
        let _ = crate::events::record(&kind, &summary, Some(session_id), None, harness, Some(&cwd), "auto", None);
    }
    crate::events::maybe_escalate(None);
}

/// Grok's Stop payload -> the common stop hook (it has no transcript path of its own).
pub fn grok_stop_hook(raw: &str) {
    let _ = grok_stop_inner(raw);
}

fn grok_stop_inner(raw: &str) -> Option<()> {
    let data: Value = serde_json::from_str(raw).ok()?;
    let map = data.as_object()?;
    let session_id = map
        .get("sessionId")
        .and_then(|v| v.as_str())
        .or_else(|| map.get("session_id").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .or_else(|| std::env::var("GROK_SESSION_ID").ok())
        .unwrap_or_default();
    let cwd = map.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
    static SID_RE: OnceLock<Regex> = OnceLock::new();
    let sid_re = SID_RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9._-]+$").unwrap());
    if session_id.is_empty() || !sid_re.is_match(&session_id) {
        return None;
    }
    let path = grok::transcript(&session_id, cwd);
    let transcript_str = path.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    if path.is_some() {
        stop_hook_inner(
            &serde_json::to_string(&json!({
                "session_id": session_id,
                "transcript_path": transcript_str,
                "cwd": cwd,
            }))
            .unwrap_or_default(),
            "grok",
        );
    }
    stop_event_inner(
        &serde_json::to_string(&json!({
            "session_id": session_id,
            "transcript_path": transcript_str,
            "cwd": cwd,
            "lastAssistantMessage": map.get("lastAssistantMessage").and_then(|v| v.as_str()).unwrap_or(""),
            "stopHookActive": map.get("stopHookActive").and_then(|v| v.as_bool()).unwrap_or(false),
        }))
        .unwrap_or_default(),
        "grok",
    );
    Some(())
}


const EVENTS: &[&str] = &["UserPromptSubmit", "PostToolUse"];

/// Hook stdin JSON -> hook stdout JSON ("" to stay silent).
pub fn deliver_output(raw: &str, harness: &str) -> String {
    if std::env::var("EVERETT_SEND").map(|v| !v.is_empty()).unwrap_or(false) {
        return String::new();
    }
    if harness == "claude" && std::env::var("CLAUDE_CODE_ENTRYPOINT").as_deref() == Ok("sdk-cli") {
        return String::new();
    }
    let data: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let Some(map) = data.as_object() else { return String::new() };
    let is_grok = harness == "grok" || map.contains_key("sessionId");
    let session_id = map
        .get("session_id")
        .and_then(|v| v.as_str())
        .or_else(|| map.get("sessionId").and_then(|v| v.as_str()))
        .unwrap_or("");
    // Harnesses send hook_event_name (Claude), hookEventName (Grok), or snake_case values.
    let raw_event = map
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .or_else(|| map.get("hookEventName").and_then(|v| v.as_str()))
        .unwrap_or("");
    let event = EVENTS
        .iter()
        .find(|e| e.eq_ignore_ascii_case(raw_event))
        .copied()
        .unwrap_or("");
    if event.is_empty() {
        return String::new();
    }
    if !crate::inbox::valid_id(session_id) {
        return String::new();
    }
    crate::inbox::touch_live(session_id, if is_grok { "grok" } else { harness }, "turn");
    crate::events::maybe_escalate(None);
    if is_grok && event == "UserPromptSubmit" {
        return String::new();
    }
    let mut parts = Vec::new();
    if crate::inbox::path(session_id).map(|p| p.exists()).unwrap_or(false) {
        parts.push(crate::inbox::take(session_id));
    }
    if let (true, Some(prompt)) = (event == "UserPromptSubmit", map.get("prompt").and_then(Value::as_str)) {
        let cwd = map
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        parts.extend(crate::skill_match::hook_context(session_id, harness, prompt, &cwd));
    }
    parts.retain(|part| !part.is_empty());
    if parts.is_empty() {
        return String::new();
    }
    serde_json::to_string(&json!({
        "hookSpecificOutput": {"hookEventName": event, "additionalContext": parts.join("\n\n")}
    }))
    .unwrap_or_default()
}

/// Write hook output without panicking on a closed stdout (a harness that
/// pipes us and exits early would otherwise kill the hook with SIGPIPE/EPIPE).
fn emit(text: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

/// One `everett hook <stem>` invocation: stdin JSON -> stdout. Always exit 0.
pub fn hook_main(stem: &str, args: &[String]) -> i32 {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook_main_inner(stem, args)));
    0
}

fn hook_main_inner(stem: &str, args: &[String]) -> i32 {
    let stdin_raw = || {
        use std::io::Read;
        let mut buf = String::new();
        let _ = std::io::stdin().read_to_string(&mut buf);
        buf
    };
    match stem {
        "claude_session_start" => {
            if std::env::var("CLAUDE_CODE_ENTRYPOINT").as_deref() == Ok("sdk-cli") {
                return 0;
            }
            let out = session_start_output(&stdin_raw());
            if !out.is_empty() {
                emit(&format!("{out}\n"));
            }
        }
        "codex_session_start" => {
            let out = session_start_output(&stdin_raw());
            if !out.is_empty() {
                emit(&format!("{out}\n"));
            }
        }
        "claude_stop" => {
            let raw = stdin_raw();
            stop_hook(&raw, "claude");
            stop_event(&raw, "claude");
        }
        "codex_stop" => {
            let raw = stdin_raw();
            stop_hook(&raw, "codex");
            stop_event(&raw, "codex");
        }
        "claude_inbox" => {
            let out = deliver_output(&stdin_raw(), "claude");
            if !out.is_empty() {
                emit(&format!("{out}\n"));
            }
        }
        "codex_inbox" => {
            let out = deliver_output(&stdin_raw(), "codex");
            if !out.is_empty() {
                emit(&format!("{out}\n"));
            }
        }
        "grok_stop" => grok_stop_hook(&stdin_raw()),
        "grok_inbox" => {
            let out = deliver_output(&stdin_raw(), "grok");
            if !out.is_empty() {
                emit(&format!("{out}\n"));
            }
        }
        "omp_bind" | "pi_bind" => {
            if let Some(sid) = args.first() {
                crate::inbox::bind_live(sid, if stem == "pi_bind" { "pi" } else { "omp" });
            }
        }
        "devin_bind" => {
            let sid = serde_json::from_str::<Value>(&stdin_raw())
                .ok()
                .and_then(|v| v.get("session_id").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default();
            crate::inbox::bind_live(&sid, "devin");
        }
        "pi_inbox" => {
            if let Some(sid) = args.first() {
                if crate::inbox::valid_id(sid) {
                    crate::inbox::touch_live(sid, "pi", "turn");
                    let text = crate::inbox::take(sid);
                    if !text.is_empty() {
                        emit(&text);
                    }
                }
            }
        }
        "omp_wake_heartbeat" => {
            if let Some(binding) = args.first().and_then(|sid| crate::omp_wake::heartbeat(sid)) {
                emit(&binding.to_string());
            }
        }
        "omp_wake_pending" => {
            if let Some(sid) = args.first() {
                emit(&serde_json::to_string(&crate::omp_wake::pending_ids(sid)).unwrap_or_default());
            }
        }
        "omp_inbox" => {
            if let Some(sid) = args.first() {
                if crate::inbox::valid_id(sid) {
                    crate::inbox::touch_live(sid, "omp", "turn");
                    let text = crate::inbox::take_excluding(sid, &crate::omp_wake::nudged_ids(sid));
                    if !text.is_empty() {
                        emit(&text);
                    }
                }
            }
        }
        "omp_card_context" | "pi_card_context" => {
            if let Some(sid) = args.first() {
                let cwd = std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                let context = session_start_context(sid, &cwd);
                if !context.is_empty() {
                    emit(&context);
                }
            }
        }
        _ => {}
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_mtime(path: &Path, sec: i64, nsec: i64) {
        let c = std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap();
        let times = [
            libc::timespec { tv_sec: sec, tv_nsec: nsec },
            libc::timespec { tv_sec: sec, tv_nsec: nsec },
        ];
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(rc, 0);
    }

    #[test]
    fn auto_card_freshness_uses_full_mtime_not_just_nanos() {
        if !crate::test_support::isolated("hooks_common::tests::auto_card_freshness_uses_full_mtime_not_just_nanos") {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("EVERETT_HOME", home.path());
        let sid = "mtime-sid";
        let transcript = home.path().join("t.jsonl");
        fs::write(
            &transcript,
            "{\"type\":\"user\",\"message\":{\"content\":\"fix the login bug\"},\"cwd\":\"/w\"}\n",
        )
        .unwrap();
        let card = home.path().join(".everett/cards").join(format!("{sid}.md"));
        fs::create_dir_all(card.parent().unwrap()).unwrap();
        let body = format!("{}\nold auto body\n", AUTO_MARKER);
        fs::write(&card, &body).unwrap();
        // Transcript at T=*.9s, auto card 2s later but with a LOWER nanosecond
        // component: an mtime_nsec-only compare would treat the card as stale
        // and regenerate it.
        set_mtime(&transcript, 1_700_000_000, 900_000_000);
        set_mtime(&card, 1_700_000_002, 100_000_000);
        stop_hook_inner(
            &json!({"session_id": sid, "transcript_path": transcript}).to_string(),
            "claude",
        );
        assert_eq!(fs::read_to_string(&card).unwrap(), body);
    }
}
