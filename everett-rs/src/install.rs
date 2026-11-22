//! `everett install-hooks` / `everett install-mcp`: print or merge hook + MCP registrations.
//! `--apply` backs up each file it changes (<file>.everett-bak-<timestamp>), merges
//! idempotently, never duplicates an Everett entry and never removes anyone else's hooks.

pub(crate) mod presence;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::error::{EverettError, Result};
use crate::proc::shlex_quote;
use crate::session::home;
use crate::timefmt::strftime_local;

type HookSpec = (&'static str, &'static str, i64);
const HOOKS: &[(&str, &[HookSpec])] = &[
    (
        "claude",
        &[
            ("SessionStart", "claude_session_start", 3),
            ("Stop", "claude_stop", 3),
            ("UserPromptSubmit", "claude_inbox", 3),
            ("PostToolUse", "claude_inbox", 3),
        ],
    ),
    (
        "codex",
        &[
            ("SessionStart", "codex_session_start", 5),
            ("Stop", "codex_stop", 3),
            ("UserPromptSubmit", "codex_inbox", 3),
            ("PostToolUse", "codex_inbox", 3),
        ],
    ),
    (
        "grok",
        &[("Stop", "grok_stop", 5), ("PostToolUse", "grok_inbox", 3)],
    ),
    (
        "devin",
        &[("SessionStart", "devin_bind", 3), ("UserPromptSubmit", "devin_bind", 3)],
    ),
];

fn hooks_for(harness: &str) -> &'static [(&'static str, &'static str, i64)] {
    HOOKS
        .iter()
        .find(|(h, _)| *h == harness)
        .map(|(_, v)| *v)
        .unwrap_or(&[])
}

pub(crate) fn settings_hook_harnesses() -> impl Iterator<Item = &'static str> {
    HOOKS.iter().map(|(harness, _)| *harness)
}

pub(crate) fn hook_events(harness: &str) -> impl Iterator<Item = &'static str> {
    hooks_for(harness).iter().map(|(event, _, _)| *event)
}

fn exe() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "everett".to_string())
}

fn hook_command(script: &str) -> String {
    format!("{} hook {}", shlex_quote(&exe()), script)
}

pub fn settings_path(harness: &str) -> PathBuf {
    home().join(match harness {
        "claude" => ".claude/settings.json",
        "codex" => ".codex/hooks.json",
        "devin" => return mcp_path("devin").with_file_name("config.json"),
        _ => ".grok/hooks/everett.json",
    })
}

pub fn pi_extension_path() -> PathBuf {
    crate::adapters::pi::agent_dir().join("extensions").join("everett.ts")
}

pub fn pi_extension_source() -> String {
    include_str!("pi_session_start.ts").replace("__EVERETT_BIN__", &exe())
}

/// Whether a harness looks installed: its home directory exists or its binary is on PATH.
pub fn harness_present(harness: &str) -> bool {
    let dir = match harness {
        "devin" => mcp_path("devin").parent().map(PathBuf::from).unwrap_or_default(),
        other => home().join(format!(".{}", other)),
    };
    dir.is_dir() || crate::proc::which(harness, &std::env::var("PATH").unwrap_or_default()).is_some()
}

pub fn omp_extension_path() -> PathBuf {
    home().join(".omp").join("agent").join("extensions").join("everett.ts")
}

/// The OMP extension body, with this binary's absolute path baked in so OMP
/// still finds it when its PATH differs from ours (basename is the fallback).
pub fn omp_extension_source() -> String {
    include_str!("omp_session_start.mjs").replace("\"__EVERETT_BIN__\"", &serde_json::to_string(&exe()).unwrap())
}

/// A hook registered by the retired Python package: `<python> <...>/everett/hooks/<script>.py`.
fn is_legacy_hook(command: &str) -> bool {
    let command = command.trim_end().trim_end_matches(['\'', '"']);
    command.ends_with(".py")
        && HOOKS
            .iter()
            .flat_map(|(_, hooks)| hooks.iter())
            .any(|(_, script, _)| command.ends_with(&format!("everett/hooks/{}.py", script)))
}

/// `shlex.split`: whitespace-separated tokens honoring 'single'/"double" quotes
/// and backslash escapes. None on an unterminated quote.
fn command_tokens(command: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    started = true;
                }
                '\\' => {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                    }
                    started = true;
                }
                _ if c.is_whitespace() => {
                    if started || !cur.is_empty() {
                        tokens.push(std::mem::take(&mut cur));
                    }
                    started = false;
                }
                _ => {
                    cur.push(c);
                    started = true;
                }
            },
        }
    }
    if quote.is_some() {
        return None;
    }
    if started || !cur.is_empty() {
        tokens.push(cur);
    }
    Some(tokens)
}

/// Some(true)=ok / Some(false)=stale for an Everett hook command registered for
/// `script`, else None. A registration that still mentions Everett + the script
/// but whose Everett binary is gone or non-executable is stale: it fails silently.
fn hook_state(command: &str, script: &str) -> Option<bool> {
    if !command.contains(script) || !command.contains("everett") {
        return None;
    }
    if is_legacy_hook(command) {
        return Some(false); // launches the retired Python package
    }
    match command_tokens(command).and_then(|t| t.first().cloned()) {
        Some(first) => Some(crate::proc::executable_ok(&first)),
        None => Some(false),
    }
}

/// 'ok' | 'stale' | 'missing' for Everett's registration of `script` in one event's entries.
fn script_state(entries: &Value, script: &str) -> &'static str {
    let mut stale = false;
    if let Some(entries) = entries.as_array() {
        for entry in entries {
            let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) else { continue };
            for hook in hooks {
                let command = hook.get("command").and_then(|c| c.as_str()).unwrap_or("");
                match hook_state(command, script) {
                    Some(true) => return "ok",
                    Some(false) => stale = true,
                    None => {}
                }
            }
        }
    }
    if stale { "stale" } else { "missing" }
}

fn omp_extension_state() -> &'static str {
    let Ok(text) = fs::read_to_string(omp_extension_path()) else {
        return "missing";
    };
    // A Python-era extension shells out to python3, not the binary.
    if text.contains("python3") {
        return "stale";
    }
    if crate::proc::executable_ok("everett") {
        "ok"
    } else {
        "stale"
    }
}

/// Per-event 'ok' | 'stale' | 'missing' so doctor/apply can repair dead registrations.
pub fn installed(harness: &str, data: Option<&Map<String, Value>>) -> std::result::Result<HashMap<String, String>, String> {
    if harness == "omp" {
        return Ok(HashMap::from([("extension".to_string(), omp_extension_state().to_string())]));
    }
    let owned;
    let data = match data {
        Some(d) => d,
        None => {
            owned = load(&settings_path(harness))?;
            &owned
        }
    };
    let empty = Map::new();
    let hooks = if data.get("hooks").map(|h| h.is_object()).unwrap_or(false) {
        data.get("hooks").and_then(|h| h.as_object()).unwrap()
    } else {
        &empty
    };
    let mut out = HashMap::new();
    for (event, script, _) in hooks_for(harness) {
        let entries = hooks.get(*event).cloned().unwrap_or(Value::Null);
        out.insert(event.to_string(), script_state(&entries, script).to_string());
    }
    Ok(out)
}

/// Merged settings, events added, events whose legacy Python hooks were replaced.
type MergedHooks = (Map<String, Value>, Vec<String>, Vec<String>);

pub fn merge(
    data: &Map<String, Value>,
    harness: &str,
    events: Option<&[String]>,
) -> std::result::Result<MergedHooks, String> {
    let mut data = data.clone();
    if !data.get("hooks").map(|h| h.is_object()).unwrap_or(true) {
        return Err("\"hooks\" is not an object".to_string());
    }
    let hooks = data.entry("hooks").or_insert(Value::Object(Map::new()));
    let hooks = hooks.as_object_mut().unwrap();
    let mut added = Vec::new();
    let mut migrated = Vec::new();
    for (event, script, timeout) in hooks_for(harness) {
        if let Some(events) = events {
            if !events.iter().any(|e| e.as_str() == *event) {
                continue;
            }
        }
        if hooks.get(*event).map(|e| !e.is_array()).unwrap_or(false) {
            return Err(format!("\"hooks.{}\" is not a list", event));
        }
        let state = script_state(hooks.get(*event).unwrap_or(&Value::Null), script);
        if state == "ok" {
            continue;
        }
        let entries = hooks.entry(event.to_string()).or_insert(Value::Array(Vec::new()));
        let entries = entries.as_array_mut().unwrap();
        if state == "stale" {
            // Replace only Everett's own dead hooks (moved binary, or a retired
            // Python registration); other hooks in this event stay.
            let mut legacy_removed = false;
            for entry in entries.iter_mut() {
                if let Some(list) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                    list.retain(|h| {
                        let command = h.get("command").and_then(|c| c.as_str()).unwrap_or("");
                        let stale = hook_state(command, script) == Some(false);
                        legacy_removed |= stale && is_legacy_hook(command);
                        !stale
                    });
                }
            }
            entries.retain(|e| e.get("hooks").and_then(|h| h.as_array()).map(|h| !h.is_empty()).unwrap_or(true));
            if legacy_removed && !migrated.iter().any(|e| e == event) {
                migrated.push(event.to_string());
            }
        }
        entries.push(json!({
            "hooks": [{"type": "command", "command": hook_command(script), "timeout": timeout}]
        }));
        if !migrated.iter().any(|e| e == event) {
            added.push(event.to_string());
        }
    }
    Ok((data, added, migrated))
}

fn load(path: &PathBuf) -> std::result::Result<Map<String, Value>, String> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(e.to_string()),
    };
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(format!("{} is not a JSON object", path.display())),
    }
}

pub fn backup(path: &PathBuf) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }
    let stamp = strftime_local("%Y%m%d-%H%M%S", crate::session::now());
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut dest = path.with_file_name(format!("{}.everett-bak-{}", name, stamp));
    let mut n = 1;
    while dest.exists() {
        dest = path.with_file_name(format!("{}.everett-bak-{}-{}", name, stamp, n));
        n += 1;
    }
    fs::copy(path, &dest).ok()?;
    Some(dest)
}

fn write_preserving_mode(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::{ErrorKind, Write};
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(contents.as_bytes())?;
    match fs::metadata(path) {
        Ok(metadata) => tmp.as_file().set_permissions(metadata.permissions())?,
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    tmp.persist(path)?;
    Ok(())
}

fn write_json(path: &Path, data: &Map<String, Value>) -> std::io::Result<()> {
    write_preserving_mode(path, &format!("{}\n", serde_json::to_string_pretty(data)?))
}

pub fn snippet(harness: &str) -> String {
    if harness == "pi" {
        return format!(
            "# Pi auto-loads extensions; write {} with:\n{}",
            pi_extension_path().display(),
            pi_extension_source()
        );
    }
    if harness == "omp" {
        return format!(
            "# OMP auto-loads extensions from {}/\n# write {} with:\n{}# or pass it per launch:\nomp --hook={}\n",
            omp_extension_path().parent().unwrap().display(),
            omp_extension_path().display(),
            omp_extension_source(),
            "<bundled omp_session_start.mjs>"
        );
    }
    let mut hooks = Map::new();
    for (event, script, timeout) in hooks_for(harness) {
        hooks.insert(
            event.to_string(),
            json!([{"hooks": [{"type": "command", "command": hook_command(script), "timeout": timeout}]}]),
        );
    }
    let note = match harness {
        "claude" => "# merge into ~/.claude/settings.json (keep your existing hooks)".to_string(),
        "codex" => "# merge into ~/.codex/hooks.json; Codex needs `hooks = true` in ~/.codex/config.toml\n# and asks you to trust new hooks on first run".to_string(),
        "devin" => format!("# merge into {} (keep your existing hooks)", settings_path("devin").display()),
        _ => "# write ~/.grok/hooks/everett.json (Grok loads every file in ~/.grok/hooks/)".to_string(),
    };
    let mut body = Map::new();
    body.insert("hooks".into(), Value::Object(hooks));
    format!("{}\n{}\n", note, serde_json::to_string_pretty(&body).unwrap())
}

/// Install for one harness; returns a one-line report.
pub fn apply(harness: &str, events: Option<&[String]>) -> std::result::Result<String, String> {
    if events.map(|e| e.is_empty()).unwrap_or(false) {
        return Ok(format!("{}: skipped (no hooks selected)", harness));
    }
    if harness == "omp" || harness == "pi" {
        let (path, source) = if harness == "pi" {
            (pi_extension_path(), pi_extension_source())
        } else {
            (omp_extension_path(), omp_extension_source())
        };
        if path.exists() && fs::read_to_string(&path).unwrap_or_default() == source {
            return Ok(format!("{}: already installed ({})", harness, path.display()));
        }
        let saved = backup(&path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&path, &source).map_err(|e| e.to_string())?;
        return Ok(format!(
            "{}: wrote {}{}",
            harness,
            path.display(),
            saved.map(|s| format!(" (backup {})", s.display())).unwrap_or_default()
        ));
    }
    let path = settings_path(harness);
    let data = load(&path)?;
    let (merged, added, migrated) = merge(&data, harness, events)?;
    if added.is_empty() && migrated.is_empty() {
        return Ok(format!("{}: already installed ({})", harness, path.display()));
    }
    let saved = backup(&path);
    write_json(&path, &merged).map_err(|e| e.to_string())?;
    let mut changes = Vec::new();
    if !added.is_empty() {
        changes.push(format!("added {}", added.join(", ")));
    }
    if !migrated.is_empty() {
        changes.push(format!("replaced legacy Python hooks for {}", migrated.join(", ")));
    }
    Ok(format!(
        "{}: {} in {}{}",
        harness,
        changes.join("; "),
        path.display(),
        saved.map(|s| format!(" (backup {})", s.display())).unwrap_or_default()
    ))
}


/// (command, args, env) that starts `everett mcp` from this very install.
pub fn mcp_launch() -> (String, Vec<String>, std::collections::HashMap<String, String>) {
    (exe(), vec!["mcp".to_string()], std::collections::HashMap::new())
}

/// The launch args for `harness`: hosts that cannot pass a session id get `--harness`, so the
/// server binds callers through the host process instead.
pub fn mcp_args(harness: &str) -> Vec<String> {
    let mut args = vec!["mcp".to_string()];
    if crate::host::DETECTABLE.contains(&harness) && harness != "pi" {
        args.extend(["--harness".to_string(), harness.to_string()]);
    }
    args
}

fn mcp_json_entry(harness: &str) -> Map<String, Value> {
    let (command, _, env) = mcp_launch();
    let args = mcp_args(harness);
    let mut entry = Map::new();
    entry.insert("type".into(), json!("stdio"));
    entry.insert("command".into(), json!(command));
    entry.insert("args".into(), json!(args));
    if !env.is_empty() {
        entry.insert(
            "env".into(),
            Value::Object(env.iter().map(|(k, v)| (k.clone(), json!(v))).collect()),
        );
    }
    entry
}

pub fn mcp_path(harness: &str) -> PathBuf {
    match harness {
        "pi" => pi_extension_path(),
        "claude" => home().join(".claude.json"),
        "codex" => home().join(".codex").join("config.toml"),
        "grok" => home().join(".grok").join("config.toml"),
        "devin" => {
            let root = if cfg!(windows) {
                std::env::var_os("APPDATA")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home().join("AppData").join("Roaming"))
            } else {
                home().join(".config")
            };
            root.join("devin").join("mcp_config.json")
        }
        _ => home().join(".omp").join("agent").join("mcp.json"),
    }
}

const TOML_MCP: &[&str] = &["codex", "grok"];

fn toml_str(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

fn codex_block() -> String {
    let (command, args, env) = mcp_launch();
    let mut lines = vec![
        "[mcp_servers.everett]".to_string(),
        format!("command = {}", toml_str(&command)),
        format!("args = [{}]", args.iter().map(|a| toml_str(a)).collect::<Vec<_>>().join(", ")),
    ];
    if !env.is_empty() {
        lines.push(format!(
            "env = {{ {} }}",
            env.iter().map(|(k, v)| format!("{} = {}", k, toml_str(v))).collect::<Vec<_>>().join(", ")
        ));
    }
    lines.join("\n") + "\n"
}

pub fn mcp_config(harness: &str) -> std::result::Result<Map<String, Value>, String> {
    let path = mcp_path(harness);
    if TOML_MCP.contains(&harness) {
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
            Err(e) => return Err(e.to_string()),
        };
        if text.trim().is_empty() {
            return Ok(Map::new());
        }
        let value: toml::Value = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let value = serde_json::to_value(value).map_err(|e| e.to_string())?;
        return match value {
            Value::Object(m) => Ok(m),
            _ => Ok(Map::new()),
        };
    }
    load(&path)
}

pub fn mcp_entry(harness: &str, data: &Map<String, Value>) -> std::result::Result<Option<Value>, String> {
    let key = if TOML_MCP.contains(&harness) { "mcp_servers" } else { "mcpServers" };
    match data.get(key) {
        None => return Ok(None),
        Some(v) if !v.is_object() => return Err(format!("{} must be an object", key)),
        _ => {}
    }
    Ok(data
        .get(key)
        .and_then(|s| s.get("everett"))
        .cloned())
}

pub fn mcp_status(harness: &str) -> (String, String) {
    let entry = match mcp_config(harness).and_then(|d| mcp_entry(harness, &d)) {
        Ok(e) => e,
        Err(e) => return ("invalid".into(), format!("cannot read {}: {}", mcp_path(harness).display(), e)),
    };
    let Some(entry) = entry else {
        return ("missing".into(), "not registered".to_string());
    };
    let Some(entry) = entry.as_object() else {
        return ("invalid".into(), "Everett MCP entry must be an object".to_string());
    };
    if entry.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false)
        || entry.get("enabled").and_then(|v| v.as_bool()) == Some(false)
    {
        return ("stale".into(), "registration is disabled".to_string());
    }
    let command = entry.get("command").and_then(|v| v.as_str()).unwrap_or("");
    if harness == "devin" && (entry.get("url").and_then(Value::as_str).map(|s| !s.is_empty()).unwrap_or(false)
        || entry.get("transport").map(|v| v.as_str() != Some("stdio")).unwrap_or(false))
    {
        return ("stale".into(), "registration is not Everett stdio".to_string());
    }
    let args: Vec<String> = match entry.get("args") {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            if items.iter().any(|x| !x.is_string()) {
                return ("invalid".into(), "registration args must be strings".to_string());
            }
            items.iter().map(|x| x.as_str().unwrap_or_default().to_string()).collect()
        }
        Some(_) => return ("invalid".into(), "registration args must be a list of strings".to_string()),
    };
    if command.is_empty() {
        return ("stale".into(), "registration has no executable".to_string());
    }
    if !crate::proc::executable_ok(command) {
        return ("stale".into(), "registered executable is missing, not executable, or not on PATH".to_string());
    }
    if is_legacy_mcp(entry) {
        return ("stale".into(), "registration launches the retired Python package".to_string());
    }
    let name = PathBuf::from(command)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !(name == "everett" && args == mcp_args(harness)) {
        return ("stale".into(), "registration does not launch Everett stdio MCP".to_string());
    }
    if let Some(env) = entry.get("env") {
        if !env.is_object() {
            return ("invalid".into(), "registration env must be an object".to_string());
        }
        for v in env.as_object().unwrap().values() {
            if !v.is_string() {
                return ("invalid".into(), "registration env values must be strings".to_string());
            }
        }
    }
    ("ready".into(), "Everett stdio MCP registered".to_string())
}

/// `<python> -m everett mcp`, as written by the retired Python package.
fn is_legacy_mcp(entry: &Map<String, Value>) -> bool {
    entry.get("args") == Some(&json!(["-m", "everett", "mcp"]))
}

fn repaired_entry(old: &Map<String, Value>, harness: &str) -> Result<Map<String, Value>> {
    let mut entry = old.clone();
    for (k, v) in mcp_json_entry(harness) {
        entry.insert(k, v);
    }
    let env = old.get("env").cloned().unwrap_or(Value::Object(Map::new()));
    let Some(env) = env.as_object() else {
        return Err(EverettError::new(2, "registration env must be an object"));
    };
    let mut env = env.clone();
    env.remove("PYTHONPATH");
    for (k, v) in mcp_launch().2 {
        env.insert(k, json!(v));
    }
    if !env.is_empty() {
        entry.insert("env".into(), Value::Object(env));
    } else {
        entry.remove("env");
    }
    entry.remove("url");
    if entry.contains_key("disabled") {
        entry.insert("disabled".into(), json!(false));
    }
    if entry.contains_key("enabled") {
        entry.insert("enabled".into(), json!(true));
    }
    Ok(entry)
}

fn toml_value(value: &Value) -> std::result::Result<String, String> {
    match value {
        Value::String(s) => Ok(toml_str(s)),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Array(a) => Ok(format!(
            "[{}]",
            a.iter().map(toml_value).collect::<std::result::Result<Vec<_>, _>>()?.join(", ")
        )),
        Value::Object(o) => Ok(format!(
            "{{ {} }}",
            o.iter()
                .map(|(k, v)| toml_value(v).map(|v| format!("{} = {}", toml_str(k), v)))
                .collect::<std::result::Result<Vec<_>, _>>()?
                .join(", ")
        )),
        _ => Err("unsupported value in Everett MCP entry; edit the entry manually".to_string()),
    }
}

fn replace_toml_entry(text: &str, entry: &Map<String, Value>) -> std::result::Result<String, String> {
    static TABLE_RE: OnceLock<Regex> = OnceLock::new();
    let table_re = TABLE_RE.get_or_init(|| Regex::new(r"^\s*\[([^\]]+)\]\s*(?:#.*)?$").unwrap());
    let mut lines: Vec<String> = Vec::new();
    let mut removing = false;
    let mut found = false;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with('[') {
            let table = table_re
                .captures(line)
                .map(|m| m[1].replace(['"', '\'', ' '], ""))
                .unwrap_or_default();
            removing = table == "mcp_servers.everett" || table.starts_with("mcp_servers.everett.");
            found |= removing;
        }
        if !removing {
            lines.push(line.to_string());
        }
    }
    if !found {
        return Err("Everett uses an inline MCP table; edit it manually before retrying --repair --apply".to_string());
    }
    let block = format!(
        "[mcp_servers.everett]\n{}",
        entry
            .iter()
            .map(|(k, v)| toml_value(v).map(|v| format!("{} = {}\n", toml_str(k), v)))
            .collect::<std::result::Result<String, _>>()?
    );
    Ok(format!("{}\n\n{}", lines.concat().trim_end(), block))
}

pub fn mcp_snippet(harness: &str) -> String {
    let (command, args, env) = mcp_launch();
    match harness {
        "pi" => format!(
            "# Pi: run `everett install-hooks --pi --apply`; the extension in {} exposes Everett tools through Pi's\n# registerTool API and starts a session-bound MCP bridge. Do not also list it in ~/.pi/agent/mcp.json.\n",
            pi_extension_path().display()
        ),
        "hermes" => format!(
            "# Recommended: install the native Hermes plugin (binds every CLI, TUI, Desktop, and gateway session):\n#   hermes plugins install raitoxlol/everett#plugins/everett-hermes --no-enable\n#   hermes plugins enable everett-hermes\n# Manual stdio MCP fallback (add under mcp_servers in ~/.hermes/config.yaml; do not combine with the plugin):\nmcp_servers:\n  everett:\n    command: {}\n    args: [{}]\n",
            serde_json::to_string(&command).unwrap(),
            mcp_args("hermes").iter().map(|a| serde_json::to_string(a).unwrap()).collect::<Vec<_>>().join(", ")
        ),
        "devin" => {
            let mut entry = mcp_json_entry(harness);
            entry.remove("type");
            format!(
                "# user scope, Devin CLI v3000.3+: merge into {}\n{}\n",
                mcp_path(harness).display(),
                serde_json::to_string_pretty(&json!({"mcpServers": {"everett": entry}})).unwrap()
            )
        }
        "claude" => {
            let env_flags = env
                .iter()
                .map(|(k, v)| format!(" -e {}={}", k, shlex_quote(v)))
                .collect::<String>();
            let mut entry = Map::new();
            let mut servers = Map::new();
            servers.insert("everett".into(), Value::Object(mcp_json_entry(harness)));
            entry.insert("mcpServers".into(), Value::Object(servers));
            format!(
                "# user scope, all projects:\nclaude mcp add --scope user{} everett -- {} {}\n# (or add under \"mcpServers\" in ~/.claude.json)\n{}\n",
                env_flags,
                shlex_quote(&command),
                args.join(" "),
                serde_json::to_string_pretty(&entry).unwrap()
            )
        }
        "codex" => format!("# append to ~/.codex/config.toml\n{}", codex_block()),
        "grok" => {
            let env_flags = env
                .iter()
                .map(|(k, v)| format!(" -e {}={}", k, shlex_quote(v)))
                .collect::<String>();
            format!(
                "# user scope (~/.grok/config.toml):\ngrok mcp add{} everett {} -- {}\n# (or append to ~/.grok/config.toml; Grok also imports Claude Code's servers)\n{}",
                env_flags,
                shlex_quote(&command),
                args.join(" "),
                codex_block()
            )
        }
        _ => {
            let mut entry = Map::new();
            let mut servers = Map::new();
            servers.insert("everett".into(), Value::Object(mcp_json_entry(harness)));
            entry.insert("mcpServers".into(), Value::Object(servers));
            format!(
                "# merge into {} (OMP also imports Claude Code's servers)\n{}\n",
                mcp_path("omp").display(),
                serde_json::to_string_pretty(&entry).unwrap()
            )
        }
    }
}

pub fn apply_mcp(harness: &str, repair: bool) -> std::result::Result<String, String> {
    if harness == "pi" {
        return apply("pi", None).map(|m| format!("{} (the extension exposes Everett tools through Pi)", m));
    }
    if harness == "hermes" {
        return Err("Install the native plugin with `hermes plugins install raitoxlol/everett#plugins/everett-hermes --no-enable && hermes plugins enable everett-hermes` (or use `everett install-mcp --hermes` for the manual YAML snippet).".into());
    }
    let path = mcp_path(harness);
    let data = mcp_config(harness)?;
    let old = mcp_entry(harness, &data)?;
    let legacy = old.as_ref().and_then(|o| o.as_object()).map(is_legacy_mcp).unwrap_or(false);
    if old.is_some() && !repair && !legacy {
        let (state, detail) = mcp_status(harness);
        if state != "ready" {
            return Err(format!("{}; run `everett install-mcp --{} --repair --apply`", detail, harness));
        }
        return Ok(format!("{}: everett MCP server already registered ({})", harness, path.display()));
    }
    if let Some(o) = &old {
        if !o.is_object() {
            return Err("Everett MCP entry must be an object; correct it before retrying".to_string());
        }
    }
    let mut entry = repaired_entry(&old.as_ref().and_then(|o| o.as_object()).cloned().unwrap_or_default(), harness)
        .map_err(|e| e.message)?;
    if harness == "devin" {
        entry.remove("type");
        entry.remove("transport");
    }
    if TOML_MCP.contains(&harness) {
        // codex/grok config.toml entries have no "type" field; codex_block()
        // omits it, so a repair must not introduce one either.
        entry.remove("type");
    }
    let saved;
    if TOML_MCP.contains(&harness) {
        let text = fs::read_to_string(&path).unwrap_or_default();
        let updated = if old.is_some() {
            replace_toml_entry(&text, &entry)?
        } else {
            format!(
                "{}{}{}{}",
                text,
                if !text.is_empty() && !text.ends_with('\n') { "\n" } else { "" },
                if !text.is_empty() { "\n" } else { "" },
                codex_block()
            )
        };
        if updated == text {
            return Ok(format!("{}: everett MCP server already registered ({})", harness, path.display()));
        }
        saved = backup(&path);
        write_preserving_mode(&path, &updated).map_err(|e| e.to_string())?;
    } else {
        let mut data = data.clone();
        match data.get("mcpServers") {
            None => {}
            Some(v) if !v.is_object() => {
                return Err(format!("\"mcpServers\" in {} is not an object", path.display()))
            }
            _ => {}
        }
        let servers = data.entry("mcpServers".to_string()).or_insert(Value::Object(Map::new()));
        let servers = servers.as_object_mut().unwrap();
        if old.as_ref().map(|o| o == &Value::Object(entry.clone())).unwrap_or(false) {
            return Ok(format!("{}: everett MCP server already registered ({})", harness, path.display()));
        }
        servers.insert("everett".into(), Value::Object(entry));
        saved = backup(&path);
        write_json(&path, &data).map_err(|e| e.to_string())?;
    }
    Ok(format!(
        "{}: registered everett MCP server in {}{}",
        harness,
        path.display(),
        saved.map(|s| format!(" (backup {})", s.display())).unwrap_or_default()
    ))
}

#[derive(Debug, Clone, PartialEq)]
enum YamlVal {
    Map(Vec<(String, YamlVal)>),
    Seq(Vec<YamlVal>),
    Scalar(String),
}

impl YamlVal {
    fn get(&self, key: &str) -> Option<&YamlVal> {
        match self {
            YamlVal::Map(entries) => entries.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_scalar(&self) -> Option<&str> {
        match self {
            YamlVal::Scalar(s) => Some(s.as_str()),
            _ => None,
        }
    }

    fn seq_contains(&self, target: &str) -> bool {
        match self {
            YamlVal::Seq(items) => items.iter().any(|item| item.as_scalar() == Some(target)),
            _ => false,
        }
    }
}

fn yaml_strip_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single && !in_double && (i == 0 || bytes[i - 1].is_ascii_whitespace()) => {
                return &line[..i];
            }
            _ => {}
        }
    }
    line
}

fn yaml_unquote(token: &str) -> String {
    let t = token.trim();
    if t.len() >= 2 && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\''))) {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

fn yaml_split_kv(text: &str) -> Option<(&str, &str)> {
    let mut in_single = false;
    let mut in_double = false;
    let mut depth = 0usize;
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'{' | b'[' if !in_single && !in_double => depth += 1,
            b'}' | b']' if !in_single && !in_double => depth = depth.saturating_sub(1),
            b':' if !in_single && !in_double && depth == 0 => {
                let key_part = text[..i].trim();
                let quoted_key = key_part.len() >= 2
                    && ((key_part.starts_with('"') && key_part.ends_with('"'))
                        || (key_part.starts_with('\'') && key_part.ends_with('\'')));
                if quoted_key || i + 1 == bytes.len() || bytes[i + 1].is_ascii_whitespace() {
                    return Some((&text[..i], text[i + 1..].trim()));
                }
            }
            _ => {}
        }
    }
    None
}

fn yaml_split_flow_items(inner: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, b) in inner.bytes().enumerate() {
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'{' | b'[' if !in_single && !in_double => depth += 1,
            b'}' | b']' if !in_single && !in_double => depth = depth.saturating_sub(1),
            b',' if !in_single && !in_double && depth == 0 => {
                let piece = inner[start..i].trim();
                if !piece.is_empty() {
                    out.push(piece);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = inner[start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

fn yaml_parse_inline(raw: &str) -> YamlVal {
    let s = raw.trim();
    if let Some(inner) = s.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
        let mut entries = Vec::new();
        for item in yaml_split_flow_items(inner) {
            if let Some((k, v)) = yaml_split_kv(item) {
                entries.push((yaml_unquote(k), yaml_parse_inline(v)));
            }
        }
        return YamlVal::Map(entries);
    }
    if let Some(inner) = s.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        return YamlVal::Seq(yaml_split_flow_items(inner).into_iter().map(yaml_parse_inline).collect());
    }
    YamlVal::Scalar(yaml_unquote(s))
}

fn yaml_parse_block(lines: &[(usize, &str)], idx: &mut usize, min_indent: usize) -> YamlVal {
    if *idx >= lines.len() || lines[*idx].0 < min_indent {
        return YamlVal::Scalar(String::new());
    }
    let base_indent = lines[*idx].0;
    if lines[*idx].1.starts_with('-') && (lines[*idx].1.len() == 1 || lines[*idx].1.as_bytes()[1].is_ascii_whitespace()) {
        let mut items = Vec::new();
        while *idx < lines.len() && lines[*idx].0 == base_indent {
            let body = lines[*idx].1;
            let Some(rest) = body.strip_prefix('-') else { break };
            if !rest.is_empty() && !rest.as_bytes()[0].is_ascii_whitespace() {
                break;
            }
            let rest = rest.trim();
            *idx += 1;
            if rest.is_empty() {
                items.push(yaml_parse_block(lines, idx, base_indent + 1));
            } else {
                items.push(yaml_parse_inline(rest));
                while *idx < lines.len() && lines[*idx].0 > base_indent {
                    *idx += 1;
                }
            }
        }
        return YamlVal::Seq(items);
    }

    let mut entries = Vec::new();
    while *idx < lines.len() {
        let (indent, body) = lines[*idx];
        if indent < base_indent {
            break;
        }
        if indent > base_indent {
            *idx += 1;
            continue;
        }
        let Some((k, v)) = yaml_split_kv(body) else {
            *idx += 1;
            continue;
        };
        let key = yaml_unquote(k);
        *idx += 1;
        let val = if !v.is_empty() {
            yaml_parse_inline(v)
        } else if *idx < lines.len()
            && (lines[*idx].0 > base_indent
                || (lines[*idx].0 == base_indent
                    && lines[*idx].1.starts_with('-')
                    && (lines[*idx].1.len() == 1 || lines[*idx].1.as_bytes()[1].is_ascii_whitespace())))
        {
            yaml_parse_block(lines, idx, base_indent)
        } else {
            YamlVal::Scalar(String::new())
        };
        entries.push((key, val));
    }
    YamlVal::Map(entries)
}

fn yaml_flow_depth_delta(text: &str) -> isize {
    let mut in_single = false;
    let mut in_double = false;
    let mut delta = 0isize;
    for b in text.bytes() {
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'{' | b'[' if !in_single && !in_double => delta += 1,
            b'}' | b']' if !in_single && !in_double => delta -= 1,
            _ => {}
        }
    }
    delta
}

fn yaml_is_block_scalar_header(val: &str) -> bool {
    let mut chars = val.chars();
    let Some(first) = chars.next() else { return false };
    if first != '|' && first != '>' {
        return false;
    }
    chars.all(|c| c == '-' || c == '+' || c.is_ascii_digit())
}

fn parse_hermes_yaml(yaml_text: &str) -> YamlVal {
    let cleaned = yaml_text.strip_prefix('\u{feff}').unwrap_or(yaml_text);
    // Join multiline flow collections while skipping block scalars (`|`/`>`) and wrapped plain scalars.
    let mut logical: Vec<(usize, String)> = Vec::new();
    let mut flow_depth = 0isize;
    let mut skip_deeper_than: Option<usize> = None;
    for raw in cleaned.lines() {
        let raw_indent = raw.len() - raw.trim_start_matches(' ').len();
        if let Some(parent_indent) = skip_deeper_than {
            if raw.trim().is_empty() || raw_indent > parent_indent {
                continue;
            }
            skip_deeper_than = None;
        }
        let stripped = yaml_strip_comment(raw).trim_end();
        let trimmed = stripped.trim();
        if trimmed.is_empty() || trimmed == "---" || trimmed == "..." {
            continue;
        }
        let indent = stripped.len() - stripped.trim_start_matches(' ').len();
        if flow_depth > 0 {
            if let Some((_, prev)) = logical.last_mut() {
                prev.push(' ');
                prev.push_str(trimmed);
            }
            flow_depth = (flow_depth + yaml_flow_depth_delta(trimmed)).max(0);
            continue;
        }
        logical.push((indent, trimmed.to_string()));
        if let Some(rest) = trimmed
            .strip_prefix('-')
            .filter(|r| r.is_empty() || r.as_bytes()[0].is_ascii_whitespace())
        {
            let item = rest.trim();
            if item.starts_with('{') || item.starts_with('[') {
                flow_depth = yaml_flow_depth_delta(item).max(0);
            } else if let Some((_, val)) = yaml_split_kv(item) {
                if val.starts_with('{') || val.starts_with('[') {
                    flow_depth = yaml_flow_depth_delta(val).max(0);
                } else if yaml_is_block_scalar_header(val) {
                    skip_deeper_than = Some(indent + 2);
                }
            } else if yaml_is_block_scalar_header(item) || !item.is_empty() {
                skip_deeper_than = Some(indent);
            }
        } else if let Some((_, val)) = yaml_split_kv(trimmed) {
            if val.starts_with('{') || val.starts_with('[') {
                flow_depth = yaml_flow_depth_delta(val).max(0);
            } else if yaml_is_block_scalar_header(val) || !val.is_empty() {
                // Any deeper lines belong to this block/wrapped scalar, never a new YAML mapping or flow collection.
                skip_deeper_than = Some(indent);
            }
        } else if indent == 0 && (trimmed.starts_with('{') || trimmed.starts_with('[')) {
            flow_depth = yaml_flow_depth_delta(trimmed).max(0);
        }
    }
    let lines: Vec<(usize, &str)> = logical.iter().map(|(i, s)| (*i, s.as_str())).collect();
    let mut idx = 0;
    yaml_parse_block(&lines, &mut idx, 0)
}

fn yaml_is_explicit_false(val: &YamlVal) -> bool {
    matches!(
        val.as_scalar().map(|s| s.trim().to_ascii_lowercase()).as_deref(),
        Some("false" | "no" | "off" | "0")
    )
}

fn hermes_yaml_flags(yaml_text: &str) -> (bool, bool, bool) {
    let root = parse_hermes_yaml(yaml_text);
    let plugins = root.get("plugins");
    let enabled = plugins
        .and_then(|p| p.get("enabled"))
        .is_some_and(|list| list.seq_contains("everett-hermes"));
    let disabled = plugins
        .and_then(|p| p.get("disabled"))
        .is_some_and(|list| list.seq_contains("everett-hermes"));
    let manual_mcp = root
        .get("mcp_servers")
        .and_then(|servers| servers.get("everett"))
        .is_some_and(|entry| !entry.get("enabled").is_some_and(yaml_is_explicit_false));
    (enabled, disabled, manual_mcp)
}

fn expand_tilde_against(raw: &str, user_home: &std::path::Path) -> PathBuf {
    if raw == "~" {
        return user_home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        return user_home.join(rest);
    }
    PathBuf::from(raw)
}

pub fn hermes_active_home() -> PathBuf {
    // Hermes resolves `~` and default `~/.hermes` against OS `$HOME`, not `EVERETT_HOME`.
    let user_home = std::env::var("HOME")
        .ok()
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(home);
    let root = match std::env::var("HERMES_HOME") {
        Ok(dir) if !dir.is_empty() => expand_tilde_against(&dir, &user_home),
        _ => user_home.join(".hermes"),
    };
    if let Ok(raw) = fs::read_to_string(root.join("active_profile")) {
        let name = raw.strip_prefix('\u{feff}').unwrap_or(&raw).trim();
        if !name.is_empty()
            && name != "default"
            && name != "."
            && name != ".."
            && !name.contains('/')
            && !name.contains('\\')
        {
            let named = root.join("profiles").join(name);
            if named.is_dir() {
                return named;
            }
        }
    }
    root
}

/// Inspect Hermes's active profile for the native `everett-hermes` plugin and any duplicate manual MCP entry.
pub fn hermes_plugin_status() -> (Option<bool>, String, Option<&'static str>) {
    let hermes_home = hermes_active_home();
    let plugin_dir = hermes_home.join("plugins").join("everett-hermes");
    let has_native = plugin_dir.join("plugin.yaml").is_file() || plugin_dir.join("plugin.yml").is_file();
    let has_portable = plugin_dir.join("plugin.json").is_file();
    let cfg_text = fs::read_to_string(hermes_home.join("config.yaml")).unwrap_or_default();
    let (enabled, disabled, manual_mcp) = hermes_yaml_flags(&cfg_text);

    if has_native {
        if enabled && !disabled {
            if manual_mcp {
                (
                    Some(false),
                    "native plugin enabled (everett-hermes), but manual mcp_servers.everett in config.yaml causes duplicate tools; remove mcp_servers.everett".into(),
                    None,
                )
            } else {
                (Some(true), "native plugin installed and enabled (everett-hermes)".into(), None)
            }
        } else {
            (
                Some(false),
                "native plugin installed but not enabled; run `hermes plugins enable everett-hermes`".into(),
                Some("hermes plugins enable everett-hermes"),
            )
        }
    } else if has_portable {
        (
            Some(false),
            "older portable everett-hermes package installed; upgrade with `hermes plugins update everett-hermes`".into(),
            Some("hermes plugins update everett-hermes"),
        )
    } else if manual_mcp {
        (
            None,
            "manual mcp_servers.everett configured (single CLI only); install native plugin with `hermes plugins install raitoxlol/everett#plugins/everett-hermes --no-enable && hermes plugins enable everett-hermes`".into(),
            None,
        )
    } else {
        (
            Some(false),
            "everett-hermes plugin not installed; run `hermes plugins install raitoxlol/everett#plugins/everett-hermes --no-enable && hermes plugins enable everett-hermes`".into(),
            Some("hermes plugins install raitoxlol/everett#plugins/everett-hermes --no-enable && hermes plugins enable everett-hermes"),
        )
    }
}

#[cfg(test)]
mod hermes_yaml_tests {
    use super::*;

    #[test]
    fn parses_flow_mappings_ignores_nested_enabled_and_honors_disabled_manual_mcp() {
        assert_eq!(hermes_yaml_flags("plugins: {enabled: [everett-hermes]}\n"), (true, false, false));
        assert_eq!(hermes_yaml_flags("plugins: {\"enabled\":[everett-hermes]}\n"), (true, false, false));
        assert_eq!(
            hermes_yaml_flags("plugins:\n  enabled: [\n    everett-hermes\n  ]\n"),
            (true, false, false)
        );
        assert_eq!(
            hermes_yaml_flags(
                "plugins:\n  entries:\n    other:\n      enabled:\n        - everett-hermes\n"
            ),
            (false, false, false)
        );
        assert_eq!(
            hermes_yaml_flags(
                "plugins:\n  enabled:\n    - everett-hermes\nmcp_servers:\n  everett:\n    enabled: false\n    command: everett\n"
            ),
            (true, false, false)
        );
        assert_eq!(
            hermes_yaml_flags(
                "plugins:\n  enabled: [everett-hermes]\nmcp_servers:\n  everett:\n    enabled: 0\n    command: everett\n"
            ),
            (true, false, false)
        );
        assert_eq!(
            hermes_yaml_flags(
                "note: literal [bracket before plugins\nplugins:\n  enabled:\n    - everett-hermes\n"
            ),
            (true, false, false)
        );
        assert_eq!(
            hermes_yaml_flags(
                "prompt: |\n  [block scalar starting with bracket\nplugins:\n  enabled:\n    - everett-hermes\n"
            ),
            (true, false, false)
        );
        assert_eq!(
            hermes_yaml_flags(
                "wrapped: first line\n  [continuation starting with bracket\nplugins:\n  enabled:\n    - everett-hermes\n"
            ),
            (true, false, false)
        );
        assert_eq!(
            hermes_yaml_flags(
                "examples:\n  - |\n    [choice]: [literal\nplugins:\n  enabled: [everett-hermes]\n"
            ),
            (true, false, false)
        );
        assert_eq!(
            hermes_yaml_flags(
                "plugins:\n  enabled: [everett-hermes]\nmcp_servers:\n  everett: {command: everett, enabled: true}\n"
            ),
            (true, false, true)
        );
    }

    #[test]
    fn resolves_saved_active_profile_under_user_home_even_when_everett_home_differs() {
        if !crate::test_support::isolated("install::hermes_yaml_tests::resolves_saved_active_profile_under_user_home_even_when_everett_home_differs") {
            return;
        }
        let user_tmp = tempfile::tempdir().unwrap();
        let everett_tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", user_tmp.path());
        std::env::set_var("EVERETT_HOME", everett_tmp.path());
        std::env::remove_var("HERMES_HOME");
        let work = user_tmp.path().join(".hermes/profiles/work");
        fs::create_dir_all(work.join("plugins/everett-hermes")).unwrap();
        fs::write(user_tmp.path().join(".hermes/active_profile"), "work\n").unwrap();
        fs::write(work.join("plugins/everett-hermes/plugin.yaml"), "name: everett-hermes\n").unwrap();
        fs::write(work.join("config.yaml"), "plugins: {enabled: [everett-hermes]}\n").unwrap();

        let (ok, detail, _) = hermes_plugin_status();
        assert_eq!(ok, Some(true), "expected active_profile work under $HOME to report ok: {detail}");
        assert!(detail.contains("native plugin"), "{detail}");

        // Explicit tilde-prefixed HERMES_HOME also expands against OS $HOME and follows active_profile.
        std::env::set_var("HERMES_HOME", "~/.hermes");
        let (ok2, detail2, _) = hermes_plugin_status();
        assert_eq!(ok2, Some(true), "expected ~/.hermes active_profile to report ok: {detail2}");
    }
}
