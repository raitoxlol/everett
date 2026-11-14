//! T3 CLI discovery, process capture and JSON receipts shared by control and wake.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::proc::{is_executable, run_capture, which, RunOutput};
use crate::session::home;

pub(crate) struct Captured {
    pub output: RunOutput,
    pub receipt: Option<Value>,
}

pub(crate) fn capture(binary: &str, args: &[&str], timeout: f64) -> Result<Captured, String> {
    let mut argv = vec![binary.to_string()];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    let env: HashMap<String, String> = std::env::vars().collect();
    let output = run_capture(&argv, None, &env, timeout)?;
    let receipt = parse_json_output(output.stdout.trim());
    Ok(Captured { output, receipt })
}

/// CLI dispatch receipts differ from read/list JSON. These shapes follow
/// cli/thread.ts (ThreadLaunchResult, finishRun) and OrchestratorMcpThreadSendResult.
pub(crate) enum Dispatch<'a> {
    Start,
    Send(&'a str),
}

pub(crate) fn validate_dispatch(
    value: &Value,
    dispatch: Dispatch<'_>,
    wait: bool,
    exit: i32,
) -> Result<(), &'static str> {
    let nonempty = |key: &str| value.get(key).and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty());
    let valid = value.is_object()
        && nonempty("threadId")
        && !value.get("error").is_some_and(|error| !error.is_null())
        && ["accepted", "ok"].iter().all(|key| matches!(value.get(*key), None | Some(Value::Bool(true))));
    let target_matches = match dispatch {
        Dispatch::Start => true,
        Dispatch::Send(thread) => value.get("threadId").and_then(Value::as_str) == Some(thread),
    };
    let shape = if wait && value.get("status").is_some() {
        nonempty("runId")
            && matches!((exit, value.get("status").and_then(Value::as_str)), (0, Some("completed")) | (2, Some("waiting")))
            && value.get("error") == Some(&Value::Null)
            && matches!(value.get("reply"), Some(Value::Null | Value::String(_)))
    } else {
        exit == 0 && match dispatch {
            // A start can create a thread without a run, including with --wait.
            Dispatch::Start => value.get("status").is_none() && nonempty("projectId")
                && (nonempty("runId") || value.get("runId") == Some(&Value::Null))
                && (!wait || value.get("runId") == Some(&Value::Null)),
            Dispatch::Send(_) => !wait && nonempty("messageId") && nonempty("runId")
                && matches!(value.get("delivery").and_then(Value::as_str), Some("started" | "queued" | "steered" | "restarted"))
                && matches!(value.get("status").and_then(Value::as_str), Some("preparing" | "queued" | "starting" | "running" | "waiting" | "completed" | "interrupted" | "failed" | "cancelled" | "rolled_back")),
        }
    };
    if valid && target_matches && shape {
        Ok(())
    } else {
        Err("t3 returned no valid dispatch JSON receipt for this command and target. Dispatch is unconfirmed; do not blindly retry.")
    }
}

/// The JSON document in `t3` stdout. Some builds print log lines (such as a
/// database migration warning) to stdout first, so take the first line that
/// starts a JSON value at column 0 and parses through the end.
fn parse_json_output(stdout: &str) -> Option<Value> {
    let mut offset = 0;
    for line in stdout.split_inclusive('\n') {
        if line.starts_with('{') || line.starts_with('[') {
            if let Ok(v) = serde_json::from_str(stdout[offset..].trim()) {
                return Some(v);
            }
        }
        offset += line.len();
    }
    None
}

/// `EVERETT_T3_BIN`, else `t3` on PATH, else T3's own install location.
pub(crate) fn binary() -> Option<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    if let Ok(bin) = std::env::var("EVERETT_T3_BIN") {
        return if bin.is_empty() {
            None
        } else if bin.contains(std::path::MAIN_SEPARATOR) {
            (Path::new(&bin).is_file() && is_executable(&bin)).then_some(bin)
        } else {
            which(&bin, &path)
        };
    }
    which("t3", &path).or_else(|| {
        let installed = home().join(".t3").join("bin").join("t3");
        let bin = installed.to_string_lossy().to_string();
        (installed.is_file() && is_executable(&bin)).then_some(bin)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_output_skips_leading_log_noise() {
        let noisy = "[19:46] WARN (#20): migration diverges {\n  divergent: [\n    '73:X'\n  ]\n}\n[\n  {\"id\": \"t1\"}\n]\n";
        assert_eq!(parse_json_output(noisy), Some(json!([{"id": "t1"}])));
        assert_eq!(
            parse_json_output("{\"ok\":true}"),
            Some(json!({"ok": true}))
        );
        assert_eq!(parse_json_output("plain text"), None);
    }
    #[test]
    fn dispatch_receipts_follow_cli_command_contracts() {
        let send = json!({"threadId":"t1","messageId":"m1","runId":"r1","status":"running","delivery":"started"});
        assert!(validate_dispatch(&send, Dispatch::Send("t1"), false, 0).is_ok());
        let mut queued = send.clone();
        queued["status"] = json!("queued");
        queued["delivery"] = json!("queued");
        assert!(validate_dispatch(&queued, Dispatch::Send("t1"), false, 0).is_ok());
        assert!(validate_dispatch(&send, Dispatch::Send("other"), false, 0).is_err());
        assert!(validate_dispatch(&send, Dispatch::Send("t1"), true, 0).is_err());
        assert!(validate_dispatch(&send, Dispatch::Send("t1"), false, 2).is_err());
        for key in ["messageId", "runId", "threadId", "status", "delivery"] {
            let mut missing = send.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(validate_dispatch(&missing, Dispatch::Send("t1"), false, 0).is_err(), "{key}");
        }
        for bad in [json!({"accepted":false}), json!({}), json!([]), json!({"accepted":true}), json!({"ok":true})] {
            assert!(validate_dispatch(&bad, Dispatch::Start, false, 0).is_err());
            assert!(validate_dispatch(&bad, Dispatch::Send("t1"), false, 0).is_err());
        }
        for (key, value) in [("accepted", json!(false)), ("ok", json!(false)), ("error", json!("rejected")), ("delivery", json!("rejected"))] {
            let mut bad = send.clone();
            bad[key] = value;
            assert!(validate_dispatch(&bad, Dispatch::Send("t1"), false, 0).is_err(), "{key}");
        }
        let start = json!({"threadId":"t1","projectId":"p1","runId":"r1"});
        assert!(validate_dispatch(&start, Dispatch::Start, false, 0).is_ok());
        assert!(validate_dispatch(&start, Dispatch::Start, true, 0).is_err());
        let idle = json!({"threadId":"t1","projectId":"p1","runId":null});
        assert!(validate_dispatch(&idle, Dispatch::Start, true, 0).is_ok());
        for (status, exit) in [("completed", 0), ("waiting", 2)] {
            let waited = json!({"threadId":"t1","runId":"r1","status":status,"reply":null,"error":null});
            assert!(validate_dispatch(&waited, Dispatch::Start, true, exit).is_ok());
            assert!(validate_dispatch(&waited, Dispatch::Send("t1"), true, exit).is_ok());
            assert!(validate_dispatch(&waited, Dispatch::Send("t1"), true, 2 - exit).is_err());
        }
        let failed = json!({"threadId":"t1","runId":"r1","status":"failed","reply":null,"error":"failed"});
        assert!(validate_dispatch(&failed, Dispatch::Send("t1"), true, 0).is_err());
    }

}
