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

pub fn read_edges(path: &Path) -> (Vec<Map<String, Value>>, Vec<Map<String, Value>>) {
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let Ok(mut f) = fs::File::open(path) else {
        return (head, tail);
    };
    let size = f.metadata().map(|m| m.len() as usize).unwrap_or(0);
    let mut buf = vec![0u8; CHUNK.min(size.max(1))];
    if f.read(&mut buf).is_err() {
        return (head, tail);
    }
    let head_text = String::from_utf8_lossy(&buf);
    let mut head_lines: Vec<&str> = head_text.split('\n').collect();
    if size > CHUNK {
        head_lines.pop();
    }
    for line in head_lines {
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line) {
            head.push(obj);
        }
    }
    if size <= CHUNK {
        return (head, tail);
    }
    if f.seek(SeekFrom::Start(CHUNK.max(size - CHUNK) as u64)).is_err() {
        return (head, tail);
    }
    let mut tbuf = Vec::new();
    if f.read_to_end(&mut tbuf).is_err() {
        return (head, tail);
    }
    let tail_text = String::from_utf8_lossy(&tbuf);
    let tail_lines: Vec<&str> = tail_text.split('\n').skip(1).collect();
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

pub fn clean(text: &str, limit: usize) -> String {
    static TAG: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = TAG.get_or_init(|| Regex::new(r"<[^>]{1,80}>").unwrap());
    let no_tags = re.replace_all(text, " ");
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
