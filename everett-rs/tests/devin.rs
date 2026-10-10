mod common;

use common::lib_home;
use everett::adapters::devin;
use everett::registry;
use everett::route::resume_command;
use everett::send::{command_for, is_busy, spawn_command};
use everett::session::{now, Session};
use serde_json::json;
use std::path::{Path, PathBuf};

const SCHEMA: &str = "CREATE TABLE sessions (id TEXT PRIMARY KEY, working_directory TEXT, title TEXT, model TEXT,
  created_at REAL, last_activity_at REAL, hidden INTEGER DEFAULT 0);";
const NODES: &str = "CREATE TABLE message_nodes (node_id INTEGER PRIMARY KEY, session_id TEXT NOT NULL,
  created_at REAL, chat_message TEXT);";

fn data(home: &Path) -> PathBuf {
    let dir = home.join(".local/share/devin/cli");
    std::env::set_var("DEVIN_HOME", &dir);
    dir
}

fn make_db(path: &Path, nodes_table: bool, rows: &[(&str, &str, &str, f64, f64, i64)], nodes: &[(&str, i64, f64, serde_json::Value)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let con = rusqlite::Connection::open(path).unwrap();
    con.execute_batch(SCHEMA).unwrap();
    if nodes_table {
        con.execute_batch(NODES).unwrap();
    }
    for (id, cwd, title, created, last, hidden) in rows {
        con.execute("INSERT INTO sessions VALUES (?1, ?2, ?3, 'swe-2-max', ?4, ?5, ?6)",
            rusqlite::params![id, cwd, title, created, last, hidden]).unwrap();
    }
    for (sid, node, ts, msg) in nodes {
        con.execute("INSERT INTO message_nodes VALUES (?1, ?2, ?3, ?4)", rusqlite::params![node, sid, ts, msg.to_string()]).unwrap();
    }
}

fn user(text: &str) -> serde_json::Value {
    json!({"role": "user", "content": text, "metadata": {"is_user_input": true}})
}

fn transcript(path: &Path, sid: &str, steps: serde_json::Value, title: Option<&str>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut doc = json!({"session_id": sid, "agent": {"model_name": "swe-2-max"}, "steps": steps});
    if let Some(t) = title {
        doc["title"] = json!(t);
    }
    std::fs::write(path, doc.to_string()).unwrap();
}

fn step(text: serde_json::Value, ts: f64) -> serde_json::Value {
    json!({"source": "user", "content": text, "metadata": {"created_at": ts}})
}

fn seeded_db(home: &Path) -> PathBuf {
    let t = now();
    let db = data(home).join("sessions.db");
    make_db(&db, true, &[
        ("d-1", "/work/api", "Ship the adapter", t - 600.0, t - 60.0, 0),
        ("d-hidden", "/work/x", "secret", t - 900.0, t - 30.0, 1),
        ("d-old", "/work/x", "old", t - 864000.0, t - 864000.0, 0),
        ("d-quiet", "", "", t - 100.0, t - 90.0, 0),
    ], &[
        ("d-1", 1, t - 590.0, user("add a devin adapter to everett")),
        ("d-1", 2, t - 580.0, json!({"role": "assistant", "content": "on it"})),
        ("d-1", 3, t - 70.0, user("and keep it read-only")),
        ("d-1", 4, t - 60.0, json!({"metadata": {"request_id": "r1"}, "tool_calls": []})),
        ("d-quiet", 10, t - 95.0, user("<system_reminder>injected</system_reminder>")),
        ("d-quiet", 11, t - 92.0, user("real ask")),
    ]);
    db
}

#[test]
fn db_sessions_list_read_only_with_hours_window() {
    let h = lib_home();
    let db = seeded_db(&h.path());
    let before = std::fs::read(&db).unwrap();
    let found = devin::scan(72.0);
    let mut ids: Vec<_> = found.iter().map(|s| s.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, ["d-1", "d-quiet"]);
    let s = found.iter().find(|s| s.id == "d-1").unwrap();
    assert_eq!((s.harness.as_str(), s.cwd.as_str(), s.title.as_str()), ("devin", "/work/api", "Ship the adapter"));
    assert_eq!((s.first_user.as_str(), s.last_user.as_str()), ("add a devin adapter to everett", "and keep it read-only"));
    assert_eq!(found.iter().find(|s| s.id == "d-quiet").unwrap().first_user, "real ask");
    assert_eq!(std::fs::read(&db).unwrap(), before);
    assert_eq!(devin::scan(1.0).len(), 2);
    assert!(devin::scan(0.0).is_empty());
}

#[test]
fn resume_spawn_and_busy() {
    let h = lib_home();
    let db = seeded_db(&h.path());
    let s = devin::scan(72.0).into_iter().find(|s| s.id == "d-1").unwrap();
    assert_eq!(command_for(&s, "go").unwrap(), ["devin", "--resume", "d-1", "--print", "go"]);
    assert!(resume_command(&s, "go").contains("devin --resume"));
    assert_eq!(spawn_command("devin", "new", "", "").unwrap(), ["devin", "-p", "new"]);
    let db_backed = Session::new("devin", "d", "", &db.to_string_lossy(), "", now() - 3600.0);
    std::fs::File::options().write(true).open(&db).unwrap().set_modified(std::time::SystemTime::now()).unwrap();
    assert!(!is_busy(&db_backed, "", now()), "other sessions keep writing the shared db");
    assert!(command_for(&Session::new("devin", "", "", "", "", 0.0), "go").is_err());
}

#[test]
fn iso_timestamps_parse() {
    let h = lib_home();
    let alt = data(&h.path()).join("alt/sessions.db");
    std::fs::create_dir_all(alt.parent().unwrap()).unwrap();
    let con = rusqlite::Connection::open(&alt).unwrap();
    con.execute_batch(SCHEMA).unwrap();
    let recent = chrono_like(now() - 3600.0);
    con.execute("INSERT INTO sessions VALUES ('iso-1', '/w', 'iso times', 'm', '2026-10-01T00:00:00Z', ?1, 0)", [recent]).unwrap();
    drop(con);
    let (sessions, _) = devin::read_db(&alt, 72.0).unwrap();
    assert_eq!(sessions[0].title, "iso times");
    assert!(sessions[0].started.starts_with("2026-10-01"), "{}", sessions[0].started);
}

fn chrono_like(epoch: f64) -> String {
    let out = std::process::Command::new("date").args(["-u", "-d", &format!("@{}", epoch as i64), "+%Y-%m-%dT%H:%M:%S+00:00"]).output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => {
            let o = std::process::Command::new("date").args(["-u", "-r", &format!("{}", epoch as i64), "+%Y-%m-%dT%H:%M:%S+00:00"]).output().unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }
    }
}

#[test]
fn broken_db_still_scans_transcripts() {
    let h = lib_home();
    let base = data(&h.path());
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("sessions.db"), "not sqlite").unwrap();
    transcript(&base.join("transcripts/t-9.json"), "t-9", json!([step(json!("fix the flaky test"), now())]), None);
    assert!(devin::scan(72.0).iter().any(|s| s.id == "t-9"));
}

#[test]
fn transcripts_list_without_db_and_db_metadata_wins() {
    let h = lib_home();
    let base = data(&h.path());
    let t = now();
    transcript(&base.join("transcripts/t-1.json"), "t-1", json!([
        step(json!("redo the hero"), t - 500.0),
        {"source": "agent", "content": "done", "metadata": {"created_at": t - 400.0}},
        step(json!([{"type": "text", "text": "make it wider"}]), t - 300.0),
    ]), Some("Landing page"));
    transcript(&base.join("transcripts/t-old.json"), "t-old", json!([step(json!("old"), t - 864000.0)]), None);
    std::fs::write(base.join("transcripts/bad.json"), "{oops").unwrap();
    let found = devin::scan(72.0);
    assert_eq!(found.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["t-1"]);
    let s = &found[0];
    assert_eq!((s.harness.as_str(), s.title.as_str(), s.first_user.as_str(), s.last_user.as_str()),
        ("devin", "Landing page", "redo the hero", "make it wider"));
    assert!(s.path.ends_with("t-1.json") && !s.started.is_empty());

    make_db(&base.join("sessions.db"), false, &[
        ("t-1", "/work/site", "", t - 500.0, t - 200.0, 0),
        ("d-only", "/work/api", "db only", t - 100.0, t - 50.0, 0),
    ], &[]);
    let found = devin::scan(72.0);
    let s = found.iter().find(|s| s.id == "t-1").unwrap();
    assert_eq!(s.cwd, "/work/site");
    assert_eq!((s.first_user.as_str(), s.last_user.as_str()), ("redo the hero", "make it wider"));
    assert!(found.iter().any(|s| s.id == "d-only"));
}

#[test]
fn devin_home_override_default_and_merged_dirs() {
    let h = lib_home();
    let base = data(&h.path());
    assert_eq!(devin::data_dir(), base);
    std::env::remove_var("DEVIN_HOME");
    assert_eq!(devin::data_dir(), h.path().join(devin::DEFAULT_REL));
    let other = h.path().join("other-devin-home");
    std::env::set_var("DEVIN_HOME", &other);
    transcript(&other.join("transcripts/t-a.json"), "t-a", json!([step(json!("override store"), now())]), None);
    transcript(&h.path().join(devin::DEFAULT_REL).join("transcripts/t-b.json"), "t-b", json!([step(json!("default store"), now())]), Some("Findable"));
    let mut ids: Vec<_> = devin::scan(72.0).into_iter().map(|s| s.id).collect();
    ids.sort();
    assert_eq!(ids, ["t-a", "t-b"]);
    let found = registry::scan(72.0, false, None, "devin");
    assert!(found.iter().all(|s| s.harness == "devin"));
    assert_eq!(registry::find("t-b", &found).unwrap().id, "t-b");
    std::env::remove_var("DEVIN_HOME");
}
