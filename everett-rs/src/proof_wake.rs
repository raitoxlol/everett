//! Observe a single explicitly selected local ping without consuming inbox data.

use std::time::{Duration, Instant};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use wait_timeout::ChildExt;

use serde::Serialize;
use serde_json::{json, Value};

use crate::{claude_wake, inbox, registry, send};
use crate::error::{EverettError, Result};
use crate::session::Session;

#[derive(Debug, Serialize)]
pub struct Report {
    pub status: String,
    pub exit_code: i32,
    pub detail: String,
    pub dry_run: bool,
    pub started_at: String,
    pub finished_at: String,
    pub total_latency_ms: f64,
    pub target_resolved: Option<Value>,
    pub dispatched: Option<Value>,
    pub picked_up: Option<Value>,
    pub replied: Option<Value>,
    pub incorrect_reply: Option<Value>,
}

fn timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl Report {
    fn finish(mut self, started: Instant, status: &str, exit_code: i32, detail: impl Into<String>) -> Self {
        self.finished_at = timestamp();
        self.total_latency_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.status = status.into();
        self.exit_code = exit_code;
        self.detail = detail.into();
        self
    }

    pub fn human(&self) -> String {
        let mut lines = vec![format!("WAKE PROOF {} (exit {})", self.status, self.exit_code)];
        for (name, stage) in [
            ("target resolved", &self.target_resolved),
            ("dispatched", &self.dispatched),
            ("picked up", &self.picked_up),
            ("replied", &self.replied),
        ] {
            lines.push(match stage {
                Some(value) => format!("  {name}: {value}"),
                None => format!("  {name}: not observed"),
            });
        }
        if let Some(reply) = &self.incorrect_reply {
            lines.push(format!("  incorrect reply: {reply}"));
        }
        lines.push(format!("{} Total latency: {:.0} ms.", self.detail, self.total_latency_ms));
        lines.join("\n")
    }
}

enum Transport {
    T3(String),
    Omp,
    ClaudeBg(claude_wake::PreparedWake),
    Resume,
}

impl Transport {
    fn name(&self) -> &'static str {
        match self {
            Self::T3(_) => "t3",
            Self::Omp => "omp",
            Self::ClaudeBg(_) => "claude_bg",
            Self::Resume => "resume",
        }
    }
}

fn resolve_transport(target: &Session, caller: &str, deadline: Instant) -> Result<Transport> {
    send::delivery_mode_for(target, "ping", None, Some(caller))?;
    if target.source == "t3code" {
        Ok(Transport::T3(crate::t3_wake::validate_target(target)?))
    } else if crate::omp_wake::available(target) {
        Ok(Transport::Omp)
    } else if send::attached(target, None) {
        Ok(Transport::ClaudeBg(claude_wake::prepare_until(target, deadline)?))
    } else {
        send::command_for(target, "Everett inbox nudge")?;
        send::require_harness(&target.harness, None)?;
        if !send::wait_idle(target, 0.0, 1.0) {
            return Err(EverettError::new(4, "Resume target is busy. Nothing was queued."));
        }
        Ok(Transport::Resume)
    }
}

struct ResumeDispatch(Child);

impl ResumeDispatch {
    fn failure(&mut self) -> Option<String> {
        match self.0.try_wait() {
            Ok(Some(status)) if !status.success() => Some(format!("resume client exited {status}")),
            Err(error) => Some(format!("cannot observe resume client: {error}")),
            _ => None,
        }
    }
}

impl Drop for ResumeDispatch {
    fn drop(&mut self) {
        // The proof owns this connector's process group, never an existing session.
        unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL); }
        let _ = self.0.kill();
        let _ = self.0.wait_timeout(Duration::from_millis(250));
    }
}

struct Dispatch {
    receipt: Value,
    resume: Option<ResumeDispatch>,
}

fn dispatch(target: &Session, body: &str, caller: &str, transport: Transport, deadline: Instant) -> Result<Dispatch> {
    send::require_local_prompt(Some(caller))?;
    send::refuse_self(target, Some(caller))?;
    let who = send::sender_info(Some(caller));
    let sender = who["sender"].as_str().unwrap_or(inbox::HUMAN);
    let hops = send::caller_hops_for(Some(sender));
    if hops >= inbox::MAX_HOPS {
        return Err(EverettError::new(7, "Hop limit reached; nothing was queued."));
    }
    let message = inbox::post_with_deadline(&target.id, body, sender, "message", "", hops + 1,
        who["from_harness"].as_str().unwrap_or(""), who["from_card"].as_str().unwrap_or(""), None, Some(deadline))?;
    let mid = message["id"].as_str().unwrap();
    let dispatch = (|| {
        let remaining = deadline.saturating_duration_since(Instant::now()).as_secs_f64();
        if remaining <= 0.0 {
            return Err(EverettError::new(4, "Proof deadline expired before dispatch."));
        }
        let (wake, resume) = match transport {
            Transport::T3(thread) => {
                let receipt = crate::t3_wake::wake_with_timeout(&thread, target, mid, remaining)?;
                (json!({"state":"prompt_sent", "transport":"t3", "thread_id":thread, "receipt":receipt}), None)
            }
            Transport::Omp => (json!({"state":"extension_pending", "transport":"omp-extension"}), None),
            Transport::ClaudeBg(prepared) => match claude_wake::wake_until(&prepared, mid, deadline) {
                claude_wake::WakeOutcome::PromptSent { client_pid, target_pid } =>
                    (json!({"state":"prompt_sent", "client_pid":client_pid, "target_pid":target_pid}), None),
                outcome => return Err(EverettError::new(4, format!("Claude dispatch not confirmed: {outcome:?}"))),
            },
            Transport::Resume => {
                let nudge = crate::t3_wake::nudge(target, mid)?;
                let argv = send::command_for(target, &nudge)?;
                let env = send::hop_env_for(Some(caller))?;
                if Instant::now() >= deadline {
                    return Err(EverettError::new(4, "Proof deadline expired before resume."));
                }
                let child = Command::new(&argv[0]).args(&argv[1..]).current_dir(&target.cwd)
                    .env_clear().envs(env).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
                    .process_group(0).spawn().map_err(|error| EverettError::new(4, format!("cannot start resume: {error}")))?;
                (json!({"state":"resume_started", "client_pid":child.id(), "command":argv}), Some(ResumeDispatch(child)))
            }
        };
        Ok(Dispatch { receipt: json!({"mode":"ping", "message_id":mid, "reply_inbox":sender, "wake":wake}), resume })
    })();
    dispatch.map_err(|error| EverettError::new(error.code, format!(
        "{}. Inbox message {mid} remains recorded and may have been read. Do not blindly retry.", error.message)))
}

pub fn run(to: &str, hours: f64, timeout: f64, dry_run: bool) -> Report {
    let started = Instant::now();
    let started_ts = crate::session::now();
    let mut report = Report {
        status: String::new(),
        exit_code: 0,
        detail: String::new(),
        dry_run,
        started_at: timestamp(),
        finished_at: String::new(),
        total_latency_ms: 0.0,
        target_resolved: None,
        dispatched: None,
        picked_up: None,
        replied: None,
        incorrect_reply: None,
    };
    // Use a monotonic budget, including dispatch, without constructing an
    // overflowing Instant from an arbitrary CLI float.
    let Ok(budget) = Duration::try_from_secs_f64(timeout) else {
        return report.finish(started, "invalid", 2, "--timeout must be a positive finite number of seconds.");
    };
    if budget.is_zero() {
        return report.finish(started, "invalid", 2, "--timeout must be greater than zero.");
    }
    let Some(deadline) = started.checked_add(budget) else {
        return report.finish(started, "invalid", 2, "--timeout is too large.");
    };
    let caller = send::caller_session_id();
    if let Err(error) = send::require_local_prompt(Some(&caller)) {
        return report.finish(started, "refused", 2, error.message);
    }
    if to.trim().is_empty() {
        return report.finish(started, "invalid", 2, "An explicit --to session is required.");
    }
    if crate::peer::target(to).is_ok() {
        return report.finish(started, "refused", 2, "Wake proofs require a local target.");
    }
    let sessions = registry::scan(hours, true, None, "");
    let target = match registry::find(to, &sessions) {
        Ok(target) => target,
        Err(error) => return report.finish(started, "invalid", 2, error.message),
    };
    if let Err(error) = send::refuse_self(&target, Some(&caller)) {
        return report.finish(started, "refused", 2, error.message);
    }
    let transport = resolve_transport(&target, &caller, deadline);
    report.target_resolved = Some(json!({
        "at": timestamp(), "session_id": target.id, "harness": target.harness,
        "transport": transport.as_ref().map(|t| json!(t.name())).unwrap_or(json!("none")),
    }));
    let transport = match transport {
        Ok(transport) => transport,
        Err(error) => return report.finish(started, "no_transport", 3, error.message),
    };
    if dry_run {
        return report.finish(started, "dry_run", 0, format!(
            "Would send one nonce canary via {} and watch the sender's inbox for its exact correlated reply. Nothing sent.", transport.name()
        ));
    }
    let expected = format!("WAKE-OK {}", uuid::Uuid::new_v4().simple());
    let body = format!("Reply through Everett with exactly: {expected}");
    let remaining = budget.saturating_sub(started.elapsed()).as_secs_f64();
    if remaining <= 0.0 {
        return report.finish(started, "dispatch_failed", 4, "Timeout expired before dispatch. Nothing sent.");
    }
    let mut dispatch = match dispatch(&target, &body, &caller, transport, deadline) {
        Ok(dispatch) => dispatch,
        Err(error) => return report.finish(started, "dispatch_failed", 4, error.message),
    };
    let receipt = &dispatch.receipt;
    let mid = receipt.get("message_id").and_then(Value::as_str).unwrap_or("").to_owned();
    let sender = receipt.get("reply_inbox").and_then(Value::as_str).unwrap_or(inbox::HUMAN).to_owned();
    report.dispatched = Some(json!({"at":timestamp(), "message_id":mid, "receipt":receipt}));
    loop {
        if report.picked_up.is_none() && inbox::done_ids(&target.id).contains(&mid) {
            report.picked_up = Some(json!({"at":timestamp(), "message_id":mid}));
        }
        for reply in inbox::read(&sender) {
            if reply.get("reply_to").and_then(Value::as_str) != Some(mid.as_str())
                || reply.get("from").and_then(Value::as_str) != Some(target.id.as_str())
                || reply.get("to").and_then(Value::as_str) != Some(sender.as_str())
                || !reply.get("ts").and_then(Value::as_f64).is_some_and(|ts| {
                    ts.is_finite() && ts >= started_ts && ts <= started_ts + timeout
                })
            {
                continue;
            }
            let stage = json!({"at":timestamp(), "message_id":reply.get("id"), "reply_to":mid});
            if reply.get("text").and_then(Value::as_str) == Some(expected.as_str()) {
                report.replied = Some(stage);
                drop(dispatch);
                return report.finish(started, "passed", 0, "Received the target's correlated reply with the exact nonce.");
            }
            report.incorrect_reply = Some(stage);
        }
        // Take a final snapshot at the deadline so a reply arriving during the
        // last sleep is checked. Its durable timestamp must still be in budget.
        if report.picked_up.is_none() {
            if let Some(failure) = dispatch.resume.as_mut().and_then(ResumeDispatch::failure) {
                drop(dispatch);
                return report.finish(started, "dispatch_failed", 4, failure);
            }
        }
        if started.elapsed() >= budget {
            break;
        }
        std::thread::sleep(Duration::from_millis(50).min(budget.saturating_sub(started.elapsed())));
    }
    drop(dispatch);
    if report.picked_up.is_none() {
        report.finish(started, "pickup_timeout", 5, "Timed out waiting for the original message to be marked done; no matching reply observed.")
    } else {
        report.finish(started, "reply_timeout", 6, "The original was marked done, but no correlated reply with the exact nonce arrived within the timeout.")
    }
}
