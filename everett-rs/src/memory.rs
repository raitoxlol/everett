//! Everett glue over the Teresis memory store. Every `learn` also lands in a
//! shared `memory.db` so other Teresis clients read the same facts; `recall`
//! searches all of them, not just the compact injected core. All writes here are
//! best effort: a missing, disabled, or broken memory.db never blocks learning.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::error::{EverettError, Result};
use crate::paths::expanduser;
use crate::teresis::store::{MemoryStore, RecallOpts, RememberOpts};

fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

/// Where this process's memory.db lives, honoring EVERETT_MEMORY_DB / EVERETT_HOME.
pub fn db_path() -> Option<PathBuf> {
    resolve_db_path(|name| std::env::var(name).ok(), &home_dir())
}

fn resolve_db_path(get: impl Fn(&str) -> Option<String>, home: &Path) -> Option<PathBuf> {
    let value = |name: &str| get(name).filter(|v| !v.is_empty());
    if let Some(explicit) = value("EVERETT_MEMORY_DB") {
        if explicit.eq_ignore_ascii_case("off") {
            return None;
        }
        return Some(expanduser(&explicit));
    }
    if let Some(everett_home) = value("EVERETT_HOME") {
        return Some(
            expanduser(&everett_home)
                .join(".local/share/teresis/memory.db"),
        );
    }
    if let Some(teresis_db) = value("TERESIS_DB") {
        return Some(expanduser(&teresis_db));
    }
    if let Some(xdg) = value("XDG_DATA_HOME") {
        return Some(expanduser(&xdg).join("teresis/memory.db"));
    }
    Some(home.join(".local/share/teresis/memory.db"))
}

/// The Teresis scope for an Everett learning: `global` or `project/<slug>`.
pub fn scope_name(scope: &str, project: &str) -> String {
    if scope == "global" {
        "global".to_string()
    } else {
        format!("project/{project}")
    }
}

fn open_existing(path: &Path) -> Result<Option<MemoryStore>> {
    if !path.exists() {
        return Ok(None);
    }
    MemoryStore::open(path).map(Some)
}

fn remember(path: &Path, text: &str, opts: &RememberOpts) -> Value {
    match MemoryStore::open(path).and_then(|mut store| store.remember(text, opts)) {
        Ok(result) => json!({
            "db": path,
            "id": result.id,
            "status": result.status,
            "replaced": result.replaced,
            "redacted": result.redacted,
        }),
        Err(e) => json!({"db": path, "error": e.message}),
    }
}

/// Save one fact to memory.db. Best effort: Null when disabled, an `error` field
/// instead of an Err on failure.
pub fn record(
    text: &str,
    scope_name: &str,
    key: Option<&str>,
    ttl_seconds: Option<u64>,
    source: &str,
    meta: Value,
) -> Value {
    let Some(path) = db_path() else {
        return Value::Null;
    };
    remember(
        &path,
        text,
        &RememberOpts {
            scope: scope_name.to_string(),
            kind: "fact".to_string(),
            key: key.map(str::to_string),
            ttl_days: ttl_seconds.map(|s| s as f64 / 86400.0),
            source: Some(source.to_string()),
            meta: Some(meta),
            ..Default::default()
        },
    )
}

/// Soft-delete every active memory in `scope_name` whose content matches `text`.
pub fn forget(text: &str, scope_name: &str) -> usize {
    let Some(path) = db_path() else {
        return 0;
    };
    let Ok(Some(mut store)) = open_existing(&path) else {
        return 0;
    };
    // Stored content is scrubbed, so look up the scrubbed text.
    let text = crate::teresis::secrets::redact_secrets(text).0;
    let mut removed = 0;
    loop {
        match store.find(&text, Some(scope_name)) {
            Ok(Some(memory)) => match store.forget_id(&memory.id, false) {
                Ok(n) => removed += n,
                Err(_) => return 0,
            },
            Ok(None) => return removed,
            Err(_) => return 0,
        }
    }
}

/// Replace an existing memory for `old` in `scope_name`, or record `new` plainly.
pub fn correct(old: &str, new: &str, scope_name: &str, source: &str, meta: Value) -> Value {
    let Some(path) = db_path() else {
        return Value::Null;
    };
    let scrubbed = crate::teresis::secrets::redact_secrets(old).0;
    let existing = open_existing(&path)
        .ok()
        .flatten()
        .and_then(|store| store.find(&scrubbed, Some(scope_name)).ok().flatten());
    let (replaces, key) = existing
        .map(|memory| (Some(memory.id), memory.key))
        .unwrap_or((None, None));
    remember(
        &path,
        new,
        &RememberOpts {
            scope: scope_name.to_string(),
            kind: "fact".to_string(),
            key,
            source: Some(source.to_string()),
            meta: Some(meta),
            replaces,
            ..Default::default()
        },
    )
}

/// Search memory.db. Missing database reads as empty; disabled is an error.
pub fn recall(
    query: &str,
    project: Option<&str>,
    scope: Option<&str>,
    limit: usize,
    explain: bool,
) -> Result<Map<String, Value>> {
    let query = query.trim();
    if query.is_empty() {
        return Err(EverettError::new(2, "The query is empty."));
    }
    if query.chars().count() > crate::core::MAX_LEARNING {
        return Err(EverettError::new(
            2,
            "Search accepts at most 500 query characters.",
        ));
    }
    let path = db_path().ok_or_else(|| {
        EverettError::new(2, "Memory is off (EVERETT_MEMORY_DB=off).")
    })?;
    let (scopes, inherit) = match scope {
        Some("global") => (vec!["global".to_string()], false),
        Some(other) if other != "project" => {
            let _ = other;
            return Err(EverettError::new(2, "scope must be \"global\" or \"project\"."));
        }
        _ => {
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let project = project
                .map(crate::core::slug)
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| crate::core::project_for(&cwd));
            if !project.is_empty() {
                (vec![format!("project/{project}")], true)
            } else if scope == Some("project") {
                return Err(EverettError::new(
                    2,
                    "No project: pass --project, or run from inside a project folder.",
                ));
            } else {
                (vec!["global".to_string()], false)
            }
        }
    };
    let searched: Vec<String> = scopes
        .iter()
        .flat_map(|s| crate::teresis::store::scope_chain(s))
        .fold(Vec::new(), |mut acc, s| {
            if !acc.contains(&s) {
                acc.push(s);
            }
            acc
        });
    let memories = match open_existing(&path)? {
        Some(mut store) => store.recall(
            query,
            &RecallOpts {
                scope: Some(scopes),
                inherit,
                k: limit,
                explain,
                ..Default::default()
            },
        )?,
        None => Vec::new(),
    };
    let mut out = Map::new();
    out.insert("query".into(), json!(query));
    out.insert("scopes".into(), json!(searched));
    out.insert("db".into(), json!(path));
    out.insert(
        "memories".into(),
        json!(memories
            .iter()
            .map(|m| {
                let mut row = json!({
                    "id": m.id,
                    "scope": m.scope,
                    "kind": m.kind,
                    "key": m.key,
                    "content": m.content,
                    "tags": m.tags,
                    "source": m.source,
                    "score": m.score,
                    "created_at": m.created_at,
                    "updated_at": m.updated_at,
                    "expires_at": m.expires_at,
                });
                if explain {
                    row["why"] = m.why.clone().unwrap_or(Value::Null);
                }
                row
            })
            .collect::<Vec<_>>()),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_db_path_precedence() {
        let home = Path::new("/home/u");
        fn get(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| v.to_string())
            }
        }
        assert_eq!(
            resolve_db_path(get(&[("EVERETT_MEMORY_DB", "off")]), home),
            None
        );
        assert_eq!(
            resolve_db_path(get(&[("EVERETT_MEMORY_DB", "OFF")]), home),
            None
        );
        assert_eq!(
            resolve_db_path(get(&[("EVERETT_MEMORY_DB", "/x/m.db"), ("EVERETT_HOME", "/eh")]), home),
            Some(PathBuf::from("/x/m.db"))
        );
        assert_eq!(
            resolve_db_path(get(&[("EVERETT_MEMORY_DB", "~/mem.db")]), home),
            Some(expanduser("~/mem.db"))
        );
        assert_eq!(
            resolve_db_path(get(&[("EVERETT_HOME", "/eh"), ("TERESIS_DB", "/tdb")]), home),
            Some(PathBuf::from("/eh/.local/share/teresis/memory.db"))
        );
        assert_eq!(
            resolve_db_path(get(&[("TERESIS_DB", "/tdb"), ("XDG_DATA_HOME", "/xdg")]), home),
            Some(PathBuf::from("/tdb"))
        );
        assert_eq!(
            resolve_db_path(get(&[("XDG_DATA_HOME", "/xdg")]), home),
            Some(PathBuf::from("/xdg/teresis/memory.db"))
        );
        assert_eq!(
            resolve_db_path(get(&[]), home),
            Some(PathBuf::from("/home/u/.local/share/teresis/memory.db"))
        );
        // Empty values count as unset.
        assert_eq!(
            resolve_db_path(get(&[("EVERETT_MEMORY_DB", ""), ("EVERETT_HOME", "")]), home),
            Some(PathBuf::from("/home/u/.local/share/teresis/memory.db"))
        );
    }
}
