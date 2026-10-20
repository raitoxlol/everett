mod common;

use common::{fixture, jev_stub, mcp_calls, plant, tool_json};
use everett::error::EverettError;
use everett::route::route_with;
use everett::session::Session;
use serde_json::{json, Map, Value};

const REJECT: &str = r#"{"error":"invalid api key"}"#;
const ACCEPT: &str = r#"{"answers":{"route":{"choice":"new","confidence":0.9}}}"#;

type Jev = dyn Fn(&str, &Map<String, Value>, &str) -> everett::error::Result<Map<String, Value>>;

fn sessions() -> Vec<Session> {
    let mut s = Session::new("claude", "c-1", "/work/app", "", "", everett::session::now());
    s.title = "fix the login button".into();
    vec![s]
}

fn http_401(_: &str, _: &Map<String, Value>, _: &str) -> everett::error::Result<Map<String, Value>> {
    Err(EverettError::new(2, "Jev unavailable or returned an invalid response: http status: 401"))
}

#[test]
fn every_jev_failure_falls_back_to_local_with_a_warning() {
    let _home = common::lib_home();
    let failures: Vec<Box<Jev>> = vec![
        Box::new(http_401),
        Box::new(|_, _, _| Err(EverettError::new(2, "Jev unavailable or returned an invalid response: timeout"))),
        Box::new(|_, _, _| Ok(json!({"choice": "s0"}).as_object().cloned().unwrap())),
    ];
    for jev in failures {
        for router in [None, Some("jev")] {
            let r = route_with("fix the login button", &sessions(), router, Some("bogus"), &*jev, None).unwrap();
            assert_eq!(r["router"], "local", "{r:?}");
            assert_eq!(r["fallback_from"], "jev");
            assert!(r["warning"].as_str().unwrap().contains("used local matching"), "{r:?}");
            assert_eq!(r["decision"], "SESSION");
        }
    }
    let rejected = route_with("x", &sessions(), None, Some("bogus"), &http_401, None).unwrap();
    assert!(rejected["warning"].as_str().unwrap().contains("key was rejected"));
}

#[test]
fn rejected_key_still_routes_through_cli_and_mcp() {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/-work-app/c-1.jsonl");
    fx.write(".everett/config.toml", "typesafe_api_key = \"bogus\"\n");
    let url = jev_stub(401, REJECT);
    let env = [("EVERETT_JEV_URL", url.as_str())];

    let out = fx.run_stdin(&["route", "something brand new", "--json"], "", &env);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["router"], "local");
    assert_eq!(r["fallback_from"], "jev");
    assert!(stderr.contains("warning: Jev routing failed (http status: 401)"), "{stderr}");

    let out = fx.run_stdin(&["send", "something brand new"], "", &env);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("used local matching"));

    let results = mcp_calls(
        &fx,
        &[],
        &env,
        &[
            ("everett_route", json!({"text": "something brand new"})),
            ("everett_send", json!({"text": "something brand new"})),
        ],
    );
    for result in &results {
        assert_ne!(result["isError"], true, "{result}");
        let body = tool_json(result);
        assert_eq!(body["router"], "local", "{body}");
        assert_eq!(body["fallback_from"], "jev");
        assert!(body["warning"].as_str().unwrap().contains("401"), "{body}");
    }
}

#[test]
fn onboard_yes_never_saves_a_rejected_key_and_skip_reports_a_saved_one() {
    let fx = fixture();
    let url = jev_stub(401, REJECT);
    let out = fx.run_stdin(
        &["onboard", "--yes", "--no-backfill", "--jev-key-env", "MY_KEY"],
        "",
        &[("MY_KEY", "bogus"), ("EVERETT_JEV_URL", url.as_str()), ("PATH", "/usr/bin:/bin")],
    );
    let text = fx.stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("new key NOT saved (validation failed: http status: 401)"), "{text}");
    assert!(!text.contains("kept anyway"), "{text}");
    assert!(text.starts_with("Everett onboarding finished with 2 warning(s):"), "{text}");
    assert!(text.contains("WARNING: No supported harness was found"), "{text}");
    assert!(!text.contains("onboarding complete"), "{text}");
    let config = std::fs::read_to_string(fx.home().join(".everett/config.toml")).unwrap_or_default();
    assert!(!config.contains("bogus"), "{config}");

    fx.write(".everett/config.toml", "typesafe_api_key = \"stale\"\n");
    let out = fx.run_stdin(&["onboard", "--yes", "--no-backfill", "--jev-key-env", "NOPE"], "", &[]);
    let text = fx.stdout(&out);
    assert!(text.contains("--jev-key-env NOPE is unset or empty; nothing saved"), "{text}");
    assert!(text.contains("a key is still set (typesafe_api_key in ~/.everett/config.toml)"), "{text}");

    let good = jev_stub(200, ACCEPT);
    let out = fx.run_stdin(
        &["onboard", "--yes", "--no-backfill", "--jev-key-env", "MY_KEY"],
        "",
        &[("MY_KEY", "fresh"), ("EVERETT_JEV_URL", good.as_str())],
    );
    let text = fx.stdout(&out);
    assert!(text.contains("(validated)"), "{text}");
    let config = std::fs::read_to_string(fx.home().join(".everett/config.toml")).unwrap();
    assert!(config.contains("\"fresh\"") && !config.contains("stale"), "{config}");
}

#[test]
fn doctor_tests_the_key_instead_of_trusting_it() {
    let fx = fixture();
    fx.write(".everett/config.toml", "typesafe_api_key = \"bogus\"\n");
    let rejected = jev_stub(401, REJECT);
    let out = fx.run_stdin(&["doctor"], "", &[("EVERETT_JEV_URL", rejected.as_str())]);
    let text = fx.stdout(&out);
    assert!(text.contains("[warn] router: jev key REJECTED (http status: 401"), "{text}");
    assert!(!text.contains("router: jev (key"), "{text}");
    assert_eq!(out.status.code(), Some(1));

    let accepted = jev_stub(200, ACCEPT);
    let out = fx.run_stdin(&["doctor"], "", &[("EVERETT_JEV_URL", accepted.as_str())]);
    let text = fx.stdout(&out);
    assert!(text.contains("[ok  ] router: jev (key from typesafe_api_key in ~/.everett/config.toml accepted by Jev"), "{text}");
}
