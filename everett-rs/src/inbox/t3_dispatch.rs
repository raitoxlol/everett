use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::EverettError;

const METADATA_KEY: &str = "t3_ping";

#[derive(Deserialize, Serialize)]
pub(crate) struct Binding {
    #[serde(rename = "thread_id")]
    pub thread: String,
    pub harness: String,
    pub data_home: String,
}

impl Binding {
    pub(crate) fn store(&self, metadata: &mut Map<String, Value>) {
        metadata.insert(METADATA_KEY.into(), serde_json::json!(self));
    }

    pub(crate) fn from_request(request: &Map<String, Value>) -> Option<Self> {
        serde_json::from_value(request.get(METADATA_KEY)?.clone()).ok()
    }
}

pub(crate) struct DispatchFailure {
    pub error: EverettError,
    pub retryable: bool,
}

impl DispatchFailure {
    pub(crate) fn uncertain(message: impl Into<String>) -> Self {
        Self { error: EverettError::new(4, message), retryable: false }
    }
}

impl From<EverettError> for DispatchFailure {
    fn from(error: EverettError) -> Self {
        Self { error, retryable: true }
    }
}

fn journal_error(error: rusqlite::Error) -> DispatchFailure {
    DispatchFailure {
        error: EverettError::new(4, format!("Cannot record T3 dispatch confirmation: {error}")),
        retryable: false,
    }
}

fn journal_path() -> std::path::PathBuf {
    crate::inbox::inbox_dir().join("t3-dispatch.sqlite")
}

pub(crate) fn forget_retired_dispatches(target: &str, live_ids: &HashSet<&str>, mailbox: &Path) -> rusqlite::Result<()> {
    if !journal_path().is_file() {
        return Ok(());
    }
    let mut journal = Connection::open_with_flags(journal_path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    journal.busy_timeout(Duration::from_millis(50))?;
    let transaction = journal.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let retired = {
        let mut query = transaction.prepare("SELECT message_id FROM dispatch WHERE target = ?1")?;
        let mut rows = query.query([target])?;
        let mut retired = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            if !live_ids.contains(id.as_str()) {
                retired.push(id);
            }
        }
        retired
    };
    // The mailbox lock is held by compaction. Confirm durable retirement before forgetting receipts.
    if retired.is_empty()
        || std::fs::File::open(mailbox).and_then(|file| file.sync_all()).is_err()
        || std::fs::File::open(crate::inbox::inbox_dir()).and_then(|dir| dir.sync_all()).is_err()
    {
        return Ok(());
    }
    for id in retired {
        transaction.execute("DELETE FROM dispatch WHERE target = ?1 AND message_id = ?2", params![target, id])?;
    }
    transaction.commit()
}

pub(crate) fn dispatch_once(
    target: &str, message_id: &str, dispatch: impl FnOnce() -> std::result::Result<String, DispatchFailure>,
) -> std::result::Result<(String, bool), DispatchFailure> {
    dispatch_once_at(&journal_path(), target, message_id, dispatch)
}

fn dispatch_once_at(
    journal: &Path, target: &str, message_id: &str,
    dispatch: impl FnOnce() -> std::result::Result<String, DispatchFailure>,
) -> std::result::Result<(String, bool), DispatchFailure> {
    let mut journal = Connection::open(journal)
        .map_err(journal_error)?;
    journal.busy_timeout(Duration::from_secs(1)).map_err(journal_error)?;
    journal.execute_batch("CREATE TABLE IF NOT EXISTS dispatch (
        target TEXT NOT NULL, message_id TEXT NOT NULL, receipt TEXT,
        PRIMARY KEY (target, message_id)
    )").map_err(journal_error)?;
    let reserve = journal.transaction_with_behavior(TransactionBehavior::Immediate).map_err(journal_error)?;
    let recorded: Option<Option<String>> = reserve.query_row(
        "SELECT receipt FROM dispatch WHERE target = ?1 AND message_id = ?2", params![target, message_id],
        |row| row.get(0),
    ).optional().map_err(journal_error)?;
    match recorded {
        Some(Some(receipt)) => return Ok((receipt, true)),
        Some(None) => return Err(DispatchFailure {
            error: EverettError::new(4, "T3 dispatch is in progress or its confirmation could not be saved. Check the original request and the T3 thread."),
            retryable: false,
        }),
        None => {}
    }
    reserve.execute("INSERT INTO dispatch (target, message_id) VALUES (?1, ?2)",
        params![target, message_id]).map_err(journal_error)?;
    reserve.commit().map_err(journal_error)?;

    match dispatch() {
        Ok(receipt) => {
            journal.execute("UPDATE dispatch SET receipt = ?3 WHERE target = ?1 AND message_id = ?2",
                params![target, message_id, receipt]).map_err(journal_error)?;
            Ok((receipt, false))
        }
        Err(failure) if failure.retryable => {
            journal.execute("DELETE FROM dispatch WHERE target = ?1 AND message_id = ?2",
                params![target, message_id]).map_err(journal_error)?;
            Err(failure)
        }
        Err(failure) => Err(failure),
    }
}

#[cfg(test)]
mod model;
