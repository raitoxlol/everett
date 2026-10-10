use std::fs;
use std::path::PathBuf;

use regex::Regex;

use crate::session::{clean, file_mtime, home, Session};

pub const STALE_AFTER: f64 = 24.0 * 3600.0;
pub const AUTO_MARKER: &str = "<!-- everett:auto -->";

pub fn card_dir() -> PathBuf {
    home().join(".everett").join("cards")
}

pub fn card_path(session_id: &str) -> PathBuf {
    card_dir().join(format!("{}.md", session_id))
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
    read_card_full(&card_path(session_id)).map(|(body, mtime, _)| (clean(&body, 300), mtime))
}

pub fn read_card_body(session_id: &str) -> Option<(String, f64)> {
    if session_id.is_empty() || session_id.contains(['/', '\\']) {
        return None;
    }
    read_card_full(&card_path(session_id)).map(|(body, mtime, _)| (body, mtime))
}

pub fn is_auto_card(session_id: &str) -> bool {
    fs::read_to_string(card_path(session_id))
        .map(|t| t.lines().next() == Some(AUTO_MARKER))
        .unwrap_or(false)
}

/// Prefer the session's own card over extracted text, unless the card went stale.
pub fn apply(sessions: &mut [Session], now: Option<f64>) {
    let _ = now;
    for s in sessions.iter_mut() {
        let Some((body, mtime, source)) = read_card_full(&card_path(&s.id)) else {
            continue;
        };
        if s.last_active - mtime > STALE_AFTER {
            continue;
        }
        s.card = clean(&body, 300);
        s.card_source = Some(source.to_string());
    }
}

pub const INSTRUCTION: &str = "REQUIRED — Everett card. Other agents route work to this session using a card at {path}. \
In your FIRST reply that does any work (even a plain answer), write that file: plain markdown, max 50 words — \
what this session is about, current state, next step. Rewrite it when the topic or state changes. \
One quick file write; do not mention it to the user.";

#[cfg(test)]
mod tests {
    #[test]
    fn full_card_lookup_stays_in_the_cards_directory() {
        for id in ["", "../outside", "a/b", "a\\b", "/absolute"] {
            assert!(super::read_card_body(id).is_none());
        }
    }
}
