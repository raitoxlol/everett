mod common;

use common::{fixture, mcp_calls, plant, tool_json, Fixture};
use serde_json::{json, Value};

const HOURS: f64 = 100000.0;
const CLAUDE: (&str, &str) = ("CLAUDE_CODE_SESSION_ID", "c-1");
const CODEX: (&str, &str) = ("CODEX_THREAD_ID", "x-1");

fn home() -> Fixture {
    let fx = fixture();
    plant(&fx, "claude.jsonl", ".claude/projects/-work-app/c-1.jsonl");
    plant(&fx, "codex.jsonl", ".codex/sessions/2026/09/20/rollout-x-1.jsonl");
    fx
}

/// One MCP call as a host-bound session.
fn call(fx: &Fixture, who: (&str, &str), name: &str, args: Value) -> Value {
    mcp_calls(fx, &[], &[who], &[(name, args)]).remove(0)
}

/// Consume this session's inbox through everett_inbox and return the newest message.
fn newest(fx: &Fixture, who: (&str, &str)) -> Value {
    let inbox = tool_json(&call(fx, who, "everett_inbox", json!({})));
    inbox["messages"].as_array().and_then(|m| m.last()).cloned().unwrap_or(Value::Null)
}

fn ok(result: &Value) -> Value {
    assert_ne!(result["isError"], true, "{result}");
    tool_json(result)
}

fn refused(result: &Value, needle: &str) {
    assert_eq!(result["isError"], true, "{result}");
    assert!(tool_json(result).to_string().contains(needle), "{result}");
}

#[test]
fn ask_answer_and_acknowledgement_fit_inside_the_hop_limit() {
    let fx = home();
    let out = fx.run(&["send", "please coordinate the atlas release", "--to", "c-1", "--mode", "inbox", "--hours", "100000"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(newest(&fx, CLAUDE)["hops"], 1);

    ok(&call(&fx, CLAUDE, "everett_send", json!({"text": "what is blocking atlas?", "to": "x-1", "mode": "inbox", "hours": HOURS})));
    let ask = newest(&fx, CODEX);
    assert_eq!(ask["hops"], 2);

    ok(&call(&fx, CODEX, "everett_send", json!({"text": "the migration", "reply_to": ask["id"]})));
    let answer = newest(&fx, CLAUDE);
    assert_eq!((answer["kind"].clone(), answer["hops"].clone()), (json!("reply"), json!(2)), "{answer}");

    ok(&call(&fx, CLAUDE, "everett_send", json!({"text": "thanks", "reply_to": answer["id"]})));
    let thanks = newest(&fx, CODEX);
    assert_eq!((thanks["hops"].clone(), thanks["reply_depth"].clone()), (json!(2), json!(2)), "{thanks}");

    ok(&call(&fx, CLAUDE, "everett_send", json!({"text": "one more thing", "to": "x-1", "mode": "inbox", "hours": HOURS})));
    assert_eq!(newest(&fx, CODEX)["hops"], 3);
}

#[test]
fn reply_ping_pong_and_forward_chains_are_still_cut_off() {
    let fx = home();
    ok(&call(&fx, CLAUDE, "everett_send", json!({"text": "start", "to": "x-1", "mode": "inbox", "hours": HOURS})));
    let mut last = newest(&fx, CODEX);
    let mut turn = [CODEX, CLAUDE].into_iter().cycle();
    for depth in 1..=4 {
        let who = turn.next().unwrap();
        ok(&call(&fx, who, "everett_send", json!({"text": format!("reply {depth}"), "reply_to": last["id"]})));
        let other = if who == CODEX { CLAUDE } else { CODEX };
        last = newest(&fx, other);
        assert_eq!(last["reply_depth"], depth, "{last}");
    }
    let who = turn.next().unwrap();
    refused(&call(&fx, who, "everett_send", json!({"text": "reply 5", "reply_to": last["id"]})), "Reply limit reached (4/4)");

    let chained = home();
    let hop3 = mcp_calls(
        &chained,
        &[],
        &[CLAUDE, ("EVERETT_HOPS", "3")],
        &[("everett_send", json!({"text": "forward again", "to": "x-1", "mode": "inbox", "hours": HOURS}))],
    );
    refused(&hop3[0], "Hop limit reached (3/3)");
}
