//! Regression for #230: a developer shell exporting DEVIN_HOME made fixture
//! children list the developer's real Devin sessions. This binary is its own
//! process, so poisoning its env cannot race any other test.

mod common;

use std::path::Path;
use std::process::Command;

const POISONED_HOMES: &[&str] = &[
    "DEVIN_HOME",
    "PI_CODING_AGENT_DIR",
    "HERMES_HOME",
    "T3CODE_HOME",
];

fn seed_devin_db(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let con = rusqlite::Connection::open(dir.join("sessions.db")).unwrap();
    let now = everett::session::now();
    con.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, working_directory TEXT, title TEXT, model TEXT,
           created_at REAL, last_activity_at REAL, hidden INTEGER DEFAULT 0);
         CREATE TABLE message_nodes (node_id INTEGER PRIMARY KEY, session_id TEXT NOT NULL,
           created_at REAL, chat_message TEXT);",
    )
    .unwrap();
    con.execute(
        "INSERT INTO sessions VALUES ('host-leak', '/work/host', 'Host session', 'm', ?1, ?2, 0)",
        rusqlite::params![now - 600.0, now - 60.0],
    )
    .unwrap();
    con.execute(
        "INSERT INTO message_nodes VALUES (1, 'host-leak', ?1, ?2)",
        rusqlite::params![
            now - 590.0,
            r#"{"role":"user","content":"host work","metadata":{"is_user_input":true}}"#
        ],
    )
    .unwrap();
}

#[test]
fn fixture_children_ignore_exported_harness_homes() {
    let host = tempfile::tempdir().unwrap();
    seed_devin_db(&host.path().join("devin"));
    for name in POISONED_HOMES {
        std::env::set_var(name, host.path().join("devin"));
    }

    let fx = common::fixture();
    let listed = fx.run(&["ls", "--json"]);
    assert_eq!(listed.status.code(), Some(0));
    assert_eq!(
        fx.stdout(&listed).trim(),
        "[]",
        "fixture child read the exported DEVIN_HOME"
    );

    // Control: the same poison reaches a child that inherits the environment,
    // so the assertion above is not passing vacuously.
    let leaky = Command::new(env!("CARGO_BIN_EXE_everett"))
        .args(["ls", "--json"])
        .env("HOME", fx.home())
        .env("EVERETT_HOME", fx.home())
        .env("EVERETT_NOTIFY", "none")
        .env("PATH", common::HERMETIC_PATH)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&leaky.stdout).contains("host-leak"),
        "poison did not reach an inheriting child: {}",
        String::from_utf8_lossy(&leaky.stdout)
    );
}
