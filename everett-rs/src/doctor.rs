//! `everett doctor`: one screen of what Everett can see and what is missing.

use serde_json::{json, Value};

use crate::adapters::devin as devin_adapter;
use crate::session::home;

pub fn stores() -> &'static [(&'static str, &'static str)] {
    // t3code's database annotates sessions of the other harnesses; devin's root moves with DEVIN_HOME.
    &[
        ("claude", ".claude/projects"),
        ("codex", ".codex/sessions"),
        ("omp", ".omp/agent/sessions"),
        ("pi", ".pi/agent/sessions"),
        ("hermes", ".hermes"),
        ("grok", ".grok/sessions"),
        ("devin", devin_adapter::DEFAULT_REL),
        ("t3code", ".t3/userdata/state.sqlite"),
    ]
}

fn which(name: &str) -> Option<String> {
    // proc::which requires an executable file, not just a name that exists.
    crate::proc::which(name, &std::env::var("PATH").unwrap_or_default())
}

pub fn detected_harnesses() -> Vec<String> {
    stores()
        .iter()
        .filter(|(h, _)| *h != "t3code")
        .filter(|(h, rel)| {
            which(h).is_some()
                || home().join(rel).exists()
                || (*h == "devin" && devin_adapter::data_dirs().iter().any(|d| d.is_dir()))
                || (*h == "devin" && crate::install::mcp_path(h).exists())
        })
        .map(|(h, _)| h.to_string())
        .collect()
}

/// Spawn the server once and check initialize + tools/list.
fn probe_mcp(launch: Option<(String, Vec<String>, std::collections::HashMap<String, String>)>) -> (bool, String) {
    let (command, args, env) = launch.unwrap_or_else(crate::install::mcp_launch);
    let requests = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "everett-doctor", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    ];
    let input: String = requests
        .iter()
        .map(|r| serde_json::to_string(r).unwrap_or_default() + "\n")
        .collect();
    let mut cmd = vec![command.clone()];
    cmd.extend(args.clone());
    let mut child_env: std::collections::HashMap<String, String> = std::env::vars().collect();
    child_env.extend(env);
    let result = match crate::proc::run_capture_stdin(&cmd, None, &child_env, 5.0, Some(&input)) {
        Ok(r) => r,
        Err(e) => {
            return (false, format!(
                "MCP stdio probe failed: {}; reinstall Everett and run `everett doctor`",
                e
            ))
        }
    };
    if result.timed_out {
        return (false, "MCP stdio probe failed: TimeoutExpired; reinstall Everett and run `everett doctor`".to_string());
    }
    let mut replies: std::collections::HashMap<i64, Value> = std::collections::HashMap::new();
    for line in result.stdout.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(id) = v.get("id").and_then(|i| i.as_i64()) {
                replies.insert(id, v);
            }
        }
    }
    let tools = replies
        .get(&2)
        .and_then(|r| r.get("result"))
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    let names: std::collections::HashSet<&str> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
        .collect();
    let expected: std::collections::HashSet<&str> =
        crate::mcp::tools().iter().filter_map(|t| t["name"].as_str()).collect();
    if result.code != 0 || names != expected || tools.len() != crate::mcp::tools().len() {
        return (false, format!(
            "MCP probe failed: expected {} tools, received {}; run `everett mcp`",
            crate::mcp::tools().len(),
            tools.len()
        ));
    }
    let has_tools_cap = replies
        .get(&1)
        .and_then(|r| r.get("result"))
        .and_then(|r| r.get("capabilities"))
        .and_then(|c| c.get("tools"))
        .is_some();
    if !has_tools_cap {
        return (false, "MCP initialize did not advertise tools".to_string());
    }
    let version = replies
        .get(&1)
        .and_then(|r| r.get("result"))
        .and_then(|r| r.get("serverInfo"))
        .and_then(|s| s.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if version != env!("CARGO_PKG_VERSION") {
        return (false, "registered server runs an older Everett; refresh it with `everett install-mcp --repair --apply`".to_string());
    }
    (true, format!("{} tools over stdio (initialize + tools/list passed)", tools.len()))
}

fn line(warnings: &mut Vec<String>, ok: Option<bool>, text: &str) {
    if ok == Some(false) {
        warnings.push(text.to_string());
    }
    let mark = match ok {
        Some(true) => "ok  ",
        Some(false) => "warn",
        None => "--  ",
    };
    println!("[{}] {}", mark, text);
}

pub fn run(hours: f64) -> i32 {
    if !hours.is_finite() || hours < 0.0 {
        eprintln!("everett doctor: --hours must be finite and nonnegative.");
        return 2;
    }
    let mut problems = 0;
    let mut next_steps: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    line(&mut warnings, Some(true), &format!("everett {} (single binary; no interpreter needed)", env!("CARGO_PKG_VERSION")));

    match crate::config::values() {
        Ok(_) => {
            let path = crate::config::config_path();
            let exists = path.exists();
            line(
                &mut warnings,
                if exists { Some(true) } else { None },
                &format!("config {}{}", path.display(), if exists { "" } else { " (none; defaults in use)" }),
            );
        }
        Err(e) => {
            problems += 1;
            line(&mut warnings, Some(false), &e);
        }
    }

    let seen = detected_harnesses();
    println!("harnesses:");
    for (harness, rel) in stores() {
        let path = if *harness == "devin" {
            devin_adapter::data_dir()
        } else {
            home().join(rel)
        };
        let found = path.exists();
        let state = if found { "found" } else { "not found" };
        let extra = if *harness == "t3code" {
            if found {
                format!(
                    "{} thread(s) with a harness session id",
                    crate::adapters::t3code::threads(&crate::adapters::t3code::db_path()).len()
                )
            } else {
                "desktop app".to_string()
            }
        } else {
            format!("cli: {}", which(harness).unwrap_or_else(|| "not on PATH".to_string()))
        };
        let cli = if *harness != "t3code" { which(harness) } else { None };
        line(
            &mut warnings,
            if cli.is_some() || found { Some(true) } else { None },
            &format!("  {:<7} {} ({}); {}", harness, path.display(), state, extra),
        );
        if *harness != "t3code" && found && cli.is_none() {
            line(&mut warnings, Some(false), &format!(
                "  {}: install its CLI and add it to PATH before headless resume", harness
            ));
        }
    }
    if seen.is_empty() {
        line(&mut warnings, Some(false), "No harness CLI or session store found. Install a supported harness first.");
        next_steps.push("Install a harness, then run `everett onboard --yes`.".to_string());
    }

    println!("hooks:");
    for harness in ["claude", "codex", "omp", "grok"] {
        if !seen.iter().any(|h| h == harness) {
            continue;
        }
        let events = match crate::install::installed(harness, None) {
            Ok(e) => e,
            Err(e) => {
                problems += 1;
                line(&mut warnings, Some(false), &format!("  {:<7} cannot read config: {}", harness, e));
                continue;
            }
        };
        let stale: Vec<&String> = events.iter().filter(|(_, s)| s.as_str() == "stale").map(|(k, _)| k).collect();
        let missing: Vec<&String> = events.iter().filter(|(_, s)| s.as_str() == "missing").map(|(k, _)| k).collect();
        if !stale.is_empty() {
            problems += 1;
            let names: Vec<String> = stale.iter().map(|m| m.to_string()).collect();
            line(&mut warnings, Some(false), &format!(
                "  {:<7} stale {}; run `everett install-hooks --{} --apply`",
                harness,
                names.join(", "),
                harness
            ));
        } else if !missing.is_empty() {
            let names: Vec<String> = missing.iter().map(|m| m.to_string()).collect();
            line(&mut warnings, Some(false), &format!(
                "  {:<7} missing {}; run `everett install-hooks --{} --apply`",
                harness,
                names.join(", "),
                harness
            ));
        } else {
            line(&mut warnings, Some(true), &format!("  {:<7} installed", harness));
        }
    }

    println!("mcp:");
    let (ok, detail) = probe_mcp(None);
    if !ok {
        problems += 1;
    }
    line(&mut warnings, Some(ok), &format!("  server: {}", detail));
    let mcp_harnesses: Vec<&str> = ["claude", "codex", "omp", "grok", "devin"]
        .iter()
        .filter(|h| seen.iter().any(|s| s == *h) || crate::install::mcp_path(h).exists())
        .cloned()
        .collect();
    let mut missing_mcp: Vec<&str> = Vec::new();
    let mut stale_mcp: Vec<&str> = Vec::new();
    for harness in &mcp_harnesses {
        let (state, detail) = crate::install::mcp_status(harness);
        let mut ready = state == "ready";
        let mut detail = detail;
        if ready {
            let data = crate::install::mcp_config(harness).unwrap_or_default();
            let entry = crate::install::mcp_entry(harness, &data).ok().flatten();
            let entry = entry.and_then(|e| e.as_object().cloned()).unwrap_or_default();
            let (probe_ok, probe_detail) = probe_mcp(Some((
                entry.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                entry
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default(),
                entry
                    .get("env")
                    .and_then(|v| v.as_object())
                    .map(|e| {
                        e.iter()
                            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
            )));
            ready = probe_ok;
            detail = probe_detail;
        }
        line(&mut warnings, Some(ready), &format!("  {:<7} {}", harness, detail));
        if state == "missing" {
            missing_mcp.push(harness);
        } else if !ready {
            problems += 1;
            stale_mcp.push(harness);
        }
    }
    if !missing_mcp.is_empty() {
        next_steps.push(format!(
            "everett install-mcp {} --apply",
            missing_mcp.iter().map(|h| format!("--{}", h)).collect::<Vec<_>>().join(" ")
        ));
    }
    if !stale_mcp.is_empty() {
        next_steps.push(format!(
            "everett install-mcp {} --repair --apply",
            stale_mcp.iter().map(|h| format!("--{}", h)).collect::<Vec<_>>().join(" ")
        ));
    }
    if !missing_mcp.is_empty() || !stale_mcp.is_empty() {
        next_steps.push("Restart your MCP client after registering or repairing Everett, then run `everett doctor`.".to_string());
    }

    let sessions = crate::registry::scan(hours, false, None, "");
    let (_, totals) = crate::cli::card_coverage(&sessions);
    line(
        &mut warnings,
        if totals[0] > 0 { Some(true) } else { None },
        &format!(
            "cards (last {} h, interactive): {} sessions, {} agent, {} auto, {} missing",
            crate::fmt::g(hours),
            totals[0],
            totals[1],
            totals[2],
            totals[3]
        ),
    );

    if !crate::route::find_api_key().is_empty() {
        line(&mut warnings, Some(true), "router: jev (key found; --router local also available)");
    } else {
        line(&mut warnings, Some(true), "router: local BM25 (no Jev key; routing stays on this machine)");
    }

    let folder = crate::trunk::vault_folder();
    line(
        &mut warnings,
        if folder.is_none() { None } else { Some(true) },
        &format!(
            "vault: {}",
            folder
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "not configured (optional)".to_string())
        ),
    );

    if cfg!(target_os = "macos") {
        let scheduled = crate::trunk_schedule::is_scheduled(None);
        let stale = scheduled && crate::trunk_schedule::is_stale(None);
        if stale {
            problems += 1;
        }
        line(
            &mut warnings,
            if stale { Some(false) } else if scheduled { Some(true) } else { None },
            &format!(
                "trunk schedule: {}",
                if stale {
                    format!("stale ({}); run `everett trunk schedule --apply`", crate::trunk_schedule::plist_path().display())
                } else if scheduled {
                    format!("scheduled ({})", crate::trunk_schedule::plist_path().display())
                } else {
                    "not scheduled (optional, macOS; `everett trunk schedule --apply`)".to_string()
                }
            ),
        );
    } else {
        line(
            &mut warnings,
            None,
            "trunk schedule: automatic scheduling requires macOS; run `everett trunk merge --llm none` manually",
        );
    }
    let account = crate::auth::load();
    line(
        &mut warnings,
        None,
        &account
            .map(|s| format!("account: {} via {} ({})", s.email, s.issuer, s.device_name))
            .unwrap_or_else(|| "account: not signed in (optional)".to_string()),
    );
    let warning_count = warnings.len() - problems;
    if warnings.is_empty() {
        println!("ready");
    } else {
        println!("{} problem(s), {} warning(s); setup needs attention", problems, warning_count);
    }
    println!("Next:");
    let steps = if next_steps.is_empty() {
        vec!["everett ls".to_string(), "Use `everett_whoami` in your MCP client to check caller identity.".to_string()]
    } else {
        next_steps
    };
    for step in steps {
        println!("  {}", step);
    }
    if problems > 0 {
        1
    } else {
        0
    }
}
