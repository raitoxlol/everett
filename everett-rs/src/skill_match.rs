//! Picks the one skill a request needs, so no client has to read the catalog:
//! a skill the user named, else the best fit among the folders they own.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::skills::{self, Skill};

/// Skills listed in the MCP instructions of every session.
const SHORTLIST_MAX: usize = 8;
const SHORTLIST_DESCRIPTION: usize = 70;
/// A suggestion needs this many distinct request terms in the skill, carrying
/// this share of the request's weight, so small talk and long pastes stay quiet.
const MIN_HITS: usize = 2;
const MIN_SHARE: f64 = 0.45;
/// Function words that say nothing about which skill fits.
const FILLER: &[&str] = &[
    "about", "after", "all", "also", "any", "been", "but", "can", "could", "did", "do", "does", "else", "get",
    "had", "has", "have", "if", "into", "its", "just", "let", "like", "me", "might", "my", "need", "no", "not",
    "now", "other", "out", "please", "should", "so", "some", "than", "that", "then", "there", "these", "they",
    "want", "was", "what", "when", "where", "which", "who", "why", "will", "would", "you", "your",
];
/// Only the start of a long prompt is read for a skill.
const MAX_REQUEST: usize = 2000;

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

/// Crude suffix stripping: "crashing", "crashed" and "crashes" all meet at "crash".
fn stem(word: &str) -> String {
    let mut base = word;
    for suffixes in [&["s"][..], &["ing", "ed"], &["e"]] {
        if let Some(rest) = suffixes.iter().find_map(|suffix| base.strip_suffix(suffix)) {
            if rest.chars().count() >= 3 {
                base = rest;
            }
        }
    }
    base.to_string()
}

fn terms(text: &str) -> Vec<String> {
    crate::route::tokens(text)
        .iter()
        .filter(|t| !FILLER.contains(&t.as_str()))
        .map(|t| stem(t))
        .collect()
}

fn clipped(text: &str) -> &str {
    match text.char_indices().nth(MAX_REQUEST) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// The skill the user asked for by name. `/name` or "use the name skill"
/// asks for any skill and wins; for owned folders the name alone, or a
/// multi-word name said in passing ("full send"), also counts.
pub fn named(text: &str, all: &[Skill]) -> Option<Skill> {
    let text = clipped(text);
    let lowered = text.to_lowercase();
    let slashed: Vec<&str> = lowered
        .split_whitespace()
        .map(|token| token.trim_start_matches(['`', '"', '\'', '(', '“', '‘']).trim_end_matches(|c: char| !c.is_alphanumeric() && c != '/'))
        .collect();
    let said = words(text);
    // (explicit, name length): an explicit request beats a passing mention, then the longer name.
    let mut best: Option<((bool, usize), &Skill)> = None;
    for skill in all {
        let parts = words(&skill.name);
        if parts.is_empty() {
            continue;
        }
        let asked: Vec<&str> = ["use", "the"].into_iter().chain(parts.iter().map(String::as_str)).chain(["skill"]).collect();
        let explicit = slashed.contains(&format!("/{}", skill.name.to_lowercase()).as_str())
            || said.windows(asked.len()).any(|w| w == asked)
            || said.windows(asked.len() - 1).any(|w| w[0] == "use" && w[1..] == asked[2..]);
        let in_passing = skills::owned(&skill.source)
            && (said == parts || (parts.len() > 1 && said.windows(parts.len()).any(|w| w == &parts[..])));
        let rank = (explicit, parts.len());
        // `all` is sorted best source first, so a strict comparison keeps it.
        if (explicit || in_passing) && best.is_none_or(|(held, _)| rank > held) {
            best = Some((rank, skill));
        }
    }
    best.map(|(_, skill)| skill.clone())
}

/// The owned skill that best fits the request, or none when nothing clearly does.
///
/// Each request term weighs as much as it is rare among the skills, and a term
/// no skill knows weighs the most. A skill fits when its terms carry enough of
/// the request's weight, so two common words in a long description are not a fit. A term from the
/// skill's name counts twice.
pub fn suggest(text: &str, all: &[Skill]) -> Option<Skill> {
    let query: HashSet<String> = terms(clipped(text)).into_iter().collect();
    let mut names = HashSet::new();
    let docs: Vec<(&Skill, HashSet<String>, Vec<String>)> = all
        .iter()
        .filter(|s| skills::owned(&s.source) && names.insert(s.name.as_str()))
        .map(|s| (s, terms(&format!("{} {}", s.name, s.description)).into_iter().collect(), terms(&s.name)))
        .collect();
    let mut df: HashMap<&str, usize> = HashMap::new();
    for term in docs.iter().flat_map(|(_, doc, _)| doc) {
        *df.entry(term).or_default() += 1;
    }
    let weight = |term: &String| {
        let seen = df.get(term.as_str()).copied().unwrap_or(0) as f64;
        (1.0 + (docs.len() as f64 - seen + 0.5) / (seen + 0.5)).ln()
    };
    let total: f64 = query.iter().map(weight).sum();
    docs.iter()
        .filter_map(|(skill, doc, name)| {
            let hits: Vec<&String> = query.iter().filter(|t| doc.contains(*t)).collect();
            let carried: f64 = hits.iter().map(|t| weight(t) * if name.contains(t) { 2.0 } else { 1.0 }).sum();
            (hits.len() >= MIN_HITS && carried / total >= MIN_SHARE).then_some((carried, *skill))
        })
        // Equal fits go to the better source, as `get` would.
        .max_by(|a, b| a.0.total_cmp(&b.0).then(skills::source_rank(&b.1.source).cmp(&skills::source_rank(&a.1.source))))
        .map(|(_, skill)| skill.clone())
}

/// `{"skill": <payload or null>}` for a task: one skill, never a list.
pub fn match_result(task: &str, cwd: &Path) -> Map<String, Value> {
    let all = skills::scan(cwd);
    let (found, how) = match named(task, &all) {
        Some(skill) => (Some(skill), "named"),
        None => (suggest(task, &all), "suggested"),
    };
    let Some(skill) = found else {
        return Map::from_iter([("skill".into(), Value::Null)]);
    };
    // A guess that happened to fit is not use; counting it would promote wrong guesses.
    if how == "named" {
        record_use(cwd, &skill.name);
    }
    Map::from_iter([
        ("skill".into(), Value::Object(skills::payload(&skill))),
        ("matched".into(), json!(how)),
    ])
}

fn state_dir() -> PathBuf {
    crate::session::home().join(".everett").join("skills")
}

/// Usage is counted per project: the nearest git root, else the directory itself.
fn project_key(cwd: &Path) -> String {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(cwd)
        .to_string_lossy()
        .to_string()
}

fn usage() -> HashMap<String, HashMap<String, u64>> {
    fs::read_to_string(state_dir().join("usage.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Count a skill as used in this project. A failed write only changes shortlist order.
pub fn record_use(cwd: &Path, name: &str) {
    let dir = state_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(_lock) = crate::storage::lock(&dir.join("usage.lock")) else {
        return;
    };
    let mut all = usage();
    *all.entry(project_key(cwd)).or_default().entry(name.to_string()).or_default() += 1;
    let _ = crate::storage::save(&dir.join("usage.json"), &serde_json::to_vec(&all).unwrap_or_default());
}

/// The few skills worth naming up front in this project: the ones used here
/// most, then the project's own, then the user's extra folders.
pub fn shortlist(cwd: &Path) -> Vec<Skill> {
    let used = usage().remove(&project_key(cwd)).unwrap_or_default();
    let mut names = HashSet::new();
    let mut ranked: Vec<(u64, u8, Skill)> = skills::scan(cwd)
        .into_iter()
        .filter(|s| skills::owned(&s.source) && names.insert(s.name.clone()))
        .filter_map(|s| {
            let count = used.get(&s.name).copied().unwrap_or(0);
            let tier = match s.source.as_str() {
                "project" => 0,
                "extra" => 1,
                _ if count > 0 => 2,
                _ => return None,
            };
            Some((count, tier, s))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.name.cmp(&b.2.name)));
    ranked.into_iter().take(SHORTLIST_MAX).map(|(_, _, s)| s).collect()
}

fn brief(description: &str, limit: usize) -> String {
    let line = description.lines().next().unwrap_or("").trim();
    if line.chars().count() <= limit {
        return line.to_string();
    }
    format!("{}…", line.chars().take(limit).collect::<String>().trim_end())
}

/// The shortlist as a paragraph for the MCP instructions; empty with no skills.
pub fn shortlist_text(cwd: &Path) -> String {
    let skills = shortlist(cwd);
    if skills.is_empty() {
        return String::new();
    }
    let mut text = String::from("Skills for this project (load one with everett_skill get when it fits):");
    for skill in &skills {
        text.push_str(&format!("\n- `{}`: {}", skill.name, brief(&skill.description, SHORTLIST_DESCRIPTION)));
    }
    text
}

/// Whether `harness` loads this skill itself, going by the skill folder it
/// lives in. Plugin marketplaces are not counted: they hold skills that were
/// never installed.
fn native_to(skill: &Skill, harness: &str) -> bool {
    let homes: &[&str] = match harness {
        "claude" => &[".claude"],
        "codex" => &[".codex", ".agents"],
        _ => &[],
    };
    std::iter::once(&skill.path).chain(skill.copies.iter().map(|(_, path)| path)).any(|path| {
        let parts: Vec<_> = path.components().map(|part| part.as_os_str()).collect();
        parts.windows(2).any(|pair| homes.iter().any(|home| pair[0] == *home) && pair[1] == "skills")
    })
}

/// What a prompt hook adds for this prompt: the full skill when the user
/// named one, one line when a skill merely fits, nothing otherwise. Each
/// skill is suggested once per session. A skill the harness loads itself,
/// under that name or as a linked copy, is left to it, so nothing lands twice.
pub fn hook_context(session_id: &str, harness: &str, prompt: &str, cwd: &Path) -> Option<String> {
    let all = skills::scan(cwd);
    let native = |skill: &Skill| all.iter().any(|s| s.name == skill.name && native_to(s, harness));
    if let Some(skill) = named(prompt, &all) {
        if native(&skill) {
            return None;
        }
        record_use(cwd, &skill.name);
        return Some(format!(
            "The user asked for the `{}` skill. Follow these instructions (skill folder: {}):\n\n{}",
            skill.name,
            skill.dir.display(),
            skills::payload(&skill)["content"].as_str().unwrap_or_default()
        ));
    }
    let skill = suggest(prompt, &all).filter(|skill| !native(skill))?;
    let hinted = state_dir().join("hinted").join(session_id);
    let before = fs::read_to_string(&hinted).unwrap_or_default();
    if before.lines().any(|name| name == skill.name) {
        return None;
    }
    if let Some(dir) = hinted.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(&hinted, format!("{before}{}\n", skill.name));
    Some(format!(
        "Everett: the `{}` skill may fit this request ({}). Load it with everett_skill get if useful.",
        skill.name,
        brief(&skill.description, 120)
    ))
}
