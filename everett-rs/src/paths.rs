use std::fs;
use std::path::{Path, PathBuf};

fn expand_match(dir: &Path, segment: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| glob_match(n, segment))
                .unwrap_or(false)
        })
        .collect()
}

fn glob_match(name: &str, pattern: &str) -> bool {
    let n: Vec<char> = name.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    glob_match_at(&n, &p)
}

fn glob_match_at(name: &[char], pat: &[char]) -> bool {
    if pat.is_empty() {
        return name.is_empty();
    }
    if pat[0] == '*' {
        for i in 0..=name.len() {
            if glob_match_at(&name[i..], &pat[1..]) {
                return true;
            }
        }
        return false;
    }
    !name.is_empty() && pat[0] == name[0] && glob_match_at(&name[1..], &pat[1..])
}

/// Expand a path pattern whose segments are literals or `*` / `*.suffix` globs.
/// Mirrors `pathlib.Path.glob` for the patterns Everett uses.
pub fn glob(base: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut out = vec![base.to_path_buf()];
    for segment in pattern.split('/') {
        let mut next = Vec::new();
        for dir in &out {
            if segment.contains('*') {
                let mut matched = expand_match(dir, segment);
                matched.sort();
                next.extend(matched);
            } else {
                next.push(dir.join(segment));
            }
        }
        out = next;
    }
    out
}

pub fn expanduser(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        return crate::session::home().join(rest);
    }
    if path == "~" {
        return crate::session::home();
    }
    PathBuf::from(path)
}

pub fn realpath(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
