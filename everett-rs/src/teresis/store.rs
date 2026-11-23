//! The memory store: one SQLite file, safe for many concurrent agent processes.
//! Port of teresis/store.py — same file format, same write/read semantics.
//! Not ported (Everett has no embedder): the vector channel, reranker, MMR,
//! `update`, `feedback`, and everything below `feedback` in store.py.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params_from_iter, Connection};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::error::{EverettError, Result};

use super::secrets;
use super::temporal;
use super::text as t;

pub const SCHEMA_VERSION: i64 = 1;
pub const DAY: f64 = 86400.0;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS memories (
    id            TEXT PRIMARY KEY,
    scope         TEXT NOT NULL,
    kind          TEXT NOT NULL DEFAULT 'fact',
    key           TEXT,
    content       TEXT NOT NULL,
    tags          TEXT NOT NULL DEFAULT '',
    meta          TEXT NOT NULL DEFAULT '{}',
    source        TEXT,
    importance    REAL NOT NULL DEFAULT 0.5,
    pinned        INTEGER NOT NULL DEFAULT 0,
    content_hash  TEXT NOT NULL,
    created_at    REAL NOT NULL,
    updated_at    REAL NOT NULL,
    accessed_at   REAL,
    access_count  INTEGER NOT NULL DEFAULT 0,
    expires_at    REAL,
    superseded_by TEXT,
    superseded_at REAL,
    deleted_at    REAL
);
CREATE INDEX IF NOT EXISTS ix_mem_scope_active ON memories(scope) WHERE superseded_by IS NULL AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS ix_mem_key ON memories(scope, key) WHERE key IS NOT NULL;
CREATE INDEX IF NOT EXISTS ix_mem_hash ON memories(scope, content_hash);

CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    content, tags, key, content='memories', content_rowid='rowid',
    tokenize='porter unicode61 remove_diacritics 2'
);
CREATE VIRTUAL TABLE IF NOT EXISTS memories_tri USING fts5(
    content, content='memories', content_rowid='rowid', tokenize='trigram'
);

CREATE TRIGGER IF NOT EXISTS mem_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content, tags, key) VALUES (new.rowid, new.content, new.tags, coalesce(new.key, ''));
    INSERT INTO memories_tri(rowid, content) VALUES (new.rowid, new.content);
END;
CREATE TRIGGER IF NOT EXISTS mem_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content, tags, key) VALUES ('delete', old.rowid, old.content, old.tags, coalesce(old.key, ''));
    INSERT INTO memories_tri(memories_tri, rowid, content) VALUES ('delete', old.rowid, old.content);
END;
CREATE TRIGGER IF NOT EXISTS mem_au AFTER UPDATE OF content, tags, key ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content, tags, key) VALUES ('delete', old.rowid, old.content, old.tags, coalesce(old.key, ''));
    INSERT INTO memories_tri(memories_tri, rowid, content) VALUES ('delete', old.rowid, old.content);
    INSERT INTO memories_fts(rowid, content, tags, key) VALUES (new.rowid, new.content, new.tags, coalesce(new.key, ''));
    INSERT INTO memories_tri(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TABLE IF NOT EXISTS vectors (
    memory_id TEXT PRIMARY KEY REFERENCES memories(id) ON DELETE CASCADE,
    model     TEXT NOT NULL,
    vec       BLOB NOT NULL
);
"#;

fn err(e: impl std::fmt::Display) -> EverettError {
    EverettError::new(2, format!("Memory store: {e}"))
}

/// `'project/teresis/agent-a' -> ['project/teresis/agent-a', 'project/teresis', 'project', 'global']`.
pub fn scope_chain(scope: &str) -> Vec<String> {
    let parts: Vec<&str> = scope.trim_matches('/').split('/').filter(|p| !p.is_empty()).collect();
    let mut chain: Vec<String> = (1..=parts.len())
        .rev()
        .map(|i| parts[..i].join("/"))
        .collect();
    if !chain.iter().any(|c| c == "global") {
        chain.push("global".to_string());
    }
    chain
}

#[derive(Debug, Clone, Serialize)]
pub struct Memory {
    pub id: String,
    pub scope: String,
    pub kind: String,
    pub content: String,
    pub key: Option<String>,
    pub tags: Vec<String>,
    pub meta: Value,
    pub source: Option<String>,
    pub importance: f64,
    pub pinned: bool,
    pub created_at: f64,
    pub updated_at: f64,
    pub accessed_at: Option<f64>,
    pub access_count: i64,
    pub expires_at: Option<f64>,
    pub superseded_by: Option<String>,
    pub superseded_at: Option<f64>,
    pub deleted_at: Option<f64>,
    pub score: Option<f64>,
    pub why: Option<Value>,
}

fn memory_from_row(row: &rusqlite::Row) -> rusqlite::Result<Memory> {
    let tags: String = row.get("tags")?;
    let meta: String = row.get("meta")?;
    Ok(Memory {
        id: row.get("id")?,
        scope: row.get("scope")?,
        kind: row.get("kind")?,
        content: row.get("content")?,
        key: row.get("key")?,
        tags: tags.split_whitespace().map(|s| s.to_string()).collect(),
        meta: serde_json::from_str(&meta).unwrap_or_else(|_| json!({})),
        source: row.get("source")?,
        importance: row.get("importance")?,
        pinned: row.get::<_, i64>("pinned")? != 0,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        accessed_at: row.get("accessed_at")?,
        access_count: row.get("access_count")?,
        expires_at: row.get("expires_at")?,
        superseded_by: row.get("superseded_by")?,
        superseded_at: row.get("superseded_at")?,
        deleted_at: row.get("deleted_at")?,
        score: None,
        why: None,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct RememberResult {
    pub id: String,
    /// created | duplicate | superseded | refined
    pub status: String,
    pub replaced: Vec<String>,
    pub redacted: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RememberOpts {
    pub scope: String,
    pub kind: String,
    pub key: Option<String>,
    pub tags: Vec<String>,
    pub importance: f64,
    pub pinned: bool,
    pub ttl_days: Option<f64>,
    pub source: Option<String>,
    pub meta: Option<Value>,
    pub replaces: Option<String>,
    pub scrub_secrets: bool,
    /// Everett mirror writes disable near-duplicate fuzzy dedupe, so distinct
    /// facts (negations, temporary refinements of permanent facts) each get a row.
    pub fuzzy_dedupe: bool,
    /// Everett mirror writes: an exact-duplicate relearn updates the stored
    /// expiry to the new TTL (NULL promotes to permanent).
    pub update_expiry_on_duplicate: bool,
}

impl Default for RememberOpts {
    fn default() -> Self {
        RememberOpts {
            scope: "global".to_string(),
            kind: "fact".to_string(),
            key: None,
            tags: Vec::new(),
            importance: 0.5,
            pinned: false,
            ttl_days: None,
            source: None,
            meta: None,
            replaces: None,
            scrub_secrets: true,
            fuzzy_dedupe: true,
            update_expiry_on_duplicate: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecallOpts {
    pub scope: Option<Vec<String>>,
    pub inherit: bool,
    pub kinds: Vec<String>,
    pub exclude_kinds: Vec<String>,
    pub tags: Vec<String>,
    pub k: usize,
    pub budget_tokens: Option<usize>,
    pub as_of: Option<f64>,
    pub touch: bool,
    pub explain: bool,
}

impl Default for RecallOpts {
    fn default() -> Self {
        RecallOpts {
            scope: None,
            inherit: true,
            kinds: Vec::new(),
            exclude_kinds: Vec::new(),
            tags: Vec::new(),
            k: 8,
            budget_tokens: None,
            as_of: None,
            touch: true,
            explain: false,
        }
    }
}

/// Retrieval tuning. Defaults match Python `Weights`.
#[derive(Debug, Clone)]
pub struct Weights {
    pub fts: f64,
    pub trigram: f64,
    pub vector: f64,
    pub candidates: usize,
    pub half_life_days: f64,
    pub recency: f64,
    pub temporal: f64,
    pub importance: f64,
    pub access: f64,
    pub pinned_boost: f64,
    pub min_trigram_overlap: f64,
    pub min_cosine: f64,
    pub sub_query: f64,
    pub dup_jaccard: f64,
    pub refine_containment: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Weights {
            fts: 1.0,
            trigram: 0.35,
            vector: 2.0,
            candidates: 60,
            half_life_days: 30.0,
            recency: 0.15,
            temporal: 0.25,
            importance: 0.2,
            access: 0.02,
            pinned_boost: 1.05,
            min_trigram_overlap: 0.5,
            min_cosine: 0.0,
            sub_query: 0.95,
            dup_jaccard: 0.9,
            refine_containment: 0.9,
        }
    }
}

type Param = rusqlite::types::Value;

/// (id, key, content) of a candidate memory row.
type DupRow = (String, Option<String>, String);

/// `json.dumps(meta)` with CPython's default separators and `ensure_ascii`.
fn py_dumps(v: &Value) -> String {
    fn esc(s: &str, out: &mut String) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                c if (c as u32) < 0x20 => {
                    out.push_str(&format!("\\u{:04x}", c as u32));
                }
                c if (0x20..0x7f).contains(&(c as u32)) => out.push(c),
                c => {
                    let mut buf = [0u16; 2];
                    for u in c.encode_utf16(&mut buf) {
                        out.push_str(&format!("\\u{:04x}", u));
                    }
                }
            }
        }
        out.push('"');
    }
    fn go(v: &Value, out: &mut String) {
        match v {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => out.push_str(&n.to_string()),
            Value::String(s) => esc(s, out),
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    go(item, out);
                }
                out.push(']');
            }
            Value::Object(map) => {
                out.push('{');
                for (i, (k, val)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    esc(k, out);
                    out.push_str(": ");
                    go(val, out);
                }
                out.push('}');
            }
        }
    }
    let mut out = String::new();
    go(v, &mut out);
    out
}

pub struct MemoryStore {
    pub path: PathBuf,
    conn: Connection,
    w: Weights,
    clock: Box<dyn Fn() -> f64 + Send>,
}

/// Create the empty file SQLite initializes, tolerating a lost create race.
fn create_empty_db(path: &Path) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(err(e)),
    }
}

impl MemoryStore {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_clock(path, Box::new(crate::session::now))
    }

    pub fn open_with_clock(path: &Path, clock: Box<dyn Fn() -> f64 + Send>) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(err)?;
        }
        if !path.exists() {
            create_empty_db(path)?;
        }
        let conn = Connection::open(path).map_err(err)?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(err)?;
        if version > SCHEMA_VERSION {
            return Err(err(format!(
                "memory.db has schema version {version}; this Everett supports {SCHEMA_VERSION}"
            )));
        }
        conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0))
            .map_err(err)?;
        conn.pragma_update(None, "synchronous", "NORMAL").map_err(err)?;
        conn.pragma_update(None, "foreign_keys", "ON").map_err(err)?;
        conn.busy_timeout(Duration::from_millis(5000)).map_err(err)?;
        conn.execute_batch(&format!(
            "BEGIN IMMEDIATE;{SCHEMA}PRAGMA user_version={SCHEMA_VERSION};COMMIT;"
        ))
        .map_err(err)?;
        Ok(MemoryStore {
            path: path.to_path_buf(),
            conn,
            w: Weights::default(),
            clock,
        })
    }

    // ------------------------------------------------------------------ infra
    fn now(&self) -> f64 {
        (self.clock)()
    }

    /// `BEGIN IMMEDIATE` write transaction: `COMMIT` on success, `ROLLBACK` on error.
    fn write_tx<T>(&mut self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN IMMEDIATE").map_err(err)?;
        match f(&self.conn) {
            Ok(v) => {
                self.conn.execute_batch("COMMIT").map_err(err)?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    fn q(&self, sql: &str, params: Vec<Param>) -> Result<Vec<Memory>> {
        let mut stmt = self.conn.prepare(sql).map_err(err)?;
        let rows = stmt
            .query_map(params_from_iter(params.iter()), memory_from_row)
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        Ok(rows)
    }

    fn active_sql(&self, alias: &str, as_of: Option<f64>) -> (String, Vec<Param>) {
        match as_of {
            None => {
                let now = self.now();
                (
                    format!(
                        "{alias}.superseded_by IS NULL AND {alias}.deleted_at IS NULL \
                         AND ({alias}.expires_at IS NULL OR {alias}.expires_at > ?)"
                    ),
                    vec![Param::Real(now)],
                )
            }
            Some(as_of) => (
                format!(
                    "{alias}.created_at <= ? AND ({alias}.superseded_at IS NULL OR {alias}.superseded_at > ?) \
                     AND ({alias}.deleted_at IS NULL OR {alias}.deleted_at > ?)"
                ),
                vec![Param::Real(as_of), Param::Real(as_of), Param::Real(as_of)],
            ),
        }
    }

    // ------------------------------------------------------------------ write
    /// Store a memory. Dedupes exact/near repeats and supersedes stale values.
    pub fn remember(&mut self, content: &str, opts: &RememberOpts) -> Result<RememberResult> {
        let mut content = content.trim().to_string();
        if content.is_empty() {
            return Err(err("content must be non-empty"));
        }
        let mut redacted: Vec<String> = Vec::new();
        if opts.scrub_secrets {
            let (scrubbed, kinds) = secrets::redact_secrets(&content);
            content = scrubbed;
            redacted = kinds;
        }
        let scope = {
            let s = opts.scope.trim_matches('/');
            if s.is_empty() { "global".to_string() } else { s.to_string() }
        };
        // Python min(1.0, max(0.0, x)): NaN sinks to 0.0
        let importance = if opts.importance.is_nan() {
            0.0
        } else {
            opts.importance.clamp(0.0, 1.0)
        };
        let tag_str = {
            let mut seen: Vec<String> = Vec::new();
            for tag in &opts.tags {
                let cleaned = tag.trim().to_lowercase().replace(' ', "-");
                if !tag.trim().is_empty() && !seen.contains(&cleaned) {
                    seen.push(cleaned);
                }
            }
            seen.join(" ")
        };
        let h = t::content_hash(&content);
        let now = self.now();
        let (active, active_params) = self.active_sql("m", None);
        let key = opts.key.clone();
        let replaces_opt = opts.replaces.clone();
        let kind = opts.kind.clone();
        let meta = py_dumps(&opts.meta.clone().unwrap_or_else(|| json!({})));
        let source = opts.source.clone();
        let pinned = opts.pinned;
        let expires_at = match opts.ttl_days {
            Some(ttl) if ttl != 0.0 => Some(now + ttl * DAY),
            _ => None,
        };

        let mut refined = false;
        let w = self.w.clone();
        self.write_tx(|c| {
            let dup: Option<DupRow> = {
                let sql = format!(
                    "SELECT * FROM memories m WHERE scope=? AND content_hash=? AND {active}"
                );
                let mut params = vec![Param::Text(scope.clone()), Param::Text(h.clone())];
                params.extend(active_params.iter().cloned());
                let mut stmt = c.prepare(&sql).map_err(err)?;
                let mut rows = stmt
                    .query_map(params_from_iter(params.iter()), |r| {
                        Ok((r.get::<_, String>("id")?, r.get::<_, Option<String>>("key")?, r.get::<_, String>("content")?))
                    })
                    .map_err(err)?;
                match rows.next() {
                    Some(Ok(row)) => Some(row),
                    Some(Err(e)) => return Err(err(e)),
                    None => None,
                }
            };
            let mut dup = dup;
            let mut replaces = replaces_opt.clone();
            if opts.fuzzy_dedupe && dup.is_none() && key.is_none() && replaces.is_none() {
                let near = Self::near_duplicate(c, &content, &scope, &kind, &active, &active_params, &w)?;
                if let Some((row, verdict)) = near {
                    if verdict == "refine" {
                        replaces = Some(row.0.clone());
                        refined = true;
                    } else {
                        dup = Some(row);
                    }
                }
            }
            if let Some((dup_id, dup_key, _)) = &dup {
                let is_dup = key.is_none() || dup_key.as_deref() == key.as_deref();
                if is_dup {
                    // The relearn's lifetime wins when it carries the row's
                    // explicit key or the row is unkeyed; an unkeyed relearn
                    // must never shorten a keyed memory's life.
                    if opts.update_expiry_on_duplicate && (key.is_some() || dup_key.is_none()) {
                        c.execute(
                            "UPDATE memories SET access_count=access_count+1, updated_at=?, importance=max(importance, ?),\
                             pinned=max(pinned, ?), expires_at=? WHERE id=?",
                            params_from_iter([
                                Param::Real(now),
                                Param::Real(importance),
                                Param::Integer(i64::from(pinned)),
                                match expires_at {
                                    Some(e) => Param::Real(e),
                                    None => Param::Null,
                                },
                                Param::Text(dup_id.clone()),
                            ]),
                        )
                        .map_err(err)?;
                    } else {
                        c.execute(
                            "UPDATE memories SET access_count=access_count+1, updated_at=?, importance=max(importance, ?),\
                             pinned=max(pinned, ?) WHERE id=?",
                            params_from_iter([
                                Param::Real(now),
                                Param::Real(importance),
                                Param::Integer(i64::from(pinned)),
                                Param::Text(dup_id.clone()),
                            ]),
                        )
                        .map_err(err)?;
                    }
                    return Ok(RememberResult {
                        id: dup_id.clone(),
                        status: "duplicate".to_string(),
                        replaced: Vec::new(),
                        redacted: redacted.clone(),
                    });
                }
            }

            let new_id = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
            let mut replaced: Vec<String> = Vec::new();
            if let Some(k) = &key {
                let sql = format!("SELECT id FROM memories m WHERE scope=? AND key=? AND {active}");
                let mut params = vec![Param::Text(scope.clone()), Param::Text(k.clone())];
                params.extend(active_params.iter().cloned());
                let mut stmt = c.prepare(&sql).map_err(err)?;
                let ids = stmt
                    .query_map(params_from_iter(params.iter()), |r| r.get::<_, String>(0))
                    .map_err(err)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(err)?;
                replaced.extend(ids);
            }
            if let Some(rid) = &replaces {
                let row = c
                    .query_row(
                        "SELECT id FROM memories WHERE id=? AND superseded_by IS NULL",
                        params_from_iter([Param::Text(rid.clone())]),
                        |r| r.get::<_, String>(0),
                    )
                    .map_err(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => {
                            err(format!("no active memory '{rid}'"))
                        }
                        e => err(e),
                    })?;
                replaced.push(row);
            }
            let mut deduped = Vec::new();
            for r in replaced {
                if !deduped.contains(&r) {
                    deduped.push(r);
                }
            }
            let replaced = deduped;

            c.execute(
                "INSERT INTO memories (id, scope, kind, key, content, tags, meta, source, importance, pinned,\
                 content_hash, created_at, updated_at, expires_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params_from_iter([
                    Param::Text(new_id.clone()),
                    Param::Text(scope.clone()),
                    Param::Text(kind.clone()),
                    match &key {
                        Some(k) => Param::Text(k.clone()),
                        None => Param::Null,
                    },
                    Param::Text(content.clone()),
                    Param::Text(tag_str.clone()),
                    Param::Text(meta.clone()),
                    match &source {
                        Some(s) => Param::Text(s.clone()),
                        None => Param::Null,
                    },
                    Param::Real(importance),
                    Param::Integer(i64::from(pinned)),
                    Param::Text(h.clone()),
                    Param::Real(now),
                    Param::Real(now),
                    match expires_at {
                        Some(e) => Param::Real(e),
                        None => Param::Null,
                    },
                ]),
            )
            .map_err(err)?;
            for rid in &replaced {
                c.execute(
                    "UPDATE memories SET superseded_by=?, superseded_at=? WHERE id=?",
                    params_from_iter([
                        Param::Text(new_id.clone()),
                        Param::Real(now),
                        Param::Text(rid.clone()),
                    ]),
                )
                .map_err(err)?;
            }
            let status = if replaced.is_empty() {
                "created"
            } else if refined {
                "refined"
            } else {
                "superseded"
            };
            Ok(RememberResult {
                id: new_id,
                status: status.to_string(),
                replaced,
                redacted: redacted.clone(),
            })
        })
    }

    fn near_duplicate(
        c: &Connection,
        content: &str,
        scope: &str,
        kind: &str,
        active: &str,
        active_params: &[Param],
        w: &Weights,
    ) -> Result<Option<(DupRow, &'static str)>> {
        let Some(q) = t::fts_query(content) else {
            return Ok(None);
        };
        let sql = format!(
            "SELECT m.* FROM memories_fts f JOIN memories m ON m.rowid=f.rowid \
             WHERE memories_fts MATCH ? AND m.scope=? AND m.kind=? AND {active} \
             ORDER BY bm25(memories_fts) LIMIT 5"
        );
        let mut params = vec![
            Param::Text(q),
            Param::Text(scope.to_string()),
            Param::Text(kind.to_string()),
        ];
        params.extend(active_params.iter().cloned());
        let mut stmt = c.prepare(&sql).map_err(err)?;
        let rows = stmt
            .query_map(params_from_iter(params.iter()), |r| {
                Ok((
                    r.get::<_, String>("id")?,
                    r.get::<_, Option<String>>("key")?,
                    r.get::<_, String>("content")?,
                ))
            })
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        let new_terms = t::term_set(content);
        for row in rows {
            let old_terms = t::term_set(&row.2);
            if t::jaccard(&new_terms, &old_terms) >= w.dup_jaccard {
                return Ok(Some((row, "dup")));
            }
            if old_terms.len() >= 3
                && new_terms.len() > old_terms.len()
                && t::containment(&new_terms, &old_terms) >= w.refine_containment
            {
                return Ok(Some((row, "refine")));
            }
        }
        Ok(None)
    }

    /// Soft-delete (default) or hard-delete by id. Returns rows affected.
    pub fn forget_id(&mut self, id: &str, hard: bool) -> Result<usize> {
        let now = self.now();
        self.write_tx(|c| {
            let n = if hard {
                c.execute("DELETE FROM memories WHERE id=?", params_from_iter([Param::Text(id.to_string())]))
                    .map_err(err)?
            } else {
                c.execute(
                    "UPDATE memories SET deleted_at=? WHERE id=? AND deleted_at IS NULL",
                    params_from_iter([Param::Real(now), Param::Text(id.to_string())]),
                )
                .map_err(err)?
            };
            Ok(n)
        })
    }

    /// Soft-delete (default) or hard-delete by scope+key. Returns rows affected.
    pub fn forget_key(&mut self, scope: &str, key: &str, hard: bool) -> Result<usize> {
        let now = self.now();
        let scope = scope.trim_matches('/').to_string();
        self.write_tx(|c| {
            let n = if hard {
                c.execute(
                    "DELETE FROM memories WHERE scope=? AND key=?",
                    params_from_iter([Param::Text(scope.clone()), Param::Text(key.to_string())]),
                )
                .map_err(err)?
            } else {
                c.execute(
                    "UPDATE memories SET deleted_at=? WHERE scope=? AND key=? AND deleted_at IS NULL",
                    params_from_iter([
                        Param::Real(now),
                        Param::Text(scope.clone()),
                        Param::Text(key.to_string()),
                    ]),
                )
                .map_err(err)?
            };
            Ok(n)
        })
    }

    // ------------------------------------------------------------------ read
    pub fn get(&self, id: &str) -> Result<Option<Memory>> {
        let rows = self.q(
            "SELECT * FROM memories WHERE id=?",
            vec![Param::Text(id.to_string())],
        )?;
        Ok(rows.into_iter().next())
    }

    /// The active memory whose normalized content equals `content` (newest first).
    pub fn find(&self, content: &str, scope: Option<&str>) -> Result<Option<Memory>> {
        let (active, mut params) = self.active_sql("m", None);
        let mut sql = format!("SELECT * FROM memories m WHERE content_hash=? AND {active}");
        let mut all = vec![Param::Text(t::content_hash(content))];
        all.append(&mut params);
        if let Some(s) = scope {
            sql += " AND scope=?";
            all.push(Param::Text(s.trim_matches('/').to_string()));
        }
        sql += " ORDER BY updated_at DESC LIMIT 1";
        Ok(self.q(&sql, all)?.into_iter().next())
    }

    /// Every active memory in exactly `scope`, newest first (the same active
    /// filter `find` uses: not superseded, not deleted, not expired).
    pub fn active_in_scope(&self, scope: &str) -> Result<Vec<Memory>> {
        let (active, mut params) = self.active_sql("m", None);
        let sql = format!(
            "SELECT * FROM memories m WHERE scope=? AND {active} ORDER BY updated_at DESC"
        );
        let mut all = vec![Param::Text(scope.trim_matches('/').to_string())];
        all.append(&mut params);
        self.q(&sql, all)
    }

    /// All versions of a keyed fact, oldest first.
    pub fn history_key(&self, scope: &str, key: &str) -> Result<Vec<Memory>> {
        self.q(
            "SELECT * FROM memories WHERE scope=? AND key=? ORDER BY created_at",
            vec![
                Param::Text(scope.trim_matches('/').to_string()),
                Param::Text(key.to_string()),
            ],
        )
    }

    fn scope_filter(scope: Option<&[String]>, inherit: bool) -> (String, Vec<Param>) {
        let Some(scopes) = scope else {
            return ("1".to_string(), Vec::new());
        };
        let mut allowed: Vec<String> = Vec::new();
        for s in scopes {
            if inherit {
                for c in scope_chain(s) {
                    allowed.push(c);
                }
            } else {
                let s = s.trim_matches('/');
                allowed.push(if s.is_empty() { "global".to_string() } else { s.to_string() });
            }
        }
        let mut deduped: Vec<String> = Vec::new();
        for a in allowed {
            if !deduped.contains(&a) {
                deduped.push(a);
            }
        }
        let sql = format!("m.scope IN ({})", vec!["?"; deduped.len()].join(","));
        (sql, deduped.into_iter().map(Param::Text).collect())
    }

    /// Hybrid retrieval: BM25 (Porter) + trigram fuzzy, fused by per-channel
    /// normalized scores, re-weighted by recency, importance, reinforcement, pinning.
    pub fn recall(&mut self, query: &str, opts: &RecallOpts) -> Result<Vec<Memory>> {
        let w = self.w.clone();
        let (active, aparams) = self.active_sql("m", opts.as_of);
        let (scope_sql, sparams) = Self::scope_filter(opts.scope.as_deref(), opts.inherit);
        let mut extra = String::new();
        let mut eparams: Vec<Param> = Vec::new();
        if !opts.kinds.is_empty() {
            extra += &format!(" AND m.kind IN ({})", vec!["?"; opts.kinds.len()].join(","));
            eparams.extend(opts.kinds.iter().cloned().map(Param::Text));
        }
        if !opts.exclude_kinds.is_empty() {
            extra += &format!(" AND m.kind NOT IN ({})", vec!["?"; opts.exclude_kinds.len()].join(","));
            eparams.extend(opts.exclude_kinds.iter().cloned().map(Param::Text));
        }
        for tag in &opts.tags {
            extra += " AND (' ' || m.tags || ' ') LIKE ?";
            eparams.push(Param::Text(format!("% {} %", tag.to_lowercase())));
        }
        let base_where = format!("{active} AND {scope_sql}{extra}");
        let mut base_params = aparams;
        base_params.extend(sparams);
        base_params.extend(eparams);
        let n = w.candidates.max(opts.k * 3);

        let query = query.trim().to_string();
        if query.is_empty() {
            return self.browse(&base_where, base_params, opts.k, opts.budget_tokens, opts.touch);
        }

        // Python `as_of or self.clock()`: 0.0 still means "now" for ranking.
        let now = opts.as_of.filter(|a| *a != 0.0).unwrap_or_else(|| self.now());
        let tw = if w.temporal > 0.0 {
            temporal::parse_time_window(&query, now)
        } else {
            None
        };
        let subs: Vec<String> = if w.sub_query > 0.0 {
            t::sub_queries(&query)
        } else {
            Vec::new()
        };

        let mut scored = self.rank_once(&query, &base_where, &base_params, n, now, &tw, opts.explain)?;
        if !subs.is_empty() && !scored.is_empty() {
            let mut order: Vec<String> = scored.iter().map(|m| m.id.clone()).collect();
            let mut by_id: HashMap<String, Memory> = scored.into_iter().map(|m| (m.id.clone(), m)).collect();
            let top_full = {
                let s = scored_score(by_id.get(&order[0]).unwrap());
                if s == 0.0 { 1.0 } else { s }
            };
            let sub_n = 30usize.max(opts.k * 2);
            for sq in &subs {
                let smems = self.rank_once(sq, &base_where, &base_params, sub_n, now, &tw, opts.explain)?;
                if smems.is_empty() {
                    continue;
                }
                let top_sub = {
                    let s = scored_score(smems.first().unwrap());
                    if s == 0.0 { 1.0 } else { s }
                };
                for mut m in smems {
                    let sc = t::round6((m.score.unwrap_or(0.0) / top_sub) * top_full * w.sub_query);
                    let better = match by_id.get(&m.id) {
                        None => true,
                        Some(existing) => sc > existing.score.unwrap_or(0.0),
                    };
                    if better {
                        m.score = Some(sc);
                        if opts.explain {
                            if let Some(why) = m.why.as_mut().and_then(|v| v.as_object_mut()) {
                                why.insert("sub_query".to_string(), json!(sq));
                            }
                        }
                        if !order.contains(&m.id) {
                            order.push(m.id.clone());
                        }
                        by_id.insert(m.id.clone(), m);
                    }
                }
            }
            scored = order.iter().filter_map(|id| by_id.get(id).cloned()).collect();
            scored.sort_by(|a, b| {
                b.score.unwrap_or(0.0).partial_cmp(&a.score.unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        let out = Self::pack(scored, opts.k, opts.budget_tokens);
        if opts.touch && opts.as_of.is_none() {
            self.touch(&out.iter().map(|m| m.id.clone()).collect::<Vec<_>>())?;
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn rank_once(
        &mut self,
        query: &str,
        base_where: &str,
        base_params: &[Param],
        n: usize,
        now: f64,
        tw: &Option<temporal::TimeWindow>,
        explain: bool,
    ) -> Result<Vec<Memory>> {
        let w = &self.w;
        // Each channel yields an ordered id->score map; parts keep first-seen id order
        // across channels so the final stable sort matches Python's dict ordering.
        let mut channels: Vec<(&'static str, Vec<(String, f64)>)> = Vec::new();
        let mut rows_by_id: HashMap<String, Memory> = HashMap::new();

        if w.fts > 0.0 {
            if let Some(fq) = t::fts_query(query) {
                let sql = format!(
                    "SELECT m.*, -bm25(memories_fts, 1.0, 0.6, 1.5) AS _s FROM memories_fts f \
                     JOIN memories m ON m.rowid=f.rowid WHERE memories_fts MATCH ? AND {base_where} \
                     ORDER BY _s DESC LIMIT ?"
                );
                let mut params = vec![Param::Text(fq)];
                params.extend(base_params.iter().cloned());
                params.push(Param::Integer(n as i64));
                let mut stmt = self.conn.prepare(&sql).map_err(err)?;
                let rows = stmt
                    .query_map(params_from_iter(params.iter()), |r| {
                        Ok((
                            r.get::<_, String>("id")?,
                            r.get::<_, f64>("_s")?,
                            memory_from_row(r)?,
                        ))
                    })
                    .map_err(err)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(err)?;
                let mut ch = Vec::new();
                for (id, s, mem) in rows {
                    ch.push((id.clone(), s.max(0.0)));
                    rows_by_id.insert(id, mem);
                }
                channels.push(("fts", ch));
            }
        }

        if w.trigram > 0.0 {
            if let Some(tq) = t::trigram_query(query) {
                let sql = format!(
                    "SELECT m.* FROM memories_tri t JOIN memories m ON m.rowid=t.rowid \
                     WHERE memories_tri MATCH ? AND {base_where} ORDER BY bm25(memories_tri) LIMIT ?"
                );
                let mut params = vec![Param::Text(tq.clone())];
                params.extend(base_params.iter().cloned());
                params.push(Param::Integer(n as i64));
                let mut stmt = self.conn.prepare(&sql).map_err(err)?;
                let rows = stmt
                    .query_map(params_from_iter(params.iter()), |r| {
                        Ok((r.get::<_, String>("id")?, memory_from_row(r)?))
                    })
                    .map_err(err)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(err)?;
                let grams: std::collections::HashSet<String> = tq
                    .split(" OR ")
                    .map(|g| g.trim_matches('"').to_string())
                    .collect();
                let mut tri = Vec::new();
                for (id, mem) in rows {
                    let body = caseless::default_case_fold_str(&mem.content);
                    let overlap = grams.iter().filter(|g| body.contains(g.as_str())).count() as f64
                        / grams.len() as f64;
                    if overlap >= w.min_trigram_overlap {
                        tri.push((id.clone(), overlap));
                        rows_by_id.entry(id).or_insert(mem);
                    }
                }
                channels.push(("trigram", tri));
            }
        }

        let weights: HashMap<&str, f64> =
            HashMap::from([("fts", w.fts), ("trigram", w.trigram), ("vector", w.vector)]);
        let live: Vec<(&'static str, &Vec<(String, f64)>)> = channels
            .iter()
            .filter(|(_, scores)| !scores.is_empty())
            .map(|(ch, scores)| (*ch, scores))
            .collect();
        if live.is_empty() {
            return Ok(Vec::new());
        }
        let total_w: f64 = live.iter().map(|(ch, _)| weights[ch]).sum();
        // parts: first-seen id order, then (channel, normalized score) in channel order
        let mut part_order: Vec<String> = Vec::new();
        let mut parts: HashMap<String, Vec<(&'static str, f64)>> = HashMap::new();
        for (ch, scores) in &live {
            let top = scores.iter().map(|(_, s)| *s).fold(0.0_f64, f64::max);
            let top = if top == 0.0 { 1.0 } else { top };
            for (mid, s) in scores.iter() {
                let p = parts.entry(mid.clone()).or_insert_with(|| {
                    part_order.push(mid.clone());
                    Vec::new()
                });
                p.push((*ch, s / top));
            }
        }

        let mut scored: Vec<Memory> = Vec::new();
        for mid in &part_order {
            let p = &parts[mid];
            let mut mem = rows_by_id[mid].clone();
            let relevance: f64 =
                p.iter().map(|(ch, v)| weights[ch] * v).sum::<f64>() / total_w;
            let ts = if mem.updated_at != 0.0 { mem.updated_at } else { mem.created_at };
            let age_days = (now - ts).max(0.0) / DAY;
            let recency = (-std::f64::consts::LN_2 * age_days / w.half_life_days).exp();
            let tboost = match tw {
                Some(tw) => 1.0 + w.temporal * tw.proximity(ts),
                None => 1.0,
            };
            let score = relevance
                * ((1.0 - w.recency) + w.recency * recency)
                * tboost
                * ((1.0 - w.importance / 2.0) + w.importance * mem.importance)
                * (1.0 + w.access * (1.0 + mem.access_count as f64).ln())
                * (if mem.pinned { w.pinned_boost } else { 1.0 });
            mem.score = Some(t::round6(score));
            if explain {
                let mut channels_map = Map::new();
                for (c, v) in p {
                    channels_map.insert(
                        c.to_string(),
                        json!(format!("{v:.3}").parse::<f64>().unwrap_or(0.0)),
                    );
                }
                let mut why = Map::new();
                why.insert(
                    "channels".to_string(),
                    Value::Object(channels_map),
                );
                why.insert(
                    "relevance".to_string(),
                    json!(format!("{relevance:.4}").parse::<f64>().unwrap_or(0.0)),
                );
                why.insert(
                    "recency".to_string(),
                    json!(format!("{recency:.4}").parse::<f64>().unwrap_or(0.0)),
                );
                if tw.is_some() {
                    why.insert(
                        "temporal".to_string(),
                        json!(format!("{tboost:.4}").parse::<f64>().unwrap_or(0.0)),
                    );
                }
                mem.why = Some(Value::Object(why));
            }
            scored.push(mem);
        }
        scored.sort_by(|a, b| {
            b.score.unwrap_or(0.0).partial_cmp(&a.score.unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(scored)
    }

    fn browse(
        &mut self,
        where_clause: &str,
        params: Vec<Param>,
        k: usize,
        budget_tokens: Option<usize>,
        touch: bool,
    ) -> Result<Vec<Memory>> {
        let limit = k.max(if matches!(budget_tokens, Some(b) if b > 0) { 200 } else { k });
        let sql = format!(
            "SELECT * FROM memories m WHERE {where_clause} ORDER BY pinned DESC, importance DESC, \
             updated_at DESC LIMIT ?"
        );
        let mut all = params;
        all.push(Param::Integer(limit as i64));
        let mems = self.q(&sql, all)?;
        let out = Self::pack(mems, k, budget_tokens);
        if touch {
            self.touch(&out.iter().map(|m| m.id.clone()).collect::<Vec<_>>())?;
        }
        Ok(out)
    }

    fn pack(mems: Vec<Memory>, k: usize, budget_tokens: Option<usize>) -> Vec<Memory> {
        match budget_tokens {
            None => mems.into_iter().take(k).collect(),
            Some(0) => mems.into_iter().take(k).collect(),
            Some(budget) => {
                let mut out = Vec::new();
                let mut used = 0usize;
                for m in mems {
                    let cost = t::estimate_tokens(&m.content) + 8;
                    if used + cost > budget {
                        continue;
                    }
                    used += cost;
                    out.push(m);
                }
                out
            }
        }
    }

    fn touch(&mut self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let now = self.now();
        let res = self.write_tx(|c| {
            for id in ids {
                c.execute(
                    "UPDATE memories SET access_count=access_count+1, accessed_at=? WHERE id=?",
                    params_from_iter([Param::Real(now), Param::Text(id.clone())]),
                )
                .map_err(err)?;
            }
            Ok(())
        });
        match res {
            Ok(()) => Ok(()),
            // reads must not fail because a writer is busy
            Err(e) if is_busy(&e) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

fn scored_score(m: &Memory) -> f64 {
    m.score.unwrap_or(0.0)
}

fn is_busy(e: &EverettError) -> bool {
    let s = e.to_string();
    s.contains("database is locked") || s.contains("database table is locked") || s.contains("SQLITE_BUSY")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn store_at(path: &Path, clock: Arc<Mutex<f64>>) -> MemoryStore {
        MemoryStore::open_with_clock(path, Box::new(move || *clock.lock().unwrap())).unwrap()
    }

    #[test]
    fn new_file_is_created_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        let clock = Arc::new(Mutex::new(1_700_000_000.0));
        let store = store_at(&path, clock);
        drop(store);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn two_stores_write_alternately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        let clock = Arc::new(Mutex::new(1_700_000_000.0));
        let mut a = store_at(&path, clock.clone());
        let mut b = store_at(&path, clock);
        let opts = |content_scope: &str| RememberOpts {
            scope: content_scope.to_string(),
            ..Default::default()
        };
        a.remember("from store a", &opts("global")).unwrap();
        b.remember("from store b", &opts("global")).unwrap();
        a.remember("more from a", &opts("global")).unwrap();
        let hits = b.recall("", &RecallOpts { k: 8, touch: false, ..Default::default() }).unwrap();
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn create_empty_db_tolerates_a_lost_create_race() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        create_empty_db(&path).unwrap();
        // A second creator finding the same file must not fail.
        create_empty_db(&path).unwrap();
        // A pre-created empty file is a valid new database.
        let clock = Arc::new(Mutex::new(1_700_000_000.0));
        let mut store = store_at(&path, clock);
        store.remember("after the race", &RememberOpts::default()).unwrap();
        let hits = store.recall("after the race", &RecallOpts::default()).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn relearn_never_expires_a_keyed_memory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        let clock = Arc::new(Mutex::new(1_700_000_000.0));
        let mut store = store_at(&path, clock);
        let keyed = store
            .remember(
                "the keyed fact",
                &RememberOpts {
                    key: Some("fact-key".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        let res = store
            .remember(
                "the keyed fact",
                &RememberOpts {
                    ttl_days: Some(1.0),
                    update_expiry_on_duplicate: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(res.status, "duplicate");
        let row = store.get(&keyed.id).unwrap().unwrap();
        assert!(row.expires_at.is_none(), "keyed memory gained an expiry");
    }

    #[test]
    fn relearn_extends_a_keyed_memorys_life() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        let clock = Arc::new(Mutex::new(1_700_000_000.0));
        let mut store = store_at(&path, clock);
        let keyed = store
            .remember(
                "the keyed fact",
                &RememberOpts {
                    key: Some("fact-key".to_string()),
                    ttl_days: Some(1.0),
                    ..Default::default()
                },
            )
            .unwrap();
        let res = store
            .remember(
                "the keyed fact",
                &RememberOpts {
                    key: Some("fact-key".to_string()),
                    update_expiry_on_duplicate: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(res.status, "duplicate");
        let row = store.get(&keyed.id).unwrap().unwrap();
        assert!(row.expires_at.is_none(), "permanent relearn did not clear the expiry");
    }

    #[test]
    fn keyed_relearn_applies_a_shorter_life() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        let clock = Arc::new(Mutex::new(1_700_000_000.0));
        let mut store = store_at(&path, clock);
        let keyed = store
            .remember(
                "the keyed fact",
                &RememberOpts {
                    key: Some("fact-key".to_string()),
                    ttl_days: Some(10.0),
                    ..Default::default()
                },
            )
            .unwrap();
        let res = store
            .remember(
                "the keyed fact",
                &RememberOpts {
                    key: Some("fact-key".to_string()),
                    ttl_days: Some(1.0),
                    update_expiry_on_duplicate: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(res.status, "duplicate");
        let row = store.get(&keyed.id).unwrap().unwrap();
        let expiry = row.expires_at.unwrap();
        assert!(
            expiry < 1_700_000_000.0 + 2.0 * 86_400.0,
            "shorter relearn did not shorten the expiry"
        );
    }
}
