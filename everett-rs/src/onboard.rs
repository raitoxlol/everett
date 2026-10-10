//! `everett onboard`: friendly first-time setup. On a TTY the ratatui wizard in `tui.rs`
//! runs the same seven steps as the Python curses TUI; `--yes` is the non-interactive path
//! and the plain sequential prompts are the fallback when no usable terminal exists.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use crate::session::home;

/// "since forever": detection counts every session Everett can see, not just a recent window.
pub const ALL_HOURS: f64 = 24.0 * 365.0 * 5.0;

/// Harnesses whose hooks live in a settings file Everett can toggle event-by-event.
pub const HOOK_HARNESSES: &[&str] = &["claude", "codex", "grok"];
pub const MCP_HARNESSES: &[&str] = &["claude", "codex", "omp", "grok", "devin"];
/// `hooks_common::auto_card` only knows how to build a fallback card for these harnesses.
pub const BACKFILL_HARNESSES: &[&str] = &["claude", "codex", "grok"];

pub fn event_label(event: &str) -> &str {
    match event {
        "SessionStart" => "session cards + shared core (SessionStart)",
        "Stop" => "automatic fallback cards + done/blocked/needs-input events (Stop)",
        "UserPromptSubmit" => "live delivery of Everett messages at each new prompt (UserPromptSubmit)",
        "PostToolUse" => "live delivery of Everett messages mid-task, after tool calls (PostToolUse)",
        "extension" => "session cards + shared core + live delivery (extension)",
        other => other,
    }
}

pub const WELCOME_LINES: &[&str] = &[
    "Everett lists recent Claude Code, Codex, OMP, Pi, Hermes, and Grok sessions on this machine.",
    "It lets those sessions hand work to each other instead of you copy-pasting between them.",
    "It shares merged memory through supported hooks, or agents can read it with everett_core.",
];

pub const JEV_LINES: &[&str] = &[
    "Jev (typesafe.ai) picks the right session when many are running, instead of a lexical guess.",
    "Without it, routing stays local. Harness calls and LLM merges use their configured providers.",
];

struct QuitOnboarding;

#[derive(Default)]
pub struct OnboardConfig {
    pub detected: Vec<(String, usize)>,
    pub hooks: Vec<(String, HashMap<String, bool>)>,
    pub mcp: Vec<(String, bool)>,
    pub backfill_enabled: bool,
    pub span_days: i64,
    pub jev_choice: String,
    pub jev_key: String,
    pub jev_found_source: String,
    pub jev_validated: Option<bool>,
    pub trunk_schedule_enabled: bool,
}

#[derive(Default)]
pub struct ApplyResult {
    pub hook_lines: Vec<String>,
    pub mcp_lines: Vec<String>,
    pub backfill_written: usize,
    pub backfill_skipped: usize,
    pub jev_line: String,
    pub trunk_schedule_line: String,
}

/// harness -> session count, including CLIs that have not created a session yet.
pub fn detect_harnesses(hours: f64) -> Vec<(String, usize)> {
    let detected = crate::doctor::detected_harnesses();
    crate::registry::ADAPTERS
        .iter()
        .filter(|h| detected.iter().any(|d| d == *h))
        .map(|h| (h.to_string(), crate::registry::scan(hours, true, None, h).len()))
        .collect()
}

pub fn default_config() -> OnboardConfig {
    let detected = detect_harnesses(ALL_HOURS);
    let mut cfg = OnboardConfig {
        detected,
        backfill_enabled: true,
        span_days: 3,
        jev_choice: "skip".to_string(),
        ..Default::default()
    };
    for harness in HOOK_HARNESSES {
        if cfg.detected.iter().any(|(h, _)| h == harness) {
            cfg.hooks.push((
                harness.to_string(),
                crate::install::HOOKS
                    .iter()
                    .find(|(h, _)| *h == *harness)
                    .map(|(_, events)| {
                        events.iter().map(|(e, _, _)| (e.to_string(), true)).collect()
                    })
                    .unwrap_or_default(),
            ));
        }
    }
    if cfg.detected.iter().any(|(h, _)| h == "omp") {
        cfg.hooks.push(("omp".to_string(), HashMap::from([("extension".to_string(), true)])));
    }
    for harness in MCP_HARNESSES {
        if cfg.detected.iter().any(|(h, _)| h == harness) {
            cfg.mcp.push((harness.to_string(), true));
        }
    }
    cfg
}

/// Jev key sources, in order, with which one matched: env, config file, ~/.hermes/.env.
pub fn find_jev_key_with_source() -> (String, String) {
    let key = std::env::var("TYPESAFE_API_KEY").unwrap_or_default().trim().to_string();
    if !key.is_empty() {
        return (key, "env".to_string());
    }
    let key = crate::config::values()
        .ok()
        .and_then(|v| v.get("typesafe_api_key").cloned())
        .unwrap_or_default()
        .trim()
        .to_string();
    if !key.is_empty() {
        return (key, "config".to_string());
    }
    if let Ok(text) = std::fs::read_to_string(home().join(".hermes").join(".env")) {
        for line in text.lines() {
            let Some((name, value)) = line.split_once('=') else { continue };
            if name.trim() == "TYPESAFE_API_KEY" {
                let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
                if !value.is_empty() {
                    return (value.to_string(), "hermes".to_string());
                }
            }
        }
    }
    (String::new(), String::new())
}

pub fn apply_hooks(cfg: &OnboardConfig) -> Vec<String> {
    let mut report = Vec::new();
    for (harness, events) in &cfg.hooks {
        if harness == "omp" {
            if *events.get("extension").unwrap_or(&false) {
                match crate::install::apply("omp", None) {
                    Ok(msg) => report.push(msg),
                    Err(e) => report.push(format!("omp: {}", e)),
                }
            } else {
                report.push("omp: skipped (not selected)".to_string());
            }
            continue;
        }
        let selected: Vec<String> = events.iter().filter(|(_, on)| **on).map(|(e, _)| e.clone()).collect();
        if selected.is_empty() {
            report.push(format!("{}: skipped (not selected)", harness));
        } else {
            match crate::install::apply(harness, Some(&selected)) {
                Ok(msg) => report.push(msg),
                Err(e) => report.push(format!("{}: {}", harness, e)),
            }
        }
    }
    report
}

pub fn apply_mcp(cfg: &OnboardConfig) -> Vec<String> {
    cfg.mcp
        .iter()
        .map(|(harness, on)| {
            if *on {
                crate::install::apply_mcp(harness, false).unwrap_or_else(|e| format!("{}: {}", harness, e))
            } else {
                format!("{}: skipped (not selected)", harness)
            }
        })
        .collect()
}

/// Persist a pasted Jev key (never a found one -- it's already wherever it was found).
pub fn apply_jev(cfg: &OnboardConfig) -> String {
    if cfg.jev_choice != "paste" || cfg.jev_key.is_empty() {
        return "jev: skipped (using local matching)".to_string();
    }
    match crate::config::set_value("typesafe_api_key", &cfg.jev_key) {
        Ok(path) => format!(
            "jev: key saved to {} ({})",
            path.display(),
            if cfg.jev_validated.unwrap_or(false) { "validated" } else { "validation failed; kept anyway" }
        ),
        Err(e) => format!("jev: could not save key: {}", e),
    }
}

pub fn apply_trunk_schedule(cfg: &OnboardConfig) -> String {
    if !cfg.trunk_schedule_enabled {
        return "trunk schedule: skipped (not selected)".to_string();
    }
    match crate::trunk_schedule::install("04:00", "claude", None, None) {
        Ok(result) => format!(
            "trunk schedule: installed ({}, nightly at 04:00, --llm claude)",
            result.get("path").and_then(|v| v.as_str()).unwrap_or("")
        ),
        Err(e) => format!("trunk schedule: {}", e.message),
    }
}

pub fn missing_card_sessions(span_days: i64) -> Vec<crate::session::Session> {
    crate::registry::scan((span_days as f64) * 24.0, false, None, "")
        .into_iter()
        .filter(|s| s.card_source.is_none() && BACKFILL_HARNESSES.contains(&s.harness.as_str()))
        .collect()
}

/// Write AUTO cards (never overwriting an existing card, agent or auto). Returns (written, skipped).
pub fn generate_backfill_cards(sessions: &[crate::session::Session], progress: Option<&dyn Fn(usize, usize)>) -> (usize, usize) {
    let (mut written, mut skipped) = (0usize, 0usize);
    let total = sessions.len();
    for (i, s) in sessions.iter().enumerate() {
        let content = crate::hooks_common::auto_card(Path::new(&s.path), &s.harness, &s.cwd, &s.id);
        if content.is_empty() {
            skipped += 1;
        } else {
            let target = crate::cards::card_path(&s.id);
            if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            use std::io::Write as _;
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&target) {
                Ok(mut f) => match f.write_all(content.as_bytes()) {
                    Ok(_) => written += 1,
                    Err(_) => skipped += 1,
                },
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => skipped += 1,
                Err(_) => skipped += 1,
            }
        }
        if let Some(p) = progress {
            p(i + 1, total);
        }
    }
    (written, skipped)
}

pub fn run_apply(cfg: &OnboardConfig, backfill_progress: Option<&dyn Fn(usize, usize)>) -> ApplyResult {
    let hook_lines = apply_hooks(cfg);
    let mcp_lines = apply_mcp(cfg);
    let jev_line = apply_jev(cfg);
    let trunk_schedule_line = apply_trunk_schedule(cfg);
    let (written, skipped) = if cfg.backfill_enabled {
        generate_backfill_cards(&missing_card_sessions(cfg.span_days), backfill_progress)
    } else {
        (0, 0)
    };
    ApplyResult {
        hook_lines,
        mcp_lines,
        backfill_written: written,
        backfill_skipped: skipped,
        jev_line,
        trunk_schedule_line,
    }
}

pub fn summary_lines(cfg: &OnboardConfig, result: &ApplyResult) -> Vec<String> {
    let mut lines = vec!["Everett onboarding complete.".to_string(), String::new(), "Hooks:".to_string()];
    if result.hook_lines.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        lines.extend(result.hook_lines.iter().map(|l| format!("  {}", l)));
    }
    lines.push("MCP:".to_string());
    if result.mcp_lines.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        lines.extend(result.mcp_lines.iter().map(|l| format!("  {}", l)));
    }
    lines.push(format!("Smarter routing: {}", result.jev_line));
    lines.push(format!("Nightly merge: {}", result.trunk_schedule_line));
    if cfg.backfill_enabled {
        lines.push(format!(
            "Backfill: {} card(s) written, {} skipped (last {} day(s))",
            result.backfill_written, result.backfill_skipped, cfg.span_days
        ));
    } else {
        lines.push("Backfill: skipped".to_string());
    }
    lines.extend([
        String::new(),
        "Try next:".to_string(),
        "  everett ls".to_string(),
        "  (from inside an agent) use everett to ask my <name> session what it is doing".to_string(),
    ]);
    lines
}

/// --yes mode: read a Jev key from the named env var, validate it, and stage it for saving.
fn apply_jev_key_env(cfg: &mut OnboardConfig, var: Option<&str>) {
    let (found_key, found_source) = find_jev_key_with_source();
    if !found_key.is_empty() {
        cfg.jev_found_source = found_source;
        cfg.jev_choice = "skip".to_string();
        return;
    }
    let Some(var) = var else { return };
    let key = std::env::var(var).unwrap_or_default().trim().to_string();
    if key.is_empty() {
        return;
    }
    cfg.jev_validated = Some(crate::route::verify_key(&key));
    cfg.jev_key = key;
    cfg.jev_choice = "paste".to_string();
}

pub fn run_yes(args: &crate::cli::Args) -> i32 {
    let mut cfg = default_config();
    cfg.span_days = args.span_days;
    cfg.backfill_enabled = !args.no_backfill;
    if args.no_mcp {
        cfg.mcp = cfg.mcp.iter().map(|(h, _)| (h.clone(), false)).collect();
    }
    apply_jev_key_env(&mut cfg, args.jev_key_env.as_deref());
    cfg.trunk_schedule_enabled = args.schedule_merge;
    let result = run_apply(&cfg, None);
    for line in summary_lines(&cfg, &result) {
        println!("{}", line);
    }
    0
}

fn ask_yes(prompt: &str, default: bool) -> Result<bool, QuitOnboarding> {
    let suffix = if default { " [Y/n] " } else { " [y/N] " };
    loop {
        print!("{}{}", prompt, suffix);
        let _ = std::io::stdout().flush();
        let mut raw = String::new();
        match std::io::stdin().read_line(&mut raw) {
            Ok(0) | Err(_) => return Err(QuitOnboarding),
            _ => {}
        }
        let raw = raw.trim().to_lowercase();
        if raw == "q" || raw == "quit" {
            return Err(QuitOnboarding);
        }
        if raw.is_empty() {
            return Ok(default);
        }
        if raw == "y" || raw == "yes" {
            return Ok(true);
        }
        if raw == "n" || raw == "no" {
            return Ok(false);
        }
    }
}

fn plain_jev_entry(cfg: &mut OnboardConfig) -> Result<(), QuitOnboarding> {
    let key = rpassword::prompt_password("  paste key (input hidden, enter to skip): ")
        .unwrap_or_default()
        .trim()
        .to_string();
    if key.is_empty() {
        cfg.jev_choice = "skip".to_string();
        return Ok(());
    }
    println!("  validating...");
    let ok = crate::route::verify_key(&key);
    println!("{}", if ok { "  ok" } else { "  failed" });
    if ok {
        cfg.jev_key = key;
        cfg.jev_choice = "paste".to_string();
        cfg.jev_validated = Some(true);
        return Ok(());
    }
    if ask_yes("  keep it anyway?", false)? {
        cfg.jev_key = key;
        cfg.jev_choice = "paste".to_string();
        cfg.jev_validated = Some(false);
    } else {
        cfg.jev_choice = "skip".to_string();
    }
    Ok(())
}

fn run_plain(args: &crate::cli::Args) -> Result<i32, QuitOnboarding> {
    println!("Everett -- first-time setup");
    for line in WELCOME_LINES {
        println!("{}", line);
    }
    println!();
    let mut cfg = default_config();
    cfg.span_days = args.span_days;
    if args.no_backfill {
        cfg.backfill_enabled = false;
    }
    if args.no_mcp {
        cfg.mcp = cfg.mcp.iter().map(|(h, _)| (h.clone(), false)).collect();
    }

    if cfg.detected.is_empty() {
        println!("No coding-agent sessions or stores were found on this machine.");
    } else {
        println!("Found:");
        for (harness, count) in &cfg.detected {
            println!("  [x] {}: {} session(s)", harness, count);
        }
    }
    println!();

    println!("Hooks -- a backup is made of any file before it changes.");
    let hook_list: Vec<(String, Vec<String>)> = cfg
        .hooks
        .iter()
        .map(|(h, events)| (h.clone(), events.keys().cloned().collect()))
        .collect();
    for (harness, events) in &hook_list {
        if harness == "omp" {
            let path = crate::install::omp_extension_path();
            let on = ask_yes(
                &format!("  {}: install {} at {}?", harness, event_label("extension"), path.display()),
                true,
            )?;
            if let Some((_, map)) = cfg.hooks.iter_mut().find(|(h, _)| *h == *harness) {
                map.insert("extension".to_string(), on);
            }
            continue;
        }
        let path = crate::install::settings_path(harness);
        for event in events {
            let on = ask_yes(
                &format!("  {}: install {} in {}?", harness, event_label(event), path.display()),
                true,
            )?;
            if let Some((_, map)) = cfg.hooks.iter_mut().find(|(h, _)| *h == *harness) {
                map.insert(event.clone(), on);
            }
        }
    }
    println!();

    println!("MCP -- lets each harness call Everett as a tool.");
    let mcp_list: Vec<String> = cfg.mcp.iter().map(|(h, _)| h.clone()).collect();
    for harness in mcp_list {
        let on = ask_yes(
            &format!(
                "  register the Everett MCP server for {} in {}?",
                harness,
                crate::install::mcp_path(&harness).display()
            ),
            true,
        )?;
        if let Some((_, val)) = cfg.mcp.iter_mut().find(|(h, _)| *h == harness) {
            *val = on;
        }
    }
    println!();

    println!("Smarter routing (optional)");
    for line in JEV_LINES {
        println!("  {}", line);
    }
    let (found_key, found_source) = find_jev_key_with_source();
    if !found_key.is_empty() {
        cfg.jev_found_source = found_source;
        println!("  Jev key found ✓ ({})", cfg.jev_found_source);
        if ask_yes("  paste a different key instead?", false)? {
            plain_jev_entry(&mut cfg)?;
        } else {
            cfg.jev_choice = "skip".to_string();
        }
    } else if ask_yes("  paste a Jev key now?", false)? {
        plain_jev_entry(&mut cfg)?;
    } else {
        cfg.jev_choice = "skip".to_string();
    }
    println!();

    cfg.trunk_schedule_enabled = cfg!(target_os = "macos")
        && ask_yes(
            "Merge shared memory nightly? (schedules `everett trunk merge` via launchd, 04:00, --llm claude)",
            false,
        )?;
    println!();

    cfg.backfill_enabled = ask_yes(
        "Make cards for your recent sessions without one? (optional, skippable)",
        cfg.backfill_enabled,
    )?;
    if cfg.backfill_enabled {
        print!("  how many days back? [1-30, default {}] ", cfg.span_days);
        let _ = std::io::stdout().flush();
        let mut raw = String::new();
        let _ = std::io::stdin().read_line(&mut raw);
        let raw = raw.trim();
        if !raw.is_empty() {
            if let Ok(days) = raw.parse::<i64>() {
                if (1..=30).contains(&days) {
                    cfg.span_days = days;
                } else {
                    println!("  not 1-30; keeping the default.");
                }
            } else {
                println!("  not 1-30; keeping the default.");
            }
        }
        let preview = missing_card_sessions(cfg.span_days);
        println!("  {} session(s) without a card in the last {} day(s).", preview.len(), cfg.span_days);
        if preview.is_empty() {
            cfg.backfill_enabled = false;
        } else if !ask_yes(&format!("  generate {} automatic card(s) now?", preview.len()), true)? {
            cfg.backfill_enabled = false;
        }
    }
    println!();

    if !ask_yes("Apply these changes now?", true)? {
        println!("Nothing was changed.");
        return Ok(0);
    }

    let result = run_apply(&cfg, None);
    println!();
    for line in summary_lines(&cfg, &result) {
        println!("{}", line);
    }
    Ok(0)
}

pub fn run(args: &crate::cli::Args) -> i32 {
    if args.schedule_merge && !cfg!(target_os = "macos") {
        eprintln!("everett onboard: --schedule-merge requires macOS; use `everett trunk merge --llm none`.");
        return 2;
    }
    if !(1..=30).contains(&args.span_days) {
        eprintln!("everett onboard: --span-days must be between 1 and 30.");
        return 2;
    }
    if args.onboard_yes {
        return run_yes(args);
    }
    if let Ok(result) = crate::tui::run(args, default_config()) {
        match result {
            Some(()) => return 0,
            None => {
                println!("Cancelled -- no changes were made.");
                return 0;
            }
        }
    }
    match run_plain(args) {
        Ok(code) => code,
        Err(QuitOnboarding) => {
            println!("Cancelled -- no changes were made.");
            0
        }
    }
}
