//! Bounded model of Dot event delivery (#206): an event whose webhook answered 2xx
//! is never posted again, and a late result never changes a refreshed subscription.
//!
//! Every response status is crossed with every interference that can land while
//! the webhook is in flight (subscription refresh, a writer holding the database,
//! both), then followed by repeated retries. Each step is compared with the small
//! spec in `Spec`, against a real SQLite store.
//! `scripts/check-delivery-model.py` mutates the real code back to the historical
//! mistakes and requires this module to fail on each.

use super::*;
use std::io::Write;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};

struct Gate {
    status: u16,
    posts: AtomicUsize,
    block: AtomicBool,
    started: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl Webhook for Gate {
    fn post(&self, _: &str, _: &[(String, String)], body: &[u8]) -> Result<(u16, Vec<u8>), &'static str> {
        let value: Value = serde_json::from_slice(body).unwrap();
        if value["type"] == "verification" {
            return Ok((200, serde_json::to_vec(&json!({"challenge":value["challenge"]})).unwrap()));
        }
        self.posts.fetch_add(1, Ordering::SeqCst);
        if self.block.swap(false, Ordering::SeqCst) {
            self.started.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
        }
        Ok((self.status, Vec::new()))
    }
}

fn principal(token: Option<&str>) -> Principal {
    Principal { owner: "owner".into(), agent: "dot-a".into(), token: token.map(Into::into) }
}

fn args() -> Value {
    json!({"name":EVENT,"arguments":{"session_id":"dot-a"},"delivery":{"mode":"webhook","url":"https://callback.example/","secret":format!("whsec_{}",STANDARD.encode([7;32]))}})
}

#[derive(Clone, Copy, Debug)]
struct Interference {
    refresh: bool,
    writer_holds_database: bool,
}

#[derive(Debug, PartialEq)]
struct Spec {
    posts: usize,
    done: bool,
    subscribed: bool,
    unrecorded_ack: bool,
}

impl Spec {
    /// The webhook answered `status` for the one in-flight claim.
    fn in_flight(status: u16, interference: Interference) -> Self {
        let mut spec = Spec { posts: 1, done: false, subscribed: true, unrecorded_ack: false };
        // An acknowledgment is tied to its claim, so a refresh cannot lose it; a busy
        // database only delays recording it. Every other result needs the same
        // subscription revision and an available database.
        let untouched = !interference.refresh && !interference.writer_holds_database;
        match status {
            200..=299 if interference.writer_holds_database => spec.unrecorded_ack = true,
            200..=299 => spec.done = true,
            410 if untouched => spec.subscribed = false,
            400..=499 if untouched => spec.done = true,
            _ => {}
        }
        spec
    }

    /// Time passes, then the delivery runs again with nothing in the way.
    fn retry(&mut self, status: u16) {
        if self.unrecorded_ack {
            self.done = true;
            self.unrecorded_ack = false;
        }
        if !self.subscribed || self.done {
            return;
        }
        self.posts += 1;
        match status {
            200..=299 => self.done = true,
            410 => self.subscribed = false,
            400..=499 => self.done = true,
            _ => {}
        }
    }
}

fn observe(events: &Events, gate: &Gate) -> Spec {
    let db = events.db.lock().unwrap();
    let subscribed: i64 = db.query_row("SELECT count(*) FROM subscriptions", [], |r| r.get(0)).unwrap();
    let done: Option<bool> = db.query_row("SELECT done FROM deliveries", [], |r| r.get(0)).optional().unwrap();
    Spec {
        posts: gate.posts.load(Ordering::SeqCst),
        done: done.unwrap_or(false),
        subscribed: subscribed > 0,
        unrecorded_ack: !events.acknowledgments.lock().unwrap().is_empty(),
    }
}

#[test]
fn delivery_outcomes_match_the_spec_under_every_interference() {
    for status in [204u16, 503, 400, 410] {
        for refresh in [false, true] {
            for writer_holds_database in [false, true] {
                let interference = Interference { refresh, writer_holds_database };
                let case = format!("status {status} {interference:?}");
                let home = tempfile::tempdir().unwrap();
                let path = home.path().join("events.sqlite");
                let (started, ready) = mpsc::channel();
                let (release, wait) = mpsc::channel();
                let gate = Arc::new(Gate {
                    status,
                    posts: AtomicUsize::new(0),
                    block: AtomicBool::new(false),
                    started,
                    release: Mutex::new(wait),
                });
                let events = Events::with_db(Connection::open(&path).unwrap(), home.path().into(), gate.clone()).unwrap();
                events.db.lock().unwrap().busy_timeout(Duration::ZERO).unwrap();
                events.subscribe(&principal(None), &args()).unwrap();
                let inbox = home.path().join(".everett/inbox");
                fs::create_dir_all(&inbox).unwrap();
                writeln!(
                    File::create(inbox.join("dot-a.jsonl")).unwrap(),
                    "{}",
                    json!({"id":"fresh","to":"dot-a","ts":now() as f64 / 1000.0})
                )
                .unwrap();

                let lease = events.leases().unwrap().remove(0);
                gate.block.store(true, Ordering::SeqCst);
                let slow = events.clone();
                let in_flight = std::thread::spawn(move || slow.deliver(&lease));
                ready.recv_timeout(Duration::from_secs(2)).unwrap();
                if refresh {
                    events.subscribe(&principal(Some("refreshed")), &args()).unwrap();
                }
                let mut writer = Connection::open(&path).unwrap();
                let held = writer_holds_database
                    .then(|| writer.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap());
                release.send(()).unwrap();
                let delivered = in_flight.join().unwrap();
                drop(held);

                let mut spec = Spec::in_flight(status, interference);
                assert_eq!(delivered.is_err(), writer_holds_database, "deliver result, {case}");
                assert_eq!(observe(&events, &gate), spec, "after the in-flight post, {case}");

                for round in 1..=3 {
                    events.db.lock().unwrap().execute("UPDATE deliveries SET next=0", []).unwrap();
                    // Straight to deliver: leases() also flushes receipts and would hide a missing flush here.
                    let token = refresh.then_some("refreshed");
                    let lease = Lease { id: Events::identity(&principal(token), &args()).unwrap().0, principal: principal(token) };
                    events.deliver(&lease).unwrap();
                    spec.retry(status);
                    assert_eq!(observe(&events, &gate), spec, "after retry {round}, {case}");
                }
                if status < 300 {
                    assert_eq!(spec.posts, 1, "a 2xx event was posted again, {case}");
                }
            }
        }
    }
}

#[test]
fn a_claim_that_cannot_commit_posts_nothing() {
    let home = tempfile::tempdir().unwrap();
    let (started, _) = mpsc::channel();
    let (_, wait) = mpsc::channel();
    let gate = Arc::new(Gate {
        status: 204,
        posts: AtomicUsize::new(0),
        block: AtomicBool::new(false),
        started,
        release: Mutex::new(wait),
    });
    let events = Events::with_db(Connection::open_in_memory().unwrap(), home.path().into(), gate.clone()).unwrap();
    events.subscribe(&principal(None), &args()).unwrap();
    let inbox = home.path().join(".everett/inbox");
    fs::create_dir_all(&inbox).unwrap();
    writeln!(
        File::create(inbox.join("dot-a.jsonl")).unwrap(),
        "{}",
        json!({"id":"fresh","to":"dot-a","ts":now() as f64 / 1000.0})
    )
    .unwrap();
    events
        .db
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TABLE claim_parents(id TEXT PRIMARY KEY);
             CREATE TABLE claim_checks(id TEXT REFERENCES claim_parents(id) DEFERRABLE INITIALLY DEFERRED);
             CREATE TRIGGER reject_claim AFTER UPDATE OF claim ON deliveries BEGIN
               INSERT INTO claim_checks VALUES (NEW.claim);
             END;",
        )
        .unwrap();

    let lease = events.leases().unwrap().remove(0);
    assert!(events.deliver(&lease).is_err());
    assert_eq!(gate.posts.load(Ordering::SeqCst), 0, "posted before the claim committed");

    events.db.lock().unwrap().execute_batch("DROP TRIGGER reject_claim").unwrap();
    events.deliver(&lease).unwrap();
    assert_eq!(gate.posts.load(Ordering::SeqCst), 1);
}
