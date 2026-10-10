use std::collections::HashSet;
use std::fs;
use std::sync::OnceLock;

use regex::Regex;

use serde_json::Value;

use crate::adapters::{claude, codex, devin, external, grok, hermes, omp, pi, t3code};
use crate::error::{EverettError, Result};
use crate::proc::ps_commands;
use crate::session::{home, now, Session};
use crate::{cards, events};

pub const LIVE_WINDOW: f64 = 600.0;
pub const CAP: usize = 40;
pub const AUTO_PREFIXES: &[&str] = &["You are ", "Automation:"];

pub const ADAPTERS: &[&str] = &["claude", "codex", "omp", "pi", "hermes", "grok", "devin"];

fn adapter_scan(name: &str, since_hours: f64) -> Vec<Session> {
    match name {
        "claude" => claude::scan(since_hours),
        "codex" => codex::scan(since_hours),
        "omp" => omp::scan(since_hours),
        "pi" => pi::scan(since_hours),
        "hermes" => hermes::scan(since_hours),
        "grok" => grok::scan(since_hours),
        "devin" => devin::scan(since_hours),
        _ => Vec::new(),
    }
}

/// Sessions Everett started headless: shown like interactive ones.
pub fn spawned_ids() -> HashSet<String> {
    let mut ids = HashSet::new();
    let Ok(text) = fs::read_to_string(home().join(".everett").join("spawned.jsonl")) else {
        return ids;
    };
    for line in text.lines() {
        if let Ok(Value::Object(d)) = serde_json::from_str::<Value>(line) {
            if let Some(id) = d.get("id").and_then(|v| v.as_str()) {
                ids.insert(id.to_string());
            }
        }
    }
    ids
}

pub fn is_auto(s: &Session, spawned: &HashSet<String>) -> bool {
    if spawned.contains(&s.id) {
        return false;
    }
    s.auto || AUTO_PREFIXES.iter().any(|p| s.first_user.starts_with(p))
}

pub fn mark_running(sessions: &mut [Session], ps_out: &str, at: f64) {
    for s in sessions.iter_mut() {
        s.running = s.running || session_running(s, Some(ps_out), Some(at));
    }
}

/// Only harness binaries count as "running", not greps/scripts mentioning the id.
/// Shared with send::is_busy so ls and send agree.
pub fn harness_bin() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(^|/)(claude|codex|omp|pi|hermes|grok|devin)(\s|$)").unwrap())
}

/// Recheck one session's running state immediately before a send.
pub fn session_running(s: &Session, ps_out: Option<&str>, at: Option<f64>) -> bool {
    if external::is_external(&s.harness) {
        return false;
    }
    let ps = ps_out.map(|p| p.to_string()).unwrap_or_else(ps_commands);
    let now = at.unwrap_or_else(now);
    (!s.id.is_empty()
        && ps.lines().any(|line| line.contains(&s.id) && harness_bin().is_match(line)))
        || (now - s.last_active) < LIVE_WINDOW
}

/// Recent local and durable external sessions; `harness` filters before the local cap.
pub fn scan(since_hours: f64, include_auto: bool, limit: Option<usize>, harness: &str) -> Vec<Session> {
    let mut sessions: Vec<Session> = Vec::new();
    for name in ADAPTERS {
        if harness.is_empty() || name == &harness {
            sessions.extend(adapter_scan(name, since_hours));
        }
    }
    sessions.extend(external::scan(harness));
    t3code::overlay(&mut sessions, since_hours, harness);
    for s in sessions.iter_mut() {
        if s.first_user.is_empty() {
            s.first_user = s.last_user.clone();
        }
    }
    if !include_auto {
        let spawned = spawned_ids();
        sessions.retain(|s| !is_auto(s, &spawned));
    }
    sessions.sort_by(|a, b| b.last_active.partial_cmp(&a.last_active).unwrap_or(std::cmp::Ordering::Equal));
    if let Some(limit) = limit {
        let mut local_count = 0;
        sessions.retain(|s| {
            if external::is_external(&s.harness) {
                return true;
            }
            local_count += 1;
            local_count <= limit
        });
    }
    let ps = ps_commands();
    mark_running(&mut sessions, &ps, now());
    cards::apply(&mut sessions, None);
    events::apply(&mut sessions);
    sessions
}

fn name(s: &Session) -> String {
    let text = if !s.card.is_empty() { s.card.clone() } else { s.title.clone() };
    let text = text.split("What:").last().unwrap_or("").trim().to_string();
    let head = match text.chars().take(40).collect::<String>().find(':') {
        Some(_) => text.split(':').next().unwrap_or("").to_string(),
        None => String::new(),
    };
    head.trim().to_lowercase()
}

/// Resolve `send --to`: exact id, unique id prefix, exact title, card name or project folder.
pub fn find(target: &str, sessions: &[Session]) -> Result<Session> {
    let target = target.trim();
    if target.is_empty() {
        return Err(EverettError::new(2, "--to needs a session id prefix or name."));
    }
    if let Some(s) = sessions.iter().find(|s| s.id == target) {
        return Ok(s.clone());
    }
    let target_fold = target.to_lowercase();
    let passes: Vec<Vec<&Session>> = vec![
        sessions.iter().filter(|s| s.id.starts_with(target)).collect(),
        sessions.iter().filter(|s| s.title.trim().to_lowercase() == target_fold).collect(),
        sessions.iter().filter(|s| name(s) == target_fold).collect(),
        sessions
            .iter()
            .filter(|s| {
                s.cwd.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_lowercase() == target_fold
            })
            .collect(),
    ];
    for hits in passes {
        if hits.len() == 1 {
            return Ok(hits[0].clone());
        }
        if hits.len() > 1 {
            let lines: Vec<String> = hits
                .iter()
                .take(10)
                .map(|s| {
                    let work = if !s.card.is_empty() {
                        s.card.clone()
                    } else if !s.title.is_empty() {
                        s.title.clone()
                    } else {
                        s.first_user.clone()
                    };
                    let work: String = work.chars().take(70).collect();
                    format!("  {}  [{}] {} — {}", s.id, s.harness, s.cwd, work)
                })
                .collect();
            return Err(EverettError::new(
                2,
                format!(
                    "\"{}\" matches {} sessions; use a longer id prefix:\n{}",
                    target,
                    hits.len(),
                    lines.join("\n")
                ),
            ));
        }
    }
    Err(EverettError::new(
        2,
        format!("no session matches \"{}\" in the look-back window (try --hours).", target),
    ))
}
