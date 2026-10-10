mod common;

use common::{lib_home, LibHome};
use everett::adapters::grok;
use everett::cards::{card_path, AUTO_MARKER};
use everett::hooks_common::grok_stop_hook;
use everett::route::resume_command;
use everett::send::{command_for, is_busy, spawn_command};
use everett::session::{now, Session};
use everett::{install, registry};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn jsonl(path: &Path, rows: &[Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, rows.iter().map(|r| format!("{}\n", r)).collect::<String>()).unwrap();
}

fn grok_session(home: &Path, sid: &str, cwd: &str, prompts: &[&str], summary: Value) -> PathBuf {
    let folder = home.join(".grok/sessions").join(grok::quote(cwd)).join(sid);
    std::fs::create_dir_all(&folder).unwrap();
    let mut meta = json!({"info": {"id": sid, "cwd": cwd}, "created_at": "2026-09-24T00:00:00Z",
        "last_active_at": "2026-09-24T00:05:00Z", "agent_name": "grok-build-plan"});
    for (k, v) in summary.as_object().unwrap() {
        meta[k] = v.clone();
    }
    std::fs::write(folder.join("summary.json"), meta.to_string()).unwrap();
    let mut rows = vec![
        json!({"type": "system", "content": "You are Grok."}),
        json!({"type": "user", "content": [{"type": "text", "text": "<user_info>\nOS Version: macos\n</user_info>"}]}),
        json!({"type": "user", "synthetic_reason": "system_reminder", "content": [{"type": "text", "text": "<system-reminder>\nskills\n</system-reminder>"}]}),
    ];
    for (i, p) in prompts.iter().enumerate() {
        rows.push(json!({"type": "user", "prompt_index": i, "content": [{"type": "text", "text": format!("<user_query>\n{}\n</user_query>", p)}]}));
    }
    rows.push(json!({"type": "assistant", "content": "Done. Next: run the integration test.", "model_id": "grok-4.7"}));
    jsonl(&folder.join("chat_history.jsonl"), &rows);
    let mut history = std::fs::OpenOptions::new().create(true).append(true).open(folder.parent().unwrap().join("prompt_history.jsonl")).unwrap();
    for p in prompts {
        writeln!(history, "{}", json!({"timestamp": "2026-09-24T00:01:00Z", "session_id": sid, "prompt": p, "is_bash": false})).unwrap();
    }
    writeln!(history, "{}", json!({"timestamp": "2026-09-24T00:02:00Z", "session_id": sid, "prompt": "ls -la", "is_bash": true})).unwrap();
    folder
}

fn age(folder: &Path, secs: u64) {
    for f in std::fs::read_dir(folder).unwrap().flatten() {
        std::fs::File::options().write(true).open(f.path()).unwrap().set_modified(SystemTime::now() - Duration::from_secs(secs)).unwrap();
    }
}

fn home(h: &LibHome) -> PathBuf {
    h.path()
}

#[test]
fn lists_sessions_with_cwd_headline_and_title() {
    let h = lib_home();
    grok_session(&home(&h), "01a0-api", "/work/api", &["add retries to the upload client", "cap at five"], json!({"generated_title": "Upload retries"}));
    let found = grok::scan(72.0);
    assert_eq!(found.len(), 1);
    let s = &found[0];
    assert_eq!((s.harness.as_str(), s.id.as_str(), s.cwd.as_str(), s.title.as_str()), ("grok", "01a0-api", "/work/api", "Upload retries"));
    assert_eq!((s.first_user.as_str(), s.last_user.as_str()), ("add retries to the upload client", "cap at five"));
    assert!(!s.auto);
}

#[test]
fn cwd_decoded_from_folder_and_chat_history_fallback() {
    let h = lib_home();
    let folder = grok_session(&home(&h), "01a0-web", "/work/my site", &["fix the hero"], json!({}));
    std::fs::remove_file(folder.parent().unwrap().join("prompt_history.jsonl")).unwrap();
    let mut meta: Value = serde_json::from_str(&std::fs::read_to_string(folder.join("summary.json")).unwrap()).unwrap();
    meta.as_object_mut().unwrap().remove("info");
    std::fs::write(folder.join("summary.json"), meta.to_string()).unwrap();
    let s = &grok::scan(72.0)[0];
    assert_eq!((s.id.as_str(), s.cwd.as_str(), s.first_user.as_str()), ("01a0-web", "/work/my site", "fix the hero"));
}

#[test]
fn empty_sessions_dropped_headless_is_auto_and_running_comes_from_active_pids() {
    let h = lib_home();
    grok_session(&home(&h), "empty", "/w", &[], json!({}));
    grok_session(&home(&h), "batch", "/w", &["nightly report"], json!({"session_kind": "headless"}));
    let found = grok::scan(72.0);
    assert_eq!(found.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["batch"]);
    assert!(found[0].auto);
    assert!(!registry::scan(72.0, false, None, "").iter().any(|s| s.id == "batch"));

    for sid in ["live", "dead"] {
        age(&grok_session(&home(&h), sid, "/w", &["hi"], json!({})), 3600);
    }
    std::fs::write(home(&h).join(".grok/active_sessions.json"), json!([
        {"session_id": "live", "pid": std::process::id(), "cwd": "/w"},
        {"session_id": "dead", "pid": 4194304 + 12345, "cwd": "/w"}]).to_string()).unwrap();
    let all = registry::scan(72.0, false, None, "");
    assert!(all.iter().find(|s| s.id == "live").unwrap().running);
    assert!(!all.iter().find(|s| s.id == "dead").unwrap().running);
}

#[test]
fn send_spawn_and_busy() {
    let h = lib_home();
    let s = Session::new("grok", "01a0-x", "/w", "/tmp/x", "", now());
    assert_eq!(command_for(&s, "go").unwrap(), ["grok", "--resume", "01a0-x", "-p", "go"]);
    assert_eq!(spawn_command("grok", "go", "u-1", "").unwrap(), ["grok", "--session-id", "u-1", "-p", "go"]);
    assert!(resume_command(&s, "go").contains("grok --resume 01a0-x"));
    let folder = grok_session(&home(&h), "b", "/w", &["hi"], json!({}));
    age(&folder, 3600);
    let b = Session::new("grok", "b", "/w", &folder.to_string_lossy(), "", now() - 3600.0);
    assert!(!is_busy(&b, "", now()), "a directory mtime is not a turn");
    std::fs::File::options().write(true).open(folder.join("chat_history.jsonl")).unwrap().set_modified(SystemTime::now()).unwrap();
    assert!(is_busy(&b, "", now()));
}

#[test]
fn stop_hook_writes_auto_card_and_ignores_bad_input() {
    let h = lib_home();
    grok_session(&home(&h), "card-1", "/work/api", &["add retries to the upload client"], json!({}));
    grok_stop_hook(&json!({"hookEventName": "stop", "hook_event_name": "Stop", "sessionId": "card-1", "cwd": "/work/api"}).to_string());
    let text = std::fs::read_to_string(card_path("card-1")).unwrap();
    assert!(text.starts_with(AUTO_MARKER));
    assert!(text.contains("Api: add retries to the upload client") && text.contains("Next: run the integration test."), "{text}");
    for raw in ["not json", "[]", &json!({"sessionId": "../x"}).to_string(), &json!({"sessionId": "missing"}).to_string()] {
        grok_stop_hook(raw);
    }
    assert!(!card_path("missing").exists());
}

#[test]
fn install_hooks_writes_grok_hook_file() {
    let h = lib_home();
    let state = install::installed("grok", None).unwrap();
    assert!(state.values().all(|v| !v) && state.contains_key("Stop") && state.contains_key("PostToolUse"));
    let report = install::apply("grok", None).unwrap();
    let path = home(&h).join(".grok/hooks/everett.json");
    assert!(report.contains(&path.to_string_lossy().to_string()));
    let data: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(data["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap().ends_with(" hook grok_stop"));
    assert!(install::installed("grok", None).unwrap().values().all(|v| *v));
    assert!(install::apply("grok", None).unwrap().contains("already installed"));
}
