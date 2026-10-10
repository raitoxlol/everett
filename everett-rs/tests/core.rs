mod common;

use common::{fixture, lib_home, LibHome};
use everett::core;
use everett::proc::RunOutput;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

fn project(h: &LibHome) -> PathBuf {
    let dir = h.path().join("src/atlas");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::create_dir_all(dir.join("pkg")).unwrap();
    dir
}

fn home(h: &LibHome) -> String {
    h.path().to_string_lossy().into()
}

fn learn(text: &str, h: &LibHome) {
    core::learn(text, None, None, Some(&home(h))).unwrap();
}

#[test]
fn secret_filter() {
    let secrets = [
        "use key sk-ant-api03-AbCdEfGhIjKlMnOpQrStUv",
        "token ghp_abcdefghijklmnopqrstuvwxyz0123",
        "AKIAIOSFODNN7EXAMPLE is the aws key",
        "password = hunter2hunter2",
        "API_KEY: 9f8e7d6c5b4a",
        "-----BEGIN OPENSSH PRIVATE KEY----- b3BlbnNzaC1r",
        "db at postgres://admin:s3cretpw@db.internal/app",
        "header Authorization: Bearer abcdefghijklmnop1234",
        "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N",
        "random Xk9fQ2LmP8vR3tW7yZ1aB4cD6eH0jN5s",
    ];
    for text in secrets {
        assert!(!core::find_secret(text).is_empty(), "{text}");
    }
    let clean = [
        "Tests run with cargo test --locked",
        "The deploy target is fly.io, region nrt; never push on Fridays",
        "Use the token budget of 4000 for summaries",
        "Path is /Users/someone/Projects/everett/everett-rs/src/hooks_common.rs",
        "Session ids look like 01a0d358-0f69-77f3-950a-8bfbe2f5a13a",
    ];
    for text in clean {
        assert_eq!(core::find_secret(text), "", "{text}");
    }
}

#[test]
fn learn_appends_entry_with_metadata() {
    let h = lib_home();
    let pkg = project(&h).join("pkg");
    std::env::set_var("EVERETT_SESSION_ID", "s-1");
    std::env::set_var("CLAUDECODE", "1");
    let entry = core::learn("Atlas uses pnpm, not npm", None, Some("project"), Some(&pkg.to_string_lossy())).unwrap();
    std::env::remove_var("EVERETT_SESSION_ID");
    std::env::remove_var("CLAUDECODE");
    assert_eq!((entry["project"].clone(), entry["scope"].clone(), entry["session"].clone(), entry["harness"].clone()),
        (json!("atlas"), json!("project"), json!("s-1"), json!("claude")));
    let stored = core::read_inbox();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0]["text"], "Atlas uses pnpm, not npm");
    assert_eq!(stored[0]["cwd"], json!(pkg.to_string_lossy()));
}

#[test]
fn learn_defaults_and_rejections() {
    let h = lib_home();
    assert_eq!(core::learn("global fact", None, None, Some(&home(&h))).unwrap()["scope"], "global");
    assert_eq!(core::learn("p fact", Some("My App"), None, None).unwrap()["project"], "my-app");
    assert!(core::learn(&"x".repeat(501), None, None, None).is_err());
    assert!(core::learn("   ", None, None, None).is_err());
    let err = core::learn("the password = hunter2hunter2", None, None, None).unwrap_err().to_string();
    assert!(err.contains("credential"), "{err}");
    assert!(core::learn("needs a project", None, Some("project"), Some(&home(&h))).is_err());
    assert_eq!(core::read_inbox().len(), 2);
}

#[test]
fn cli_learn_rejects_secret_with_exit_2() {
    let fx = fixture();
    let out = fx.run(&["learn", "key sk-proj-abcdefghijklmnopqrstu"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Secrets never go into the shared core"));
    assert!(!fx.home().join(".everett/core/inbox.jsonl").exists());
}

fn snapshots() -> Vec<PathBuf> {
    let mut snaps: Vec<_> = std::fs::read_dir(core::history_dir()).unwrap().map(|e| e.unwrap().path()).collect();
    snaps.sort();
    snaps
}

#[test]
fn deterministic_merge_dedupes_and_archives() {
    let h = lib_home();
    learn("Everett tests use a temp HOME", &h);
    core::learn("Atlas deploys from main", Some("atlas"), None, None).unwrap();
    learn("everett tests use a temp home.", &h);
    core::merge("none", false).unwrap();
    let text = std::fs::read_to_string(core::global_path()).unwrap();
    assert_eq!(text.matches("temp").count(), 1);
    assert!(text.contains("everett tests use a temp home."));
    assert!(std::fs::read_to_string(core::project_path("atlas")).unwrap().contains("Atlas deploys from main"));
    assert!(!core::inbox_path().exists());
    let snaps = snapshots();
    assert_eq!(snaps.len(), 1);
    assert_eq!(std::fs::read_to_string(snaps[0].join("inbox.jsonl")).unwrap().lines().count(), 3);

    learn("Second round fact", &h);
    let before = std::fs::read_to_string(core::global_path()).unwrap();
    core::merge("none", false).unwrap();
    let snaps = snapshots();
    assert_eq!(snaps.len(), 2);
    let last = snaps.last().unwrap();
    assert_eq!(std::fs::read_to_string(last.join("core.md")).unwrap(), before);
    assert!(std::fs::read_to_string(last.join("projects/atlas.md")).unwrap().contains("Atlas deploys"));
}

#[test]
fn dry_run_writes_nothing_and_empty_inbox_reports() {
    let fx = fixture();
    let out = fx.stdout(&fx.run(&["trunk", "merge", "--llm", "none"]));
    assert!(out.contains("nothing to merge"), "{out}");
    fx.run(&["learn", "A fact"]);
    let out = fx.stdout(&fx.run(&["trunk", "merge", "--llm", "none", "--dry-run"]));
    assert!(out.contains("A fact"), "{out}");
    assert!(!fx.home().join(".everett/core/core.md").exists());
    let inbox = std::fs::read_to_string(fx.home().join(".everett/core/inbox.jsonl")).unwrap();
    assert_eq!(inbox.lines().count(), 1);
}

#[test]
fn word_cap_drops_oldest() {
    let h = lib_home();
    for i in 0..80 {
        learn(&format!("fact number {} about the build system and its many quirks", i), &h);
    }
    core::merge("none", false).unwrap();
    let text = std::fs::read_to_string(core::global_path()).unwrap();
    assert!(core::words(&text) <= core::CORE_WORDS);
    assert!(text.contains("fact number 79 ") && !text.contains("fact number 0 "));
}

type Calls = RefCell<Vec<(Vec<String>, HashMap<String, String>)>>;

#[allow(clippy::type_complexity)]
fn reply_runner<'a>(stdout: &'a str, calls: &'a Calls)
    -> impl Fn(&[String], Option<&str>, &HashMap<String, String>, f64) -> Result<RunOutput, String> + 'a {
    move |cmd, _cwd, env, _t| {
        calls.borrow_mut().push((cmd.to_vec(), env.clone()));
        Ok(RunOutput { code: 0, stdout: stdout.to_string(), stderr: String::new(), timed_out: false })
    }
}

#[test]
fn llm_merge_uses_headless_harness() {
    let h = lib_home();
    learn("Old: deploy on Fridays", &h);
    let reply = json!({"global": "# Everett core\n\n- Never deploy on Fridays (was: deploy on Fridays)\n",
        "projects": {"Atlas": "# Project core: atlas\n\n- Uses pnpm\n"}}).to_string();
    let calls = RefCell::new(Vec::new());
    let result = core::merge_with("claude", false, &reply_runner(&reply, &calls)).unwrap();
    let (command, env) = calls.borrow()[0].clone();
    assert_eq!(&command[..2], ["claude", "--print"]);
    assert!(command.last().unwrap().contains("Old: deploy on Fridays"));
    assert_eq!(env["EVERETT_SEND"], "1");
    assert!(std::fs::read_to_string(core::global_path()).unwrap().contains("Never deploy"));
    assert!(std::fs::read_to_string(core::project_path("atlas")).unwrap().contains("pnpm"));
    assert_eq!(result["merged"], 1);
}

#[test]
fn bad_llm_output_changes_nothing_and_output_is_capped() {
    let h = lib_home();
    learn("A fact", &h);
    let calls = RefCell::new(Vec::new());
    for stdout in ["I merged it for you!".to_string(), json!({"global": "token = abcdefghijklmnop"}).to_string()] {
        assert!(core::merge_with("codex", false, &reply_runner(&stdout, &calls)).is_err(), "{stdout}");
    }
    assert!(!core::global_path().exists());
    assert_eq!(core::read_inbox().len(), 1);
    let long: String = "# T\n".to_string()
        + &(0..40).map(|i| format!("- item {} {}\n", i, "word ".repeat(20))).collect::<String>();
    core::merge_with("claude", false, &reply_runner(&json!({"global": long}).to_string(), &calls)).unwrap();
    assert!(core::words(&std::fs::read_to_string(core::global_path()).unwrap()) <= core::CORE_WORDS);
}

#[test]
fn vault_mirror() {
    let h = lib_home();
    learn("Mirror me", &h);
    std::env::set_var("EVERETT_VAULT", h.path().join("vault"));
    let result = core::merge("none", false);
    std::env::remove_var("EVERETT_VAULT");
    let mirror = h.path().join("vault/Everett/Core.md");
    assert_eq!(result.unwrap()["mirror"], json!(mirror.to_string_lossy()));
    assert!(std::fs::read_to_string(mirror).unwrap().contains("Mirror me"));
}

fn fill(fx: &common::Fixture) {
    fx.write(".everett/core/core.md", &("# Everett core\n\n".to_string()
        + &(0..40).map(|i| format!("- global fact {} {}\n", i, "g ".repeat(10))).collect::<String>()));
    fx.write(".everett/core/projects/atlas.md", "# Project core: atlas\n\n- Atlas uses pnpm\n");
    std::fs::create_dir_all(fx.home().join("src/atlas/.git")).unwrap();
    std::fs::create_dir_all(fx.home().join("src/atlas/pkg")).unwrap();
}

#[test]
fn session_start_hooks_inject_bounded_core() {
    let fx = fixture();
    fill(&fx);
    let pkg = fx.home().join("src/atlas/pkg");
    for stem in ["claude_session_start", "codex_session_start"] {
        let started = std::time::Instant::now();
        let out = fx.run_stdin(&["hook", stem], &json!({"session_id": "abc", "cwd": pkg}).to_string(), &[("PATH", "/nonexistent")]);
        assert_eq!(out.status.code(), Some(0));
        assert!(started.elapsed().as_secs_f64() < 5.0, "{stem}");
        let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
        let ctx = parsed["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
        for needle in [".everett/cards/abc.md", "Atlas uses pnpm", "global fact 0", "everett learn"] {
            assert!(ctx.contains(needle), "{stem}: {needle}");
        }
        let shared = ctx.split_once("Everett shared core").unwrap().1;
        assert!(core::words(shared) <= core::CONTEXT_WORDS);
    }
    let out = fx.run_stdin(&["hook", "codex_session_start"], &json!({"session_id": "abc"}).to_string(), &[("EVERETT_SEND", "1")]);
    assert_eq!(out.stdout.len(), 0, "headless sends stay silent");
    let out = fx.run_stdin(&["hook", "omp_card_context", "o-1"], "", &[]);
    let text = fx.stdout(&out);
    assert!(text.contains("o-1.md") && text.contains("global fact 0"), "{text}");
}

#[test]
fn context_without_core_is_just_the_learn_line() {
    let h = lib_home();
    assert_eq!(core::context(&home(&h)), core::LEARN_LINE);
}

#[test]
fn broken_core_keeps_card_and_stays_silent() {
    let fx = fixture();
    std::fs::create_dir_all(fx.home().join(".everett/core/core.md")).unwrap();
    let out = fx.run_stdin(&["hook", "claude_session_start"], &json!({"session_id": "abc", "cwd": fx.home()}).to_string(), &[]);
    assert_eq!((out.status.code(), out.stderr.len()), (Some(0), 0));
    assert!(fx.stdout(&out).contains(".everett/cards/abc.md"));
    let out = fx.run_stdin(&["hook", "claude_session_start"], "garbage", &[]);
    assert_eq!((out.status.code(), out.stdout.len(), out.stderr.len()), (Some(0), 0, 0));
}

#[test]
fn core_cli_show_edit_path_history_and_trunk_alias() {
    let fx = fixture();
    fx.run(&["learn", "Shown fact"]);
    assert!(fx.stdout(&fx.run(&["core"])).contains("1 learning(s) waiting"));
    fx.run(&["trunk", "merge", "--llm", "none"]);
    let text = [fx.run(&["core", "show"]), fx.run(&["core", "edit-path"]), fx.run(&["core", "history"])]
        .iter().map(|o| fx.stdout(o)).collect::<String>();
    assert!(text.contains("Shown fact"));
    assert!(text.contains(&fx.home().join(".everett/core/core.md").to_string_lossy().to_string()), "{text}");
    assert!(text.contains(&fx.home().join(".everett/core/history").to_string_lossy().to_string()), "{text}");
    let vault = fx.home().join("v");
    let vault = vault.to_string_lossy();
    assert_eq!(fx.run_stdin(&["trunk"], "", &[("EVERETT_VAULT", &vault)]).status.code(), Some(0));
    assert_eq!(fx.run_stdin(&["trunk", "view"], "", &[("EVERETT_VAULT", &vault)]).status.code(), Some(0));
    assert!(fx.home().join("v/Everett/Trunk.md").exists());
}
