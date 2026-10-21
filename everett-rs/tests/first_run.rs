mod common;

use common::fixture;
use std::process::Command;
use std::time::Duration;

fn command(home: &std::path::Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_everett"));
    cmd.args(args)
        .env_clear()
        .env("HOME", home)
        .env("EVERETT_HOME", home)
        .env("PATH", "/nonexistent")
        .env("TERM", "dumb")
        .env("EVERETT_NOTIFY", "none");
    cmd
}

fn terminal(home: &std::path::Path, args: &[&str]) -> expectrl::Session {
    let mut session = expectrl::Session::spawn(command(home, args)).unwrap();
    session.set_expect_timeout(Some(Duration::from_secs(10)));
    session
}

#[test]
fn first_terminal_run_onboards_and_cancellation_keeps_it_pending() {
    let fx = fixture();
    for _ in 0..2 {
        let mut session = terminal(&fx.home(), &[]);
        session.expect("Everett -- first-time setup").unwrap();
        session.expect("paste a Jev key now?").unwrap();
        session.send_line("q").unwrap();
        session
            .expect("Cancelled -- no changes were made.")
            .unwrap();
        session.expect(expectrl::Eof).unwrap();
    }
}

#[test]
fn completed_setup_stops_auto_onboarding_but_explicit_onboard_runs_again() {
    let fx = fixture();
    let out = command(&fx.home(), &["onboard", "--yes", "--no-backfill"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Everett onboarding"));

    let mut session = terminal(&fx.home(), &[]);
    session.expect("Usage: everett").unwrap();
    session.expect(expectrl::Eof).unwrap();

    let mut session = terminal(&fx.home(), &["onboard"]);
    session.expect("Everett -- first-time setup").unwrap();
    session.expect("paste a Jev key now?").unwrap();
    session.send_line("q").unwrap();
    session
        .expect("Cancelled -- no changes were made.")
        .unwrap();
    session.expect(expectrl::Eof).unwrap();

    let mut session = terminal(&fx.home(), &[]);
    session.expect("Usage: everett").unwrap();
    session.expect(expectrl::Eof).unwrap();
}

#[test]
fn explicit_commands_and_noninteractive_bare_runs_do_not_start_setup() {
    let fx = fixture();
    for args in [&[][..], &["--help"], &["--version"], &["ls", "--json"]] {
        let out = command(&fx.home(), args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        if args == ["--version"] {
            assert!(text.starts_with("everett "), "{text}");
        } else if args == ["ls", "--json"] {
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&text).unwrap(),
                serde_json::json!([])
            );
        } else {
            assert!(text.contains("Usage: everett"), "{text}");
        }
        assert!(!text.contains("Everett -- first-time setup"), "{text}");
    }
    let mut session = terminal(&fx.home(), &[]);
    session.expect("Everett -- first-time setup").unwrap();
    session.expect("paste a Jev key now?").unwrap();
    session.send_line("q").unwrap();
    session.expect(expectrl::Eof).unwrap();
}
