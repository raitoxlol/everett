use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CHUNK: usize = 64 * 1024;
pub const USER_WRAPPERS: &[&str] = &["<pasted_content"];

/// Real HTML/XML document tags a user might paste. Anything else starting with
/// '<' is treated as harness-injected (harnesses inject far more tags than we
/// can enumerate).
pub const PASTEABLE_TAGS: &[&str] = &[
    "!doctype", "?xml", "html", "head", "body", "div", "span", "p", "a", "img", "svg", "path",
    "script", "style", "link", "meta", "table", "tr", "td", "th", "ul", "ol", "li", "section",
    "article", "header", "footer", "nav", "main", "form", "input", "button", "label", "select",
    "textarea", "h1", "h2", "h3", "h4", "h5", "h6", "br", "hr", "pre", "code", "iframe", "video",
    "canvas", "template", "slot",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub harness: String,
    pub id: String,
    pub cwd: String,
    pub path: String,
    pub started: String,
    pub last_active: f64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub first_user: String,
    #[serde(default)]
    pub last_user: String,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub auto: bool,
    #[serde(default)]
    pub card: String,
    #[serde(default)]
    pub card_source: Option<String>,
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub source: String,
    #[serde(skip)]
    pub(crate) t3_thread_id: Option<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub state_kind: String,
}

impl Session {
    pub fn new(harness: &str, id: &str, cwd: &str, path: &str, started: &str, last_active: f64) -> Self {
        Session {
            harness: harness.into(),
            id: id.into(),
            cwd: cwd.into(),
            path: path.into(),
            started: started.into(),
            last_active,
            title: String::new(),
            first_user: String::new(),
            last_user: String::new(),
            running: false,
            auto: false,
            card: String::new(),
            card_source: None,
            profile: String::new(),
            source: String::new(),
            t3_thread_id: None,
            state: String::new(),
            state_kind: String::new(),
        }
    }

    pub fn to_dict(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// `Session(**dict)` — the router emits sessions as to_dict() maps.
    pub fn from_dict(d: &Map<String, Value>) -> Self {
        serde_json::from_value(Value::Object(d.clone()))
            .unwrap_or_else(|_| Session::new("", "", "", "", "", 0.0))
    }
}

pub fn warn(message: &str) {
    eprintln!("everett: {}", message);
}

/// Short name for where a session runs: its folder, or the repo behind a T3 or Claude worktree.
/// Codex app chat folders (`Codex/<date>/<prompt-slug>`) are not projects and read as "chat".
pub fn project_label(cwd: &str) -> String {
    project_in(cwd, &home()).unwrap_or_else(|label| label.to_string())
}

/// The project a session runs in; None for the home directory, a chat folder, or no folder.
pub fn project(cwd: &str) -> Option<String> {
    project_in(cwd, &home()).ok()
}

pub fn project_match_key(session: &Session) -> String {
    project(&session.cwd).unwrap_or_default().to_lowercase()
}

/// The project name, or the label shown for a folder that is not a project.
fn project_in(cwd: &str, home: &Path) -> Result<String, &'static str> {
    let path = Path::new(cwd.trim_end_matches('/'));
    if path.as_os_str().is_empty() {
        return Err("-");
    }
    if path == home {
        return Err("~");
    }
    let parts: Vec<&str> = path.iter().filter_map(|part| part.to_str()).collect();
    if let Some(worktree) = parts.windows(3).find(|w| w[0] == ".t3" && w[1] == "worktrees") {
        return Ok(worktree[2].to_string());
    }
    if let Some(i) = parts.windows(2).position(|w| w == [".claude", "worktrees"]) {
        if i > 0 {
            return Ok(parts[i - 1].to_string());
        }
    }
    let dated = |s: &str| {
        s.len() == 10
            && s.char_indices()
                .all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() })
    };
    let n = parts.len();
    if n >= 3 && parts[n - 3].eq_ignore_ascii_case("codex") && dated(parts[n - 2]) {
        return Err("chat");
    }
    Ok(parts[n - 1].to_string())
}

pub fn home() -> PathBuf {
    if let Ok(v) = std::env::var("EVERETT_HOME") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Ok(v) = std::env::var("HOME") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    PathBuf::from(".")
}

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn file_mtime(path: &Path) -> f64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// First-window rows, last-window rows.
pub type EdgeRows = (Vec<Map<String, Value>>, Vec<Map<String, Value>>);

pub fn read_edges(path: &Path) -> EdgeRows {
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let Ok(mut f) = fs::File::open(path) else {
        return (head, tail);
    };
    let size = f.metadata().map(|m| m.len() as usize).unwrap_or(0);
    let mut buf = vec![0u8; CHUNK.min(size.max(1))];
    let Ok(n) = f.read(&mut buf) else {
        return (head, tail);
    };
    let head_text = String::from_utf8_lossy(&buf[..n]);
    let mut head_lines: Vec<&str> = head_text.split('\n').collect();
    if size > CHUNK {
        // The last element is a partial line — unless the byte right after the
        // window is its newline, in which case it is a complete record.
        let boundary_newline = f
            .seek(SeekFrom::Start(CHUNK as u64))
            .and_then(|_| {
                let mut b = [0u8; 1];
                f.read(&mut b).map(|n| n == 1 && b[0] == b'\n')
            })
            .unwrap_or(false);
        if !(boundary_newline && head_lines.last().is_some_and(|l| !l.is_empty())) {
            head_lines.pop();
        }
    }
    for line in head_lines {
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line) {
            head.push(obj);
        }
    }
    if size <= CHUNK {
        return (head, tail);
    }
    let offset = CHUNK.max(size - CHUNK) as u64;
    let mut prev = [0u8; 1];
    let tail_partial = f
        .seek(SeekFrom::Start(offset - 1))
        .and_then(|_| f.read(&mut prev))
        .map(|n| n == 1 && prev[0] != b'\n')
        .unwrap_or(true);
    if f.seek(SeekFrom::Start(offset)).is_err() {
        return (head, tail);
    }
    let mut tbuf = Vec::new();
    if f.read_to_end(&mut tbuf).is_err() {
        return (head, tail);
    }
    let tail_text = String::from_utf8_lossy(&tbuf);
    let tail_lines: Vec<&str> = tail_text
        .split('\n')
        .skip(if tail_partial { 1 } else { 0 })
        .collect();
    for line in tail_lines {
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line) {
            tail.push(obj);
        }
    }
    (head, tail)
}

pub fn scan_full(path: &Path, limit_lines: usize) -> Vec<Map<String, Value>> {
    let mut rows = Vec::new();
    let Ok(text) = fs::read_to_string(path) else {
        return rows;
    };
    for line in text.lines().take(limit_lines) {
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line) {
            rows.push(obj);
        }
    }
    rows
}

/// Blank out the `candidates` matches that read as markup, leaving comparisons such as
/// `a<b+1 && c>d` alone: a tag name ends at whitespace, `/` or the closing bracket.
pub(crate) fn strip_tags<'a>(candidates: &Regex, text: &'a str) -> std::borrow::Cow<'a, str> {
    candidates.replace_all(text, |found: &regex::Captures| {
        let inner = &found[0][1..found[0].len() - 1];
        if inner.starts_with(['!', '?']) {
            return " ".to_string();
        }
        let inner = inner.strip_prefix('/').unwrap_or(inner);
        let rest = inner.trim_start_matches(|c: char| c.is_ascii_alphanumeric() || "_:.-".contains(c));
        let tag = inner.starts_with(|c: char| c.is_ascii_alphabetic())
            && (rest.is_empty() || rest == "/" || rest.starts_with(char::is_whitespace));
        if tag { " ".to_string() } else { found[0].to_string() }
    })
}

pub fn clean(text: &str, limit: usize) -> String {
    static TAG: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = TAG.get_or_init(|| Regex::new(r"<[A-Za-z/!?][^>]{0,79}>").unwrap());
    let no_tags = strip_tags(re, text);
    let collapsed: String = no_tags.split_whitespace().collect::<Vec<_>>().join(" ");
    let take: String = collapsed.chars().take(limit).collect();
    take
}

pub fn is_injected(text: &str) -> bool {
    let stripped = text.trim_start();
    if stripped.is_empty() {
        return true;
    }
    if USER_WRAPPERS.iter().any(|w| stripped.starts_with(w)) {
        return false;
    }
    let Some(rest) = stripped.strip_prefix('<') else { return false };
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(rest.len());
    if end == 0 {
        return true;
    }
    !PASTEABLE_TAGS.contains(&rest[..end].to_lowercase().as_str())
}

pub fn recent_files(paths: Vec<PathBuf>, since_hours: f64) -> Vec<PathBuf> {
    let cutoff = now() - since_hours * 3600.0;
    paths
        .into_iter()
        .filter(|p| file_mtime(p) >= cutoff)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_label_names_the_project_not_the_path() {
        let home = Path::new("/Users/w");
        for (cwd, want) in [
            ("", "-"),
            ("/", "-"),
            ("/Users/w", "~"),
            ("/Users/w/", "~"),
            ("/Users/w/Projects/everett", "everett"),
            ("/Users/w/.t3/worktrees/everett/t3-dcb3322b", "everett"),
            ("/Users/w/src/api/.claude/worktrees/fix-retry", "api"),
            (
                "/Users/w/Documents/Codex/2026-10-19/yo-my-hermes-agent-on-mynuc",
                "chat",
            ),
            ("/Users/w/notes/2026-10-19/standup", "standup"),
        ] {
            assert_eq!(project_in(cwd, home).unwrap_or_else(|label| label.to_string()), want, "{cwd}");
        }
    }

    #[test]
    fn read_edges_keeps_tail_line_at_record_boundary() {
        // Build a file whose tail window starts exactly on a record boundary:
        // the byte at offset-1 is '\n' and the tagged record begins there.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let tagged = serde_json::to_string(&serde_json::json!({"type": "user", "i": 999999})).unwrap();
        // offset = CHUNK.max(size - CHUNK), so the boundary must be past CHUNK.
        let start = CHUNK + tagged.len() + 1; // tail starts at `start` → size = start + CHUNK
        let first = {
            let base = serde_json::to_string(&serde_json::json!({"type": "user", "i": 0})).unwrap();
            format!("{}{}\n", " ".repeat(start - base.len() - 1), base)
        };
        let mut body = first + &tagged + "\n";
        let short = serde_json::to_string(&serde_json::json!({"type": "user", "i": 1})).unwrap();
        let need = start + CHUNK - body.len();
        assert!(need > short.len() + 1, "test setup: cannot pad to boundary");
        body.push_str(&" ".repeat(need - short.len() - 1));
        body.push_str(&short);
        body.push('\n');
        let bytes = body.into_bytes();
        assert_eq!(bytes.len(), start + CHUNK, "test setup: wrong size");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(bytes[start - 1], b'\n', "test setup: boundary not aligned");
        let (_, tail) = read_edges(&path);
        assert!(
            tail.first().and_then(|m| m.get("i")).and_then(|v| v.as_i64()) == Some(999999),
            "boundary record was dropped"
        );
    }

    #[test]
    fn read_edges_keeps_head_line_ending_at_window_boundary() {
        // The tagged record's last byte is at CHUNK-1 and its newline sits at
        // CHUNK: the head window ends on a complete record, not a partial line.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let tagged = serde_json::to_string(&serde_json::json!({"type": "user", "i": 999999})).unwrap();
        let rec0 = serde_json::to_string(&serde_json::json!({"type": "user", "i": 0})).unwrap();
        let mut bytes = format!("{rec0}\n").into_bytes();
        let pad = CHUNK - tagged.len() - bytes.len();
        bytes.extend(std::iter::repeat_n(b' ', pad));
        bytes.extend_from_slice(tagged.as_bytes());
        assert_eq!(bytes.len(), CHUNK, "test setup: tagged record must end at the boundary");
        bytes.push(b'\n');
        let tail_rec = serde_json::to_string(&serde_json::json!({"type": "user", "i": 1})).unwrap();
        bytes.extend_from_slice(tail_rec.as_bytes());
        bytes.push(b'\n');
        assert!(bytes.len() > CHUNK && bytes.len() <= 2 * CHUNK, "test setup: wrong size");
        std::fs::write(&path, &bytes).unwrap();
        let (head, tail) = read_edges(&path);
        assert!(head.iter().any(|m| m["i"] == 0), "first record missing: {head:?}");
        assert!(
            head.iter().any(|m| m["i"] == 999999),
            "boundary record dropped from head: {head:?}"
        );
        assert!(tail.iter().any(|m| m["i"] == 1), "tail record missing: {tail:?}");
    }
}
