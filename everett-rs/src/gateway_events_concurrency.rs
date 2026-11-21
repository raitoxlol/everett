use super::*;
use std::io::Write;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
    mpsc,
};
struct Gate {
    status: u16,
    deliveries: AtomicUsize,
    block: AtomicBool,
    started: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl Webhook for Gate {
    fn post(
        &self,
        _: &str,
        _: &[(String, String)],
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), &'static str> {
        if self.block.swap(false, Ordering::SeqCst) {
            self.started.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
        }
        let value: Value = serde_json::from_slice(body).unwrap();
        if value["type"] != "verification" {
            self.deliveries.fetch_add(1, Ordering::SeqCst);
        }
        Ok((
            if value["type"] == "verification" {
                200
            } else {
                self.status
            },
            serde_json::to_vec(&json!({"challenge":value["challenge"]})).unwrap(),
        ))
    }
}
fn p() -> Principal {
    Principal {
        owner: "owner".into(),
        agent: "dot-a".into(),
        token: None,
    }
}
fn args() -> Value {
    json!({"name":EVENT,"arguments":{"session_id":"dot-a"},"delivery":{"mode":"webhook","url":"https://callback.example/","secret":format!("whsec_{}",STANDARD.encode([7;32]))}})
}
#[test]
fn slow_verification_and_delivery_do_not_lock_unrelated_operations() {
    for delivery in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let (started, ready) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let gate = Arc::new(Gate {
            status: 410,
            deliveries: AtomicUsize::new(0),
            block: AtomicBool::new(false),
            started,
            release: Mutex::new(wait),
        });
        let events = Events::with_db(
            Connection::open_in_memory().unwrap(),
            home.path().into(),
            gate.clone(),
        )
        .unwrap();
        let mut unrelated = args();
        unrelated["delivery"]["url"] = json!("https://callback.example/unrelated");
        events.subscribe(&p(), &unrelated).unwrap();
        if delivery {
            events.subscribe(&p(), &args()).unwrap();
            fs::create_dir_all(home.path().join(".everett/inbox")).unwrap();
            let mut f = File::create(home.path().join(".everett/inbox/dot-a.jsonl")).unwrap();
            writeln!(
                f,
                "{}",
                json!({"id":"fresh","to":"dot-a","ts":now() as f64/1000.})
            )
            .unwrap();
        }
        gate.block.store(true, Ordering::SeqCst);
        let slow = events.clone();
        let thread = std::thread::spawn(move || {
            if delivery {
                slow.deliver(
                    &slow
                        .leases()
                        .unwrap()
                        .into_iter()
                        .find(|l| l.id == Events::identity(&p(), &args()).unwrap().0)
                        .unwrap(),
                )
            } else {
                slow.subscribe(&p(), &args()).map(|_| ())
            }
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let (tx, rx) = mpsc::channel();
        let fast = events.clone();
        let other = std::thread::spawn(move || {
            let result = fast.leases().is_ok()
                && fast.subscribe(&p(), &unrelated).is_ok()
                && fast.unsubscribe(&p(), &unrelated).is_ok();
            tx.send(result).unwrap();
        });
        let independent = rx.recv_timeout(Duration::from_millis(300));
        release.send(()).unwrap();
        thread.join().unwrap().unwrap();
        other.join().unwrap();
        assert_eq!(
            independent.ok(),
            Some(true),
            "network callback held global database lock, delivery={delivery}"
        );
    }
}

#[test]
fn verification_rechecks_refresh_unsubscribe_capacity_and_expiry() {
    for race in ["refresh", "unsubscribe", "capacity", "expiry"] {
        let home = tempfile::tempdir().unwrap();
        let (started, ready) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let gate = Arc::new(Gate {
            status: 410,
            deliveries: AtomicUsize::new(0),
            block: AtomicBool::new(true),
            started,
            release: Mutex::new(wait),
        });
        let events = Events::with_db(
            Connection::open_in_memory().unwrap(),
            home.path().into(),
            gate,
        )
        .unwrap();
        let slow = events.clone();
        let clock = Arc::new(AtomicI64::new(now()));
        let deadline = (race == "expiry").then(|| clock.load(Ordering::SeqCst) + 100);
        let test_clock = clock.clone();
        let thread = std::thread::spawn(move || {
            slow.subscribe_with_clock(&p(), &args(), deadline, || test_clock.load(Ordering::SeqCst))
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        match race {
            "refresh" => {
                let mut fresh = p();
                fresh.token = Some("fresh".into());
                events.subscribe(&fresh, &args()).unwrap();
            }
            "unsubscribe" => {
                events.unsubscribe(&p(), &args()).unwrap();
            }
            "capacity" => {
                for n in 0..MAX_SUBSCRIPTIONS {
                    let mut a = args();
                    a["delivery"]["url"] = json!(format!("https://callback.example/{n}"));
                    events.subscribe(&p(), &a).unwrap();
                }
            }
            "expiry" => clock.store(deadline.unwrap(), Ordering::SeqCst),
            _ => unreachable!(),
        }
        release.send(()).unwrap();
        let error = thread.join().unwrap().expect_err("stale verification accepted");
        if race == "expiry" {
            assert_eq!(error.reason, "token_expired");
        }
        let leases = events.leases().unwrap();
        match race {
            "refresh" => assert_eq!(leases[0].principal.token.as_deref(), Some("fresh")),
            "capacity" => assert_eq!(leases.len(), MAX_SUBSCRIPTIONS as usize),
            _ => assert!(leases.is_empty()),
        }
    }
}
#[test]
fn late_delivery_cannot_mutate_refreshed_subscription() {
    let home = tempfile::tempdir().unwrap();
    let (started, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let gate = Arc::new(Gate {
        status: 410,
        deliveries: AtomicUsize::new(0),
        block: AtomicBool::new(false),
        started,
        release: Mutex::new(wait),
    });
    let events = Events::with_db(
        Connection::open_in_memory().unwrap(),
        home.path().into(),
        gate.clone(),
    )
    .unwrap();
    events.subscribe(&p(), &args()).unwrap();
    fs::create_dir_all(home.path().join(".everett/inbox")).unwrap();
    let mut f = File::create(home.path().join(".everett/inbox/dot-a.jsonl")).unwrap();
    writeln!(
        f,
        "{}",
        json!({"id":"fresh","to":"dot-a","ts":now() as f64/1000.})
    )
    .unwrap();
    let lease = events.leases().unwrap().remove(0);
    gate.block.store(true, Ordering::SeqCst);
    let slow = events.clone();
    let thread = std::thread::spawn(move || slow.deliver(&lease));
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut fresh = p();
    fresh.token = Some("replacement".into());
    events.subscribe(&fresh, &args()).unwrap();
    events.deliver(&events.leases().unwrap()[0]).unwrap();
    release.send(()).unwrap();
    thread.join().unwrap().unwrap();
    let db = events.db.lock().unwrap();
    assert_eq!(
        db.query_row("SELECT done FROM deliveries", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0,
        "late acknowledgment modified replacement"
    );
    assert_eq!(
        db.query_row("SELECT token FROM subscriptions", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "replacement"
    );
}

#[test]
fn sqlite_writer_wait_cannot_extend_verified_deadline() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("events.sqlite");
    let (started, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let gate = Arc::new(Gate { status: 410, deliveries: AtomicUsize::new(0), block: AtomicBool::new(true), started, release: Mutex::new(wait) });
    let events = Events::with_db(Connection::open(&path).unwrap(), home.path().into(), gate).unwrap();
    let slow = events.clone();
    let deadline = now() + 400;
    let thread = std::thread::spawn(move || slow.subscribe_until(&p(), &args(), Some(deadline)));
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut writer = Connection::open(path).unwrap();
    let tx = writer.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    release.send(()).unwrap();
    std::thread::sleep(Duration::from_millis(550));
    tx.commit().unwrap();
    assert_eq!(thread.join().unwrap().unwrap_err().reason, "token_expired");
    assert_eq!(events.db.lock().unwrap().query_row("SELECT count(*) FROM subscriptions", [], |r| r.get::<_,i64>(0)).unwrap(), 0);
}

#[test]
fn acknowledged_event_is_not_replayed_after_subscription_refresh() {
    let home = tempfile::tempdir().unwrap();
    let (started, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let gate = Arc::new(Gate {
        status: 204,
        deliveries: AtomicUsize::new(0),
        block: AtomicBool::new(false),
        started,
        release: Mutex::new(wait),
    });
    let events = Events::with_db(Connection::open_in_memory().unwrap(), home.path().into(), gate.clone()).unwrap();
    events.subscribe(&p(), &args()).unwrap();
    let inbox = home.path().join(".everett/inbox");
    fs::create_dir_all(&inbox).unwrap();
    writeln!(File::create(inbox.join("dot-a.jsonl")).unwrap(),
        "{}", json!({"id":"fresh","to":"dot-a","ts":now() as f64 / 1000.0})).unwrap();
    let lease = events.leases().unwrap().remove(0);
    gate.block.store(true, Ordering::SeqCst);
    let slow = events.clone();
    let callback = std::thread::spawn(move || slow.deliver(&lease));
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut refreshed = p();
    refreshed.token = Some("refreshed".into());
    let result = events.subscribe(&refreshed, &args());
    release.send(()).unwrap();
    callback.join().unwrap().unwrap();
    result.unwrap();

    events.db.lock().unwrap().execute("UPDATE deliveries SET next=0", []).unwrap();
    events.deliver(&events.leases().unwrap()[0]).unwrap();
    assert_eq!(gate.deliveries.load(Ordering::SeqCst), 1);
    let done: bool = events.db.lock().unwrap().query_row("SELECT done FROM deliveries", [], |row| row.get(0)).unwrap();
    assert!(done);
}

#[test]
fn webhook_waits_for_the_delivery_claim_to_commit() {
    let home = tempfile::tempdir().unwrap();
    let (started, _) = mpsc::channel();
    let (_, wait) = mpsc::channel();
    let gate = Arc::new(Gate {
        status: 204,
        deliveries: AtomicUsize::new(0),
        block: AtomicBool::new(false),
        started,
        release: Mutex::new(wait),
    });
    let events = Events::with_db(Connection::open_in_memory().unwrap(), home.path().into(), gate.clone()).unwrap();
    events.subscribe(&p(), &args()).unwrap();
    let inbox = home.path().join(".everett/inbox");
    fs::create_dir_all(&inbox).unwrap();
    writeln!(File::create(inbox.join("dot-a.jsonl")).unwrap(),
        "{}", json!({"id":"fresh","to":"dot-a","ts":now() as f64 / 1000.0})).unwrap();
    events.db.lock().unwrap().execute_batch(
        "CREATE TABLE claim_parents(id TEXT PRIMARY KEY);
         CREATE TABLE claim_checks(id TEXT REFERENCES claim_parents(id) DEFERRABLE INITIALLY DEFERRED);
         CREATE TRIGGER reject_claim AFTER UPDATE OF claim ON deliveries BEGIN
           INSERT INTO claim_checks VALUES (NEW.claim);
         END;",
    ).unwrap();

    let lease = events.leases().unwrap().remove(0);
    let delivery = events.deliver(&lease);
    assert_eq!(gate.deliveries.load(Ordering::SeqCst), 0);
    assert_eq!(delivery.unwrap_err().reason, "event storage unavailable");

    events.db.lock().unwrap().execute_batch("DROP TRIGGER reject_claim").unwrap();
    events.deliver(&lease).unwrap();
    assert_eq!(gate.deliveries.load(Ordering::SeqCst), 1);
}

#[test]
fn acknowledged_event_is_not_replayed_after_a_busy_receipt_write() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("events.sqlite");
    let (started, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let gate = Arc::new(Gate {
        status: 204,
        deliveries: AtomicUsize::new(0),
        block: AtomicBool::new(false),
        started,
        release: Mutex::new(wait),
    });
    let events = Events::with_db(Connection::open(&path).unwrap(), home.path().into(), gate.clone()).unwrap();
    events.db.lock().unwrap().busy_timeout(Duration::ZERO).unwrap();
    events.subscribe(&p(), &args()).unwrap();
    let inbox = home.path().join(".everett/inbox");
    fs::create_dir_all(&inbox).unwrap();
    writeln!(File::create(inbox.join("dot-a.jsonl")).unwrap(),
        "{}", json!({"id":"fresh","to":"dot-a","ts":now() as f64 / 1000.0})).unwrap();
    let lease = events.leases().unwrap().remove(0);
    gate.block.store(true, Ordering::SeqCst);
    let slow = events.clone();
    let callback = std::thread::spawn(move || slow.deliver(&lease));
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut refreshed = p();
    refreshed.token = Some("refreshed".into());
    events.subscribe(&refreshed, &args()).unwrap();

    let mut writer = Connection::open(path).unwrap();
    let tx = writer.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    tx.execute("UPDATE deliveries SET next=0", []).unwrap();
    release.send(()).unwrap();
    assert_eq!(callback.join().unwrap().unwrap_err().reason, "event storage unavailable");
    tx.commit().unwrap();

    events.deliver(&events.leases().unwrap()[0]).unwrap();
    assert_eq!(gate.deliveries.load(Ordering::SeqCst), 1);
    let done: bool = events.db.lock().unwrap().query_row("SELECT done FROM deliveries", [], |row| row.get(0)).unwrap();
    assert!(done);
}
