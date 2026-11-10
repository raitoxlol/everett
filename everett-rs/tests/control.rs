mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use common::{fixture, mcp_calls, tool_json};
use serde_json::{json, Value};

const THREAD: &str = "thread-123";
const CLAUDE: &str = "0f2c7f4e-1111-4a3b-9c1d-123456789abc";

/// Records argv and answers like `t3 ... --json`; exit 2 mimics "waiting for an answer".
fn fake_t3(fx: &common::Fixture, exit: i32) -> String {
    let path = fx.home().join("fake-t3");
    fs::write(&path, format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/t3-argv\"\necho 'noise' >&2\nprintf '%s\\n' '{{\"ok\":true,\"id\":\"{THREAD}\"}}'\nexit {exit}\n"
    )).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path.to_string_lossy().into_owned()
}

fn argv(fx: &common::Fixture) -> Vec<String> {
    fs::read_to_string(fx.home().join("t3-argv")).unwrap().lines().map(str::to_string).collect()
}

fn call(fx: &common::Fixture, t3: &str, args: Value) -> Value {
    let out = mcp_calls(fx, &[], &[("EVERETT_T3_BIN", t3), ("EVERETT_SESSION_ID", "caller")],
        &[("everett_control", args)]);
    out[0].clone()
}

#[test]
fn t3_threads_start_send_and_read_use_the_t3_cli() {
    let fx = fixture();
    let t3 = fake_t3(&fx, 0);

    let r = tool_json(&call(&fx, &t3, json!({"action":"threads","project":"/work/p"})));
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(argv(&fx), ["thread", "list", "--json", "--project", "/work/p"]);

    let r = tool_json(&call(&fx, &t3, json!({"action":"start","project":"/work/p","text":"build it",
        "title":"T","provider":"codex","model":"gpt","worktree":"main","branch":"b","wait":true})));
    assert_eq!((r["via"].clone(), r["started"].clone(), r["result"]["id"].clone()), (json!("t3"), json!(true), json!(THREAD)), "{r}");
    assert_eq!(argv(&fx), ["thread", "start", "--json", "--title", "T", "--provider", "codex", "--model", "gpt",
        "--worktree", "main", "--branch", "b", "--wait", "/work/p", "build it"]);

    let r = tool_json(&call(&fx, &t3, json!({"action":"send","thread":THREAD,"text":"next step"})));
    assert_eq!(r["sent"], true, "{r}");
    assert_eq!(argv(&fx), ["thread", "send", "--json", THREAD, "next step"]);

    let r = tool_json(&call(&fx, &t3, json!({"action":"wake","thread":THREAD})));
    assert_eq!(r["sent"], true, "{r}");
    assert!(argv(&fx)[4].contains("everett_inbox"));

    let r = tool_json(&call(&fx, &t3, json!({"action":"read","thread":THREAD,"messages":3})));
    assert_eq!(r["thread"], THREAD, "{r}");
    assert_eq!(argv(&fx), ["thread", "show", "--json", "--messages", "3", THREAD]);
}

#[test]
fn t3_waiting_for_answer_is_reported_not_failed() {
    let fx = fixture();
    let t3 = fake_t3(&fx, 2);
    let r = tool_json(&call(&fx, &t3, json!({"action":"send","thread":THREAD,"text":"go","wait":true})));
    assert_eq!(r["result"]["waiting_for_answer"], true, "{r}");
}

#[test]
fn t3_failure_and_bad_arguments_are_errors() {
    let fx = fixture();
    let t3 = fake_t3(&fx, 1);
    let r = call(&fx, &t3, json!({"action":"send","thread":THREAD,"text":"go"}));
    assert_eq!(r["isError"], true, "{r}");
    let r = call(&fx, &t3, json!({"action":"start","project":"/p","text":"x","provider":"codex"}));
    assert_eq!(r["isError"], true, "{r}");
    let r = call(&fx, &t3, json!({"action":"start","text":"x"}));
    assert_eq!(r["isError"], true, "{r}");
    let r = call(&fx, &t3, json!({"action":"send","thread":THREAD,"text":"   "}));
    assert_eq!(r["isError"], true, "{r}");
}

#[test]
fn read_returns_any_harness_transcript_tail_and_stop_is_explicit() {
    let fx = fixture();
    let t3 = fake_t3(&fx, 0);
    let cwd = fx.home().to_string_lossy().into_owned();
    let rows = [
        json!({"type":"user","sessionId":CLAUDE,"cwd":cwd,"timestamp":"2026-11-10T00:00:00Z","message":{"role":"user","content":"first ask"}}),
        json!({"type":"assistant","sessionId":CLAUDE,"cwd":cwd,"timestamp":"2026-11-10T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"first answer"}]}}),
        json!({"type":"user","sessionId":CLAUDE,"cwd":cwd,"timestamp":"2026-11-10T00:00:02Z","message":{"role":"user","content":"second ask"}}),
    ];
    fx.write(&format!(".claude/projects/p/{CLAUDE}.jsonl"), &common::jsonl(&rows));

    let r = tool_json(&call(&fx, &t3, json!({"action":"read","session":CLAUDE,"messages":2})));
    assert_eq!(r["via"], "transcript", "{r}");
    assert_eq!(r["harness"], "claude");
    assert_eq!(r["messages"], json!([{"role":"assistant","text":"first answer"},{"role":"user","text":"second ask"}]));
    assert!(!fx.home().join("t3-argv").exists(), "a non-T3 read must not call t3");

    let r = call(&fx, &t3, json!({"action":"stop","session":CLAUDE}));
    assert_eq!(r["isError"], true, "{r}");
    assert!(r.to_string().contains("not supported for claude"), "{r}");
    let r = call(&fx, &t3, json!({"action":"stop","thread":THREAD}));
    assert!(r.to_string().contains("not supported for t3"), "{r}");
}

#[test]
fn read_handles_hermes_db_and_grok_session_dirs() {
    let fx = fixture();
    let t3 = fake_t3(&fx, 0);
    let db = fx.home().join(".hermes/state.db");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    let con = rusqlite::Connection::open(&db).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    con.execute_batch("CREATE TABLE sessions (id TEXT, source TEXT, started_at REAL, cwd TEXT, title TEXT,
            profile_name TEXT, last_activity_at REAL);
         CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT, role TEXT, content TEXT);").unwrap();
    con.execute("INSERT INTO sessions VALUES ('h-1','cli',?1,'/work/herm','Hermes run','',?1)", [now - 60.0]).unwrap();
    for (role, text) in [("user", "hermes ask"), ("tool", "skip"), ("assistant", "hermes answer"), ("user", "hermes again")] {
        con.execute("INSERT INTO messages (session_id, role, content) VALUES ('h-1',?1,?2)", [role, text]).unwrap();
    }
    drop(con);
    let r = tool_json(&call(&fx, &t3, json!({"action":"read","session":"h-1","messages":2})));
    assert_eq!(r["harness"], "hermes", "{r}");
    assert_eq!(r["messages"], json!([{"role":"assistant","text":"hermes answer"},{"role":"user","text":"hermes again"}]));

    let sid = "4d1c2b3a-0000-4000-8000-00000000g0k1";
    let folder = fx.home().join(".grok/sessions").join(everett::adapters::grok::quote("/work/grok")).join(sid);
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("summary.json"), json!({"info":{"id":sid,"cwd":"/work/grok"},"created_at":"2026-11-10T00:00:00Z","last_active_at":"2026-11-10T00:05:00Z"}).to_string()).unwrap();
    fs::write(folder.join("chat_history.jsonl"), common::jsonl(&[
        json!({"type":"system","content":"You are Grok."}),
        json!({"type":"user","synthetic_reason":"system_reminder","content":[{"type":"text","text":"reminder"}]}),
        json!({"type":"user","content":[{"type":"text","text":"grok ask"}]}),
        json!({"type":"assistant","content":"grok answer"}),
    ])).unwrap();
    let r = tool_json(&call(&fx, &t3, json!({"action":"read","session":sid})));
    assert_eq!(r["harness"], "grok", "{r}");
    assert_eq!(r["messages"], json!([{"role":"user","text":"grok ask"},{"role":"assistant","text":"grok answer"}]));
}

#[test]
fn hop_limit_blocks_starts_and_sends() {
    let fx = fixture();
    let t3 = fake_t3(&fx, 0);
    for args in [json!({"action":"start","project":"/p","text":"x"}), json!({"action":"send","thread":THREAD,"text":"x"}),
                 json!({"action":"start","harness":"codex","cwd":"/","text":"x"})] {
        let out = mcp_calls(&fx, &[], &[("EVERETT_T3_BIN", t3.as_str()), ("EVERETT_SESSION_ID", "caller"), ("EVERETT_HOPS", "99")],
            &[("everett_control", args.clone())]);
        assert_eq!(out[0]["isError"], true, "{args}: {}", out[0]);
        assert!(out[0].to_string().contains("Hop limit"), "{}", out[0]);
    }
    assert!(!fx.home().join("t3-argv").exists(), "t3 must not run past the hop limit");
}

#[test]
fn remote_derived_caller_is_refused_for_every_action() {
    let fx = fixture();
    let t3 = fake_t3(&fx, 0);
    fx.write(".everett/inbox/relay-context/caller.json", &json!({"hops": 1}).to_string());
    for action in ["threads", "read", "stop"] {
        let r = call(&fx, &t3, json!({"action": action, "thread": THREAD}));
        assert_eq!(r["isError"], true, "{action}: {r}");
        assert!(r.to_string().contains("Remote-derived"), "{action}: {r}");
    }
    assert!(!fx.home().join("t3-argv").exists(), "t3 must not run for a remote-derived caller");
}

#[test]
fn t3_send_carries_hops_to_the_receiving_session() {
    let fx = fixture();
    let t3 = fake_t3(&fx, 0);
    let db = fx.home().join(".t3/userdata/statev2.sqlite");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    let con = rusqlite::Connection::open(&db).unwrap();
    con.execute_batch(include_str!("../../tests/fixtures/t3_v2.sql")).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    con.execute("INSERT INTO projection_projects VALUES ('p','Project','/work/project',NULL,'[]',?1,?1,NULL)", [&now]).unwrap();
    con.execute("INSERT INTO orchestration_v2_projection_threads (thread_id,project_id,title,default_provider,runtime_mode,interaction_mode,active_provider_thread_id,created_at,updated_at,payload_json) VALUES (?1,'p','T','codex','full-access','default','provider',?2,?2,'{}')", [THREAD, &now]).unwrap();
    con.execute("INSERT INTO orchestration_v2_projection_provider_threads (provider_thread_id,thread_id,provider,driver,status,updated_at,payload_json) VALUES ('provider',?1,'codex','codex','idle',?2,?3)",
        [THREAD, &now, &json!({"driver":"codex","nativeThreadRef":{"driver":"codex","nativeId":"recv-session","strength":"strong"}}).to_string()]).unwrap();
    drop(con);
    let send = |caller: &str, hops: &str| mcp_calls(&fx, &[], &[("EVERETT_T3_BIN", t3.as_str()), ("EVERETT_SESSION_ID", caller), ("EVERETT_HOPS", hops)],
        &[("everett_control", json!({"action":"send","thread":THREAD,"text":"go"}))])[0].clone();
    // The fake t3 snapshots the receiver's hop record while it is still running.
    fs::write(&t3, "#!/bin/sh\ncat \"$HOME/.everett/inbox/live/recv-session.json\" > \"$HOME/hops-during-send\" 2>/dev/null\nprintf '%s\\n' '{\"ok\":true}'\n").unwrap();
    assert_ne!(send("caller", "2")["isError"], true);
    let during: Value = serde_json::from_str(&fs::read_to_string(fx.home().join("hops-during-send")).unwrap()).unwrap();
    assert_eq!(during["hops"], 3, "hop state must exist before the receiver runs");
    // The receiving session now carries hop 3 (the limit), so it cannot forward again.
    let r = send("recv-session", "0");
    assert_eq!(r["isError"], true, "{r}");
    assert!(r.to_string().contains("Hop limit"), "{r}");
}
