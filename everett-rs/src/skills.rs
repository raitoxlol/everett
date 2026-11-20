//! Skill hub: one namespace over every harness's SKILL.md playbooks.
//! Scans the well-known skill roots on this machine (Claude Code, Codex,
//! OMP/Pi-style `.agents`, Devin plugin caches, Hermes optional skills,
//! project-local skill dirs) plus extra configured roots, and serves them to
//! MCP clients as the `everett_skill` tool and as MCP prompts.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

/// Deepest SKILL.md below a root (plugin caches nest ~6 levels).
const MAX_DEPTH: usize = 10;
/// Bound on directory entries visited per scan, across all roots.
const MAX_ENTRIES: usize = 100_000;
/// Largest SKILL.md read into a tool result.
const MAX_CONTENT: usize = 128 * 1024;
/// Sibling files listed alongside a skill's content.
const MAX_FILES: usize = 50;
/// Rescan at most this often; skills rarely change mid-session.
const CACHE_TTL: Duration = Duration::from_secs(15);

const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", "__pycache__"];

#[derive(Clone, Debug)]
pub struct Skill {
    /// Frontmatter `name`, else the skill directory's name.
    pub name: String,
    pub description: String,
    /// Where the skill was found: claude, codex, agents, devin, hermes,
    /// claude-plugins, devin-plugins, project, extra.
    pub source: String,
    /// Absolute path of the SKILL.md file.
    pub path: PathBuf,
    /// The skill's directory (siblings of SKILL.md live here).
    pub dir: PathBuf,
    /// Hash of the instructions, so identical copies collapse to one.
    digest: u64,
    /// The identical copies that collapsed into this one: (source, SKILL.md path).
    pub copies: Vec<(String, PathBuf)>,
}

/// Lower rank wins when several sources provide the same skill name.
pub(crate) fn source_rank(source: &str) -> u8 {
    match source {
        "project" => 0,
        "extra" => 1,
        "agents" => 2,
        "claude" => 3,
        "codex" => 4,
        "devin" => 5,
        "claude-plugins" => 6,
        "devin-plugins" => 7,
        _ => 8,
    }
}

/// Skill folders the user keeps themselves. Only these are suggested or
/// shortlisted unasked; plugin caches and bundled catalogs need the skill named.
pub fn owned(source: &str) -> bool {
    source_rank(source) <= 5
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(crate::session::home)
}

/// (root, source label) pairs scanned recursively for SKILL.md files.
fn roots(cwd: &Path) -> Vec<(PathBuf, String)> {
    let home = home_dir();
    let mut out = Vec::new();
    for (rel, source) in [
        (".claude/skills", "claude"),
        (".codex/skills", "codex"),
        (".agents/skills", "agents"),
        (".config/devin/skills", "devin"),
        (".devin/skills", "devin"),
        (".claude/plugins", "claude-plugins"),
        (
            ".local/share/devin/cli/plugins_cache/cache",
            "devin-plugins",
        ),
        (".hermes/hermes-agent/optional-skills", "hermes"),
    ] {
        out.push((home.join(rel), source.to_string()));
    }
    // Extra roots, each walked like the built-ins: EVERETT_SKILL_DIRS, else
    // `[skills] dirs` in config.toml (same path-list syntax, `~/` allowed).
    let extra = crate::config::get("skills.dirs", Some("EVERETT_SKILL_DIRS"), "");
    for dir in std::env::split_paths(&extra) {
        let dir = match dir.strip_prefix("~") {
            Ok(rest) => home.join(rest),
            Err(_) => dir,
        };
        if dir.is_dir() {
            out.push((dir, "extra".to_string()));
        }
    }
    // Project-local skill dirs on the cwd's ancestor chain (repo root included).
    let mut dir = Some(cwd);
    for _ in 0..8 {
        let Some(d) = dir else { break };
        if d == home.as_path() {
            break;
        }
        for sub in [
            ".claude/skills",
            ".codex/skills",
            ".agents/skills",
            ".devin/skills",
            "skills",
        ] {
            out.push((d.join(sub), "project".to_string()));
        }
        dir = d.parent();
    }
    out
}

/// Parse the YAML-ish frontmatter of a SKILL.md: `name` and `description`.
/// Handles single-line values, quotes, and `>`/`|` block scalars.
fn frontmatter(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let mut name = None;
    let mut description = None;
    let mut pending: Option<String> = None;
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some(key) = pending.take() {
            // Continuation of a block scalar: indented lines belong to it.
            if line.starts_with(' ') || line.starts_with('\t') {
                let value = line.trim();
                if !value.is_empty() {
                    let entry = match key.as_str() {
                        "name" => &mut name,
                        "description" => &mut description,
                        _ => continue,
                    };
                    let joined = match entry {
                        Some(prev) => format!("{} {}", prev, value),
                        None => value.to_string(),
                    };
                    *entry = Some(joined);
                }
                pending = Some(key);
                continue;
            }
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if !matches!(key, "name" | "description") {
            continue;
        }
        let value = value.trim();
        if matches!(value, ">" | ">-" | ">+" | "|" | "|-" | "|+") {
            pending = Some(key.to_string());
            continue;
        }
        let value = value.trim_matches('"').trim_matches('\'').to_string();
        if value.is_empty() {
            continue;
        }
        match key {
            "name" => name = Some(value),
            _ => description = Some(value),
        }
    }
    (name, description)
}

fn fallback_description(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line == "---" {
            continue;
        }
        return line.chars().take(200).collect();
    }
    String::new()
}

fn load_skill(path: &Path, source: &str) -> Option<Skill> {
    let dir = path.parent()?.to_path_buf();
    let text = fs::read_to_string(path).ok()?;
    let (fm_name, fm_desc) = frontmatter(&text);
    let name = fm_name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
        dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    });
    if name.is_empty() {
        return None;
    }
    Some(Skill {
        name,
        description: fm_desc
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| fallback_description(&text)),
        source: source.to_string(),
        path: path.to_path_buf(),
        dir,
        digest: {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            strip_frontmatter(&text).hash(&mut hasher);
            hasher.finish()
        },
        copies: Vec::new(),
    })
}

/// Walk `root` for SKILL.md files. `visited` holds canonical dirs so
/// symlinks cannot loop.
fn walk(
    root: &Path,
    source: &str,
    visited: &mut HashSet<PathBuf>,
    out: &mut Vec<Skill>,
    budget: &mut usize,
) {
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH || *budget == 0 {
            continue;
        }
        let canon = fs::canonicalize(&dir).unwrap_or(dir.clone());
        if !visited.insert(canon) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if *budget == 0 {
                break;
            }
            *budget -= 1;
            let path = entry.path();
            let fname = entry.file_name();
            let fname = fname.to_string_lossy();
            // Follow symlinked dirs (skills are often symlinked) but the
            // canonical-dir set above keeps cycles finite.
            if path.is_dir() {
                if SKIP_DIRS.contains(&fname.as_ref()) {
                    continue;
                }
                stack.push((path, depth + 1));
            } else if fname.eq_ignore_ascii_case("skill.md") {
                if let Some(skill) = load_skill(&path, source) {
                    out.push(skill);
                }
            }
        }
    }
}

fn scan_uncached(cwd: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();
    let mut budget = MAX_ENTRIES;
    for (root, source) in roots(cwd) {
        // Per root, so a skill linked into several harness folders is seen in each.
        walk(&root, &source, &mut HashSet::new(), &mut skills, &mut budget);
    }
    skills.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(source_rank(&a.source).cmp(&source_rank(&b.source)))
            .then(a.path.cmp(&b.path))
    });
    // The same skill installed for several harnesses is one skill; the best source keeps it.
    let mut kept: Vec<Skill> = Vec::new();
    for skill in skills {
        match kept.iter_mut().rev().take_while(|k| k.name == skill.name).find(|k| k.digest == skill.digest) {
            Some(original) => original.copies.push((skill.source, skill.path)),
            None => kept.push(skill),
        }
    }
    kept
}

struct CacheEntry {
    home: PathBuf,
    cwd: PathBuf,
    at: Instant,
    skills: Vec<Skill>,
}

fn cache() -> &'static Mutex<Option<CacheEntry>> {
    static CACHE: OnceLock<Mutex<Option<CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Every skill visible from `cwd`, sorted by name then source priority.
pub fn scan(cwd: &Path) -> Vec<Skill> {
    let home = home_dir();
    let cwd = cwd.to_path_buf();
    if let Ok(guard) = cache().lock() {
        if let Some(entry) = guard.as_ref() {
            if entry.home == home && entry.cwd == cwd && entry.at.elapsed() < CACHE_TTL {
                return entry.skills.clone();
            }
        }
    }
    let skills = scan_uncached(&cwd);
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(CacheEntry { home, cwd, at: Instant::now(), skills: skills.clone() });
    }
    skills
}

/// All skills with this name (any source), best source first.
pub fn find(name: &str, source: Option<&str>) -> Vec<Skill> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut matches: Vec<Skill> = scan(&cwd)
        .into_iter()
        .filter(|s| {
            (s.name == name || s.name.eq_ignore_ascii_case(name))
                && source.is_none_or(|src| s.source == src || s.copies.iter().any(|(copy, _)| copy == src))
        })
        .collect();
    matches.sort_by(|a, b| {
        source_rank(&a.source)
            .cmp(&source_rank(&b.source))
            .then(a.path.cmp(&b.path))
    });
    matches
}

fn strip_frontmatter(text: &str) -> String {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return text.to_string();
    }
    let rest: Vec<&str> = lines.skip_while(|line| line.trim() != "---").skip(1).collect();
    rest.join("\n").trim().to_string()
}

/// SKILL.md text minus the YAML frontmatter block (for prompt bodies).
pub fn body(skill: &Skill) -> String {
    strip_frontmatter(&fs::read_to_string(&skill.path).unwrap_or_default())
}

/// Sibling files inside the skill directory, for the agent to read on demand.
pub fn files(skill: &Skill) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&skill.dir) else {
        return out;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_FILES {
            break;
        }
        let path = entry.path();
        if path.is_file() && path != skill.path {
            if let Ok(rel) = path.strip_prefix(&skill.dir) {
                out.push(rel.to_string_lossy().to_string());
            }
        }
    }
    out.sort();
    out
}

fn truncated(text: &str) -> String {
    if text.len() <= MAX_CONTENT {
        return text.to_string();
    }
    let mut end = MAX_CONTENT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…[truncated]", &text[..end])
}

pub fn to_dict(skill: &Skill) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("name".into(), json!(skill.name));
    m.insert("description".into(), json!(skill.description));
    m.insert("source".into(), json!(skill.source));
    m.insert("path".into(), json!(skill.path.to_string_lossy()));
    m
}

/// Skills matching `query` in name or description; name matches first
/// (exact, then partial) so `prove` is not buried under `approve`.
pub fn search(query: Option<&str>) -> Vec<Skill> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let skills = scan(&cwd);
    let Some(q) = query.map(str::to_lowercase).filter(|q| !q.is_empty()) else {
        return skills;
    };
    let mut ranked: Vec<(u8, Skill)> = skills
        .into_iter()
        .filter_map(|s| {
            let name = s.name.to_lowercase();
            let rank = if name == q {
                0
            } else if name.contains(&q) {
                1
            } else if s.description.to_lowercase().contains(&q) {
                2
            } else {
                return None;
            };
            Some((rank, s))
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, s)| s).collect()
}

/// `everett_skill` tool payloads.
pub fn list_result(query: Option<&str>) -> Map<String, Value> {
    let skills: Vec<Value> = search(query).iter().map(|s| Value::Object(to_dict(s))).collect();
    let count = skills.len();
    Map::from_iter([
        ("skills".into(), json!(skills)),
        ("count".into(), json!(count)),
    ])
}

/// The skill payload from the best source that has it. Other sources with a
/// different version are named in `also_in` for a `source`-qualified retry.
pub fn get_result(name: &str, source: Option<&str>) -> Map<String, Value> {
    let matches = find(name, source);
    let Some(skill) = matches.first() else {
        return Map::from_iter([("error".into(), json!(format!("no skill named \"{}\"", name)))]);
    };
    let mut m = payload(skill);
    if matches.len() > 1 {
        let others: Vec<&str> = matches[1..].iter().map(|s| s.source.as_str()).collect();
        m.insert("also_in".into(), json!(others));
    }
    m
}

/// A skill with its instructions and sibling files.
pub fn payload(skill: &Skill) -> Map<String, Value> {
    let mut m = to_dict(skill);
    m.insert("dir".into(), json!(skill.dir.to_string_lossy()));
    m.insert("files".into(), json!(files(skill)));
    m.insert("content".into(), json!(truncated(&body(skill))));
    m
}

/// MCP prompt name: [A-Za-z0-9_-], lowercased, collisions resolved by scan order.
pub fn prompt_name(skill: &Skill) -> Option<String> {
    let mut out = String::new();
    for ch in skill.name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() || out.len() > 64 {
        None
    } else {
        Some(out)
    }
}

/// Skills as MCP prompts, deduped by prompt name (best source wins).
pub fn prompts() -> Vec<(String, Skill)> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for skill in scan(&cwd) {
        if let Some(name) = prompt_name(&skill) {
            if seen.insert(name.clone()) {
                out.push((name, skill));
            }
        }
    }
    out
}

/// Resolve an MCP prompt name back to its skill.
pub fn prompt_skill(name: &str) -> Option<Skill> {
    prompts()
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, s)| s)
}
