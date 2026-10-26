use std::fs::{self, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::time::Duration;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{core_dir, norm, CoreScope};
use crate::error::{EverettError, Result};

#[derive(Serialize, Deserialize)]
struct Fact {
    #[serde(default)]
    id: String,
    ts: f64,
    session: String,
    harness: String,
    cwd: String,
    project: String,
    scope: String,
    text: String,
    #[serde(default)]
    expires_at: Option<i64>,
}

pub(super) struct FactStore(Connection);

fn error(error: impl std::fmt::Display) -> EverettError {
    EverettError::new(2, format!("Shared fact store: {error}"))
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS facts (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL,
    project TEXT NOT NULL,
    normalized TEXT NOT NULL,
    text TEXT NOT NULL,
    ts REAL NOT NULL,
    expires_at INTEGER,
    payload TEXT NOT NULL,
    UNIQUE(scope, project, normalized)
);
CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(text, content='facts', content_rowid='rowid');
CREATE TRIGGER IF NOT EXISTS facts_insert AFTER INSERT ON facts BEGIN
    INSERT INTO facts_fts(rowid, text) VALUES (new.rowid, new.text);
END;
CREATE TRIGGER IF NOT EXISTS facts_delete AFTER DELETE ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
END;
CREATE TRIGGER IF NOT EXISTS facts_update AFTER UPDATE ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
    INSERT INTO facts_fts(rowid, text) VALUES (new.rowid, new.text);
END;
";

impl FactStore {
    pub(super) fn open(writable: bool) -> Result<Option<Self>> {
        let path = core_dir().join("facts.sqlite3");
        if !writable && !path.exists() {
            return Ok(None);
        }
        if writable {
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .mode(0o600)
                .open(&path)
                .map_err(error)?;
            file.set_permissions(PermissionsExt::from_mode(0o600))
                .map_err(error)?;
            for suffix in ["-wal", "-shm", "-journal"] {
                let sidecar = path.with_file_name(format!("facts.sqlite3{suffix}"));
                match fs::set_permissions(sidecar, PermissionsExt::from_mode(0o600)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(error(e)),
                }
            }
        }
        let flags = if writable {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let db = Connection::open_with_flags(path, flags).map_err(error)?;
        db.busy_timeout(Duration::from_secs(2)).map_err(error)?;
        if writable {
            db.pragma_update(None, "journal_mode", "DELETE")
                .map_err(error)?;
            db.execute_batch(SCHEMA).map_err(error)?;
        }
        Ok(Some(Self(db)))
    }

    pub(super) fn remember(
        &mut self,
        entry: &mut Map<String, Value>,
        persist_pending: impl FnOnce(&Map<String, Value>) -> Result<()>,
    ) -> Result<()> {
        let transaction = self.0.transaction().map_err(error)?;
        remember(&transaction, entry)?;
        persist_pending(entry)?;
        transaction.commit().map_err(error)
    }

    pub(super) fn count(&self, scope: &CoreScope, target: &str) -> Result<usize> {
        self.0
            .query_row(
                "SELECT count(*) FROM facts WHERE scope = ?1 AND project = ?2 AND normalized = ?3",
                params![scope.name(), scope.key(), target],
                |row| row.get(0),
            )
            .map_err(error)
    }

    pub(super) fn change(
        &mut self,
        scope: &CoreScope,
        target: &str,
        replacement: Option<&mut Map<String, Value>>,
    ) -> Result<()> {
        let transaction = self.0.transaction().map_err(error)?;
        transaction
            .execute(
                "DELETE FROM facts WHERE scope = ?1 AND project = ?2 AND normalized = ?3",
                params![scope.name(), scope.key(), target],
            )
            .map_err(error)?;
        if let Some(entry) = replacement {
            remember(&transaction, entry)?;
        }
        transaction.commit().map_err(error)
    }

    pub(super) fn search(
        &self,
        query: &str,
        project: &str,
        scope: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Value>> {
        let mut statement = self
            .0
            .prepare(
                "SELECT facts.payload FROM facts JOIN facts_fts ON facts.rowid = facts_fts.rowid
             WHERE facts_fts MATCH ?1 AND (facts.scope = 'global' OR facts.project = ?2)
             AND (?3 IS NULL OR facts.scope = ?3)
             AND (facts.expires_at IS NULL OR facts.expires_at > ?4)
             ORDER BY bm25(facts_fts), facts.ts DESC, facts.id LIMIT ?5",
            )
            .map_err(error)?;
        let rows = statement
            .query_map(
                params![query, project, scope, crate::session::now(), limit],
                |row| row.get::<_, String>(0),
            )
            .map_err(error)?;
        rows.map(|row| {
            let fact: Fact = serde_json::from_str(&row.map_err(error)?).map_err(error)?;
            serde_json::to_value(fact).map_err(error)
        })
        .collect()
    }
}

fn remember(db: &Connection, entry: &mut Map<String, Value>) -> Result<()> {
    let mut fact: Fact = serde_json::from_value(Value::Object(entry.clone())).map_err(error)?;
    let project = if fact.scope == "project" {
        fact.project.as_str()
    } else {
        ""
    };
    let normalized = norm(&fact.text);
    fact.id = db
        .query_row(
            "SELECT id FROM facts WHERE scope = ?1 AND project = ?2 AND normalized = ?3",
            params![fact.scope, project, normalized],
            |row| row.get(0),
        )
        .optional()
        .map_err(error)?
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    db.execute(
        "INSERT INTO facts(id, scope, project, normalized, text, ts, expires_at, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(scope, project, normalized) DO UPDATE SET
         text = excluded.text, ts = excluded.ts, expires_at = excluded.expires_at, payload = excluded.payload",
        params![fact.id, fact.scope, project, normalized, fact.text, fact.ts, fact.expires_at, serde_json::to_string(&fact).map_err(error)?],
    ).map_err(error)?;
    entry.insert("id".into(), Value::String(fact.id));
    Ok(())
}
