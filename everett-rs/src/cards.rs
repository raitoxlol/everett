use std::fs;
use std::path::PathBuf;

pub mod auto;

use regex::Regex;

use crate::session::{clean, file_mtime, home, Session};

pub const STALE_AFTER: f64 = 24.0 * 3600.0;
pub const AUTO_MARKER: &str = "<!-- everett:auto -->";

pub fn card_dir() -> PathBuf {
    home().join(".everett").join("cards")
}

/// Where this session's card lives, or None when the id could escape cards/.
/// Same rule as `inbox::valid_id`: [A-Za-z0-9._-]+, never '.'/'..'.
pub fn card_path(session_id: &str) -> Option<PathBuf> {
    if !crate::inbox::valid_id(session_id) {
        return None;
    }
    Some(card_dir().join(format!("{}.md", session_id)))
}

fn read_card_full(path: &PathBuf) -> Option<(String, f64, &'static str)> {
    let text = fs::read_to_string(path).ok()?;
    let mtime = file_mtime(path);
    let source = if text.lines().next() == Some(AUTO_MARKER) { "auto" } else { "agent" };
    let text = text.strip_prefix(&format!("{}\n", AUTO_MARKER)).unwrap_or(&text).to_string();
    let frontmatter = Regex::new(r"(?s)\A---\n.*?\n---\n").unwrap();
    let text = frontmatter.replace(&text, "");
    let headings = Regex::new(r"(?m)^#+\s*").unwrap();
    let text = headings.replace_all(&text, "");
    let body = text.trim().to_string();
    if body.is_empty() { None } else { Some((body, mtime, source)) }
}

pub fn read_card(session_id: &str) -> Option<(String, f64)> {
    read_card_full(&card_path(session_id)?).map(|(body, mtime, _)| (clean(&body, 300), mtime))
}

pub fn read_card_body(session_id: &str) -> Option<(String, f64)> {
    read_card_full(&card_path(session_id)?).map(|(body, mtime, _)| (body, mtime))
}

pub(crate) fn status(session_id: &str, now: f64) -> crate::delivery_status::AgentStatus {
    use crate::delivery_status::{unix_ms, AgentStatus, CardFreshness};
    let Some((body, mtime, source)) = card_path(session_id).as_ref().and_then(read_card_full) else {
        return AgentStatus::unknown(None);
    };
    let timestamp = unix_ms(mtime);
    if source != "agent" || timestamp.is_none() || mtime > now {
        return AgentStatus::unknown(timestamp);
    }
    if now - mtime >= STALE_AFTER {
        return AgentStatus { freshness: CardFreshness::Stale, ..AgentStatus::unknown(timestamp) };
    }
    let field = |name: &str| {
        let mut fields = body.lines().filter_map(|line| {
            let (key, value) = line.trim().split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| clean(value, 300))
        });
        let value = fields.next()?;
        (!value.is_empty() && fields.next().is_none()).then_some(value)
    };
    let Some(state) = field("state") else { return AgentStatus::unknown(timestamp) };
    AgentStatus {
        freshness: CardFreshness::Current,
        what: field("what"),
        state,
        next: field("next"),
        updated_at_ms: timestamp,
    }
}

pub fn is_auto_card(session_id: &str) -> bool {
    card_path(session_id)
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|t| t.lines().next() == Some(AUTO_MARKER))
        .unwrap_or(false)
}

/// Prefer the session's own card over extracted text, unless the card went stale.
pub fn apply(sessions: &mut [Session]) {
    let now = crate::session::now();
    for s in sessions.iter_mut() {
        let Some(path) = card_path(&s.id) else { continue };
        let Some((body, mtime, source)) = read_card_full(&path) else {
            continue;
        };
        if now - mtime >= STALE_AFTER {
            continue;
        }
        s.card = clean(&body, 300);
        s.card_source = Some(source.to_string());
    }
}

#[derive(Default)]
pub struct Counts {
    pub agent: usize,
    pub auto: usize,
    pub missing: usize,
}

impl Counts {
    pub fn total(&self) -> usize {
        self.agent + self.auto + self.missing
    }

    fn record(&mut self, source: Option<&str>) {
        match source {
            Some("agent") => self.agent += 1,
            Some("auto") => self.auto += 1,
            _ => self.missing += 1,
        }
    }
}

pub fn coverage(sessions: &[Session]) -> (Vec<(&str, Counts)>, Counts) {
    let mut rows: Vec<_> = crate::harness::NATIVE.iter().map(|&harness| (harness, Counts::default())).collect();
    let mut totals = Counts::default();
    for session in sessions {
        let harness = session.harness.as_str();
        let source = session.card_source.as_deref();
        let row = match rows.iter_mut().find(|(name, _)| *name == harness) {
            Some((_, counts)) => counts,
            None => {
                rows.push((harness, Counts::default()));
                &mut rows.last_mut().unwrap().1
            }
        };
        row.record(source);
        totals.record(source);
    }
    (rows, totals)
}

/// Rewrite every AUTO-marked card among `sessions` with the current builder. Never touches a
/// missing card or an agent-written one. Returns (rewritten, skipped).
pub fn regenerate_auto(sessions: &[Session], dry_run: bool) -> std::result::Result<(usize, usize), String> {
    let mut rewritten = 0usize;
    let mut skipped = 0usize;
    for s in sessions {
        if !auto::supports(&s.harness) || s.path.is_empty() {
            skipped += 1;
            continue;
        }
        if !is_auto_card(&s.id) {
            skipped += 1;
            continue;
        }
        let content = auto::for_session(s);
        if content.is_empty() {
            skipped += 1;
            continue;
        }
        let Some(target) = card_path(&s.id) else {
            skipped += 1; // invalid session id: nothing safe to write
            continue;
        };
        let current = std::fs::read_to_string(&target).ok();
        if current.as_deref() == Some(content.as_str()) {
            skipped += 1;
            continue;
        }
        if !dry_run {
            std::fs::write(&target, &content)
                .map_err(|e| format!("cannot write {}: {}", target.display(), e))?;
        }
        rewritten += 1;
    }
    Ok((rewritten, skipped))
}

pub const INSTRUCTION: &str = "REQUIRED — Everett card. Other agents route work to this session using a card at {path}. \
In your FIRST reply that does any work (even a plain answer), write that file: plain markdown, max 50 words — \
use three lines labeled What:, State:, and Next:. Rewrite it when the topic or state changes. \
One quick file write; do not mention it to the user.";

#[cfg(test)]
mod tests {
    #[test]
    fn full_card_lookup_stays_in_the_cards_directory() {
        for id in ["", "../outside", "a/b", "a\\b", "/absolute", "..", ".", "a/../b", "../../etc/evil"] {
            assert!(super::read_card_body(id).is_none(), "{id}");
            assert!(super::card_path(id).is_none(), "{id}");
        }
    }
}
