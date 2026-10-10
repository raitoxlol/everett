use everett::route::{is_coordinator, local_route, relevant_sessions, route_with};
use everett::session::{now, Session};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::rc::Rc;

const ASK: &str = "what's the status of atlas";

fn session(harness: &str, id: &str, cwd: &str, age: f64, title: &str, card: &str, first: &str, last: &str) -> Session {
    let mut s = Session::new(harness, id, cwd, &format!("/tmp/{}.jsonl", id), "", now() - age);
    s.title = title.into();
    s.card = card.into();
    s.first_user = first.into();
    s.last_user = last.into();
    s
}

fn coordinator() -> Session {
    session("claude", "coord-1", "/Users/alice", 300.0, "Session sweep",
        "Coordinator session: using Everett to list sessions and ask the Atlas session about its status.",
        "use everett to check on my other sessions", "everett_send: asking the atlas session for a status update")
}

fn worker() -> Session {
    session("codex", "work-1", "/Users/alice/Atlas", 3600.0, "Atlas ingest pipeline",
        "Implementing the Atlas ingest pipeline; wiring up the scheduler next.",
        "build the atlas ingest pipeline", "wired up the scheduler for atlas ingest")
}

fn caller() -> Session {
    session("claude", "caller-1", "/Users/alice", 10.0, "Routing question", "Asking everett to route a request about atlas.", "", "")
}

fn ids(pool: &[Session]) -> Vec<&str> {
    pool.iter().map(|s| s.id.as_str()).collect()
}

#[test]
fn coordinator_detection() {
    assert!(is_coordinator(&coordinator()));
    assert!(!is_coordinator(&worker()));
}

#[test]
fn relevant_sessions_drop_coordinators_and_the_caller_but_never_strand() {
    let pool = relevant_sessions(ASK, &[coordinator(), worker()], None);
    assert_eq!(ids(&pool), ["work-1"]);
    let pool = relevant_sessions(ASK, &[worker(), caller()], Some("caller-1"));
    assert!(!ids(&pool).contains(&"caller-1"));
    let pool = relevant_sessions(ASK, &[coordinator()], None);
    assert_eq!(ids(&pool), ["coord-1"]);
}

#[test]
fn local_router_picks_the_worker() {
    let r = local_route(ASK, &[coordinator(), worker()], Some(now()), None);
    if r["decision"] == "SESSION" {
        assert_eq!(r["session"]["id"], "work-1");
    } else {
        assert_eq!(r["decision"], "ASK");
        assert!(r.get("suggested").and_then(|v| v.as_str()).unwrap_or("").contains("Atlas"), "{r:?}");
    }
    let r = local_route(ASK, &[coordinator(), worker(), caller()], Some(now()), Some("caller-1"));
    assert_ne!(r.get("session").map(|s| s["id"].clone()), Some(json!("caller-1")));
}

fn choose(option: &str, confidence: f64) -> Map<String, Value> {
    json!({"choice": option, "confidence": confidence}).as_object().unwrap().clone()
}

#[test]
fn jev_only_sees_the_worker_as_a_candidate() {
    let seen = Rc::new(RefCell::new(Map::new()));
    let s1 = seen.clone();
    let jev = move |_: &str, crit: &Map<String, Value>, _: &str| {
        *s1.borrow_mut() = crit.clone();
        Ok(crit.iter().find(|(_, d)| d.as_str().unwrap_or("").contains("ingest pipeline"))
            .map(|(k, _)| choose(k, 0.9))
            .unwrap_or_else(|| choose("none", 0.0)))
    };
    let r = route_with(ASK, &[coordinator(), worker()], None, Some("k"), &jev, None).unwrap();
    assert_eq!((r["decision"].clone(), r["session"]["id"].clone()), (json!("SESSION"), json!("work-1")));
    assert!(!seen.borrow().iter().any(|(k, v)| k != "new" && k != "none" && v.as_str().unwrap_or("").contains("coord-1")));

    let s2 = seen.clone();
    let jev_none = move |_: &str, crit: &Map<String, Value>, _: &str| {
        *s2.borrow_mut() = crit.clone();
        Ok(choose("none", 0.0))
    };
    route_with(ASK, &[coordinator(), worker(), caller()], None, Some("k"), &jev_none, Some("caller-1")).unwrap();
    let offered: String = seen.borrow().values().map(|v| v.as_str().unwrap_or("").to_string()).collect();
    assert!(!offered.contains("caller-1"));
}
