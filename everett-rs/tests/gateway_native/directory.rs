//! everett_directory labels the gateway's exact destination allowlist so a
//! client resolves a named session or T3 Code thread to the ID to send to.
use super::*;

pub(super) fn t3(c: &Ctx, thread: &str, native: &str, title: &str) {
    let path = c.home.join(".t3/userdata/statev2.sqlite");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(include_str!("../../../tests/fixtures/t3_v2.sql"))
        .unwrap();
    t3_at(c, thread, native, title, &chrono::Utc::now().to_rfc3339());
}

pub(super) fn t3_at(c: &Ctx, thread: &str, native: &str, title: &str, now: &str) {
    let path = c.home.join(".t3/userdata/statev2.sqlite");
    let con = rusqlite::Connection::open(&path).unwrap();
    let now = now.to_string();
    con.execute(
        "INSERT OR IGNORE INTO projection_projects VALUES ('p','Project','/work/project',NULL,'[]',?1,?1,NULL)",
        [&now],
    )
    .unwrap();
    let provider = format!("provider-{thread}");
    con.execute(
        "INSERT INTO orchestration_v2_projection_threads \
         (thread_id,project_id,title,default_provider,provider_instance_id,runtime_mode,interaction_mode, \
          active_provider_thread_id,created_at,updated_at,payload_json) \
         VALUES (?1,'p',?2,'custom-instance','custom-instance','full-access','default',?3,?4,?4,?5)",
        rusqlite::params![thread, title, provider, now, json!({"worktreePath":"/work/tree"}).to_string()],
    )
    .unwrap();
    con.execute(
        "INSERT INTO orchestration_v2_projection_provider_threads \
         (provider_thread_id,thread_id,provider,driver,provider_instance_id,status,updated_at,payload_json) \
         VALUES (?1,?2,'custom-instance',?3,'custom-instance','idle',?4,?5)",
        rusqlite::params![
            provider,
            thread,
            "codex",
            now,
            json!({"driver":"codex","providerInstanceId":"custom-instance",
                   "nativeThreadRef":{"driver":"codex","nativeId":native,"strength":"strong"}})
            .to_string()
        ],
    )
    .unwrap();
    con.execute(
        "INSERT INTO orchestration_v2_projection_messages \
         (message_id,thread_id,role,streaming,created_at,updated_at,payload_json) \
         VALUES (?1,?2,'user',0,?3,?3,?4)",
        rusqlite::params![
            format!("{thread}-0"),
            thread,
            now,
            json!({"text":"first real request"}).to_string()
        ],
    )
    .unwrap();
}

pub(super) fn entries(response: &Value) -> std::collections::HashMap<String, Value> {
    success(response)["destinations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["id"].as_str().unwrap().to_string(),
                entry.clone(),
            )
        })
        .collect()
}

fn error_text(response: &Value) -> String {
    assert_eq!(response["result"]["isError"], true, "{response}");
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

pub(super) fn labels_and_thread_send() {
    let c = ctx();
    t3(&c, "chat-teresis", "native-teresis", "Teresis refactor");
    t3_at(
        &c,
        "chat-teresis-old",
        "native-teresis-old",
        "Old Teresis thread",
        &(chrono::Utc::now() - chrono::Duration::days(31)).to_rfc3339(),
    );
    assert!(c
        .cli(&["external", "add", "--id", "agent-two", "--harness", "grok-bot"])
        .status
        .success());
    let mut peer = c.peer(&json!({"destinations": [
        "native-teresis", "chat-teresis", "chat-teresis-old", "agent-two", "queue-only-local", "missing-dest"
    ]}));
    let tools = peer.request("tools/list", None)["result"]["tools"]
        .as_array()
        .unwrap()
        .clone();
    let directory = tools
        .iter()
        .find(|tool| tool["name"] == "everett_directory")
        .expect("everett_directory advertised");
    assert_eq!(directory["annotations"]["readOnlyHint"], true);
    let identity = success(&peer.call("everett_whoami", json!({})));
    assert_eq!(
        identity["capabilities"]["destination_directory"],
        true,
        "{identity}"
    );

    let listed = entries(&peer.call("everett_directory", json!({})));
    assert_eq!(listed.len(), 6, "{listed:?}");
    let thread = &listed["chat-teresis"];
    assert_eq!(thread["kind"], "t3_thread");
    assert_eq!(thread["label"], "Teresis refactor");
    assert_eq!(thread["session_id"], "native-teresis");
    assert_eq!(thread["actions"], json!(["send"]));
    let stale = &listed["chat-teresis-old"];
    assert_eq!(stale["kind"], "t3_thread");
    assert_eq!(stale["state"], "inactive");
    assert_eq!(stale["session_id"], "native-teresis-old");
    assert_eq!(stale["actions"], json!(["send"]));
    let session = &listed["native-teresis"];
    assert_eq!(session["kind"], "session");
    assert_eq!(session["t3_thread_id"], "chat-teresis");
    assert_eq!(session["project"], "tree");
    assert_eq!(session["actions"], json!(["send"]));
    let conversation = &listed["agent-two"];
    assert_eq!(conversation["kind"], "conversation");
    assert_eq!(conversation["harness"], "grok-bot");
    assert_eq!(conversation["actions"], json!(["send"]));
    for id in ["queue-only-local", "missing-dest"] {
        assert_eq!(listed[id]["kind"], "unknown", "{id}");
        assert_eq!(listed[id]["actions"], json!([]));
    }

    let delivered = success(&peer.call(
        "everett_send",
        json!({"to": "chat-teresis", "text": "Continue the Teresis refactor"}),
    ));
    assert_eq!(delivered["queued"], true);
    assert_eq!(delivered["resolved_from"], "chat-teresis");
    assert_eq!(delivered["resolved_to"], "native-teresis");
    let inbox = std::fs::read_to_string(c.home.join(".everett/inbox/native-teresis.jsonl")).unwrap();
    assert!(inbox.contains("Continue the Teresis refactor"), "{inbox}");
    let direct = success(&peer.call(
        "everett_send",
        json!({"to": "native-teresis", "text": "Direct provider session"}),
    ));
    assert_eq!(direct["queued"], true);

    let stale_send = success(&peer.call(
        "everett_send",
        json!({"to": "chat-teresis-old", "text": "Old thread still reachable"}),
    ));
    assert_eq!(stale_send["queued"], true, "{stale_send}");
    assert_eq!(stale_send["resolved_from"], "chat-teresis-old");
    assert_eq!(stale_send["resolved_to"], "native-teresis-old");
    let stale_inbox = std::fs::read_to_string(
        c.home.join(".everett/inbox/native-teresis-old.jsonl"),
    )
    .unwrap();
    assert!(stale_inbox.contains("Old thread still reachable"), "{stale_inbox}");

    let blocked = error_text(&peer.call(
        "everett_send",
        json!({"to": "queue-only-local", "text": "no live session"}),
    ));
    assert!(blocked.contains("exact session or T3 thread id"), "{blocked}");
    assert!(blocked.contains("everett_directory"), "{blocked}");
    assert!(!c.home.join(".everett/inbox/queue-only-local.jsonl").exists());
    let unnamed = error_text(&peer.call(
        "everett_send",
        json!({"to": "Teresis refactor", "text": "names are not IDs"}),
    ));
    assert!(unnamed.contains("everett_directory"), "{unnamed}");
}
