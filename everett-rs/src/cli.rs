//! CLI commands: thin shell over the store/parsing layer. Everything user-visible
//! (text, json, exit code) happens here; the modules it calls stay pure of stdout.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::session::{now, project_label, Session};

pub fn cmd_external_add(id: &str, harness: &str, title: &str, cwd: &str, device: Option<&str>, replace: bool) -> i32 {
    match crate::adapters::external::register(id, harness, title, cwd, device, replace) {
        Ok(record) => {
            println!("{}", serde_json::to_string(&record).unwrap());
            0
        }
        Err(error) => {
            eprintln!("everett.external: {error}");
            2
        }
    }
}

pub fn cmd_external_list() -> i32 {
    println!("{}", serde_json::to_string(&crate::adapters::external::records()).unwrap());
    0
}

pub fn cmd_external_remove(id: &str) -> i32 {
    match crate::adapters::external::remove(id) {
        Ok(()) => {
            println!("Removed registration {id}. Its inbox is retained.");
            0
        }
        Err(error) => {
            eprintln!("everett.external: {error}");
            2
        }
    }
}

#[derive(Debug, Default)]
pub struct Args {
    pub peer: Option<String>,
    pub idempotency_key: Option<String>,
    pub hours: f64,
    pub json: bool,
    pub all: bool,
    pub harness: Option<String>,
    pub regenerate_auto: bool,
    pub dry_run: bool,
    pub regen_hours: Option<f64>,
    pub text: Option<String>,
    pub router: Option<String>,
    pub timeout: f64,
    pub to: Option<String>,
    pub spawn: bool,
    pub dir: Option<String>,
    pub spawn_harness: Option<String>,
    pub wake: bool,
    pub mode: String,
    pub wait: f64,
    pub kind: Option<String>,
    pub message: Option<String>,
    pub project: Option<String>,
    pub session: Option<String>,
    pub since: String,
    pub check: bool,
    pub target: Option<String>,
    pub as_: Option<String>,
    pub remove: bool,
    pub id: Option<String>,
    pub peek: bool,
    pub action: Option<String>,
    pub llm: Option<String>,
    pub at: String,
    pub apply: bool,
    pub fact: Option<String>,
    pub scope: Option<String>,
    pub claude: bool,
    pub codex: bool,
    pub omp: bool,
    pub grok: bool,
    pub devin: bool,
    pub pi: bool,
    pub hermes: bool,
    pub repair: bool,
    pub onboard_yes: bool,
    pub span_days: i64,
    pub no_backfill: bool,
    pub no_mcp: bool,
    pub jev_key_env: Option<String>,
    pub schedule_merge: bool,
    pub device: Option<String>,
    pub open: bool,
    pub force: bool,
    pub revoke_device: bool,
    pub auth_provider: Option<String>,
    pub auth_issuer: Option<String>,
    pub auth_cancel: bool,
    pub auth_no_wait: bool,
}

fn ago(ts: f64) -> String {
    let d = (crate::session::now() - ts) as i64;
    if d < 3600 {
        format!("{}m", d / 60)
    } else {
        format!("{}h", d / 3600)
    }
}

fn width() -> usize {
    terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(100)
}

pub fn cmd_ls(args: &Args) -> i32 {
    if let Some(peer) = &args.peer {
        return match crate::peer::ls(peer, args.hours, args.all, args.harness.as_deref().unwrap_or("")) {
            Ok(rows) => {
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&rows).unwrap());
                } else {
                    for row in rows.as_array().unwrap() {
                        println!("{}  {}  {}", row["id"].as_str().unwrap_or(""), row["cwd"].as_str().unwrap_or(""), row["card"].as_str().filter(|s| !s.is_empty()).or_else(|| row["title"].as_str()).unwrap_or(""));
                    }
                    if rows.as_array().unwrap().is_empty() { println!("no sessions on {peer}"); }
                }
                0
            }
            Err(e) => { eprintln!("everett: {e}"); e.code }
        };
    }
    let sessions = crate::registry::scan(args.hours, args.all, Some(crate::registry::CAP), args.harness.as_deref().unwrap_or(""));
    if args.json {
        println!("{}", serde_json::to_string_pretty(&sessions.iter().map(|s| s.to_dict()).collect::<Vec<_>>()).unwrap_or_default());
        return 0;
    }
    if sessions.is_empty() {
        println!("no sessions");
        return 0;
    }
    let wide = sessions.iter().map(|s| project_label(&s.cwd).chars().count()).max().unwrap_or(0).min(20);
    let harness_wide = sessions.iter().map(|s| s.harness.chars().count()).max().unwrap_or(0).max(6);
    for s in &sessions {
        let dot = if s.running { "●" } else { "○" };
        let mut project = project_label(&s.cwd);
        if project.chars().count() > wide {
            project = format!("{}…", project.chars().take(wide.saturating_sub(1)).collect::<String>());
        }
        let mut work = if !s.card.is_empty() {
            s.card.clone()
        } else if !s.title.is_empty() {
            s.title.clone()
        } else if !s.first_user.is_empty() {
            s.first_user.clone()
        } else {
            "(no text)".to_string()
        };
        if s.source == "t3code" {
            work = format!("[t3code] {}", work);
        }
        if ["blocked", "needs-input"].contains(&s.state_kind.as_str()) {
            work = format!("⚠ {} · {}", s.state, work);
        }
        let line = format!(
            "{} {:<hw$} {:>4}  {:<width$}  {}",
            dot, s.harness, ago(s.last_active), project, work, hw = harness_wide, width = wide
        );
        let line: String = line.chars().take(width()).collect();
        println!("{}", line);
    }
    0
}

/// Per-harness and total counts of agent/auto/missing cards.
pub type Coverage = (Vec<(String, HashMap<String, i64>)>, [i64; 5]);

pub fn card_coverage(sessions: &[Session]) -> Coverage {
    let mut order: Vec<String> = Vec::new();
    let mut counts: HashMap<String, HashMap<String, i64>> = HashMap::new();
    for harness in crate::registry::ADAPTERS {
        order.push(harness.to_string());
        counts.insert(harness.to_string(), HashMap::from([
            ("total".to_string(), 0i64), ("agent".to_string(), 0), ("auto".to_string(), 0), ("missing".to_string(), 0),
        ]));
    }
    for session in sessions {
        let entry = counts.entry(session.harness.clone()).or_insert_with(|| HashMap::from([
            ("total".to_string(), 0i64), ("agent".to_string(), 0), ("auto".to_string(), 0), ("missing".to_string(), 0),
        ]));
        if !order.iter().any(|o| o == &session.harness) {
            order.push(session.harness.clone());
        }
        *entry.get_mut("total").unwrap() += 1;
        let source = match session.card_source.as_deref() {
            Some("agent") => "agent",
            Some("auto") => "auto",
            _ => "missing",
        };
        *entry.get_mut(source).unwrap() += 1;
    }
    let totals = [
        counts.values().map(|m| m["total"]).sum(),
        counts.values().map(|m| m["agent"]).sum(),
        counts.values().map(|m| m["auto"]).sum(),
        counts.values().map(|m| m["missing"]).sum(),
        0,
    ];
    let rows = order
        .iter()
        .filter_map(|o| counts.get(o).map(|m| (o.clone(), m.clone())))
        .collect();
    (rows, totals)
}

const REGENERATE_HARNESSES: &[&str] = &["claude", "codex", "grok"];

/// Rewrite every AUTO-marked card among `sessions` with the current builder. Never touches a
/// missing card or an agent-written one. Returns (rewritten, skipped).
pub fn regenerate_auto_cards(sessions: &[Session], dry_run: bool) -> std::result::Result<(usize, usize), String> {
    let mut rewritten = 0usize;
    let mut skipped = 0usize;
    for s in sessions {
        if !REGENERATE_HARNESSES.contains(&s.harness.as_str()) || s.path.is_empty() {
            skipped += 1;
            continue;
        }
        if !crate::cards::is_auto_card(&s.id) {
            skipped += 1;
            continue;
        }
        let content = crate::hooks_common::auto_card(Path::new(&s.path), &s.harness, &s.cwd, &s.id);
        if content.is_empty() {
            skipped += 1;
            continue;
        }
        let Some(target) = crate::cards::card_path(&s.id) else {
            skipped += 1; // invalid session id: nothing safe to write
            continue;
        };
        let current = std::fs::read_to_string(&target).ok();
        if current.as_deref() == Some(content.as_str()) {
            skipped += 1;
            continue;
        }
        if !dry_run {
            std::fs::write(&target, &content)
                .map_err(|e| format!("cannot write {}: {}", target.display(), e))?;
        }
        rewritten += 1;
    }
    Ok((rewritten, skipped))
}

pub fn cmd_cards(args: &Args) -> i32 {
    if args.regenerate_auto {
        let hours = args.regen_hours.unwrap_or(args.hours);
        let sessions = crate::registry::scan(hours, true, None, "");
        let (rewritten, skipped) = match regenerate_auto_cards(&sessions, args.dry_run) {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("everett: {e}");
                return 2;
            }
        };
        let label = if args.dry_run { "Would rewrite" } else { "Rewrote" };
        println!("{} {} auto card(s), skipped {} (last {} hours).", label, rewritten, skipped, crate::fmt::g(hours));
        return 0;
    }
    let hours = args.regen_hours.unwrap_or(args.hours);
    let sessions = crate::registry::scan(hours, true, None, "");
    let (counts, totals) = card_coverage(&sessions);
    println!("Sessions (last {} hours): {}", crate::fmt::g(hours), totals[0]);
    println!("Agent cards: {}", totals[1]);
    println!("Auto cards: {}", totals[2]);
    println!("Missing: {}", totals[3]);
    println!("Per harness:");
    for (harness, item) in &counts {
        println!(
            "  {}: {} sessions, {} agent, {} auto, {} missing",
            harness, item["total"], item["agent"], item["auto"], item["missing"]
        );
    }
    0
}

fn session_label(session: &Session) -> String {
    let work = if !session.card.is_empty() {
        session.card.clone()
    } else if !session.title.is_empty() {
        session.title.clone()
    } else {
        session.first_user.chars().take(80).collect::<String>()
    };
    format!("[{}] {} — {}", session.harness, session.cwd, work)
}

pub fn cmd_route(args: &Args) -> i32 {
    let sessions = crate::registry::scan(args.hours, false, Some(crate::registry::CAP), "");
    let caller = crate::send::caller_session_id();
    let r = match crate::route::route(
        args.text.as_deref().unwrap_or(""),
        &sessions,
        args.router.as_deref(),
        if caller.is_empty() { None } else { Some(caller.as_str()) },
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("everett: {}", e.message);
            return e.code;
        }
    };
    warn_fallback(&r);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
        return 0;
    }
    println!(
        "{}  (choice={}, confidence={:.2}, router={})",
        r.get("decision").and_then(|v| v.as_str()).unwrap_or(""),
        r.get("choice").and_then(|v| v.as_str()).unwrap_or(""),
        r.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0),
        r.get("router").and_then(|v| v.as_str()).unwrap_or("jev")
    );
    if r.get("decision").and_then(|v| v.as_str()) == Some("SESSION") {
        if let Some(s) = r.get("session").and_then(|v| v.as_object()) {
            let title = s.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let first = s.get("first_user").and_then(|v| v.as_str()).unwrap_or("");
            let desc = if !title.is_empty() { title.to_string() } else { first.chars().take(80).collect() };
            println!(
                "  → [{}] {} — {}",
                s.get("harness").and_then(|v| v.as_str()).unwrap_or(""),
                s.get("cwd").and_then(|v| v.as_str()).unwrap_or(""),
                desc
            );
        }
    }
    if let Some(suggested) = r.get("suggested").and_then(|v| v.as_str()) {
        if !suggested.is_empty() {
            println!("  closest: {}", suggested);
        }
    }
    if let Some(cands) = r.get("candidates").and_then(|v| v.as_array()) {
        for c in cands {
            println!("  candidate: {}", c.as_str().unwrap_or(""));
        }
    }
    if let Some(command) = r.get("command").and_then(|v| v.as_str()) {
        if !command.is_empty() {
            println!("  {}", command);
        }
    }
    0
}

fn warn_fallback(r: &Map<String, Value>) {
    if let Some(warning) = r.get("warning").and_then(|v| v.as_str()) {
        eprintln!("everett: warning: {}", warning);
    }
}

fn emit(json_mode: bool, payload: &Map<String, Value>, lines: Vec<Option<String>>) {
    if json_mode {
        println!("{}", serde_json::to_string_pretty(&Value::Object(payload.clone())).unwrap_or_default());
    } else {
        println!("{}", lines.iter().flatten().cloned().collect::<Vec<_>>().join("\n"));
    }
}

fn spawn_cmd(args: &Args, r: &Map<String, Value>, sessions: &[Session]) -> i32 {
    let harness = args.spawn_harness.clone().unwrap_or_else(crate::route::default_harness);
    let cwd = match &args.dir {
        Some(d) => crate::paths::realpath(&crate::paths::expanduser(d)).to_string_lossy().to_string(),
        None => {
            let bd = crate::route::best_dir(args.text.as_deref().unwrap_or(""), sessions);
            if !bd.is_empty() {
                bd
            } else {
                std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
            }
        }
    };
    if args.dry_run {
        let command = crate::send::spawn_command(
            &harness,
            args.text.as_deref().unwrap_or(""),
            if harness == "claude" || harness == "grok" { "<new-session-id>" } else { "" },
            "",
        )
        .unwrap_or_default();
        let mut payload = r.clone();
        payload.insert("delivered".into(), json!(false));
        payload.insert("dry_run".into(), json!(true));
        payload.insert("spawn".into(), json!(true));
        payload.insert("harness".into(), json!(harness));
        payload.insert("cwd".into(), json!(cwd));
        payload.insert("command".into(), json!(command));
        emit(
            args.json,
            &payload,
            vec![
                Some(format!("DRY RUN  spawn NEW [{}] {}", harness, cwd)),
                Some(format!("  {}", crate::send::format_command(&command))),
            ],
        );
        return 0;
    }
    let env = match crate::send::hop_env() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("everett: {}", e.message);
            return e.code;
        }
    };
    match crate::send::spawn(&harness, args.text.as_deref().unwrap_or(""), &cwd, args.timeout, Some(&env)) {
        Ok(result) => {
            let mut payload = r.clone();
            payload.insert("delivered".into(), json!(true));
            payload.insert("spawn".into(), json!(true));
            payload.insert("harness".into(), json!(harness));
            payload.insert("cwd".into(), json!(cwd));
            payload.insert("session_id".into(), json!(result.session_id));
            payload.insert("command".into(), json!(result.command));
            payload.insert("reply".into(), json!(result.reply));
            emit(
                args.json,
                &payload,
                vec![
                    Some(format!(
                        "SPAWNED  [{}] {}  session {}",
                        harness,
                        cwd,
                        if result.session_id.is_empty() { "(id not found)" } else { &result.session_id }
                    )),
                    if result.reply.is_empty() { None } else { Some(result.reply) },
                ],
            );
            0
        }
        Err(e) => {
            eprintln!("everett: {}", e.message);
            e.code
        }
    }
}

fn cmd_remote_send(args: &Args, to: &str) -> i32 {
    if args.spawn || args.mode == "resume" || !args.wait.is_finite() || args.wait != 0.0 {
        eprintln!("everett: remote sends are inbox-only; no --spawn, --mode resume, or --wait. Poll everett peer status.");
        return 2;
    }
    if args.mode == "ping" {
        eprintln!("everett: No immediate remote ping transport. Nothing was queued. Use --mode inbox only for legacy relay/hooks/polling.");
        return 4;
    }
    if args.dry_run {
        return match crate::peer::target(to) {
            Ok((peer, _, _)) => {
                println!("{}", json!({"peer":peer,"target":to,"mode":"inbox","dry_run":true,"pickup":if crate::relay::configured(peer) {"Cloudflare relay"} else {"status polling"}}));
                0
            }
            Err(e) => { eprintln!("everett: {e}"); e.code }
        };
    }
    match crate::peer::send(to, args.text.as_deref().unwrap_or(""), args.idempotency_key.as_deref(), None) {
        Ok(value) => {
            if args.json {
                println!("{}", serde_json::to_string_pretty(&value).unwrap());
            } else {
                println!("{}  remote inbox {} message {}. Poll: everett peer status {} {}", value["state"].as_str().unwrap_or("queued").to_uppercase(), to,
                    value["message_id"].as_str().unwrap_or(""), value["peer"].as_str().unwrap_or(""), value["message_id"].as_str().unwrap_or(""));
                println!("idempotency key: {}", value["idempotency_key"].as_str().unwrap_or(""));
            }
            0
        }
        Err(e) => { eprintln!("everett: {e}"); e.code }
    }
}

pub fn cmd_proof_wake(args: &Args) -> i32 {
    let report = crate::proof_wake::run(args.to.as_deref().unwrap_or(""), args.hours, args.timeout, args.dry_run);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        println!("{}", report.human());
    }
    report.exit_code
}

pub fn cmd_send(args: &Args) -> i32 {
    if args.wake {
        if args.to.is_none() || args.spawn || args.mode == "resume" {
            eprintln!("everett: --wake requires one exact --to Claude session and cannot be combined with --spawn or --mode resume.");
            return 2;
        }
        if !std::io::stdin().is_terminal() {
            eprintln!("everett: --wake is available only from a local interactive terminal.");
            return 2;
        }
        let caller = crate::send::caller_session_id();
        if !caller.is_empty() || crate::send::current_hops() > 0 {
            eprintln!("everett: Claude wake is restricted to direct local human sends.");
            return 2;
        }
        if let Err(e) = crate::send::require_local_prompt(None) {
            eprintln!("everett: {}", e.message);
            return e.code;
        }
    }
    if let Some(to) = args.to.as_deref().filter(|to| crate::peer::target(to).is_ok()) {
        if args.wake {
            eprintln!("everett: --wake cannot target a remote peer session.");
            return 2;
        }
        return cmd_remote_send(args, to);
    }
    if !args.timeout.is_finite() || args.timeout <= 0.0 {
        eprintln!("everett: --timeout must be a finite number greater than zero.");
        return 2;
    }
    if let Err(e) = crate::send::guard_delivery(&args.mode, args.spawn, None) {
        eprintln!("everett: {}", e.message);
        return e.code;
    }
    if let Err(e) = crate::send::hop_env() {
        eprintln!("everett: {}", e.message);
        return e.code;
    }
    let mut r = Map::new();
    let session;
    if let Some(to) = &args.to {
        if args.spawn {
            eprintln!("everett: --to and --spawn are exclusive.");
            return 2;
        }
        let sessions = crate::registry::scan(args.hours, true, None, "");
        session = match crate::registry::find(to, &sessions) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("everett: {}", e.message);
                return 2;
            }
        };
        if args.wake && session.id != to.as_str() {
            eprintln!("everett: --wake requires the full exact session ID; prefixes and names are not allowed.");
            return 2;
        }
        r.insert("input".into(), json!(args.text.clone().unwrap_or_default()));
        r.insert("decision".into(), json!("SESSION"));
        r.insert("choice".into(), json!("to"));
        r.insert("confidence".into(), json!(1.0));
        r.insert("router".into(), json!("direct"));
        r.insert("session".into(), session.to_dict());
    } else {
        let sessions = crate::registry::scan(args.hours, false, Some(crate::registry::CAP), "");
        let caller = crate::send::caller_session_id();
        let result = match crate::route::route(
            args.text.as_deref().unwrap_or(""),
            &sessions,
            args.router.as_deref(),
            if caller.is_empty() { None } else { Some(caller.as_str()) },
        ) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("everett: {}", e.message);
                return e.code;
            }
        };
        warn_fallback(&result);
        if result.get("decision").and_then(|v| v.as_str()) == Some("NEW")
            && args.spawn
            && result.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0) >= crate::route::MIN_CONFIDENCE
        {
            return spawn_cmd(args, &result, &sessions);
        }
        if result.get("decision").and_then(|v| v.as_str()) != Some("SESSION") {
            let mut r = result.clone();
            r.insert("delivered".into(), json!(false));
            let hint = if r.get("decision").and_then(|v| v.as_str()) == Some("NEW") {
                "  nothing was sent (add --spawn to start a new session)"
            } else {
                "  nothing was sent"
            };
            let mut lines: Vec<Option<String>> = vec![Some(format!(
                "{}  (choice={}, confidence={:.2}, router={})",
                r.get("decision").and_then(|v| v.as_str()).unwrap_or(""),
                r.get("choice").and_then(|v| v.as_str()).unwrap_or(""),
                r.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0),
                r.get("router").and_then(|v| v.as_str()).unwrap_or("local")
            ))];
            if let Some(cands) = r.get("candidates").and_then(|v| v.as_array()) {
                for c in cands {
                    lines.push(Some(format!("  candidate: {}", c.as_str().unwrap_or(""))));
                }
            }
            lines.push(Some(hint.to_string()));
            emit(args.json, &r, lines);
            return 0;
        }
        r = result;
        session = Session::from_dict(r.get("session").and_then(|v| v.as_object()).unwrap());
    }
    let label = session_label(&session);
    let routed_line = if args.to.is_some() {
        None
    } else {
        Some(format!(
            "routed by {} → {} ({:.2})",
            r.get("router").and_then(|v| v.as_str()).unwrap_or("local"),
            label,
            r.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0)
        ))
    };
    let caller = crate::send::caller_session_id();
    if let Err(e) = crate::send::refuse_self(&session, if caller.is_empty() { None } else { Some(&caller) }) {
        eprintln!("everett: {}", e.message);
        return e.code;
    }
    let mode = if args.wake { "inbox".to_string() } else { match crate::send::delivery_mode(&session, &args.mode, None) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("everett: {}", e.message);
            return e.code;
        }
    }};
    if mode == "inbox" {
        if args.wake {
            return send_wake_cmd(args, &r, &session, &label, routed_line);
        }
        return send_inbox_cmd(args, &r, &session, &label, routed_line);
    }
    if mode == "ping" && !args.dry_run {
        return match crate::send::ping_as(&session, args.text.as_deref().unwrap_or(""), args.timeout, None, None) {
            Ok(result) => { let mut payload = r.clone(); payload.extend(result); emit(args.json, &payload, vec![routed_line, Some(format!("PINGED  {label}")), payload.get("reply").and_then(Value::as_str).map(str::to_string)]); 0 },
            Err(error) => { eprintln!("everett: {}", error.message); error.code },
        };
    }
    let command = match crate::send::command_for(&session, args.text.as_deref().unwrap_or("")) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("everett: {}", e.message);
            return e.code;
        }
    };
    if args.dry_run {
        let blocked_running = session.running || crate::registry::session_running(&session, None, None);
        let mut payload = r.clone();
        payload.insert("manual_command".into(), r.get("command").cloned().unwrap_or(Value::Null));
        payload.insert("delivered".into(), json!(false));
        payload.insert("dry_run".into(), json!(true));
        payload.insert("command".into(), json!(command));
        payload.insert("blocked_running".into(), json!(blocked_running));
        emit(
            args.json,
            &payload,
            vec![
                routed_line,
                Some(format!("DRY RUN  {}", label)),
                Some(format!("  {}", crate::send::format_command(&command))),
                if blocked_running { Some("  blocked: target session is marked running".to_string()) } else { None },
            ],
        );
        return 0;
    }
    let env = match crate::send::hop_env() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("everett: {}", e.message);
            return e.code;
        }
    };
    match crate::send::send(&session, args.text.as_deref().unwrap_or(""), args.timeout, 120.0, Some(&env)) {
        Ok(result) => {
            let mut payload = r.clone();
            payload.insert("manual_command".into(), r.get("command").cloned().unwrap_or(Value::Null));
            payload.insert("delivered".into(), json!(true));
            payload.insert("command".into(), json!(result.command));
            payload.insert("reply".into(), json!(result.reply));
            emit(
                args.json,
                &payload,
                vec![
                    routed_line,
                    Some(format!("SENT  {}", label)),
                    if result.reply.is_empty() { None } else { Some(result.reply) },
                ],
            );
            0
        }
        Err(e) => {
            eprintln!("everett: {}", e.message);
            e.code
        }
    }
}

fn send_wake_cmd(args: &Args, r: &Map<String, Value>, session: &Session, label: &str, routed_line: Option<String>) -> i32 {
    if let Err(e) = crate::claude_wake::validate_target(session) {
        eprintln!("everett: {}", e.message);
        return e.code;
    }
    if args.dry_run {
        let mut payload = r.clone();
        payload.insert("dry_run".into(), json!(true));
        payload.insert("queued".into(), json!(false));
        payload.insert("mode".into(), json!("inbox"));
        payload.insert("wake".into(), json!("claude-background"));
        emit(args.json, &payload, vec![
            routed_line,
            Some(format!("DRY RUN  {}", label)),
            Some("  would queue the request, then attach through a PTY with a fixed inbox nudge".into()),
        ]);
        return 0;
    }
    if !args.wait.is_finite() || args.wait < 0.0 {
        eprintln!("everett: --wait must be a finite number of seconds, 0 or more.");
        return 2;
    }
    let prepared = match crate::claude_wake::prepare(session) {
        Ok(prepared) => prepared,
        Err(error) => {
            eprintln!("everett: {}", error.message);
            return error.code;
        }
    };
    let queued = match crate::send::send_inbox(session, args.text.as_deref().unwrap_or(""), 0.0, None, 1.0, None) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("everett: {}", error.message);
            return error.code;
        }
    };
    let message_id = queued.get("message_id").and_then(Value::as_str).unwrap_or("").to_string();
    let wake = crate::claude_wake::wake(&prepared, &message_id);
    let (reply, reply_error) = if matches!(&wake, crate::claude_wake::WakeOutcome::Cancelled { .. }) {
        (None, None)
    } else if args.wait > 0.0 && !message_id.is_empty() {
        match wait_target_reply(crate::inbox::HUMAN, &message_id, &session.id, args.wait, 1.0) {
            Ok(reply) => (reply, None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
    };
    let reply_text = reply.as_ref().map(crate::inbox::consumption_text);
    let mut payload = r.clone();
    payload.insert("mode".into(), json!("inbox"));
    payload.insert("queued".into(), json!(true));
    payload.insert("message_id".into(), json!(message_id));
    payload.insert("reply_inbox".into(), json!(crate::inbox::HUMAN));
    payload.insert("wake".into(), wake_json(&wake));
    payload.insert("reply".into(), json!(reply_text));
    payload.insert("reply_received".into(), json!(reply.is_some()));
    payload.insert("reply_id".into(), reply.as_ref().and_then(|m| m.get("id")).cloned().unwrap_or(Value::Null));
    payload.insert("reply_to".into(), reply.as_ref().and_then(|m| m.get("reply_to")).cloned().unwrap_or(Value::Null));
    if let Some(error) = &reply_error { payload.insert("reply_error".into(), json!(error)); }
    let wake_line = match &wake {
        crate::claude_wake::WakeOutcome::PromptSent { client_pid, target_pid } => Some(format!("WAKE PROMPT SENT  client {} / background {}", client_pid, target_pid)),
        crate::claude_wake::WakeOutcome::NotAttached { detail } => Some(format!("WAKE DEFERRED  {}", detail)),
        crate::claude_wake::WakeOutcome::TimedOut { client_pid, target_pid } => Some(format!("WAKE TIMED OUT  client {} / background {}", client_pid, target_pid)),
        crate::claude_wake::WakeOutcome::Cancelled { client_pid, target_pid } => Some(format!("WAKE CANCELLED  client {} / background {}; request remains recorded; it may already be read", client_pid, target_pid)),
        crate::claude_wake::WakeOutcome::Failed { detail } => Some(format!("WAKE FAILED  {}", detail)),
    };
    let mut lines = vec![routed_line, Some(format!("QUEUED  {}", label)), Some(format!("  Everett message {}", message_id)), wake_line];
    if let Some(reply) = reply_text {
        lines.push(Some(reply));
    } else if let Some(error) = &reply_error {
        lines.push(Some(format!("  reply wait failed: {}; message {} remains recorded; it may already be read", error, message_id)));
    } else if args.wait > 0.0 {
        lines.push(Some(format!("  no linked reply within {}s; the request remains recorded; it may already be read", crate::fmt::g(args.wait))));
    }
    emit(args.json, &payload, lines);
    if reply.is_some() || (args.wait == 0.0 && matches!(&wake, crate::claude_wake::WakeOutcome::PromptSent { .. })) {
        0
    } else if matches!(&wake, crate::claude_wake::WakeOutcome::Cancelled { .. }) {
        130
    } else if reply_error.is_some()
        || matches!(&wake, crate::claude_wake::WakeOutcome::Failed { .. })
    {
        5
    } else {
        4
    }
}

fn wait_target_reply(sender: &str, message_id: &str, target_id: &str, wait: f64, poll: f64) -> std::result::Result<Option<Map<String, Value>>, String> {
    let deadline = crate::session::now() + wait.max(0.0);
    loop {
        for item in crate::inbox::pending(sender, None) {
            if is_target_reply(&item, message_id, target_id) {
                if let Some(id) = item.get("id").and_then(Value::as_str) {
                    crate::inbox::persist_relay_context(sender, &[id]).map_err(|error| error.message)?;
                    crate::inbox::mark_done(sender, &[id]);
                }
                return Ok(Some(item));
            }
        }
        let now = crate::session::now();
        if now >= deadline { return Ok(None); }
        std::thread::sleep(std::time::Duration::from_secs_f64(poll.min(deadline - now).max(0.001)));
    }
}

fn is_target_reply(item: &Map<String, Value>, message_id: &str, target_id: &str) -> bool {
    item.get("reply_to").and_then(Value::as_str) == Some(message_id)
        && item.get("from").and_then(Value::as_str) == Some(target_id)
}

fn wake_json(outcome: &crate::claude_wake::WakeOutcome) -> Value {
    match outcome {
        crate::claude_wake::WakeOutcome::PromptSent { client_pid, target_pid } => json!({"state":"prompt_sent","client_pid":client_pid,"target_pid":target_pid}),
        crate::claude_wake::WakeOutcome::NotAttached { detail } => json!({"state":"not_attached","detail":detail}),
        crate::claude_wake::WakeOutcome::TimedOut { client_pid, target_pid } => json!({"state":"timed_out","client_pid":client_pid,"target_pid":target_pid}),
        crate::claude_wake::WakeOutcome::Cancelled { client_pid, target_pid } => json!({"state":"cancelled","client_pid":client_pid,"target_pid":target_pid}),
        crate::claude_wake::WakeOutcome::Failed { detail } => json!({"state":"failed","detail":detail}),
    }
}

fn send_inbox_cmd(args: &Args, r: &Map<String, Value>, session: &Session, label: &str, routed_line: Option<String>) -> i32 {
    if args.dry_run {
        let mut payload = r.clone();
        payload.insert("delivered".into(), json!(false));
        payload.insert("queued".into(), json!(false));
        payload.insert("wake".into(), json!(false));
        payload.insert("dry_run".into(), json!(true));
        payload.insert("mode".into(), json!("inbox"));
        emit(
            args.json,
            &payload,
            vec![
                routed_line,
                Some(format!("DRY RUN  {}", label)),
                Some("  would explicitly queue for hooks or polling; no ping or wake".to_string()),
            ],
        );
        return 0;
    }
    if !args.wait.is_finite() || args.wait < 0.0 {
        eprintln!("everett: --wait must be a finite number of seconds, 0 or more.");
        return 2;
    }
    let result = match crate::send::send_inbox(session, args.text.as_deref().unwrap_or(""), args.wait, None, 1.0, args.idempotency_key.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("everett: {}", e.message);
            return e.code;
        }
    };
    let picked_up = result.get("delivered").and_then(Value::as_bool) == Some(true);
    let webhook_state = result.get("webhook").and_then(|value| value.get("state")).and_then(Value::as_str);
    let wake_clause = match webhook_state {
        Some("accepted") => "owner webhook returned HTTP 200; routine acceptance is not inbox consumption",
        Some(_) => "owner webhook did not accept a routine run",
        None => "no ping or wake was sent",
    };
    let mut lines = vec![
        routed_line,
        Some(format!("{}  {}", if picked_up { "PICKED UP" } else { "QUEUED" }, label)),
        Some(format!(
            "  message {}: {wake_clause}; {}",
            result.get("message_id").and_then(|v| v.as_str()).unwrap_or(""),
            if picked_up {
                "inbox pickup was observed".to_string()
            } else if result.get("hooked").and_then(|v| v.as_bool()).unwrap_or(false) {
                "hooks or `everett inbox` must pick it up".to_string()
            } else {
                format!("no {} inbox hook; it must run `everett inbox`", session.harness)
            }
        )),
    ];
    if let Some(webhook) = result.get("webhook") {
        lines.push(Some(format!(
            "  webhook {}",
            serde_json::to_string(webhook).unwrap_or_else(|_| "{}".to_string())
        )));
    }
    if args.wait > 0.0 {
        match result.get("reply") {
            Some(Value::String(reply)) => lines.push(Some(reply.clone())),
            _ => lines.push(Some(format!(
                "  no reply within {}s; check inbox {} (`everett inbox`)",
                crate::fmt::g(args.wait),
                result.get("reply_inbox").and_then(|v| v.as_str()).unwrap_or("")
            ))),
        }
    } else {
        lines.push(Some(format!(
            "  replies go to inbox {} (`everett inbox`); add --wait S to wait",
            result.get("reply_inbox").and_then(|v| v.as_str()).unwrap_or("")
        )));
    }
    let mut payload = r.clone();
    for (k, v) in &result {
        payload.insert(k.clone(), v.clone());
    }
    emit(args.json, &payload, lines);
    0
}

pub fn cmd_reply(args: &Args) -> i32 {
    let caller = crate::send::caller_session_id();
    match crate::send::reply(
        args.id.as_deref().unwrap_or(""),
        args.text.as_deref().unwrap_or(""),
        (!caller.is_empty()).then_some(caller.as_str()),
    ) {
        Ok(result) => {
            if result.get("mode").and_then(Value::as_str) == Some("peer-reply") {
                println!("RETAINED  reply to {} for status polling by the sender; no callback or remote inbox delivery claimed", result["reply_to"].as_str().unwrap_or(""));
                return 0;
            }
            println!(
                "REPLIED  to {} (inbox {}), message {}",
                result.get("reply_to").and_then(|v| v.as_str()).unwrap_or(""),
                result.get("to").and_then(|v| v.as_str()).unwrap_or(""),
                result.get("message_id").and_then(|v| v.as_str()).unwrap_or("")
            );
            0
        }
        Err(e) => {
            eprintln!("everett: {}", e.message);
            e.code
        }
    }
}

pub fn cmd_inbox(args: &Args) -> i32 {
    let caller = crate::send::caller_session_id();
    let sid = args
        .session
        .clone()
        .unwrap_or(if caller.is_empty() { crate::inbox::HUMAN.to_string() } else { caller });
    let mut items = crate::inbox::pending(&sid, None);
    if let Some(mid) = args.message.as_deref() {
        items.retain(|m| m.get("id").and_then(Value::as_str) == Some(mid));
    }
    let viewed_ids: Vec<&str> = items
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .collect();
    if let Err(error) = crate::inbox::persist_relay_context(&sid, &viewed_ids) {
        eprintln!("everett: {error}");
        return error.code;
    }
    if !args.peek {
        let ids: Vec<&str> = items
            .iter()
            .filter_map(|m| m.get("id").and_then(|v| v.as_str()))
            .collect();
        crate::inbox::mark_done(&sid, &ids);
        if let Some(hops) = items.iter().filter(|m| m.contains_key("peer_client") || m.contains_key("peer_reply_id") || m.contains_key("relay_device"))
            .filter_map(|m| m.get("hops").and_then(Value::as_i64)).max() {
            crate::inbox::record_hops(&sid, hops);
        }
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"session": sid, "messages": items.iter().cloned().map(crate::inbox::consumption_message).collect::<Vec<_>>()})).unwrap_or_default()
        );
        return 0;
    }
    if items.is_empty() {
        println!("inbox {}: empty", sid);
        return 0;
    }
    for m in &items {
        let kind = match m.get("kind").and_then(|v| v.as_str()) {
            Some("reply") => format!("reply to {}", m.get("reply_to").and_then(|v| v.as_str()).unwrap_or("")),
            Some("event") => "event".to_string(),
            _ => "message".to_string(),
        };
        println!(
            "{}  {} ago  from {}  ({})",
            m.get("id").and_then(|v| v.as_str()).unwrap_or(""),
            ago(m.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0)),
            m.get("from").and_then(|v| v.as_str()).unwrap_or(""),
            kind
        );
        let text = crate::inbox::consumption_text(m);
        println!("  {}", text.replace('\n', "\n  "));
    }
    0
}

pub fn cmd_event(args: &Args) -> i32 {
    match crate::events::record(
        args.kind.as_deref().unwrap_or(""),
        args.message.as_deref().unwrap_or(""),
        args.session.as_deref(),
        args.project.as_deref(),
        "",
        None,
        "manual",
        None,
    ) {
        Ok(Some(event)) => {
            println!(
                "event {}: {} [{}] {}{}",
                event.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                event.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
                event.get("project").and_then(|v| v.as_str()).filter(|p| !p.is_empty()).unwrap_or("-"),
                event.get("text").and_then(|v| v.as_str()).unwrap_or(""),
                if event.get("session").and_then(|v| v.as_str()).map(|s| s.is_empty()).unwrap_or(true) {
                    "  (no session detected; not attached to a card)"
                } else {
                    ""
                }
            );
            0
        }
        Ok(None) => 0,
        Err(e) => {
            eprintln!("everett: {}", e.message);
            e.code
        }
    }
}

pub fn cmd_events(args: &Args) -> i32 {
    let window = match crate::events::parse_since(&args.since) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("everett: {}", e.message);
            return e.code;
        }
    };
    if args.check {
        for data in crate::events::check_escalations(None, None) {
            let tag = data
                .get("project")
                .and_then(|v| v.as_str())
                .filter(|p| !p.is_empty())
                .map(|p| p.to_string())
                .unwrap_or_else(|| {
                    data.get("session")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .chars()
                        .take(8)
                        .collect()
                });
            println!("escalated: {} [{}]", crate::events::describe(&data, None), tag);
        }
    }
    let filter = args.session.clone().unwrap_or_default();
    let items: Vec<Map<String, Value>> = crate::events::read(now() - window)
        .into_iter()
        .filter(|e| filter.is_empty() || e.get("session").and_then(|v| v.as_str()).unwrap_or("").starts_with(&filter))
        .collect();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&items).unwrap_or_default());
        return 0;
    }
    if items.is_empty() {
        println!("no events in the last {}", args.since);
        return 0;
    }
    for e in &items {
        let where_ = e.get("project").and_then(|v| v.as_str()).filter(|p| !p.is_empty()).unwrap_or("-");
        let who = format!(
            "{} {}",
            e.get("harness").and_then(|v| v.as_str()).filter(|h| !h.is_empty()).unwrap_or("?"),
            e.get("session").and_then(|v| v.as_str()).unwrap_or("-").chars().take(8).collect::<String>()
        );
        let stamp = crate::timefmt::strftime_local("%m-%d %H:%M", e.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0));
        let text = e.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let mut line = format!("{}  {:<11} {:<16} {:<14} {}", stamp, e.get("kind").and_then(|v| v.as_str()).unwrap_or(""), who, where_, text);
        let maxw = width() + 40;
        if line.chars().count() > maxw {
            line = line.chars().take(maxw).collect();
        }
        if e.get("source").and_then(|v| v.as_str()) == Some("auto") {
            line.push_str("  (auto)");
        }
        println!("{}", line);
    }
    0
}

pub fn cmd_subscribe(args: &Args) -> i32 {
    let caller = crate::send::caller_session_id();
    let subscriber = args.as_.clone().unwrap_or(caller);
    let target = crate::events::resolve_target(args.target.as_deref().unwrap_or(""));
    match crate::events::subscribe(&subscriber, &target, args.remove) {
        Ok(current) => {
            let verb = if args.remove { "unsubscribed from" } else { "subscribed to" };
            println!(
                "{} {} {}; now following: {}",
                subscriber,
                verb,
                target,
                if current.is_empty() { "nothing".to_string() } else { current.join(", ") }
            );
            0
        }
        Err(e) => {
            eprintln!("everett: {}", e.message);
            e.code
        }
    }
}

pub fn cmd_trunk(args: &Args) -> i32 {
    match args.action.as_deref().unwrap_or("view") {
        "merge" => cmd_merge(args),
        "schedule" => cmd_trunk_schedule(args),
        _ => {
            let sessions = crate::registry::scan(args.hours, false, Some(crate::registry::CAP), "");
            if args.dry_run {
                println!("{}", crate::trunk::render(&sessions, None));
                return 0;
            }
            match crate::trunk::write(&sessions, None) {
                Ok(path) => {
                    println!("wrote {}", path.display());
                    0
                }
                Err(e) => {
                    eprintln!("everett: {}", e.message);
                    2
                }
            }
        }
    }
}

fn cmd_merge(args: &Args) -> i32 {
    let llm = args.llm.clone().unwrap_or_else(crate::core::default_llm);
    let result = match crate::core::merge(&llm, args.dry_run) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("everett: {}", e.message);
            return e.code;
        }
    };
    if !result.get("merged").and_then(|v| v.as_i64()).map(|m| m > 0).unwrap_or(false) {
        println!("inbox is empty; nothing to merge");
        return 0;
    }
    let verb = if args.dry_run { "would write" } else { "wrote" };
    println!(
        "merged {} learning(s) with --llm {}",
        result.get("merged").and_then(|v| v.as_i64()).unwrap_or(0),
        llm
    );
    if let Some(files) = result.get("files").and_then(|v| v.as_object()) {
        for (path, text) in files {
            let text = text.as_str().unwrap_or("");
            println!("{} {} ({} words)", verb, path, crate::core::words(text));
            if args.dry_run {
                println!("{}", text);
            }
        }
    }
    if let Some(history) = result.get("history").and_then(|v| v.as_str()) {
        println!("previous core and inbox saved to {}", history);
    }
    if let Some(mirror) = result.get("mirror").and_then(|v| v.as_str()) {
        println!("mirrored to {}", mirror);
    }
    0
}

fn cmd_trunk_schedule(args: &Args) -> i32 {
    let Some(scheduler) = crate::trunk_schedule::Scheduler::current() else {
        eprintln!("everett: automatic scheduling requires macOS launchd or Linux systemd. Run `everett trunk merge --llm none` manually.");
        return 2;
    };
    let llm = args.llm.clone().unwrap_or_else(|| "claude".to_string());
    if args.remove {
        let path = scheduler.path();
        if !args.apply {
            println!("would remove {} (run again with --apply)", path.display());
            return 0;
        }
        let result = match scheduler.remove() {
            Ok(result) => result,
            Err(error) => {
                eprintln!("everett: {}", error.message);
                return error.code;
            }
        };
        if result.get("removed").and_then(|v| v.as_bool()).unwrap_or(false) {
            println!("removed {}", result.get("path").and_then(|v| v.as_str()).unwrap_or(""));
        } else {
            println!("not scheduled ({})", result.get("path").and_then(|v| v.as_str()).unwrap_or(""));
        }
        return 0;
    }
    let preview = match scheduler.preview(&args.at, &llm) {
        Ok(preview) => preview,
        Err(error) => {
            eprintln!("everett: {}", error.message);
            return error.code;
        }
    };
    if !args.apply {
        println!("{}", preview);
        println!("# run again with --apply to install it");
        return 0;
    }
    match scheduler.install(&args.at, &llm) {
        Ok(result) => {
            println!("wrote {}", result.get("path").and_then(|v| v.as_str()).unwrap_or(""));
            println!("{} activation: ok", scheduler.name());
            println!("logs: {}", scheduler.logs());
            0
        }
        Err(e) => {
            eprintln!("everett: {}", e.message);
            e.code
        }
    }
}

pub fn cmd_learn(args: &Args) -> i32 {
    match crate::core::learn(
        args.fact.as_deref().unwrap_or(""),
        args.project.as_deref(),
        args.scope.as_deref(),
        None,
    ) {
        Ok(entry) => {
            let where_ = if entry.get("scope").and_then(|v| v.as_str()) == Some("project") {
                format!("project {}", entry.get("project").and_then(|v| v.as_str()).unwrap_or(""))
            } else {
                "global".to_string()
            };
            println!("learned ({}); it joins the shared core at the next `everett trunk merge`", where_);
            0
        }
        Err(e) => {
            eprintln!("everett: {}", e.message);
            e.code
        }
    }
}

pub fn cmd_core(args: &Args) -> i32 {
    let project = match &args.project {
        Some(p) => crate::core::slug(p),
        None => crate::core::project_for(
            &std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        ),
    };
    match args.action.as_deref().unwrap_or("show") {
        "edit-path" => {
            println!(
                "{}",
                (if args.project.is_some() {
                    crate::core::project_path(&project)
                } else {
                    crate::core::global_path()
                })
                .display()
            );
            0
        }
        "history" => {
            let mut snaps: Vec<PathBuf> = std::fs::read_dir(crate::core::history_dir())
                .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
                .unwrap_or_default();
            snaps.sort();
            if snaps.is_empty() {
                println!("no merges yet");
            } else {
                for snap in snaps {
                    println!("{}", snap.display());
                }
            }
            0
        }
        _ => {
            let text = if args.project.is_none() {
                crate::core::context(
                    &std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
                )
            } else {
                format!(
                    "{}\n\n{}",
                    crate::core::read_file(&crate::core::global_path()).trim(),
                    crate::core::read_file(&crate::core::project_path(&project)).trim()
                )
                .trim()
                .to_string()
            };
            println!("{}", if text.is_empty() { "(empty)" } else { &text });
            let pending = crate::core::read_inbox().len();
            if pending > 0 {
                println!("\n({} learning(s) waiting in the inbox; run `everett trunk merge`)", pending);
            }
            0
        }
    }
}

fn harnesses(args: &Args, mcp: bool) -> Vec<String> {
    let mut choices = vec!["claude", "codex"];
    for (h, asked) in [("omp", args.omp), ("grok", args.grok), ("pi", args.pi), ("devin", args.devin)] {
        if asked || crate::install::harness_present(h) {
            choices.push(h);
        }
    }
    if mcp && args.hermes {
        choices.push("hermes");
    }
    let mut picked: Vec<String> = Vec::new();
    for h in &choices {
        if match *h {
            "claude" => args.claude,
            "codex" => args.codex,
            "omp" => args.omp,
            "grok" => args.grok,
            "devin" => args.devin,
            "pi" => args.pi,
            "hermes" => args.hermes,
            _ => false,
        } {
            picked.push(h.to_string());
        }
    }
    if picked.is_empty() {
        choices.iter().map(|h| h.to_string()).collect()
    } else {
        picked
    }
}

pub fn cmd_install_hooks(args: &Args) -> i32 {
    for harness in harnesses(args, false) {
        if args.apply {
            match crate::install::apply(&harness, None) {
                Ok(msg) => println!("{}", msg),
                Err(e) => {
                    eprintln!("everett: {}: {}", harness, e);
                    return 1;
                }
            }
        } else {
            println!("## {}", harness);
            println!("{}", crate::install::snippet(&harness));
        }
    }
    if !args.apply {
        println!("# run again with --apply to merge these (a backup is written first)");
    }
    0
}

pub fn cmd_mcp(_args: &Args) -> i32 {
    crate::mcp::serve()
}

pub fn cmd_install_mcp(args: &Args) -> i32 {
    if args.repair && !args.apply {
        eprintln!("everett: --repair requires --apply (backs up and refreshes the Everett entry).");
        return 2;
    }
    for harness in harnesses(args, true) {
        if args.apply {
            match crate::install::apply_mcp(&harness, args.repair) {
                Ok(msg) => println!("{}", msg),
                Err(e) => {
                    eprintln!("everett: {}: {}", harness, e);
                    return 1;
                }
            }
        } else {
            println!("## {}", harness);
            println!("{}", crate::install::mcp_snippet(&harness));
        }
    }
    if !args.apply {
        println!("# run again with --apply to register it (a backup is written first)");
    }
    0
}

pub fn cmd_doctor(args: &Args) -> i32 {
    crate::doctor::run(args.hours)
}

fn auth_provider(args: &Args) -> crate::error::Result<()> {
    match args.auth_provider.as_deref() {
        Some("cloudflare") => Ok(()),
        Some(_) => Err(crate::error::EverettError::new(2, "auth.provider must be cloudflare")),
        None => crate::cloudflare_auth::require_provider(),
    }
}

pub fn cmd_login(args: &Args) -> i32 {
    match auth_provider(args) {
        Ok(()) => crate::cloudflare_auth::cmd_login(args),
        Err(e) => { eprintln!("everett login: {e}"); e.code },
    }
}

pub fn cmd_whoami(args: &Args) -> i32 {
    match auth_provider(args) {
        Ok(()) => crate::cloudflare_auth::cmd_whoami(args),
        Err(e) => { eprintln!("everett whoami: {e}"); e.code },
    }
}

pub fn cmd_logout(args: &Args) -> i32 {
    match auth_provider(args) {
        Ok(()) => crate::cloudflare_auth::cmd_logout(args),
        Err(e) => { eprintln!("everett logout: {e}"); e.code },
    }
}

pub fn cmd_onboard(args: &Args) -> i32 {
    crate::onboard::run(args)
}

pub fn cmd_hook(stem: &str, rest: &[String]) -> i32 {
    crate::hooks_common::hook_main(stem, rest)
}

#[cfg(test)]
mod wake_reply_tests {
    use super::*;

    #[test]
    fn wake_reply_must_match_both_message_and_target_session() {
        let correct = serde_json::from_value::<Map<String, Value>>(json!({
            "id":"mreply00001", "reply_to":"mrequest0001", "from":"target-session"
        }))
        .unwrap();
        let wrong_target = serde_json::from_value::<Map<String, Value>>(json!({
            "id":"mreply00002", "reply_to":"mrequest0001", "from":"other-session"
        }))
        .unwrap();
        let wrong_message = serde_json::from_value::<Map<String, Value>>(json!({
            "id":"mreply00003", "reply_to":"other-message", "from":"target-session"
        }))
        .unwrap();
        assert!(is_target_reply(
            &correct,
            "mrequest0001",
            "target-session"
        ));
        assert!(!is_target_reply(
            &wrong_target,
            "mrequest0001",
            "target-session"
        ));
        assert!(!is_target_reply(
            &wrong_message,
            "mrequest0001",
            "target-session"
        ));
    }

    #[test]
    fn wake_cli_rejects_noninteractive_invocation_before_session_lookup() {
        let args = Args {
            wake: true,
            to: Some("1f57ab77-c669-4bb6-9ac8-f9a9073de8ea".into()),
            text: Some("request".into()),
            ..Args::default()
        };
        assert_eq!(cmd_send(&args), 2);
    }
}
