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
}
