use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use fs2::FileExt;
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::config;
use crate::error::{EverettError, Result};
use crate::paths::{expanduser, realpath};
use crate::proc::{run_capture, RunOutput};

/// Injectable runner for the merge LLM subprocess (tests substitute a fake).
type LlmRunner<'a> = dyn Fn(&[String], Option<&str>, &HashMap<String, String>, f64) -> std::result::Result<RunOutput, String> + 'a;
use crate::send::{caller_session_id, child_env, spawn_command};
use crate::session::{home, now};
use crate::timefmt::strftime_local;

pub const MAX_LEARNING: usize = 500;
pub const CORE_WORDS: usize = 300;
pub const CONTEXT_WORDS: usize = 350;
pub const LEARN_LINE: &str =
    "When you learn something other sessions should know (a decision, a convention, a gotcha), \
     run `everett learn \"<fact>\"` (add `--project <name>` for project-only facts).";

struct FileLock(std::fs::File);

fn locked(name: &str, blocking: bool) -> Result<FileLock> {
    let _ = fs::create_dir_all(core_dir());
    let f = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open(core_dir().join(name))
        .map_err(|e| EverettError::new(2, e.to_string()))?;
    let _ = f.set_permissions(PermissionsExt::from_mode(0o600));
    if blocking {
        f.lock_exclusive().map_err(|e| EverettError::new(2, e.to_string()))?;
    } else {
        f.try_lock_exclusive().map_err(|_| {
            EverettError::new(4, "A memory merge is already running. Try again after it finishes.")
        })?;
    }
    Ok(FileLock(f))
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

// ---- paths ----

pub fn core_dir() -> PathBuf {
    home().join(".everett").join("core")
}

pub fn inbox_path() -> PathBuf {
    core_dir().join("inbox.jsonl")
}

pub fn global_path() -> PathBuf {
    core_dir().join("core.md")
}

pub fn project_path(project: &str) -> PathBuf {
    core_dir().join("projects").join(format!("{}.md", slug(project)))
}

pub fn history_dir() -> PathBuf {
    core_dir().join("history")
}

fn slug_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[^a-z0-9._-]+").unwrap())
}

pub fn slug(name: &str) -> String {
    let lowered = name.trim().to_lowercase();
    let dashed = slug_re().replace_all(&lowered, "-");
    let trimmed = dashed.trim_matches(|c| c == '-' || c == '.');
    trimmed.chars().take(64).collect()
}

/// Project name for a directory: the enclosing git repo's folder, else the folder itself.
/// The home directory and / are not projects.
pub fn project_for(cwd: &str) -> String {
    if cwd.is_empty() {
        return String::new();
    }
    let path = realpath(&expanduser(cwd));
    let (home_dir, env_home) = (realpath(&home()), realpath(&dirs_home()));
    let is_stop = |p: &Path| -> bool { p == Path::new("/") || p == home_dir || p == env_home };
    if is_stop(&path) {
        return String::new();
    }
    let mut probe = path.as_path();
    for _ in 0..12 {
        if probe.join(".git").exists() {
            return probe
                .file_name()
                .map(|n| slug(&n.to_string_lossy()))
                .unwrap_or_default();
        }
        let parent = probe.parent().unwrap_or(probe);
        if parent == probe || is_stop(parent) {
            break;
        }
        probe = parent;
    }
    slug(&path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

// ---- secret filter ----

const SECRET_PATTERNS: &[(&str, &str)] = &[
    (r"-----BEGIN [A-Z ]*PRIVATE KEY-----", "a private key"),
    (r"\b(?:sk|pk|rk)-(?:ant-|proj-|live-|test-)?[A-Za-z0-9_-]{16,}", "an API key"),
    (r"\bgh[pousr]_[A-Za-z0-9]{20,}|\bgithub_pat_[A-Za-z0-9_]{20,}", "a GitHub token"),
    (r"\bxox[abposr]-[A-Za-z0-9-]{10,}", "a Slack token"),
    (r"\bAKIA[0-9A-Z]{16}\b", "an AWS access key"),
    (r"\bAIza[0-9A-Za-z_-]{30,}", "a Google API key"),
    (r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}", "a JWT"),
    (r"\b[Bb]earer\s+[A-Za-z0-9._~+/=-]{16,}", "a bearer token"),
    (
        r"(?i)\b(?:password|passwd|pwd|secret|api[_-]?key|access[_-]?token|auth[_-]?token|token)\b\s*[:=]\s*\S{6,}",
        "a credential assignment",
    ),
    (r"(?i)\b[a-z][a-z0-9+.-]*://[^/\s:@]+:[^/\s@]+@", "a URL with a password"),
];

fn secret_res() -> &'static Vec<Regex> {
    static RES: OnceLock<Vec<Regex>> = OnceLock::new();
    RES.get_or_init(|| SECRET_PATTERNS.iter().map(|(p, _)| Regex::new(p).unwrap()).collect())
}

fn entropy(token: &str) -> f64 {
    let mut counts: HashMap<char, usize> = HashMap::new();
    for c in token.chars() {
        *counts.entry(c).or_insert(0) += 1;
    }
    let n = token.chars().count() as f64;
    -counts
        .values()
        .map(|c| {
            let p = *c as f64 / n;
            p * p.log2()
        })
        .sum::<f64>()
}

/// Return what kind of secret the text seems to contain, or "".
pub fn find_secret(text: &str) -> String {
    for (i, (_, kind)) in SECRET_PATTERNS.iter().enumerate() {
        if secret_res()[i].is_match(text) {
            return kind.to_string();
        }
    }
    static TOKEN_RE: OnceLock<Regex> = OnceLock::new();
    let token_re = TOKEN_RE.get_or_init(|| Regex::new(r"[A-Za-z0-9_+/=.-]{24,}").unwrap());
    static LOWER_RE: OnceLock<Regex> = OnceLock::new();
    let lower = LOWER_RE.get_or_init(|| Regex::new(r"[a-z]").unwrap());
    static UPPER_RE: OnceLock<Regex> = OnceLock::new();
    let upper = UPPER_RE.get_or_init(|| Regex::new(r"[A-Z]").unwrap());
    static DIGIT_RE: OnceLock<Regex> = OnceLock::new();
    let digit = DIGIT_RE.get_or_init(|| Regex::new(r"[0-9]").unwrap());
    static PLAIN_RE: OnceLock<Regex> = OnceLock::new();
    let plain = PLAIN_RE.get_or_init(|| Regex::new(r"^[a-z0-9._/-]+$").unwrap());
    for m in token_re.find_iter(text) {
        let token = m.as_str();
        let classes = [lower, upper, digit].iter().filter(|re| re.is_match(token)).count();
        if classes >= 3 && entropy(token) >= 4.0 && !plain.is_match(token) {
            return "a high-entropy token".to_string();
        }
    }
    String::new()
}

// ---- push: learn ----

pub fn detect_harness() -> String {
    if std::env::var("CLAUDECODE").is_ok() || std::env::var("CLAUDE_CODE_ENTRYPOINT").is_ok() {
        return "claude".to_string();
    }
    for (k, _) in std::env::vars() {
        if k.starts_with("CODEX_") {
            return "codex".to_string();
        }
    }
    for (k, _) in std::env::vars() {
        if k.starts_with("OMP_") {
            return "omp".to_string();
        }
    }
    for (k, _) in std::env::vars() {
        if k.starts_with("HERMES_") {
            return "hermes".to_string();
        }
    }
    String::new()
}

/// Validate and append one learning to the inbox.
pub fn learn(text: &str, project: Option<&str>, scope: Option<&str>, cwd: Option<&str>) -> Result<Map<String, Value>> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err(EverettError::new(2, "The learning is empty."));
    }
    if text.chars().count() > MAX_LEARNING {
        return Err(EverettError::new(
            2,
            format!(
                "The learning is {} characters; the limit is {}. Write the one fact other sessions need.",
                text.chars().count(),
                MAX_LEARNING
            ),
        ));
    }
    let kind = find_secret(&text);
    if !kind.is_empty() {
        return Err(EverettError::new(
            2,
            format!("Rejected: this looks like it contains {}. Secrets never go into the shared core.", kind),
        ));
    }
    let cwd = cwd.map(|c| c.to_string()).unwrap_or_else(|| {
        std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
    });
    let scope = scope
        .map(|s| s.to_string())
        .unwrap_or_else(|| if project.is_some() { "project".to_string() } else { "global".to_string() });
    if scope != "global" && scope != "project" {
        return Err(EverettError::new(2, "scope must be \"global\" or \"project\"."));
    }
    let project = match project {
        Some(p) if !p.is_empty() => slug(p),
        _ => project_for(&cwd),
    };
    if scope == "project" && project.is_empty() {
        return Err(EverettError::new(
            2,
            "No project: pass --project, or run from inside a project folder.",
        ));
    }
    let mut entry = Map::new();
    entry.insert("ts".into(), json!(now()));
    entry.insert("session".into(), json!(caller_session_id()));
    entry.insert("harness".into(), json!(detect_harness()));
    entry.insert("cwd".into(), json!(cwd));
    entry.insert("project".into(), json!(project));
    entry.insert("scope".into(), json!(scope));
    entry.insert("text".into(), json!(text));
    let _guard = locked(".inbox.lock", true)?;
    let mut f = OpenOptions::new().create(true).append(true).open(inbox_path()).map_err(|e| {
        EverettError::new(2, e.to_string())
    })?;
    let _ = f.set_permissions(PermissionsExt::from_mode(0o600));
    f.write_all((serde_json::to_string(&entry).unwrap() + "\n").as_bytes())
        .map_err(|e| EverettError::new(2, e.to_string()))?;
    Ok(entry)
}

pub fn read_inbox() -> Vec<Map<String, Value>> {
    let Ok(raw) = fs::read(inbox_path()) else {
        return Vec::new();
    };
    parse_inbox(&raw)
}

fn parse_inbox(raw: &[u8]) -> Vec<Map<String, Value>> {
    let mut items = Vec::new();
    for line in raw.split(|b| *b == b'\n') {
        if let Ok(text) = std::str::from_utf8(line) {
            if let Ok(Value::Object(item)) = serde_json::from_str::<Value>(text) {
                if item.get("text").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false) {
                    items.push(item);
                }
            }
        }
    }
    items
}

// ---- merge ----

pub fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

fn bullets(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("- ") || l.starts_with("* "))
        .map(|l| l[2..].trim().to_string())
        .filter(|b| !b.is_empty())
        .collect()
}

fn norm_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[\W_]+").unwrap())
}

fn norm(text: &str) -> String {
    norm_re().replace_all(&text.to_lowercase(), " ").trim().to_string()
}

/// Markdown bullets under a title, dropping the oldest bullets until it fits the word cap.
fn render(title: &str, bullets: &[String], limit: usize) -> String {
    let mut bullets: Vec<String> = bullets.to_vec();
    while !bullets.is_empty()
        && words(&format!(
            "# {}\n{}",
            title,
            bullets.iter().map(|b| format!("- {}", b)).collect::<Vec<_>>().join("\n")
        )) > limit
    {
        bullets.remove(0);
    }
    format!("# {}\n\n{}", title, bullets.iter().map(|b| format!("- {}\n", b)).collect::<String>())
}

/// Enforce the word cap on LLM output: drop oldest bullets, else cut words.
fn cap(markdown: &str, limit: usize) -> String {
    if words(markdown) <= limit {
        return markdown.trim().to_string() + "\n";
    }
    let mut lines: Vec<String> = markdown.trim().lines().map(|l| l.to_string()).collect();
    while words(&lines.join("\n")) > limit
        && lines.iter().any(|l| l.trim_start().starts_with("- ") || l.trim_start().starts_with("* "))
    {
        let index = lines
            .iter()
            .position(|l| l.trim_start().starts_with("- ") || l.trim_start().starts_with("* "))
            .unwrap_or(0);
        lines.remove(index);
    }
    let text = lines.join("\n");
    if words(&text) > limit {
        let kept: Vec<&str> = text.split_whitespace().take(limit).collect();
        return kept.join(" ").trim().to_string() + "\n";
    }
    text.trim().to_string() + "\n"
}

pub fn read_file(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn title(project: &str) -> String {
    if project.is_empty() {
        "Everett core".to_string()
    } else {
        format!("Project core: {}", project)
    }
}

/// Deterministic merge: append new facts, drop exact/normalized duplicates, keep newest last.
pub fn merge_none(current: &HashMap<String, String>, items: &[Map<String, Value>]) -> HashMap<String, String> {
    let mut sorted: Vec<&Map<String, Value>> = items.iter().collect();
    sorted.sort_by(|a, b| {
        let ta = a.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let tb = b.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
        ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut grouped: HashMap<String, Vec<&Map<String, Value>>> = HashMap::new();
    for item in sorted {
        let key = if item.get("scope").and_then(|v| v.as_str()) == Some("project") {
            item.get("project").and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else {
            String::new()
        };
        grouped.entry(key).or_default().push(item);
    }
    let mut result = current.clone();
    for (key, group) in &grouped {
        let mut list = bullets(current.get(key).map(|s| s.as_str()).unwrap_or(""));
        for item in group {
            let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
            list.retain(|b| norm(b) != norm(text));
            list.push(text.to_string());
        }
        if !list.is_empty() {
            result.insert(key.clone(), render(&title(key), &list, CORE_WORDS));
        }
    }
    result
}

const MERGE_PROMPT: &str = r##"You maintain the shared core memory for many parallel coding-agent sessions.
Merge the NEW LEARNINGS into the CURRENT CORE files.
Rules:
- Keep only durable facts other sessions need: decisions, conventions, gotchas, paths, commands.
- Deduplicate. When facts contradict, the newer one wins; note it briefly, e.g. "(was: X)".
- Drop stale, one-off, or session-specific items.
- Each file is Markdown: one "# " title line, then "- " bullets. At most {limit} words per file.
- Never include secrets, tokens, or passwords.
Reply with ONLY a JSON object, no prose and no code fence:
{"global": "<markdown>", "projects": {"<project>": "<markdown>"}}
Include every project that has a core or a new learning.

CURRENT CORE:
{current}

NEW LEARNINGS (oldest first; scope and project given):
{items}
"##;

/// Slow path: a headless harness run (claude -p / codex exec) merges the core.
pub fn merge_llm(
    current: &HashMap<String, String>,
    items: &[Map<String, Value>],
    llm: &str,
    timeout: f64,
    runner: &LlmRunner<'_>,
) -> Result<HashMap<String, String>> {
    let mut ordered: Vec<(&String, &String)> = current.iter().collect();
    ordered.sort_by_key(|(k, _)| if k.is_empty() { (0, String::new()) } else { (1, (*k).clone()) });
    let shown = ordered
        .iter()
        .map(|(k, v)| {
            format!("## {}\n{}", if k.is_empty() { "global".to_string() } else { format!("project {}", k) }, v.trim())
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let shown = if shown.is_empty() { "(empty)".to_string() } else { shown };
    let mut sorted: Vec<&Map<String, Value>> = items.iter().collect();
    sorted.sort_by(|a, b| {
        let ta = a.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let tb = b.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
        ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
    });
    let lines = sorted
        .iter()
        .map(|i| {
            let scope = i.get("scope").and_then(|v| v.as_str()).unwrap_or("");
            let proj = if scope == "project" {
                format!(":{}", i.get("project").and_then(|v| v.as_str()).unwrap_or(""))
            } else {
                String::new()
            };
            format!("- [{}{}] {}", scope, proj, i.get("text").and_then(|v| v.as_str()).unwrap_or(""))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = MERGE_PROMPT
        .replace("{limit}", &CORE_WORDS.to_string())
        .replace("{current}", &shown)
        .replace("{items}", &lines);
    let work = tempfile::tempdir().map_err(|e| EverettError::new(5, format!("{} merge failed to run: {}", llm, e)))?;
    let out_file = if llm == "codex" {
        work.path().join("out.txt").to_string_lossy().to_string()
    } else {
        String::new()
    };
    let command = spawn_command(llm, &prompt, "", &out_file)?;
    let env = child_env(&HashMap::new());
    let result = runner(&command, Some(&work.path().to_string_lossy()), &env, timeout)
        .map_err(|e| EverettError::new(5, format!("{} merge failed to run: {}", llm, e)))?;
    if result.timed_out {
        return Err(EverettError::new(5, format!("{} merge failed to run: timed out", llm)));
    }
    if result.code != 0 {
        let detail: String = result
            .stderr
            .trim()
            .chars()
            .rev()
            .take(500)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return Err(EverettError::new(6, format!("{} merge exited {}: {}", llm, result.code, detail)));
    }
    let text = if !out_file.is_empty() {
        read_file(Path::new(&out_file))
    } else {
        String::new()
    };
    let text = if text.is_empty() { result.stdout.clone() } else { text };
    static BRACE_RE: OnceLock<Regex> = OnceLock::new();
    let brace = BRACE_RE.get_or_init(|| Regex::new(r"(?s)\{.*\}").unwrap());
    let data: Option<Map<String, Value>> = brace
        .find(&text)
        .and_then(|m| serde_json::from_str::<Value>(m.as_str()).ok())
        .and_then(|v| v.as_object().cloned());
    let valid = data
        .as_ref()
        .map(|d| d.get("global").map(|v| v.is_string()).unwrap_or(false) && d.get("projects").map(|v| v.is_object()).unwrap_or(true))
        .unwrap_or(false);
    if !valid {
        return Err(EverettError::new(
            6,
            format!("{} returned no valid JSON core; nothing was changed.", llm),
        ));
    }
    let data = data.unwrap();
    let mut merged: HashMap<String, String> = HashMap::new();
    merged.insert(String::new(), data.get("global").and_then(|v| v.as_str()).unwrap_or("").to_string());
    if let Some(projects) = data.get("projects").and_then(|v| v.as_object()) {
        for (name, body) in projects {
            if let Some(body) = body.as_str() {
                let name_slug = slug(name);
                if !name_slug.is_empty() {
                    merged.insert(name_slug, body.to_string());
                }
            }
        }
    }
    let mut out = HashMap::new();
    for (key, body) in merged {
        let secret = find_secret(&body);
        if !secret.is_empty() {
            return Err(EverettError::new(
                6,
                format!(
                    "{} output for {} looks like it contains a secret; nothing was changed.",
                    llm,
                    if key.is_empty() { "global" } else { &key }
                ),
            ));
        }
        let capped = if body.trim().is_empty() { String::new() } else { cap(&body, CORE_WORDS) };
        if !capped.is_empty() {
            out.insert(key, capped);
        }
    }
    Ok(out)
}

pub fn current_core() -> HashMap<String, String> {
    let mut core = HashMap::new();
    if global_path().exists() {
        core.insert(String::new(), read_file(&global_path()));
    }
    let mut paths = crate::paths::glob(&core_dir().join("projects"), "*.md");
    paths.sort();
    for path in paths {
        if let Some(stem) = path.file_stem() {
            core.insert(stem.to_string_lossy().to_string(), read_file(&path));
        }
    }
    core
}

fn write_file(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(".{}.tmp", path.file_name().unwrap_or_default().to_string_lossy()));
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    if !src.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Merge the inbox into the core.
pub fn merge(llm: &str, dry_run: bool) -> Result<Map<String, Value>> {
    merge_with(llm, dry_run, &|cmd, cwd, env, t| run_capture(cmd, cwd, env, t))
}

pub fn merge_with(
    llm: &str,
    dry_run: bool,
    runner: &LlmRunner<'_>,
) -> Result<Map<String, Value>> {
    let _merge_guard = if dry_run { None } else { Some(locked(".merge.lock", false)?) };
    let snapshot = if dry_run {
        fs::read(inbox_path()).unwrap_or_default()
    } else {
        let _inbox_guard = locked(".inbox.lock", true)?;
        fs::read(inbox_path()).unwrap_or_default()
    };
    let items = parse_inbox(&snapshot);
    if items.is_empty() {
        let mut r = Map::new();
        r.insert("merged".into(), json!(0));
        r.insert("files".into(), json!({}));
        r.insert("history".into(), Value::Null);
        return Ok(r);
    }
    let current = current_core();
    let result = match llm {
        "none" => merge_none(&current, &items),
        "claude" | "codex" => merge_llm(&current, &items, llm, 300.0, runner)?,
        _ => return Err(EverettError::new(2, "--llm must be claude, codex, or none.")),
    };
    let files: HashMap<String, String> = result
        .iter()
        .map(|(k, v)| {
            (
                if k.is_empty() { global_path() } else { project_path(k) }
                    .to_string_lossy()
                    .to_string(),
                v.clone(),
            )
        })
        .collect();
    if dry_run {
        let mut r = Map::new();
        r.insert("merged".into(), json!(items.len()));
        r.insert(
            "files".into(),
            serde_json::to_value(&files).unwrap_or(Value::Null),
        );
        r.insert("history".into(), Value::Null);
        r.insert("dry_run".into(), json!(true));
        return Ok(r);
    }
    let stamp = strftime_local("%Y%m%d-%H%M%S", now());
    let mut snap = history_dir().join(&stamp);
    let mut n = 1;
    while snap.exists() {
        snap = history_dir().join(format!("{}-{}", stamp, n));
        n += 1;
    }
    fs::create_dir_all(&snap).map_err(|e| EverettError::new(2, e.to_string()))?;
    if global_path().exists() {
        let _ = fs::copy(global_path(), snap.join("core.md"));
    }
    if core_dir().join("projects").is_dir() {
        let _ = copy_dir(&core_dir().join("projects"), &snap.join("projects"));
    }
    {
        let _inbox_guard = locked(".inbox.lock", true)?;
        let pending = fs::read(inbox_path()).unwrap_or_default();
        if !pending.starts_with(&snapshot) {
            return Err(EverettError::new(
                4,
                "Pending facts changed during the merge. Nothing was consumed; retry the merge.",
            ));
        }
        for (path, text) in &files {
            write_file(Path::new(path), text).map_err(|e| EverettError::new(2, e.to_string()))?;
        }
        fs::write(snap.join("inbox.jsonl"), &snapshot).map_err(|e| EverettError::new(2, e.to_string()))?;
        let _ = fs::set_permissions(snap.join("inbox.jsonl"), PermissionsExt::from_mode(0o600));
        let late = &pending[snapshot.len()..];
        if !late.is_empty() {
            let tmp = inbox_path().with_file_name(".inbox.jsonl.tmp");
            fs::write(&tmp, late).map_err(|e| EverettError::new(2, e.to_string()))?;
            let _ = fs::set_permissions(&tmp, PermissionsExt::from_mode(0o600));
            fs::rename(&tmp, inbox_path()).map_err(|e| EverettError::new(2, e.to_string()))?;
        } else {
            let _ = fs::remove_file(inbox_path());
        }
    }
    let mirror = write_vault_mirror(&result);
    let mut r = Map::new();
    r.insert("merged".into(), json!(items.len()));
    r.insert("files".into(), serde_json::to_value(&files).unwrap_or(Value::Null));
    r.insert("history".into(), json!(snap.to_string_lossy()));
    r.insert("mirror".into(), mirror.map(|p| json!(p.to_string_lossy())).unwrap_or(Value::Null));
    Ok(r)
}

pub fn write_vault_mirror(result: &HashMap<String, String>) -> Option<PathBuf> {
    let folder = crate::trunk::vault_folder()?;
    let mut parts: Vec<String> = vec![
        "---".into(),
        "title: Everett Core".into(),
        "source: everett trunk merge".into(),
        "tags: [everett, generated]".into(),
        "---".into(),
        "".into(),
        "Generated by `everett trunk merge`. Do not edit here; use `everett learn`.".into(),
        "".into(),
    ];
    if let Some(global) = result.get("") {
        parts.push(global.trim().to_string());
    }
    let mut keys: Vec<&String> = result.keys().filter(|k| !k.is_empty()).collect();
    keys.sort();
    for key in keys {
        parts.push(String::new());
        parts.push(result[key].trim().to_string());
    }
    let path = folder.join("Core.md");
    if write_file(&path, &(parts.join("\n") + "\n")).is_ok() {
        Some(path)
    } else {
        None
    }
}

// ---- pull: context for new sessions ----

fn trim(text: &str, budget: usize) -> String {
    if budget == 0 {
        return String::new();
    }
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() <= budget {
        return text.trim().to_string();
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut count = 0usize;
    for line in text.trim().lines() {
        let n = words(line);
        if count + n > budget {
            break;
        }
        kept.push(line);
        count += n;
    }
    kept.join("\n").trim().to_string()
}

/// Global core + this project's core, bounded to CONTEXT_WORDS in total, plus the learn line.
pub fn context(cwd: &str) -> String {
    let project = project_for(cwd);
    let project_text = if !project.is_empty() {
        read_file(&project_path(&project)).trim().to_string()
    } else {
        String::new()
    };
    let global_text = read_file(&global_path()).trim().to_string();
    let budget = CONTEXT_WORDS.saturating_sub(words(LEARN_LINE)).saturating_sub(12);
    let project_text = trim(&project_text, budget);
    let global_text = trim(&global_text, budget.saturating_sub(words(&project_text)));
    let body = [global_text, project_text]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let header = "Everett shared core (the same for every session; facts other sessions learned):";
    if body.is_empty() {
        LEARN_LINE.to_string()
    } else {
        format!("{}\n{}\n\n{}", header, body, LEARN_LINE)
    }
}

pub fn default_llm() -> String {
    config::get("merge_llm", Some("EVERETT_MERGE_LLM"), "claude")
}
