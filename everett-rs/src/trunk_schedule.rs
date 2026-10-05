//! `everett trunk schedule`: a macOS LaunchAgent that runs `everett trunk merge` nightly.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::error::{EverettError, Result};
use crate::proc::run_capture;
use crate::session::home;

pub const LABEL: &str = "dev.everett.core-merge";

pub fn plist_path() -> PathBuf {
    home().join("Library").join("LaunchAgents").join(format!("{}.plist", LABEL))
}

pub fn log_path() -> PathBuf {
    home().join(".everett").join("logs").join("merge.log")
}

/// The `everett` binary invocation for this install.
fn everett_command() -> Vec<String> {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "everett".to_string());
    vec![exe]
}

fn parse_at(at: &str) -> Result<(i64, i64)> {
    let at = if at.is_empty() { "04:00" } else { at };
    let (hour_s, minute_s) = at.split_once(':').unwrap_or((at, ""));
    let hour: i64 = hour_s
        .parse()
        .map_err(|_| EverettError::new(2, format!("--at must be HH:MM, got {:?}", at)))?;
    let minute: i64 = if minute_s.is_empty() {
        0
    } else {
        minute_s
            .parse()
            .map_err(|_| EverettError::new(2, format!("--at must be HH:MM, got {:?}", at)))?
    };
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return Err(EverettError::new(2, format!("--at must be HH:MM (00-23:00-59), got {:?}", at)));
    }
    Ok((hour, minute))
}

pub fn plist_data(at: &str, llm: &str) -> Result<Map<String, Value>> {
    let (hour, minute) = parse_at(at)?;
    let log = log_path().to_string_lossy().to_string();
    let mut env = Map::new();
    env.insert(
        "PATH".into(),
        json!(std::env::var("PATH").unwrap_or_else(|_| ":/bin:/usr/bin".to_string())),
    );
    for (k, v) in crate::install::mcp_launch().2 {
        env.insert(k, json!(v));
    }
    let mut args = everett_command();
    args.extend(["trunk", "merge", "--llm", llm].iter().map(|s| s.to_string()));
    let mut d = Map::new();
    d.insert("Label".into(), json!(LABEL));
    d.insert("ProgramArguments".into(), json!(args));
    d.insert("EnvironmentVariables".into(), Value::Object(env));
    let mut interval = Map::new();
    interval.insert("Hour".into(), json!(hour));
    interval.insert("Minute".into(), json!(minute));
    d.insert("StartCalendarInterval".into(), Value::Object(interval));
    d.insert("StandardOutPath".into(), json!(log));
    d.insert("StandardErrorPath".into(), json!(log));
    d.insert("RunAtLoad".into(), json!(false));
    Ok(d)
}

fn plist_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn plist_value(v: &Value, depth: usize, out: &mut Vec<String>) {
    let ind = "\t".repeat(depth);
    match v {
        Value::Object(map) => {
            out.push(format!("{}<dict>", ind));
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for key in keys {
                out.push(format!("{}<key>{}</key>", "\t".repeat(depth + 1), plist_escape(key)));
                plist_value(&map[key], depth + 1, out);
            }
            out.push(format!("{}</dict>", ind));
        }
        Value::Array(items) => {
            out.push(format!("{}<array>", ind));
            for item in items {
                plist_value(item, depth + 1, out);
            }
            out.push(format!("{}</array>", ind));
        }
        Value::String(s) => out.push(format!("{}<string>{}</string>", ind, plist_escape(s))),
        Value::Number(n) => out.push(format!("{}<integer>{}</integer>", ind, n)),
        Value::Bool(b) => out.push(format!("{}<{} />", ind, if *b { "true" } else { "false" })),
        _ => out.push(format!("{}<string></string>", ind)),
    }
}

pub fn render_plist(at: &str, llm: &str) -> Result<Vec<u8>> {
    let data = plist_data(at, llm)?;
    let mut out = vec![
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_string(),
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">"
            .to_string(),
        "<plist version=\"1.0\">".to_string(),
    ];
    plist_value(&Value::Object(data), 0, &mut out);
    out.push("</plist>".to_string());
    Ok((out.join("\n") + "\n").into_bytes())
}

pub fn is_scheduled(path: Option<&PathBuf>) -> bool {
    path.cloned().unwrap_or_else(plist_path).exists()
}

fn domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

/// Write the plist (idempotent) and `launchctl bootstrap` it.
pub fn install(at: &str, llm: &str, path: Option<&PathBuf>, log: Option<&PathBuf>) -> Result<Map<String, Value>> {
    let target = path.cloned().unwrap_or_else(plist_path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| EverettError::new(2, e.to_string()))?;
    }
    if let Some(parent) = log.cloned().unwrap_or_else(log_path).parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&target, render_plist(at, llm)?).map_err(|e| EverettError::new(2, e.to_string()))?;
    let _ = fs::set_permissions(&target, PermissionsExt::from_mode(0o644));
    let result = run_capture(
        &[
            "launchctl".to_string(),
            "bootstrap".to_string(),
            domain(),
            target.to_string_lossy().to_string(),
        ],
        None,
        &HashMap::new(),
        15.0,
    );
    let (ok, stderr) = match result {
        Ok(r) => (r.code == 0, r.stderr.trim().to_string()),
        Err(e) => (false, e),
    };
    let mut r = Map::new();
    r.insert("path".into(), json!(target.to_string_lossy()));
    r.insert("launchctl_ok".into(), json!(ok));
    r.insert("stderr".into(), json!(stderr));
    Ok(r)
}

/// Bootout and delete the plist. Safe to call when nothing is scheduled.
pub fn remove(path: Option<&PathBuf>) -> Map<String, Value> {
    let target = path.cloned().unwrap_or_else(plist_path);
    let existed = target.exists();
    if existed {
        let _ = run_capture(
            &[
                "launchctl".to_string(),
                "bootout".to_string(),
                domain(),
                target.to_string_lossy().to_string(),
            ],
            None,
            &HashMap::new(),
            15.0,
        );
        let _ = fs::remove_file(&target);
    }
    let mut r = Map::new();
    r.insert("path".into(), json!(target.to_string_lossy()));
    r.insert("removed".into(), json!(existed));
    r
}
