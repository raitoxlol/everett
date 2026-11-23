//! Private files must go through `storage::save`. Replacing a file with `fs::rename`
//! or `fs::copy` creates it under the caller's umask, which made inbox compaction
//! (#236), live records (#246) and a Codex config rewrite (#260) world-readable.
//!
//! Each non-test source file may use these calls no more often than the baseline.
//! New uses fail; removing one fails until the baseline is lowered, so it only shrinks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const CALLS: &[&str] = &["fs::rename(", "fs::copy("];
const BASELINE: &str = "tests/storage_discipline.baseline";

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Drop `#[cfg(test)]`-gated items so test code never counts. The attribute's
/// item ends at the first `;` outside braces (a `use` or `mod` declaration) or
/// when its braces balance (a `mod tests { ... }` or `fn` body). Splitting at
/// the first `#[cfg(test)]` would silently skip the rest of a file — gateway.rs
/// and relay/mod.rs keep almost all of their production code after one.
fn production_source(text: &str) -> String {
    let mut kept = String::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        let mut line = lines[i];
        i += 1;
        if !line.trim_start().starts_with("#[cfg(test)]") {
            kept.push_str(line);
            kept.push('\n');
            continue;
        }
        let mut depth = 0usize;
        let mut opened = false;
        let mut terminated = false;
        // The item may sit on the attribute's own line (`#[cfg(test)] mod tests;`),
        // so scan its remainder before consuming the next lines.
        let mut j = line.find("#[cfg(test)]").unwrap() + "#[cfg(test)]".len();
        loop {
            while j < line.len() && !terminated {
                match line.as_bytes()[j] {
                    b'{' => {
                        depth += 1;
                        opened = true;
                    }
                    b'}' => depth = depth.saturating_sub(1),
                    b';' if !opened && depth == 0 => terminated = true,
                    _ => {}
                }
                j += 1;
            }
            if terminated || (opened && depth == 0) {
                break;
            }
            if i >= lines.len() {
                break;
            }
            line = lines[i];
            i += 1;
            j = 0;
        }
    }
    kept
}

fn is_test_file(name: &str) -> bool {
    name == "tests.rs"
        || ["_tests.rs", "_model.rs", "_concurrency.rs", "_regressions.rs"].iter().any(|suffix| name.ends_with(suffix))
}

fn production_counts() -> BTreeMap<String, usize> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    let mut counts = BTreeMap::new();
    for file in files {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        if is_test_file(&name) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        let production = production_source(&text);
        let count: usize = CALLS.iter().map(|call| production.matches(call).count()).sum();
        if count > 0 {
            counts.insert(file.strip_prefix(root).unwrap().to_string_lossy().to_string(), count);
        }
    }
    counts
}

fn baseline() -> BTreeMap<String, usize> {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(BASELINE)).unwrap();
    text.lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(path, count)| (path.to_string(), count.parse().unwrap()))
        .collect()
}

#[test]
fn test_item_on_the_attribute_line_does_not_swallow_the_next_item() {
    let text = "#[cfg(test)] mod tests;\nfn write(path: &Path) {\n    fs::rename(path, path).unwrap();\n}\n";
    assert_eq!(1, CALLS.iter().map(|c| production_source(text).matches(c).count()).sum::<usize>());
}

#[test]
fn test_module_block_still_hides_its_calls() {
    let text = "#[cfg(test)]\nmod tests {\n    fn write() {\n        fs::rename(\"a\", \"b\").unwrap();\n    }\n}\n";
    assert_eq!(0, CALLS.iter().map(|c| production_source(text).matches(c).count()).sum::<usize>());
}

#[test]
fn replacing_files_stays_within_the_baseline() {
    let allowed = baseline();
    let found = production_counts();
    let mut problems = Vec::new();
    for (file, count) in &found {
        let limit = allowed.get(file).copied().unwrap_or(0);
        if *count > limit {
            problems.push(format!(
                "{file}: {count} fs::rename/fs::copy calls, baseline {limit}. Write private files with \
                 crate::storage::save(path, bytes) (0600 temp file, fsync, atomic persist) instead of creating \
                 them under the umask."
            ));
        }
    }
    for (file, limit) in &allowed {
        let count = found.get(file).copied().unwrap_or(0);
        if count < *limit {
            problems.push(format!("{file}: now {count}, baseline {limit}. Lower the line in {BASELINE}."));
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}
