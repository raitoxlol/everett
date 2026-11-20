//! One regression per everyday defect: project naming in routing and cards, text cleaning,
//! timestamp parsing, and session ages in `everett ls`.

mod common;

use common::{fixture, lib_home, plant};
use everett::hooks_common::auto_card;
use everett::route::{describe, local_route, project_name, rank, relevant_sessions};
use everett::session::{clean, now, Session};
use everett::timefmt::{epoch_from_iso, iso_from_epoch};
use serde_json::json;

const WORKTREE: &str = "/Users/w/.t3/worktrees/everett/t3-dcb3322b";

fn session(id: &str, cwd: &str, card: &str) -> Session {
    let mut s = Session::new("claude", id, cwd, &format!("/tmp/{id}.jsonl"), "", now() - 300.0);
    s.card = card.into();
    s
}

fn card(user: &str, cwd: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    let row = json!({"type": "user", "message": {"content": user}});
    std::fs::write(&transcript, format!("{row}\n")).unwrap();
    auto_card(&transcript, "claude", cwd, "")
}

#[test]
fn a_request_naming_a_hyphenated_project_reaches_its_coordinator() {
    let coordinator = session("coord-1", "/Users/alice/web-mesh", "Coordinator session: delegating to the console workers.");
    let worker = session("work-1", "/Users/alice/Atlas", "Implementing the Atlas ingest pipeline.");
    let pool = relevant_sessions("what's the status of web-mesh", &[worker.clone(), coordinator.clone()], None);
    let ids: Vec<&str> = pool.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["coord-1", "work-1"]);
    for request in ["what's the status of the mesh", "check web latency and mesh reliability"] {
        let pool = relevant_sessions(request, &[worker.clone(), coordinator.clone()], None);
        assert_eq!(pool.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["work-1"], "{request}");
    }
}

#[test]
fn a_project_name_made_of_short_and_common_words_still_matches() {
    let coordinator = session("coord-1", "/Users/alice/r-and-d", "Coordinator session: delegating to the lab workers.");
    let worker = session("work-1", "/Users/alice/Atlas", "Implementing the Atlas ingest pipeline.");
    let pool = relevant_sessions("r-and-d budget", &[worker, coordinator], None);
    assert_eq!(pool[0].id, "coord-1");
    let quiet = session("lab-1", "/Users/alice/r-and-d", "unrelated");
    assert!(rank("r-and-d budget", std::slice::from_ref(&quiet), None)[0].0 > 0.0, "a named project is boosted");
    let routed = local_route("r-and-d", &[quiet], None, None);
    assert_eq!(routed["decision"], "SESSION", "{routed:?}");
}

#[test]
fn a_worktree_session_routes_under_its_repository() {
    let s = session("wt-1", WORKTREE, "");
    assert_eq!(project_name(&s), "everett");
    assert!(describe(&s).contains("project=everett "), "{}", describe(&s));
    let s = session("wt-2", "/Users/w/src/Api/.claude/worktrees/fix-retry/", "");
    assert_eq!(project_name(&s), "api");
    let s = session("wt-3", "/Users/w/.t3/worktrees/chat/t3-1a2b3c4d", "");
    assert_eq!(project_name(&s), "chat");
    let s = session("chat-1", "/Users/w/Documents/Codex/2026-10-19/chat", "");
    assert_eq!(project_name(&s), "", "a Codex chat folder is not a project");
}

#[test]
fn send_to_a_repository_name_finds_its_worktree_session() {
    let _h = lib_home();
    let sessions = [session("wt-1", WORKTREE, ""), session("other-1", "/Users/w/Projects/atlas", "")];
    assert_eq!(everett::registry::find("everett", &sessions).unwrap().id, "wt-1");
    assert_eq!(everett::registry::find("Atlas", &sessions).unwrap().id, "other-1");
    let sessions = [
        session("wt-1", WORKTREE, "What: Everett: Fix the login flow. State: Fix the login flow."),
        session("wt-2", "/Users/w/.t3/worktrees/everett/t3-0a1b2c3d", ""),
    ];
    let error = everett::registry::find("everett", &sessions).unwrap_err();
    assert!(error.message.contains("matches 2 sessions"), "{}", error.message);
    let sessions = [
        session("in-1", "/Users/w/Projects/atlas", "What: Atlas: Fix the login flow."),
        session("out-1", "/Users/w/Projects/other", "What: Atlas: Plan the atlas launch."),
    ];
    let error = everett::registry::find("atlas", &sessions).unwrap_err();
    assert!(error.message.contains("matches 2 sessions"), "{}", error.message);
}

#[test]
fn an_automatic_card_names_the_repository_behind_a_worktree() {
    let _h = lib_home();
    let c = card("Fix the login flow.", WORKTREE);
    assert!(c.contains("What: Everett: Fix the login flow."), "{c}");
}

#[test]
fn comparisons_survive_text_cleaning() {
    assert_eq!(clean("retry while attempts < 3 and delay > 2", 200), "retry while attempts < 3 and delay > 2");
    assert_eq!(clean("a<b+1 && c>d", 200), "a<b+1 && c>d");
    assert_eq!(clean("<system-reminder>context</system-reminder> ship <b>it</b><!-- note -->", 200), "context ship it");
    let _h = lib_home();
    let c = card("Retry while attempts < 3 and delay > 2 seconds.", "/work/atlas");
    assert!(c.contains("What: Atlas: Retry while attempts < 3 and delay > 2 seconds."), "{c}");
}

#[test]
fn a_date_without_a_time_parses_as_local_midnight() {
    assert_eq!(epoch_from_iso("2025-01-01"), epoch_from_iso("2025-01-01T00:00:00"));
    assert!(epoch_from_iso("2025-01-01").is_some());
    assert_eq!(epoch_from_iso("2025-13-01"), None);
}

#[test]
fn a_fraction_that_rounds_up_carries_into_the_next_second() {
    assert_eq!(iso_from_epoch(9.9999996), "1970-01-01T00:00:10+00:00");
    assert_eq!(iso_from_epoch(59.9999996), "1970-01-01T00:01:00+00:00");
    assert_eq!(iso_from_epoch(59.5), "1970-01-01T00:00:59.500000+00:00");
    assert_eq!(iso_from_epoch(-0.5), "1969-12-31T23:59:59.500000+00:00");
}

#[test]
fn ls_shows_old_sessions_in_days() {
    let fx = fixture();
    let path = plant(&fx, "claude.jsonl", ".claude/projects/app/claude.jsonl");
    let aged = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 86400 + 3600);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(aged).unwrap();
    let out = fx.stdout(&fx.run(&["ls", "--hours", "200"]));
    let columns: Vec<&str> = out.split_whitespace().collect();
    assert!(columns.contains(&"3d"), "{out}");
    assert!(!columns.contains(&"72h"), "{out}");
}
