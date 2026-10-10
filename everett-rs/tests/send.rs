mod common;

use common::{fixture, lib_home};
use everett::adapters::{hermes, pi};
use everett::registry;
use everett::send::{command_for, hop_env, is_busy, refuse_self, spawn, spawn_command, MAX_HOPS};
use everett::session::{now, Session};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn make_db(path: &Path, rows: &[(&str, &str, f64, &str, &str, Option<&str>, Option<&str>, i64, f64)], messages: &[(&str, &str, &str, f64)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let con = rusqlite::Connection::open(path).unwrap();
    con.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, started_at REAL NOT NULL, ended_at REAL,
           cwd TEXT, title TEXT, profile_name TEXT, parent_session_id TEXT, hidden INTEGER DEFAULT 0,
           archived INTEGER DEFAULT 0, last_activity_at REAL);
         CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, role TEXT NOT NULL,
           content TEXT, timestamp REAL NOT NULL);",
    )
    .unwrap();
    for r in rows {
        con.execute("INSERT INTO sessions (id, source, started_at, cwd, title, profile_name, parent_session_id, hidden, last_activity_at)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)", rusqlite::params![r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8]).unwrap();
    }
    for m in messages {
        con.execute("INSERT INTO messages (session_id, role, content, timestamp) VALUES (?1,?2,?3,?4)",
            rusqlite::params![m.0, m.1, m.2, m.3]).unwrap();
    }
}

fn hermes_home(home: &Path) -> std::path::PathBuf {
    let t = now();
    let db = home.join(".hermes/profiles/max/state.db");
    make_db(&db, &[
        ("h-cli", "cli", t - 600.0, "/work/api", "API retries", Some("max"), None, 0, t - 60.0),
        ("h-tg", "telegram", t - 900.0, "", "", Some("max"), None, 0, t - 120.0),
        ("h-cron", "cron", t - 900.0, "", "nightly", Some("max"), None, 0, t - 30.0),
        ("h-hidden", "cli", t - 900.0, "", "x", Some("max"), None, 1, t - 30.0),
        ("h-child", "cli", t - 900.0, "", "x", Some("max"), Some("h-cli"), 0, t - 30.0),
        ("h-old", "cli", t - 864000.0, "", "old", Some("max"), None, 0, t - 864000.0),
    ], &[
        ("h-cli", "user", "add retries to the upload client", t - 590.0),
        ("h-cli", "assistant", "ok", t - 580.0),
        ("h-cli", "user", "cap them at five", t - 70.0),
        ("h-tg", "user", "what is on my calendar", t - 130.0),
    ]);
    make_db(&home.join(".hermes/state.db"), &[("root-1", "cli", t - 60.0, "/w", "Root", None, None, 0, t - 50.0)],
        &[("root-1", "user", "hello root", t - 55.0)]);
    db
}

fn by_id(sessions: Vec<Session>) -> HashMap<String, Session> {
    sessions.into_iter().map(|s| (s.id.clone(), s)).collect()
}

#[test]
fn hermes_reads_sessions_read_only_and_skips_broken_dbs() {
    let h = lib_home();
    let db = hermes_home(&h.path());
    std::fs::create_dir_all(h.path().join(".hermes/profiles/bad")).unwrap();
    std::fs::write(h.path().join(".hermes/profiles/bad/state.db"), "not sqlite").unwrap();
    let before = std::fs::read(&db).unwrap();
    let found = by_id(hermes::scan(72.0));
    let mut ids: Vec<_> = found.keys().cloned().collect();
    ids.sort();
    assert_eq!(ids, ["h-cli", "h-cron", "h-tg", "root-1"]);
    let s = &found["h-cli"];
    assert_eq!((s.harness.as_str(), s.profile.as_str(), s.cwd.as_str(), s.title.as_str()), ("hermes", "max", "/work/api", "API retries"));
    assert_eq!((s.first_user.as_str(), s.last_user.as_str()), ("add retries to the upload client", "cap them at five"));
    assert!(found["h-cron"].auto && !found["h-tg"].auto);
    assert_eq!(found["root-1"].profile, "default");
    assert_eq!(std::fs::read(&db).unwrap(), before);
}

#[test]
fn hermes_resume_commands_and_chat_sessions_refused() {
    let h = lib_home();
    let db = hermes_home(&h.path());
    let found = by_id(hermes::scan(72.0));
    assert_eq!(command_for(&found["h-cli"], "go").unwrap(), ["hermes", "-p", "max", "chat", "--resume", "h-cli", "-Q", "-q", "go"]);
    assert_eq!(command_for(&found["root-1"], "go").unwrap()[..3], ["hermes", "-p", "default"]);
    assert!(command_for(&found["h-tg"], "go").is_err());
    let mut s = Session::new("hermes", "h", "", &db.to_string_lossy(), "", now() - 3600.0);
    s.source = "cli".into();
    assert!(!is_busy(&s, "", now()), "busy follows session activity, not the shared db mtime");
}

#[test]
fn pi_sessions_share_omp_format() {
    let h = lib_home();
    let rows = [
        json!({"type": "session", "version": 3, "id": "pi-1", "timestamp": "2026-09-24T00:00:00Z", "cwd": "/work/site"}),
        json!({"type": "message", "message": {"role": "user", "content": [{"type": "text", "text": "redo the hero"}]}}),
        json!({"type": "message", "message": {"role": "assistant", "content": [{"type": "text", "text": "done"}]}}),
    ];
    let path = h.path().join(".pi/agent/sessions/--work-site--/2026-09-24_pi-1.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, common::jsonl(&rows)).unwrap();
    let found = pi::scan(72.0);
    assert_eq!(found.len(), 1);
    let s = &found[0];
    assert_eq!((s.harness.as_str(), s.id.as_str(), s.cwd.as_str(), s.first_user.as_str()), ("pi", "pi-1", "/work/site", "redo the hero"));
    assert_eq!(command_for(s, "go").unwrap(), ["pi", "--session", &path.to_string_lossy(), "-p", "go"]);
}

fn direct_sessions() -> Vec<Session> {
    let mut a = Session::new("claude", "abc-111", "/work/atlas", "", "", 0.0);
    a.card = "Atlas: release notes".into();
    let mut b = Session::new("codex", "abd-222", "/work/app", "", "", 0.0);
    b.title = "Login bug".into();
    vec![a, b, Session::new("omp", "xyz-333", "/work/app", "", "", 0.0)]
}

#[test]
fn find_by_exact_prefix_name_and_folder() {
    let sessions = direct_sessions();
    assert_eq!(registry::find("abc", &sessions).unwrap().id, "abc-111");
    assert_eq!(registry::find("xyz-333", &sessions).unwrap().id, "xyz-333");
    assert_eq!(registry::find("Atlas", &sessions).unwrap().id, "abc-111");
    let err = registry::find("ab", &sessions).unwrap_err().to_string();
    assert!(err.contains("abc-111") && err.contains("abd-222"), "{err}");
    assert!(registry::find("app", &sessions).is_err(), "two sessions in that folder");
    assert!(registry::find("nothing", &sessions).is_err());
}

fn fake_bin(dir: &Path, name: &str, script: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{}\n", script)).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn cli_send_to_skips_routing_and_ambiguity_exits_2() {
    let fx = fixture();
    let cwd = fx.home().to_string_lossy().to_string();
    for id in ["c-1", "c-2"] {
        fx.write(&format!(".claude/projects/p/{}.jsonl", id), &common::jsonl(&[json!({"type": "user", "sessionId": id,
            "cwd": cwd, "timestamp": "2026-01-01T00:00:00Z", "message": {"role": "user", "content": format!("task {}", id)}})]));
        let quiet = std::time::SystemTime::now() - std::time::Duration::from_secs(300);
        std::fs::File::options().write(true).open(fx.home().join(format!(".claude/projects/p/{}.jsonl", id)))
            .unwrap().set_modified(quiet).unwrap();
    }
    let bin = fx.home().join("bin");
    fake_bin(&bin, "claude", "printf '%s\\n' \"$@\" > \"$HOME/claude-args\"; echo done");
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let out = fx.run_stdin(&["send", "--to", "c-1", "ship it", "--mode", "resume", "--json"], "", &[("PATH", &path)]);
    assert_eq!(out.status.code(), Some(0), "{}{}", fx.stdout(&out), String::from_utf8_lossy(&out.stderr));
    let result: Value = serde_json::from_str(&fx.stdout(&out)).unwrap();
    assert_eq!((result["router"].clone(), result["reply"].clone()), (json!("direct"), json!("done")));
    let args = std::fs::read_to_string(fx.home().join("claude-args")).unwrap();
    assert_eq!(args.lines().collect::<Vec<_>>(), ["--resume", "c-1", "--print", "ship it"]);
    let out = fx.run(&["send", "--to", "c-", "x"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn hop_guard_and_self_send() {
    let _h = lib_home();
    std::env::set_var("EVERETT_HOPS", "1");
    let env = hop_env().unwrap();
    assert_eq!((env["EVERETT_HOPS"].as_str(), env["EVERETT_SEND"].as_str()), ("2", "1"));
    std::env::set_var("EVERETT_HOPS", MAX_HOPS.to_string());
    assert_eq!(hop_env().unwrap_err().code, 7);
    std::env::remove_var("EVERETT_HOPS");
    let s = Session::new("claude", "me-123", "", "", "", 0.0);
    assert!(refuse_self(&s, Some("me-123")).is_err());
    assert!(refuse_self(&s, Some("other")).is_ok());
    std::env::set_var("EVERETT_SESSION_ID", "me-123");
    assert!(refuse_self(&s, None).is_err());
    std::env::remove_var("EVERETT_SESSION_ID");
    let fx = fixture();
    assert_eq!(fx.run_stdin(&["send", "anything"], "", &[("EVERETT_HOPS", "3")]).status.code(), Some(7));
}

#[test]
fn spawn_commands() {
    assert_eq!(spawn_command("claude", "hi", "u-1", "").unwrap(), ["claude", "--session-id", "u-1", "--print", "hi"]);
    assert_eq!(spawn_command("codex", "hi", "", "/t/o").unwrap(), ["codex", "exec", "--skip-git-repo-check", "-o", "/t/o", "hi"]);
    assert_eq!(spawn_command("omp", "hi", "", "").unwrap(), ["omp", "-p", "hi"]);
    assert!(spawn_command("vim", "hi", "", "").is_err());
}

fn bin_env(bin: &Path) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("PATH".into(), format!("{}:/usr/bin:/bin", bin.display()));
    env
}

#[test]
fn claude_spawn_preassigns_id_and_records_it() {
    let h = lib_home();
    let bin = h.path().join("bin");
    fake_bin(&bin, "claude", "printf '%s\\n' \"$PWD\" \"$@\" > \"$HOME/claude-args\"; echo OK");
    let cwd = h.path().to_string_lossy().to_string();
    let result = spawn("claude", "reply OK", &cwd, 20.0, Some(&bin_env(&bin))).unwrap();
    assert_eq!(result.reply, "OK");
    let args = std::fs::read_to_string(h.path().join("claude-args")).unwrap();
    let args: Vec<_> = args.lines().collect();
    assert_eq!(std::fs::canonicalize(args[0]).unwrap(), std::fs::canonicalize(&cwd).unwrap());
    assert_eq!(&args[1..4], ["--session-id", result.session_id.as_str(), "--print"]);
    let spawned = registry::spawned_ids();
    assert!(spawned.contains(&result.session_id));
    let mut hidden = Session::new("claude", &result.session_id, "", "", "", 0.0);
    hidden.auto = true;
    assert!(!registry::is_auto(&hidden, &spawned));
}

#[test]
fn codex_spawn_reads_last_message_and_session_id() {
    let h = lib_home();
    let bin = h.path().join("bin");
    fake_bin(&bin, "codex", "while [ \"$1\" != -o ]; do shift; done; printf 'the answer' > \"$2\"; echo noise; \
        echo 'session id: 01a0d358-0f69-77f3-950a-8bfbe2f5a13a' >&2");
    let result = spawn("codex", "q", &h.path().to_string_lossy(), 20.0, Some(&bin_env(&bin))).unwrap();
    assert_eq!((result.reply.as_str(), result.session_id.as_str()), ("the answer", "01a0d358-0f69-77f3-950a-8bfbe2f5a13a"));
}

#[test]
fn spawn_errors() {
    let h = lib_home();
    let bin = h.path().join("bin");
    fake_bin(&bin, "claude", "echo boom >&2; exit 3");
    let env = bin_env(&bin);
    assert!(spawn("claude", "x", &h.path().join("missing").to_string_lossy(), 20.0, Some(&env)).is_err());
    let err = spawn("claude", "x", &h.path().to_string_lossy(), 20.0, Some(&env)).err().unwrap();
    assert_eq!(err.code, 6);
    assert!(err.to_string().contains("boom"));
    let mut no_harness = env.clone();
    no_harness.insert("PATH".into(), "/nonexistent".into());
    assert_eq!(spawn("claude", "x", &h.path().to_string_lossy(), 20.0, Some(&no_harness)).err().unwrap().code, 5);
}

#[test]
fn spawn_dir_defaults_to_best_match() {
    let mut s = Session::new("claude", "a", "/work/atlas", "", "", now());
    s.card = "atlas release".into();
    assert_eq!(everett::route::best_dir("atlas docs", std::slice::from_ref(&s)), "/work/atlas");
    assert_eq!(everett::route::best_dir("penguins", &[s]), "");
}
