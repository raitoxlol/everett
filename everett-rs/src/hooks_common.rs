//! Shared card context and stop-hook logic for Claude Code, Codex, and Grok hooks.
//! Every public entry swallows all errors — a hook must never break a harness session.

use std::fs::{self, OpenOptions};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::adapters::{claude, codex, grok};
use crate::cards::{AUTO_MARKER, INSTRUCTION};
use crate::session::{clean, home, read_edges, scan_full};

const SKIP_ENV: &str = "EVERETT_SEND";

/// Card instruction plus the shared core for a new session ('' to stay silent).
pub fn session_start_context(session_id: &str, cwd: &str) -> String {
    if std::env::var(SKIP_ENV).is_ok() || session_id.is_empty() {
        return String::new();
    }
    let path = crate::cards::card_path(session_id);
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

fn table_separator() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*\|?\s*:?-{3,}:?\s*(?:\|\s*:?-{3,}:?\s*)+\|?\s*$").unwrap())
}

fn tool_noise() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^\s*(?:assistant\s+to=|user\s+to=|to=(?:functions|mcp__|tool)|\[?(?:tool call|tool result|function call|function result)\]?:?)",
        )
        .unwrap()
    })
}

pub fn word_limit(text: &str, limit: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= limit {
        return words.join(" ");
    }
    if limit == 0 {
        return "…".to_string();
    }
    format!("{} …", words[..limit - 1].join(" "))
}

fn sentence(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"[.!?。！？](\s|$)").unwrap());
    match re.find(&flat) {
        Some(m) => {
            let end = m.start() + flat[m.start()..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            flat[..end].trim().to_string()
        }
        None => flat,
    }
}

fn setext_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*(?:=+|-+)\s*$").unwrap())
}

fn fence_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*(`{3,}|~{3,})").unwrap())
}

fn tool_call_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\s*\[?(?:tool|function)_(?:call|result)\b").unwrap())
}

fn rule_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*(?:[-*_]\s*){3,}$").unwrap())
}

fn heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s{0,3}#{1,6}\s+").unwrap())
}

fn bold_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*(?:\*\*|__)(.+?)(?:\*\*|__)\s*$").unwrap())
}

/// Cleaned lines with (line, is_heading, is_bold_only).
fn markdown_lines(text: &str) -> Vec<(String, bool, bool)> {
    let source: Vec<&str> = text.lines().collect();
    let mut table_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut setext_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for index in 0..source.len().saturating_sub(1) {
        if !source[index].trim().is_empty() && setext_re().is_match(source[index + 1]) {
            setext_lines.insert(index);
        }
    }
    for (index, line) in source.iter().enumerate() {
        if !table_separator().is_match(line) {
            continue;
        }
        table_lines.insert(index);
        for step in [-1i32, 1i32] {
            let mut cursor = index as i32 + step;
            while cursor >= 0 && (cursor as usize) < source.len() && source[cursor as usize].contains('|') {
                table_lines.insert(cursor as usize);
                cursor += step;
            }
        }
    }

    static HTML_COMMENT: OnceLock<Regex> = OnceLock::new();
    let html_comment = HTML_COMMENT.get_or_init(|| Regex::new(r"<!--.*?-->").unwrap());
    static LINK_DEF: OnceLock<Regex> = OnceLock::new();
    let link_def = LINK_DEF.get_or_init(|| Regex::new(r"^\s*\[[^]]+\]:\s*\S+").unwrap());
    static INLINE_LINK: OnceLock<Regex> = OnceLock::new();
    let inline_link = INLINE_LINK.get_or_init(|| Regex::new(r"!?\[([^]]*)\]\([^)]*\)").unwrap());
    static REF_LINK: OnceLock<Regex> = OnceLock::new();
    let ref_link = REF_LINK.get_or_init(|| Regex::new(r"\[([^]]+)\]\[[^]]*\]").unwrap());
    static ANGLE_URL: OnceLock<Regex> = OnceLock::new();
    let angle_url = ANGLE_URL.get_or_init(|| Regex::new(r"<https?://[^>]+>").unwrap());
    static URL: OnceLock<Regex> = OnceLock::new();
    let url = URL.get_or_init(|| Regex::new(r"https?://\S+").unwrap());
    static TAG: OnceLock<Regex> = OnceLock::new();
    let tag = TAG.get_or_init(|| Regex::new(r"<[^>]{1,200}>").unwrap());
    static QUOTE: OnceLock<Regex> = OnceLock::new();
    let quote = QUOTE.get_or_init(|| Regex::new(r"^\s*>+\s?").unwrap());
    static BULLET: OnceLock<Regex> = OnceLock::new();
    let bullet = BULLET.get_or_init(|| Regex::new(r"^\s*(?:[-+*]|\d+[.)])\s+").unwrap());
    static MARKS: OnceLock<Regex> = OnceLock::new();
    let marks = MARKS.get_or_init(|| Regex::new(r"[*_~`]").unwrap());

    let mut cleaned: Vec<(String, bool, bool)> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for (index, raw_line) in source.iter().enumerate() {
        let fence_match = fence_re().captures(raw_line);
        if let Some(f) = fence {
            if let Some(m) = &fence_match {
                let run = m.get(1).unwrap().as_str();
                if run.chars().next() == Some(f.0) && run.len() >= f.1 {
                    fence = None;
                }
            }
            continue;
        }
        if let Some(m) = &fence_match {
            let run = m.get(1).unwrap().as_str();
            fence = Some((run.chars().next().unwrap_or('`'), run.len()));
            continue;
        }
        if table_lines.contains(&index) || table_separator().is_match(raw_line) {
            continue;
        }
        if setext_lines.contains(&(index + 1)) || setext_re().is_match(raw_line) {
            continue;
        }
        if raw_line.trim_start().starts_with('|') && raw_line.trim_end().ends_with('|') {
            continue;
        }
        if tool_noise().is_match(raw_line) || tool_call_re().is_match(raw_line) {
            continue;
        }
        if rule_re().is_match(raw_line) {
            continue;
        }

        let heading = heading_re().is_match(raw_line) || setext_lines.contains(&index);
        let bold_only = bold_re().is_match(raw_line);
        let line = html_comment.replace_all(raw_line, " ");
        if link_def.is_match(&line) {
            continue;
        }
        let line = inline_link.replace_all(&line, "$1");
        let line = ref_link.replace_all(&line, "$1");
        let line = angle_url.replace_all(&line, " ");
        let line = url.replace_all(&line, " ");
        let line = tag.replace_all(&line, " ");
        let line = heading_re().replace_all(&line, "");
        let line = quote.replace_all(&line, "");
        let line = bullet.replace_all(&line, "");
        let line = marks.replace_all(&line, "");
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !line.is_empty() && !tool_noise().is_match(&line) {
            cleaned.push((line, heading, bold_only));
        }
    }
    cleaned
}

fn intent(text: &str) -> String {
    let lines = markdown_lines(text);
    let body = lines.iter().find(|(_, heading, _)| !heading).map(|(l, _, _)| l.clone()).unwrap_or_default();
    sentence(&body)
}

fn next_marker_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bnext\b|次のステップ|次は|次に").unwrap())
}

/// Python's `re.split(r'(?<=[.!?。！？])\s+', line)`: split on whitespace runs that
/// immediately follow a sentence-ending mark.
fn split_sentences(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut prev_end = false;
    while i < bytes.len() {
        let ch_len = utf8_len(bytes[i]);
        let chunk = &line[i..i + ch_len];
        if chunk.chars().next().map(|c| c.is_whitespace()).unwrap_or(false) && prev_end {
            let mut end = i;
            while end < bytes.len() && line[end..].chars().next().map(|c| c.is_whitespace()).unwrap_or(false) {
                end += utf8_len(bytes[end]);
            }
            out.push(line[start..i].to_string());
            start = end;
            i = end;
            prev_end = false;
            continue;
        }
        prev_end = matches!(chunk, "." | "!" | "?" | "。" | "！" | "？");
        i += ch_len;
    }
    out.push(line[start..].to_string());
    out
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

fn next_step(text: &str) -> String {
    let lines = markdown_lines(text);
    let explicit: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, (line, heading, bold_only))| {
            !heading && (next_marker_re().is_match(line) || *bold_only)
        })
        .map(|(i, _)| i)
        .collect();
    static STRIP_NEXT: OnceLock<Regex> = OnceLock::new();
    let strip_next = STRIP_NEXT.get_or_init(|| {
        Regex::new(r"(?i)^\s*(?:the\s+)?next(?:\s+step)?\s*(?:is\s+to\s+)?[:,—–-]?\s*").unwrap()
    });
    static STRIP_JP1: OnceLock<Regex> = OnceLock::new();
    let strip_jp1 = STRIP_JP1.get_or_init(|| Regex::new(r"^\s*次のステップ\s*[:：、]?\s*").unwrap());
    static STRIP_JP2: OnceLock<Regex> = OnceLock::new();
    let strip_jp2 = STRIP_JP2.get_or_init(|| Regex::new(r"^\s*次(?:は|に)\s*[:：、]?\s*").unwrap());
    for index in explicit.iter().rev() {
        let (line, _, bold_only) = &lines[*index];
        let mut line = line.clone();
        let sentences = split_sentences(&line);
        if !bold_only {
            line = sentences
                .iter()
                .find(|s| next_marker_re().is_match(s))
                .cloned()
                .unwrap_or(line);
        } else {
            line = sentences.first().cloned().unwrap_or(line);
        }
        let line = strip_next.replace(&line, "");
        let line = strip_jp1.replace(&line, "");
        let line = strip_jp2.replace(&line, "");
        let line = line.to_string();
        if !line.is_empty() {
            return sentence(&line);
        }
        for (following, heading, _) in &lines[*index + 1..] {
            if !heading && !following.is_empty() {
                return sentence(following);
            }
        }
    }
    lines
        .iter()
        .find(|(_, heading, _)| !heading)
        .map(|(l, _, _)| sentence(l))
        .unwrap_or_default()
}

fn project_name(cwd: &str) -> String {
    let path = if cwd.is_empty() { None } else { Some(crate::paths::expanduser(cwd)) };
    let home_dir = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| home());
    let is_generic = path
        .as_ref()
        .map(|p| p == &home_dir || p == Path::new("/"))
        .unwrap_or(true);
    if is_generic {
        return "Project".to_string();
    }
    let name = path
        .unwrap()
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let name = name.trim();
    if name.is_empty() {
        return "Project".to_string();
    }
    let mut chars = name.chars();
    format!("{}{}", chars.next().unwrap().to_uppercase(), chars.as_str())
}

/// T3 Code's thread title, then the harness's own session headline — used only when a
/// session has no usable user text anywhere in it.
fn thread_headline(harness: &str, session_id: &str) -> String {
    if session_id.is_empty() {
        return String::new();
    }
    if let Ok(threads) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::adapters::t3code::threads(&crate::adapters::t3code::db_path())
    })) {
        if let Some((_, title)) = threads.get(&(harness.to_string(), session_id.to_string())) {
            if !title.is_empty() {
                return clean(title, 120);
            }
        }
    }
    if harness == "codex" {
        let names = std::panic::catch_unwind(std::panic::AssertUnwindSafe(codex::thread_names)).unwrap_or_default();
        if let Some(name) = names.get(session_id) {
            if !name.is_empty() {
                return clean(name, 120);
            }
        }
    }
    String::new()
}

type Adapter = (
    fn(&Map<String, Value>, bool) -> String,
    fn(&Map<String, Value>, bool) -> String,
);

fn adapter_fns(harness: &str) -> Option<Adapter> {
    match harness {
        "claude" => Some((claude::user_text, claude::assistant_text)),
        "codex" => Some((codex::user_text, codex::assistant_text)),
        "grok" => Some((grok::user_text, grok::assistant_text)),
        _ => None,
    }
}

/// Build the deterministic fallback card body for a transcript ('' when nothing usable).
pub fn auto_card(transcript: &Path, harness: &str, cwd: &str, session_id: &str) -> String {
    let Some((user_text, assistant_text)) = adapter_fns(harness) else {
        return String::new();
    };
    let (head, tail) = read_edges(transcript);
    let rows: Vec<&Map<String, Value>> = head.iter().chain(tail.iter()).collect();
    let mut users: Vec<String> = rows
        .iter()
        .filter_map(|row| {
            let t = user_text(row, true);
            if t.is_empty() { None } else { Some(t) }
        })
        .collect();
    let mut tail_users: Vec<String> = tail
        .iter()
        .filter_map(|row| {
            let t = user_text(row, true);
            if t.is_empty() { None } else { Some(t) }
        })
        .collect();
    let mut assistants: Vec<String> = rows
        .iter()
        .filter_map(|row| {
            let t = assistant_text(row, true);
            if t.is_empty() { None } else { Some(t) }
        })
        .collect();
    if users.is_empty() {
        // read_edges' fixed byte window can miss real user text when a session opens with an
        // oversized injected preamble; fall back to a full bounded scan.
        let full_rows = scan_full(transcript, 20000);
        let full_users: Vec<String> = full_rows
            .iter()
            .filter_map(|row| {
                let t = user_text(row, true);
                if t.is_empty() { None } else { Some(t) }
            })
            .collect();
        if !full_users.is_empty() {
            users = full_users.clone();
            tail_users = full_users;
            if assistants.is_empty() {
                assistants = full_rows
                    .iter()
                    .filter_map(|row| {
                        let t = assistant_text(row, true);
                        if t.is_empty() { None } else { Some(t) }
                    })
                    .collect();
            }
        }
    }
    if users.is_empty() && assistants.is_empty() && session_id.is_empty() {
        return String::new();
    }
    let mut cwd = cwd.to_string();
    if cwd.is_empty() {
        cwd = rows
            .iter()
            .find_map(|row| {
                row.get("cwd")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        row.get("payload")
                            .and_then(|p| p.get("cwd"))
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                    })
            })
            .unwrap_or_default();
    }
    if cwd.is_empty() {
        cwd = std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    }
    let latest_users = if !tail_users.is_empty() { &tail_users } else { &users };
    let mut what = users.iter().find_map(|t| {
        let i = intent(t);
        if i.is_empty() { None } else { Some(i) }
    }).unwrap_or_default();
    let mut state = latest_users
        .iter()
        .rev()
        .find_map(|t| {
            let i = intent(t);
            if i.is_empty() { None } else { Some(i) }
        })
        .unwrap_or_default();
    let next = assistants
        .iter()
        .rev()
        .find_map(|t| {
            let s = next_step(t);
            if s.is_empty() { None } else { Some(s) }
        })
        .unwrap_or_default();
    let mut project = project_name(&cwd);
    if what.is_empty() && state.is_empty() {
        let headline = thread_headline(harness, session_id);
        if !headline.is_empty() {
            what = headline.clone();
            state = headline;
        } else if project != "Project" {
            what = project.clone();
            state = project.clone();
            project = "Project".to_string();
        } else {
            return String::new();
        }
    } else if what.is_empty() {
        what = state.clone();
    } else if state.is_empty() {
        state = what.clone();
    }
    if project != "Project" {
        what = format!("{}: {}", project, what);
    }
    // The marker and labels occupy six words; these limits leave 44 for content.
    let mut lines = vec![
        AUTO_MARKER.to_string(),
        format!("What: {}", word_limit(&what, 14)),
        format!("State: {}", word_limit(&state, 14)),
    ];
    if !next.is_empty() {
        lines.push(format!("Next: {}", word_limit(&next, 16)));
    }
    lines.join("\n") + "\n"
}

/// Write a deterministic fallback card, silently ignoring every hook error.
pub fn stop_hook(raw: &str, harness: &str) {
    let _ = stop_hook_inner(raw, harness);
}

fn stop_hook_inner(raw: &str, harness: &str) -> Option<()> {
    if std::env::var(SKIP_ENV).is_ok()
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
    let transcript_mtime = transcript.metadata().ok()?.mtime_nsec();
    let target = crate::cards::card_path(session_id);
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
            || current_stat.as_ref().map(|m| m.mtime_nsec()).unwrap_or(0) >= transcript_mtime)
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
        || latest_stat.mtime_nsec() != current_stat.as_ref().map(|m| m.mtime_nsec()).unwrap_or(0)
        || latest != current
    {
        return None;
    }
    let tmp = target.with_file_name(format!(".{}.{}.tmp", target.file_name()?.to_string_lossy(), std::process::id()));
    fs::write(&tmp, &content).ok()?;
    let still_ok = !target.is_symlink()
        && fs::read_to_string(&target).ok().as_deref() == Some(current.as_str())
        && target.metadata().ok().map(|m| m.mtime_nsec()) == current_stat.as_ref().map(|m| m.mtime_nsec());
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
    if std::env::var(SKIP_ENV).is_ok()
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

// ---- live delivery (UserPromptSubmit / PostToolUse hooks) ----

const EVENTS: &[&str] = &["UserPromptSubmit", "PostToolUse"];

/// Hook stdin JSON -> hook stdout JSON ("" to stay silent).
pub fn deliver_output(raw: &str, harness: &str) -> String {
    if std::env::var("EVERETT_SEND").is_ok() {
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
    let event = map.get("hook_event_name").and_then(|v| v.as_str()).unwrap_or("");
    if !EVENTS.contains(&event) {
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
    if !crate::inbox::path(session_id).map(|p| p.exists()).unwrap_or(false) {
        return String::new();
    }
    let text = crate::inbox::take(session_id);
    if text.is_empty() {
        return String::new();
    }
    serde_json::to_string(&json!({
        "hookSpecificOutput": {"hookEventName": event, "additionalContext": text}
    }))
    .unwrap_or_default()
}

/// One `everett hook <stem>` invocation: stdin JSON -> stdout. Always exit 0.
pub fn hook_main(stem: &str, args: &[String]) -> i32 {
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
                println!("{}", out);
            }
        }
        "codex_session_start" => {
            let out = session_start_output(&stdin_raw());
            if !out.is_empty() {
                println!("{}", out);
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
                print!("{}\n", out);
            }
        }
        "codex_inbox" => {
            let out = deliver_output(&stdin_raw(), "codex");
            if !out.is_empty() {
                print!("{}\n", out);
            }
        }
        "grok_stop" => grok_stop_hook(&stdin_raw()),
        "grok_inbox" => {
            let out = deliver_output(&stdin_raw(), "grok");
            if !out.is_empty() {
                print!("{}\n", out);
            }
        }
        "omp_inbox" => {
            if let Some(sid) = args.first() {
                if crate::inbox::valid_id(sid) {
                    crate::inbox::touch_live(sid, "omp", "turn");
                    let text = crate::inbox::take(sid);
                    if !text.is_empty() {
                        print!("{}", text);
                    }
                }
            }
        }
        "omp_card_context" => {
            if let Some(sid) = args.first() {
                let cwd = std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                let context = session_start_context(sid, &cwd);
                if !context.is_empty() {
                    print!("{}", context);
                }
            }
        }
        _ => {}
    }
    0
}
