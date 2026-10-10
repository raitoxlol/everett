//! `everett install-hooks` / `everett install-mcp`: print or merge hook + MCP registrations.
//! `--apply` backs up each file it changes (<file>.everett-bak-<timestamp>), merges
//! idempotently, never duplicates an Everett entry and never removes anyone else's hooks.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::error::{EverettError, Result};
use crate::proc::shlex_quote;
use crate::session::home;
use crate::timefmt::strftime_local;

// harness -> [(event, hook subcommand, timeout)]
pub const HOOKS: &[(&str, &[(&str, &str, i64)])] = &[
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
];

fn hooks_for(harness: &str) -> &'static [(&'static str, &'static str, i64)] {
    HOOKS
        .iter()
        .find(|(h, _)| *h == harness)
        .map(|(_, v)| *v)
        .unwrap_or(&[])
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
        _ => ".grok/hooks/everett.json",
    })
}

pub fn omp_extension_path() -> PathBuf {
    home().join(".omp").join("agent").join("extensions").join("everett.ts")
}

/// The OMP extension body, adapted to shell out to this binary instead of python3 scripts.
pub fn omp_extension_source() -> String {
    include_str!("omp_session_start.mjs").to_string()
}

fn is_native_hook(command: &str, script: &str) -> bool {
    command.contains("everett") && command.trim_end().ends_with(&format!(" hook {}", script))
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

fn has_script(entries: &Value, script: &str) -> bool {
    let Some(entries) = entries.as_array() else { return false };
    entries
        .iter()
        .filter_map(|entry| entry.get("hooks").and_then(|h| h.as_array()))
        .flatten()
        .filter_map(|hook| hook.get("command").and_then(|c| c.as_str()))
        .any(|command| is_native_hook(command, script))
}

/// Drop legacy Python hooks (and entries left empty); returns whether anything changed.
fn remove_legacy_hooks(entries: &mut Vec<Value>) -> bool {
    let before = entries.clone();
    for entry in entries.iter_mut() {
        if let Some(hooks) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            hooks.retain(|hook| !hook.get("command").and_then(|c| c.as_str()).map(is_legacy_hook).unwrap_or(false));
        }
    }
    entries.retain(|entry| entry.get("hooks").and_then(|h| h.as_array()).map(|h| !h.is_empty()).unwrap_or(true));
    *entries != before
}

/// Which of Everett's hook events are registered for a harness.
pub fn installed(harness: &str, data: Option<&Map<String, Value>>) -> std::result::Result<HashMap<String, bool>, String> {
    use std::collections::HashMap;
    if harness == "omp" {
        let current = fs::read_to_string(omp_extension_path()).map(|text| !text.contains("python3")).unwrap_or(false);
        return Ok(HashMap::from([("extension".to_string(), current)]));
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
        out.insert(event.to_string(), has_script(&entries, script));
    }
    Ok(out)
}

/// Return (merged settings, events added, events whose legacy Python hooks were replaced).
/// Pure: does not touch the input.
pub fn merge(
    data: &Map<String, Value>,
    harness: &str,
    events: Option<&[String]>,
) -> std::result::Result<(Map<String, Value>, Vec<String>, Vec<String>), String> {
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
        let entries = hooks.entry(event.to_string()).or_insert(Value::Array(Vec::new()));
        let entries = entries.as_array_mut().unwrap();
        if remove_legacy_hooks(entries) && !migrated.iter().any(|e| e == event) {
            migrated.push(event.to_string());
        }
        if has_script(&Value::Array(entries.clone()), script) {
            continue;
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

fn write_json(path: &PathBuf, data: &Map<String, Value>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!("{}.everett-tmp", path.file_name().unwrap_or_default().to_string_lossy()));
    fs::write(&tmp, serde_json::to_string_pretty(data)? + "\n")?;
    fs::rename(&tmp, path)
}

pub fn snippet(harness: &str) -> String {
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
        _ => "# write ~/.grok/hooks/everett.json (Grok loads every file in ~/.grok/hooks/)".to_string(),
    };
    let mut body = Map::new();
    body.insert("hooks".into(), Value::Object(hooks));
    format!("{}\n{}\n", note, serde_json::to_string_pretty(&body).unwrap())
}

/// Install for one harness; returns a one-line report.
pub fn apply(harness: &str, events: Option<&[String]>) -> std::result::Result<String, String> {
    if harness == "omp" {
        let path = omp_extension_path();
        let source = omp_extension_source();
        if path.exists() && fs::read_to_string(&path).unwrap_or_default() == source {
            return Ok(format!("omp: already installed ({})", path.display()));
        }
        let saved = backup(&path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&path, &source).map_err(|e| e.to_string())?;
        return Ok(format!(
            "omp: wrote {}{}",
            path.display(),
            saved.map(|s| format!(" (backup {})", s.display())).unwrap_or_default()
        ));
    }
    if events.map(|e| e.is_empty()).unwrap_or(false) {
        return Ok(format!("{}: skipped (no hooks selected)", harness));
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

// ---- MCP registration (`everett install-mcp`) ----

/// (command, args, env) that starts `everett mcp` from this very install.
pub fn mcp_launch() -> (String, Vec<String>, std::collections::HashMap<String, String>) {
    (exe(), vec!["mcp".to_string()], std::collections::HashMap::new())
}

fn mcp_json_entry() -> Map<String, Value> {
    let (command, args, env) = mcp_launch();
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
    let args: Vec<String> = entry
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    if command.is_empty() {
        return ("stale".into(), "registration has no executable".to_string());
    }
    let is_exec = PathBuf::from(command).is_file();
    let path_env = std::env::var("PATH").unwrap_or_default();
    if !is_exec && crate::proc::which(command, &path_env).is_none() {
        return ("stale".into(), "registered executable is missing or not on PATH".to_string());
    }
    if is_legacy_mcp(entry) {
        return ("stale".into(), "registration launches the retired Python package".to_string());
    }
    let name = PathBuf::from(command)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !(name == "everett" && args == ["mcp"].map(String::from)) {
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

fn repaired_entry(old: &Map<String, Value>) -> Result<Map<String, Value>> {
    let mut entry = old.clone();
    for (k, v) in mcp_json_entry() {
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
                .map(|m| m[1].replace('"', "").replace('\'', "").replace(' ', ""))
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
        "devin" => {
            let mut entry = mcp_json_entry();
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
            servers.insert("everett".into(), Value::Object(mcp_json_entry()));
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
            servers.insert("everett".into(), Value::Object(mcp_json_entry()));
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
    let mut entry = repaired_entry(&old.as_ref().and_then(|o| o.as_object()).cloned().unwrap_or_default())
        .map_err(|e| e.message)?;
    if harness == "devin" {
        entry.remove("type");
        entry.remove("transport");
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
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_file_name(format!("{}.everett-tmp", path.file_name().unwrap_or_default().to_string_lossy()));
        fs::write(&tmp, &updated).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
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
