//! Bounded model of `dispatch_once` (#205): a T3 nudge may reach the thread at
//! most once per request unless an earlier attempt provably did not.
//!
//! The spec below is a three-state machine. Every attempt sequence up to
//! `MAX_ATTEMPTS`, and every attempt that starts while another is inside its
//! dispatch window, is replayed against a real SQLite journal and compared
//! with the spec. `scripts/check-dispatch-model.py` mutates the real code to
//! the historical bugs and requires this module to fail on each.

use std::path::Path;
use std::sync::mpsc::channel;
use std::sync::{Arc, Barrier};
use std::thread;

use rusqlite::Connection;

use super::{dispatch_once_at, DispatchFailure};
use crate::error::EverettError;

const MAX_ATTEMPTS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Outcome {
    Confirmed,
    Retryable,
    Uncertain,
    Crash,
    ConfirmedButJournalRejectsWrite,
}

const OUTCOMES: [Outcome; 5] = [
    Outcome::Confirmed,
    Outcome::Retryable,
    Outcome::Uncertain,
    Outcome::Crash,
    Outcome::ConfirmedButJournalRejectsWrite,
];

#[derive(Clone, Debug, PartialEq)]
enum State {
    Absent,
    Reserved,
    Confirmed(String),
}

#[derive(Debug, PartialEq)]
enum Observed {
    Receipt { receipt: String, reused: bool },
    Retryable,
    Held,
}

/// What the spec says an attempt does: whether it dispatches, what the caller sees, the next state.
fn spec(state: &State, outcome: Outcome, receipt: &str) -> (bool, Observed, State) {
    match state {
        State::Confirmed(saved) => (false, Observed::Receipt { receipt: saved.clone(), reused: true }, state.clone()),
        State::Reserved => (false, Observed::Held, State::Reserved),
        State::Absent => match outcome {
            Outcome::Confirmed => (
                true,
                Observed::Receipt { receipt: receipt.into(), reused: false },
                State::Confirmed(receipt.into()),
            ),
            Outcome::Retryable => (true, Observed::Retryable, State::Absent),
            Outcome::Uncertain | Outcome::ConfirmedButJournalRejectsWrite => (true, Observed::Held, State::Reserved),
            Outcome::Crash => (true, Observed::Held, State::Reserved),
        },
    }
}

fn failure(retryable: bool) -> DispatchFailure {
    DispatchFailure { error: EverettError::new(4, "fixture"), retryable }
}

/// Run one attempt. `dispatched` records whether the dispatch closure ran.
fn attempt(journal: &Path, outcome: Outcome, receipt: &str) -> (bool, Observed) {
    let mut dispatched = false;
    let journal_path = journal.to_path_buf();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dispatch_once_at(journal, "t3-target", "message-1", || {
            dispatched = true;
            match outcome {
                Outcome::Confirmed => Ok(receipt.to_string()),
                Outcome::Retryable => Err(failure(true)),
                Outcome::Uncertain => Err(failure(false)),
                Outcome::Crash => panic!("process died inside dispatch"),
                Outcome::ConfirmedButJournalRejectsWrite => {
                    reject_receipt_writes(&journal_path);
                    Ok(receipt.to_string())
                }
            }
        })
    }));
    if outcome == Outcome::ConfirmedButJournalRejectsWrite {
        allow_receipt_writes(journal);
    }
    let observed = match result {
        Ok(Ok((receipt, reused))) => Observed::Receipt { receipt, reused },
        Ok(Err(failure)) if failure.retryable => Observed::Retryable,
        Ok(Err(_)) | Err(_) => Observed::Held,
    };
    (dispatched, observed)
}

fn reject_receipt_writes(journal: &Path) {
    Connection::open(journal)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_receipt BEFORE UPDATE ON dispatch
             BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
        )
        .unwrap();
}

fn allow_receipt_writes(journal: &Path) {
    Connection::open(journal).unwrap().execute_batch("DROP TRIGGER IF EXISTS reject_receipt").unwrap();
}

fn journal_state(journal: &Path) -> State {
    let Ok(con) = Connection::open(journal) else { return State::Absent };
    let row: rusqlite::Result<Option<String>> = con.query_row(
        "SELECT receipt FROM dispatch WHERE target = 't3-target' AND message_id = 'message-1'",
        [],
        |row| row.get(0),
    );
    match row {
        Ok(Some(receipt)) => State::Confirmed(receipt),
        Ok(None) => State::Reserved,
        Err(_) => State::Absent,
    }
}

fn sequences(len: usize) -> Vec<Vec<Outcome>> {
    (0..len).fold(vec![vec![]], |acc, _| {
        acc.iter()
            .flat_map(|prefix| {
                OUTCOMES.iter().map(move |outcome| {
                    let mut next = prefix.clone();
                    next.push(*outcome);
                    next
                })
            })
            .collect()
    })
}

#[test]
fn every_sequential_attempt_history_matches_the_spec() {
    for len in 1..=MAX_ATTEMPTS {
        for outcomes in sequences(len) {
            let dir = tempfile::tempdir().unwrap();
            let journal = dir.path().join("t3-dispatch.sqlite");
            let mut state = State::Absent;
            let mut dispatches_since_open = 0;
            for (n, outcome) in outcomes.iter().enumerate() {
                let receipt = format!("receipt-{n}");
                let (spec_dispatches, spec_observed, next) = spec(&state, *outcome, &receipt);
                let (dispatched, observed) = attempt(&journal, *outcome, &receipt);
                assert_eq!(
                    (dispatched, &observed),
                    (spec_dispatches, &spec_observed),
                    "attempt {n} of {outcomes:?} from {state:?}"
                );
                assert_eq!(journal_state(&journal), next, "journal after attempt {n} of {outcomes:?}");
                if dispatched {
                    dispatches_since_open += 1;
                }
                if state != State::Absent {
                    assert!(!dispatched, "dispatched again from {state:?} in {outcomes:?}");
                }
                state = next;
            }
            assert!(
                dispatches_since_open <= outcomes.iter().filter(|o| **o == Outcome::Retryable).count() + 1,
                "{outcomes:?}"
            );
        }
    }
}

#[test]
fn an_attempt_inside_another_attempts_dispatch_window_never_dispatches() {
    for first in OUTCOMES {
        for second in OUTCOMES {
            let dir = tempfile::tempdir().unwrap();
            let journal = Arc::new(dir.path().join("t3-dispatch.sqlite"));
            let (entered_tx, entered_rx) = channel();
            let (release_tx, release_rx) = channel::<()>();
            let first_journal = Arc::clone(&journal);
            let holder = thread::spawn(move || {
                let mut dispatched = false;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    dispatch_once_at(&first_journal, "t3-target", "message-1", || {
                        dispatched = true;
                        entered_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        match first {
                            Outcome::Confirmed | Outcome::ConfirmedButJournalRejectsWrite => Ok("receipt-first".into()),
                            Outcome::Retryable => Err(failure(true)),
                            Outcome::Uncertain => Err(failure(false)),
                            Outcome::Crash => panic!("process died inside dispatch"),
                        }
                    })
                }));
                (dispatched, result.is_ok())
            });
            entered_rx.recv().unwrap();

            assert_eq!(
                journal_state(&journal),
                State::Reserved,
                "a dispatch is in flight but the journal does not say so ({first:?})"
            );
            let (dispatched, observed) = attempt(&journal, second, "receipt-second");
            assert!(!dispatched, "{second:?} dispatched while {first:?} was in flight");
            assert_eq!(observed, Observed::Held, "{second:?} during {first:?}");

            release_tx.send(()).unwrap();
            let (first_dispatched, _) = holder.join().unwrap();
            assert!(first_dispatched);
        }
    }
}

#[test]
fn racing_callers_dispatch_once() {
    for round in 0..40 {
        let dir = tempfile::tempdir().unwrap();
        let journal = Arc::new(dir.path().join("t3-dispatch.sqlite"));
        let racers = 4;
        let start = Arc::new(Barrier::new(racers));
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handles: Vec<_> = (0..racers)
            .map(|n| {
                let (journal, start, count) = (Arc::clone(&journal), Arc::clone(&start), Arc::clone(&count));
                thread::spawn(move || {
                    start.wait();
                    dispatch_once_at(&journal, "t3-target", "message-1", || {
                        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Ok(format!("receipt-{n}"))
                    })
                    .is_ok()
                })
            })
            .collect();
        let successes = handles.into_iter().map(|h| h.join().unwrap()).filter(|ok| *ok).count();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1, "round {round}");
        assert!(successes >= 1, "round {round}");
    }
}
