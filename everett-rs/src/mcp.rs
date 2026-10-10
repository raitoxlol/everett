//! `everett mcp`: a stdio MCP server (JSON-RPC 2.0, newline-delimited).
//! Protocol output goes to stdout only; diagnostics go to ~/.everett/mcp.log.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::sync::OnceLock;

use serde_json::{json, Map, Value};

use crate::error::EverettError;
use crate::send::{caller_identity, delivery_mode, refuse_self};
use crate::session::{now, Session};

pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
/// Largest encoded tool result the gateway forwards; bigger replies would be
/// dropped after delivery, so batch under it instead of consuming the inbox.
const MAX_TOOL_RESULT: usize = 240 * 1024;

const INSTRUCTIONS: &str =
    "Everett is the layer above every coding-agent session on this machine (Claude Code, Codex, OMP, Pi, Hermes). \
     To hand off work, call everett_send with just `text` describing the task and let Everett route it -- do not \
     eyeball everett_ls and pick a target yourself. Only pass `to` when the user named a specific session, or when \
     you are replying to one. everett_ls is for a quick overview of what other sessions are doing, not for picking \
     a send target. everett_route is the read-only preview of that same routing decision, for when you want to see \
     it before sending. Answer messages you receive with everett_send(reply_to=...). Use everett_learn to push \
     durable facts that every session should know into the shared core, and everett_core to read it. Keep your own \
     card current with everett_card so others can route to you, and report done / blocked / needs-input with \
     everett_event.";

fn s(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}

fn hours_prop() -> Value {
    json!({"type": "number", "description": "Look-back window in hours (default 72).", "minimum": 0})
}

pub fn tools() -> &'static Vec<Value> {
    static TOOLS: OnceLock<Vec<Value>> = OnceLock::new();
    TOOLS.get_or_init(|| {
        vec![
            json!({
                "name": "everett_ls",
                "description": "List recent coding-agent sessions on this machine (all harnesses), newest first, each with its card: what it is working on, state, next step. For overview only -- to see what the parallel sessions are doing. Do not use it to hand-pick a target session to send to; call everett_send with just the task text and let it route, or use everett_route to preview the routing decision.",
                "inputSchema": {"type": "object", "properties": {
                    "hours": hours_prop(),
                    "harness": {"type": "string", "enum": ["claude", "codex", "omp", "pi", "hermes", "grok", "devin", "openai-dot", "grok-bot"], "description": "Only this harness."},
                }, "additionalProperties": false},
            }),
            json!({
                "name": "everett_route",
                "description": "Preview which existing session a request would continue, without sending anything. Returns decision SESSION (with the session), NEW (belongs to no session), or ASK (ambiguous; candidate sessions are given), plus a confidence and which router (jev or local) decided. Usually you want everett_send instead, which does this routing itself -- use everett_route only when you want to see the decision before committing to it.",
                "inputSchema": {"type": "object", "properties": {
                    "text": s("The request, as you would send it."),
                    "router": {"type": "string", "enum": ["local", "jev"], "description": "Default: jev if configured, else local."},
                    "session_id": s("Your own session id, to exclude it from routing."),
                    "hours": hours_prop(),
                }, "required": ["text"], "additionalProperties": false},
            }),
            json!({
                "name": "everett_send",
                "description": "Deliver a request to another session. This is the default way to hand off work: pass just `text` describing the task and Everett routes it for you (jev when configured, else the local matcher) -- do not call everett_ls and pick a target by hand. Only pass `to` (session id prefix, exact title, card name, or project folder) when the user named a specific session, or when replying to one; it skips routing entirely. The response always reports the route decision -- which router decided (jev/local), the chosen session, and its confidence -- so you can tell the user e.g. \"jev picked ...\". A SESSION decision is delivered. On ASK (ambiguous) nothing is sent; the candidate sessions come back for you to disambiguate, then call again with `to`. A session with a live harness process (someone has it open) gets the request in its inbox, injected at its next turn or tool call; pass `wait` to wait for the reply, or it arrives in YOUR inbox later. An idle session is resumed headless and its reply returned. To answer an Everett message you received, pass reply_to=<message id> and text. A NEW decision starts a new headless session only when spawn=true. It refuses to send to your own session and refuses requests forwarded more than 3 times.",
                "inputSchema": {"type": "object", "properties": {
                    "text": s("The request for the other session. Make it self-contained."),
                    "to": s("Target session: id prefix, exact title, card name, or project folder name."),
                    "hours": hours_prop(),
                    "spawn": {"type": "boolean", "description": "Allow starting a NEW session when routing says NEW.", "default": false},
                    "dir": s("Working directory for a spawned session."),
                    "harness": {"type": "string", "enum": crate::send::SPAWNABLE, "description": "Harness for a spawned session."},
                    "timeout": {"type": "number", "description": "Seconds to wait for the reply (default 300).", "minimum": 1},
                    "session_id": s("Your own session id, if Everett cannot detect it (see everett_whoami)."),
                    "mode": {"type": "string", "enum": crate::send::MODES, "description": "auto (default): inbox when the target has a live harness process, else headless resume."},
                    "wait": {"type": "number", "minimum": 0, "description": "Inbox mode: seconds to wait for the reply (default 0: it arrives in your inbox later)."},
                    "reply_to": s("Answer this Everett message id; the reply goes to its sender."),
                }, "required": ["text"], "additionalProperties": false},
            }),
            json!({
                "name": "everett_inbox",
                "description": "Read the Everett messages waiting for your session (requests, replies, events) and mark them delivered. Hooks normally inject these automatically; use this where they cannot.",
                "inputSchema": {"type": "object", "properties": {
                    "session_id": s("Your session id, if Everett cannot detect it."),
                    "peek": {"type": "boolean", "description": "Do not mark them delivered.", "default": false},
                }, "additionalProperties": false},
            }),
            json!({
                "name": "everett_event",
                "description": "Report what your session is doing so other sessions and the human stay aware: done (finished a task), blocked (cannot continue; say on what), needs-input (waiting on the human), or info. blocked and needs-input notify the human. Subscribers get it in their inbox. Call everett_subscribe to start following a session or project yourself.",
                "inputSchema": {"type": "object", "properties": {
                    "kind": {"type": "string", "enum": crate::events::KINDS},
                    "message": {"type": "string", "description": "One line, e.g. \"waiting on staging key\".", "maxLength": crate::events::MAX_TEXT},
                    "project": s("Project name (default: the current folder's project)."),
                    "session_id": s("Your session id, if Everett cannot detect it."),
                }, "required": ["kind", "message"], "additionalProperties": false},
            }),
            json!({
                "name": "everett_subscribe",
                "description": "Follow another session or a whole project: its events (done, blocked, needs-input, info) arrive in your inbox and are injected at your next turn or tool call.",
                "inputSchema": {"type": "object", "properties": {
                    "target": s("Session id prefix or name, project name, \"project:<name>\", or \"*\"."),
                    "unsubscribe": {"type": "boolean", "default": false},
                    "session_id": s("Your session id, if Everett cannot detect it."),
                }, "required": ["target"], "additionalProperties": false},
            }),
            json!({
                "name": "everett_learn",
                "description": "Push one durable fact (a decision, convention, gotcha, path, or command) into the shared core that every session starts from. Max 500 characters. Anything that looks like a secret is rejected. Use scope \"project\" for facts only this project needs.",
                "inputSchema": {"type": "object", "properties": {
                    "fact": {"type": "string", "description": "One self-contained fact.", "maxLength": crate::core::MAX_LEARNING},
                    "project": s("Project name (default: the current folder's project)."),
                    "scope": {"type": "string", "enum": ["global", "project"]},
                }, "required": ["fact"], "additionalProperties": false},
            }),
            json!({
                "name": "everett_core",
                "description": "Read the shared core: the global facts plus this (or the named) project's facts.",
                "inputSchema": {"type": "object", "properties": {
                    "project": s("Project name (default: the current folder's project)."),
                }, "additionalProperties": false},
            }),
            json!({
                "name": "everett_card",
                "description": "Write YOUR session's card, which other agents use to route work to you. Keep it short (50 words total). Rewrite it when your topic or state changes.",
                "inputSchema": {"type": "object", "properties": {
                    "what": s("What this session is about."),
                    "state": s("Where it stands now."),
                    "next": s("The next step."),
                    "session_id": s("Your session id, if Everett cannot detect it."),
                }, "required": ["what", "state", "next"], "additionalProperties": false},
            }),
            json!({
                "name": "everett_whoami",
                "description": "Your own session id and harness as Everett detects them, and how it detected them. If none is detected, pass session_id explicitly to everett_send and everett_card.",
                "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            }),
        ]
    })
}

fn tool_names() -> Vec<&'static str> {
    tools().iter().map(|t| t["name"].as_str().unwrap()).collect()
}

enum ToolFailure {
    Tool(EverettError),
    Params(String),
}

impl From<EverettError> for ToolFailure {
    fn from(e: EverettError) -> Self {
        ToolFailure::Tool(e)
    }
}

// ---- logging ----

fn log(event: &str, fields: Map<String, Value>) {
    let mut doc = Map::new();
    doc.insert("ts".into(), json!(crate::timefmt::strftime_local("%Y-%m-%dT%H:%M:%S", now())));
    doc.insert("event".into(), json!(event));
    for (k, v) in fields {
        let v = match v.as_str() {
            Some(s) if s.chars().count() > 200 => json!(format!("{}…", s.chars().take(200).collect::<String>())),
            _ => v,
        };
        doc.insert(k, v);
    }
    let path = crate::session::home().join(".everett").join("mcp.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all((serde_json::to_string(&doc).unwrap_or_default() + "\n").as_bytes());
    }
}

// ---- tools ----

fn brief(s: &Session) -> Map<String, Value> {
    let mut b = Map::new();
    b.insert("id".into(), json!(s.id));
    b.insert("harness".into(), json!(s.harness));
    b.insert("cwd".into(), json!(s.cwd));
    b.insert("running".into(), json!(s.running));
    b.insert(
        "minutes_ago".into(),
        json!(((now() - s.last_active).max(0.0) / 60.0) as i64),
    );
    b.insert("card".into(), json!(s.card));
    b.insert("card_source".into(), s.card_source.clone().map(|v| json!(v)).unwrap_or(Value::Null));
    b.insert("title".into(), json!(s.title));
    b.insert("first_request".into(), json!(s.first_user.chars().take(160).collect::<String>()));
    b.insert("last_request".into(), json!(s.last_user.chars().take(160).collect::<String>()));
    if !s.profile.is_empty() {
        b.insert("profile".into(), json!(s.profile));
    }
    if !s.source.is_empty() {
        b.insert("source".into(), json!(s.source));
    }
    if !s.state.is_empty() {
        b.insert("state".into(), json!(s.state));
        b.insert("state_kind".into(), json!(s.state_kind));
    }
    b
}

fn str_arg(args: &Map<String, Value>, key: &str, required: bool) -> std::result::Result<String, ToolFailure> {
    match args.get(key) {
        None | Some(Value::Null) => {
            if required {
                Err(ToolFailure::Params(format!("\"{}\" is required", key)))
            } else {
                Ok(String::new())
            }
        }
        Some(Value::String(v)) => {
            if required && v.trim().is_empty() {
                Err(ToolFailure::Params(format!("\"{}\" must not be empty", key)))
            } else {
                Ok(v.clone())
            }
        }
        _ => Err(ToolFailure::Params(format!("\"{}\" must be a string", key))),
    }
}

fn hours_arg(args: &Map<String, Value>) -> std::result::Result<f64, ToolFailure> {
    match args.get("hours") {
        None | Some(Value::Null) => Ok(72.0),
        Some(v) => {
            let ok = v.as_f64().map(|h| h.is_finite() && h >= 0.0).unwrap_or(false)
                && !v.is_boolean();
            if ok {
                Ok(v.as_f64().unwrap())
            } else {
                Err(ToolFailure::Params("\"hours\" must be a finite non-negative number".to_string()))
            }
        }
    }
}

fn err2(code: i32, msg: impl Into<String>) -> ToolFailure {
    ToolFailure::Tool(EverettError::new(code, msg.into()))
}

fn tool_ls(args: &Map<String, Value>) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let hours = hours_arg(args)?;
    let harness = str_arg(args, "harness", false)?;
    let sessions = crate::registry::scan(hours, false, Some(crate::registry::CAP), &harness);
    let me = caller_identity()
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let list: Vec<Value> = sessions
        .iter()
        .map(|s| {
            let mut b = brief(s);
            if !me.is_empty() && s.id == me {
                b.insert("you".into(), json!(true));
            }
            Value::Object(b)
        })
        .collect();
    let mut out = Map::new();
    out.insert("sessions".into(), json!(list));
    Ok(out)
}

fn tool_route(args: &Map<String, Value>) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let text = str_arg(args, "text", true)?;
    let router = str_arg(args, "router", false)?;
    let caller = str_arg(args, "session_id", false)?.pipe(|c| {
        if c.is_empty() {
            caller_identity().get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else {
            c
        }
    });
    let sessions = crate::registry::scan(hours_arg(args)?, false, Some(crate::registry::CAP), "");
    let r = crate::route::route(
        &text,
        &sessions,
        if router.is_empty() { None } else { Some(&router) },
        if caller.is_empty() { None } else { Some(&caller) },
    )
    .map_err(ToolFailure::Tool)?;
    let mut out = Map::new();
    for k in ["decision", "confidence", "router", "suggested", "candidates", "command"] {
        if let Some(v) = r.get(k) {
            if !v.is_null() {
                out.insert(k.to_string(), v.clone());
            }
        }
    }
    if let Some(sess) = r.get("session").and_then(|v| v.as_object()) {
        out.insert("session".into(), Value::Object(brief(&Session::from_dict(sess))));
    }
    Ok(out)
}

fn tool_send(args: &Map<String, Value>) -> std::result::Result<Map<String, Value>, ToolFailure> {
    use crate::send::{send, send_inbox, spawn};
    let text = str_arg(args, "text", true)?;
    let to = str_arg(args, "to", false)?;
    let hours = hours_arg(args)?;
    let spawn_ok = args.get("spawn").map(|v| v.as_bool()).unwrap_or(Some(false)).unwrap_or(false);
    if args.get("spawn").map(|v| !v.is_boolean()).unwrap_or(false) {
        return Err(ToolFailure::Params("\"spawn\" must be a boolean".to_string()));
    }
    let timeout = args.get("timeout").and_then(|v| v.as_f64()).unwrap_or(300.0);
    if args.get("timeout").map(|v| v.is_boolean() || v.as_f64().map(|t| t <= 0.0).unwrap_or(true)).unwrap_or(false) {
        return Err(ToolFailure::Params("\"timeout\" must be a positive number".to_string()));
    }
    let caller = str_arg(args, "session_id", false)?.pipe(|c| {
        if c.is_empty() {
            caller_identity().get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else {
            c
        }
    });
    let mode = str_arg(args, "mode", false)?;
    let mode = if mode.is_empty() { "auto".to_string() } else { mode };
    if !crate::send::MODES.contains(&mode.as_str()) {
        return Err(ToolFailure::Params(format!("\"mode\" must be one of {}", crate::send::MODES.join(", "))));
    }
    let wait = args.get("wait").and_then(|v| v.as_f64()).unwrap_or(0.0);
    if args.get("wait").map(|v| v.is_boolean() || v.as_f64().map(|w| w < 0.0).unwrap_or(true)).unwrap_or(false) {
        return Err(ToolFailure::Params("\"wait\" must be a non-negative number".to_string()));
    }
    let reply_to = str_arg(args, "reply_to", false)?;
    let exact_ids = std::env::var("EVERETT_GATEWAY_EXACT_IDS").as_deref() == Ok("1");
    if exact_ids && (spawn_ok || mode != "inbox" || (to.is_empty() && reply_to.is_empty())) {
        return Err(err2(2, "Gateway delivery requires inbox mode and an explicit exact destination or reply."));
    }
    if !reply_to.is_empty() {
        let result = crate::send::reply(&reply_to, &text, if caller.is_empty() { None } else { Some(&caller) }).map_err(ToolFailure::Tool)?;
        let mut out = Map::new();
        out.insert("delivered".into(), json!(true));
        for (k, v) in result {
            out.insert(k, v);
        }
        return Ok(out);
    }
    let env = crate::send::hop_env().map_err(ToolFailure::Tool)?;
    let mut decision = Map::new();
    let session;
    if !to.is_empty() {
        if spawn_ok {
            return Err(ToolFailure::Params("\"to\" and \"spawn\" are exclusive".to_string()));
        }
        let sessions = crate::registry::scan(hours, true, None, "");
        let found = if exact_ids {
            let mut matches = sessions.iter().filter(|s| s.id == to);
            match (matches.next(), matches.next()) {
                (Some(s), None) => Ok(s.clone()),
                _ => Err(crate::error::EverettError::new(2, "Gateway destination must match one exact session id.")),
            }
        } else {
            crate::registry::find(&to, &sessions)
        }.map_err(|e| {
            err2(2, format!(
                "{} In MCP, use everett_ls with a larger hours window (e.g. 168), then pass its exact session id and the same hours to everett_send.",
                e.message
            ))
        })?;
        session = found;
        decision.insert("decision".into(), json!("SESSION"));
        decision.insert("router".into(), json!("direct"));
        decision.insert("confidence".into(), json!(1.0));
    } else {
        let sessions = crate::registry::scan(hours, false, Some(crate::registry::CAP), "");
        let r = crate::route::route(
            &text,
            &sessions,
            None,
            if caller.is_empty() { None } else { Some(&caller) },
        )
        .map_err(ToolFailure::Tool)?;
        decision.insert("decision".into(), r.get("decision").cloned().unwrap_or(Value::Null));
        decision.insert("confidence".into(), r.get("confidence").cloned().unwrap_or(Value::Null));
        decision.insert("router".into(), r.get("router").cloned().unwrap_or(Value::Null));
        if r.get("decision").and_then(|v| v.as_str()) == Some("NEW") {
            if !spawn_ok
                || r.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0) < crate::route::MIN_CONFIDENCE
            {
                let mut out = decision.clone();
                out.insert("delivered".into(), json!(false));
                out.insert(
                    "note".into(),
                    json!("No existing session fits. Nothing was sent; call again with spawn=true to start one."),
                );
                return Ok(out);
            }
            let harness = str_arg(args, "harness", false)?.pipe(|h| {
                if h.is_empty() { crate::route::default_harness() } else { h }
            });
            let cwd = match args.get("dir").and_then(|v| v.as_str()) {
                Some(d) => crate::paths::realpath(&crate::paths::expanduser(d)).to_string_lossy().to_string(),
                None => {
                    let bd = crate::route::best_dir(&text, &sessions);
                    if !bd.is_empty() {
                        bd
                    } else {
                        std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
                    }
                }
            };
            let result = spawn(&harness, &text, &cwd, timeout, Some(&env)).map_err(ToolFailure::Tool)?;
            let mut out = decision.clone();
            out.insert("delivered".into(), json!(true));
            out.insert("spawned".into(), json!(true));
            out.insert("harness".into(), json!(harness));
            out.insert("cwd".into(), json!(cwd));
            out.insert("session_id".into(), json!(result.session_id));
            out.insert("reply".into(), json!(result.reply));
            return Ok(out);
        }
        if r.get("decision").and_then(|v| v.as_str()) != Some("SESSION") {
            let mut out = decision.clone();
            out.insert("delivered".into(), json!(false));
            out.insert("suggested".into(), r.get("suggested").cloned().unwrap_or(Value::Null));
            out.insert("candidates".into(), r.get("candidates").cloned().unwrap_or(json!([])));
            out.insert(
                "note".into(),
                json!("Ambiguous. Nothing was sent; pass `to` with the session you mean."),
            );
            return Ok(out);
        }
        let Some(session_obj) = r.get("session").and_then(|v| v.as_object()) else {
            return Err(err2(6, "router returned SESSION without a session object"));
        };
        session = Session::from_dict(session_obj);
    }
    refuse_self(&session, if caller.is_empty() { None } else { Some(&caller) }).map_err(ToolFailure::Tool)?;
    if delivery_mode(&session, &mode, None)? == "inbox" {
        let result = send_inbox(&session, &text, wait, if caller.is_empty() { None } else { Some(&caller) }, 1.0).map_err(ToolFailure::Tool)?;
        let note = if crate::adapters::external::is_external(&session.harness) {
            "Queued for the external agent to poll through MCP. \
             No provider wake-up or consumption is confirmed. Read replies with everett_inbox.".to_string()
        } else {
            result.get("pickup_note").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!(
                "Queued; it is injected into that live session at its next turn or tool call.{}",
                if result.get("reply").map(|r| !r.is_null()).unwrap_or(false) {
                    ""
                } else {
                    " Its reply will arrive in your inbox (injected by your hooks, or read it with everett_inbox)."
                }
            ))
        };
        let mut out = decision.clone();
        out.insert("delivered".into(), json!(true));
        out.insert("session".into(), Value::Object(brief(&session)));
        for (k, v) in &result {
            out.insert(k.clone(), v.clone());
        }
        out.insert("note".into(), json!(note));
        return Ok(out);
    }
    crate::send::command_for(&session, &text).map_err(ToolFailure::Tool)?;
    let result = send(&session, &text, timeout, crate::send::IDLE_WAIT, Some(&env)).map_err(ToolFailure::Tool)?;
    let mut out = decision;
    out.insert("delivered".into(), json!(true));
    out.insert("session".into(), Value::Object(brief(&session)));
    out.insert("reply".into(), json!(result.reply));
    Ok(out)
}

fn tool_learn(args: &Map<String, Value>) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let fact = str_arg(args, "fact", true)?;
    let project = str_arg(args, "project", false)?;
    let scope = str_arg(args, "scope", false)?;
    let entry = crate::core::learn(
        &fact,
        if project.is_empty() { None } else { Some(&project) },
        if scope.is_empty() { None } else { Some(&scope) },
        None,
    )
    .map_err(ToolFailure::Tool)?;
    let mut out = Map::new();
    out.insert("learned".into(), json!(true));
    out.insert("scope".into(), entry.get("scope").cloned().unwrap_or(Value::Null));
    out.insert("project".into(), entry.get("project").cloned().unwrap_or(Value::Null));
    out.insert(
        "note".into(),
        json!("Queued in the inbox; it reaches every session after the next `everett trunk merge`."),
    );
    Ok(out)
}

fn tool_core(args: &Map<String, Value>) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let project = str_arg(args, "project", false)?;
    let text = if !project.is_empty() {
        [crate::core::global_path(), crate::core::project_path(&project)]
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    } else {
        crate::core::context(
            &std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        )
    };
    let mut out = Map::new();
    out.insert(
        "core".into(),
        json!(if text.is_empty() { "(the shared core is empty)".to_string() } else { text }),
    );
    out.insert("pending_learnings".into(), json!(crate::core::read_inbox().len()));
    Ok(out)
}

fn tool_card(args: &Map<String, Value>) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let mut parts = Map::new();
    for k in ["what", "state", "next"] {
        let v = str_arg(args, k, true)?;
        parts.insert(k.to_string(), json!(v.split_whitespace().collect::<Vec<_>>().join(" ")));
    }
    let sid = str_arg(args, "session_id", false)?.pipe(|c| {
        if c.is_empty() {
            caller_identity().get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else {
            c
        }
    });
    if sid.is_empty() {
        return Err(ToolFailure::Tool(EverettError::new(
            2,
            "Cannot tell which session you are. Pass session_id (your harness session id).",
        )));
    }
    if !crate::inbox::valid_id(&sid) {
        return Err(ToolFailure::Params("\"session_id\" is not a valid session id".to_string()));
    }
    let body = format!(
        "What: {}\nState: {}\nNext: {}\n",
        crate::hooks_common::word_limit(parts["what"].as_str().unwrap_or(""), 16),
        crate::hooks_common::word_limit(parts["state"].as_str().unwrap_or(""), 14),
        crate::hooks_common::word_limit(parts["next"].as_str().unwrap_or(""), 14)
    );
    let Some(path) = crate::cards::card_path(&sid) else {
        return Err(err2(2, format!("not a valid session id for a card: {sid:?}")));
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_file_name(format!(".{}.tmp", path.file_name().unwrap_or_default().to_string_lossy()));
    std::fs::write(&tmp, &body).map_err(|e| err2(2, e.to_string()))?;
    std::fs::rename(&tmp, &path).map_err(|e| err2(2, e.to_string()))?;
    let mut out = Map::new();
    out.insert("written".into(), json!(path.to_string_lossy()));
    out.insert("card".into(), json!(body.trim()));
    Ok(out)
}

fn tool_inbox(args: &Map<String, Value>) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let sid = str_arg(args, "session_id", false)?.pipe(|c| {
        if c.is_empty() {
            caller_identity().get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else {
            c
        }
    });
    if sid.is_empty() {
        return Err(ToolFailure::Tool(EverettError::new(
            2,
            "Cannot tell which session you are. Pass session_id (your harness session id).",
        )));
    }
    if !crate::inbox::valid_id(&sid) {
        return Err(ToolFailure::Params("\"session_id\" is not a valid session id".to_string()));
    }
    let peek = args.get("peek").map(|v| v.as_bool()).unwrap_or(Some(false)).unwrap_or(false);
    if args.get("peek").map(|v| !v.is_boolean()).unwrap_or(false) {
        return Err(ToolFailure::Params("\"peek\" must be a boolean".to_string()));
    }
    let items = crate::inbox::pending(&sid, None);
    let mut messages: Vec<Value> = Vec::new();
    let mut out = Map::new();
    out.insert("session".into(), json!(sid));
    out.insert("messages".into(), json!(messages));
    let size_of = |out: &Map<String, Value>| serde_json::to_vec(&out).map(|v| v.len()).unwrap_or(usize::MAX);
    for m in &items {
        let mut msg = Map::new();
        for k in ["id", "kind", "from", "from_harness", "from_card", "text", "reply_to", "hops", "ts"] {
            msg.insert(k.to_string(), m.get(k).cloned().unwrap_or(Value::Null));
        }
        messages.push(Value::Object(msg));
        out.insert("messages".into(), json!(messages));
        if size_of(&out) > MAX_TOOL_RESULT {
            let last = messages.pop().unwrap();
            out.insert("messages".into(), json!(messages));
            if messages.is_empty() {
                // The first pending message alone exceeds the limit: deliver it
                // truncated rather than leaving this inbox stuck forever.
                let mut msg = last.as_object().cloned().unwrap_or_default();
                let text = msg.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let mut keep = text.chars().count();
                loop {
                    msg.insert("text".into(), json!(text.chars().take(keep).collect::<String>()));
                    msg.insert("truncated".into(), json!(true));
                    messages.push(Value::Object(msg.clone()));
                    out.insert("messages".into(), json!(messages));
                    if size_of(&out) <= MAX_TOOL_RESULT || keep == 0 {
                        break;
                    }
                    messages.pop();
                    keep = keep * 9 / 10;
                }
                out.insert("messages".into(), json!(messages));
            }
            break;
        }
    }
    if !peek {
        let ids: Vec<&str> = messages
            .iter()
            .filter_map(|m| m.get("id").and_then(|v| v.as_str()))
            .collect();
        crate::inbox::mark_done(&sid, &ids);
    }
    Ok(out)
}

fn tool_event(args: &Map<String, Value>) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let kind = str_arg(args, "kind", true)?;
    let message = str_arg(args, "message", true)?;
    let project = str_arg(args, "project", false)?;
    let sid = str_arg(args, "session_id", false)?.pipe(|c| {
        if c.is_empty() {
            caller_identity().get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else {
            c
        }
    });
    if !sid.is_empty() && !crate::inbox::valid_id(&sid) {
        return Err(ToolFailure::Params("\"session_id\" is not a valid session id".to_string()));
    }
    let event = crate::events::record(
        &kind,
        &message,
        if sid.is_empty() { None } else { Some(&sid) },
        if project.is_empty() { None } else { Some(&project) },
        "",
        None,
        "manual",
        None,
    )
    .map_err(ToolFailure::Tool)?;
    let mut out = Map::new();
    out.insert("recorded".into(), json!(true));
    out.insert("event".into(), event.map(Value::Object).unwrap_or(Value::Null));
    if sid.is_empty() {
        out.insert(
            "note".into(),
            json!("No session detected: the event is logged but not attached to a card. Pass session_id."),
        );
    }
    Ok(out)
}

fn tool_subscribe(args: &Map<String, Value>) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let sid = str_arg(args, "session_id", false)?.pipe(|c| {
        if c.is_empty() {
            caller_identity().get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else {
            c
        }
    });
    let remove = args.get("unsubscribe").map(|v| v.as_bool()).unwrap_or(Some(false)).unwrap_or(false);
    if args.get("unsubscribe").map(|v| !v.is_boolean()).unwrap_or(false) {
        return Err(ToolFailure::Params("\"unsubscribe\" must be a boolean".to_string()));
    }
    let target = crate::events::resolve_target(&str_arg(args, "target", true)?);
    let current = crate::events::subscribe(&sid, &target, remove).map_err(ToolFailure::Tool)?;
    let mut out = Map::new();
    out.insert("subscriber".into(), json!(sid));
    out.insert("following".into(), json!(current));
    Ok(out)
}

fn tool_whoami(_args: &Map<String, Value>) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let ident = caller_identity();
    let mut out = ident.clone();
    let hops = std::env::var("EVERETT_HOPS").unwrap_or_else(|_| "0".to_string());
    out.insert("hops".into(), json!(hops.parse::<i64>().unwrap_or(0)));
    out.insert(
        "cwd".into(),
        json!(std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()),
    );
    let has_session = ident.get("session_id").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
    out.insert(
        "note".into(),
        json!(if has_session {
            ""
        } else {
            "Not detected. Claude Code, Codex, Hermes, and Pi expose it; others must pass session_id explicitly."
        }),
    );
    Ok(out)
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl Pipe for String {}

fn call_handler(name: &str, args: &Map<String, Value>) -> std::result::Result<Map<String, Value>, ToolFailure> {
    match name {
        "everett_ls" => tool_ls(args),
        "everett_route" => tool_route(args),
        "everett_send" => tool_send(args),
        "everett_learn" => tool_learn(args),
        "everett_core" => tool_core(args),
        "everett_card" => tool_card(args),
        "everett_whoami" => tool_whoami(args),
        "everett_inbox" => tool_inbox(args),
        "everett_event" => tool_event(args),
        "everett_subscribe" => tool_subscribe(args),
        _ => Err(ToolFailure::Params(format!("unknown tool: {}", name))),
    }
}

fn call_tool(params: &Map<String, Value>) -> std::result::Result<Map<String, Value>, Value> {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if std::env::var("EVERETT_MCP_PANIC_TEST").as_deref() == Ok("1") && name == "everett_ls" {
        panic!("injected test panic"); // tests: a panicking tool must not kill the server
    }
    if name.is_empty() || !tool_names().contains(&name) {
        return Err(json!({"code": INVALID_PARAMS, "message": format!("unknown tool: {}", if name.is_empty() { Value::Null.to_string() } else { name.to_string() })}));
    }
    let args = match params.get("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(a)) => a.clone(),
        _ => return Err(json!({"code": INVALID_PARAMS, "message": "\"arguments\" must be an object"})),
    };
    let schema = tools()
        .iter()
        .find(|t| t["name"].as_str() == Some(name))
        .and_then(|t| t["inputSchema"].as_object())
        .cloned()
        .unwrap_or_default();
    for key in schema.get("required").and_then(|r| r.as_array()).cloned().unwrap_or_default() {
        if let Some(k) = key.as_str() {
            if !args.contains_key(k) {
                return Err(json!({"code": INVALID_PARAMS, "message": format!("\"{}\" is required", k)}));
            }
        }
    }
    let properties = schema.get("properties").and_then(|p| p.as_object()).cloned().unwrap_or_default();
    for (key, value) in &args {
        let Some(spec) = properties.get(key).and_then(|v| v.as_object()) else {
            return Err(json!({"code": INVALID_PARAMS, "message": "unexpected argument; use the tool inputSchema"}));
        };
        match spec.get("type").and_then(|v| v.as_str()) {
            Some("string") => {
                if !value.is_string() {
                    return Err(json!({"code": INVALID_PARAMS, "message": format!("\"{}\" must be a string", key)}));
                }
            }
            Some("boolean") => {
                if !value.is_boolean() {
                    return Err(json!({"code": INVALID_PARAMS, "message": format!("\"{}\" must be a boolean", key)}));
                }
            }
            Some("number")
                if (value.as_f64().map(|n| !n.is_finite()).unwrap_or(true) || value.is_boolean()) => {
                    return Err(json!({"code": INVALID_PARAMS, "message": format!("\"{}\" must be a finite number", key)}));
                }
            _ => {}
        }
        if let Some(values) = spec.get("enum").and_then(|e| e.as_array()) {
            if !values.contains(value) {
                let opts = values.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ");
                return Err(json!({"code": INVALID_PARAMS, "message": format!("\"{}\" must be one of {}", key, opts)}));
            }
        }
        if let Some(min) = spec.get("minimum").and_then(|m| m.as_f64()) {
            if value.as_f64().map(|v| v < min).unwrap_or(false) {
                return Err(json!({"code": INVALID_PARAMS, "message": format!("\"{}\" must be at least {}", key, crate::fmt::g(min))}));
            }
        }
        if let Some(max) = spec.get("maxLength").and_then(|m| m.as_i64()) {
            if value.as_str().map(|s| s.chars().count() > max as usize).unwrap_or(false) {
                return Err(json!({"code": INVALID_PARAMS, "message": format!("\"{}\" exceeds {} characters", key, max)}));
            }
        }
    }
    let started = now();
    let mut log_fields = Map::new();
    log_fields.insert("tool".into(), json!(name));
    let mut keys: Vec<&String> = args.keys().collect();
    keys.sort();
    log_fields.insert("args".into(), json!(format!("{:?}", keys)));
    log_fields.insert(
        "caller".into(),
        json!(caller_identity().get("session_id").and_then(|v| v.as_str()).unwrap_or("")),
    );
    log("call", log_fields);
    let (data, is_error) = match call_handler(name, &args) {
        Ok(d) => (d, false),
        Err(ToolFailure::Tool(e)) => {
            let mut d = Map::new();
            d.insert("error".into(), json!(e.message));
            (d, true)
        }
        Err(ToolFailure::Params(m)) => return Err(json!({"code": INVALID_PARAMS, "message": m})),
    };
    let mut done = Map::new();
    done.insert("tool".into(), json!(name));
    done.insert("ok".into(), json!(!is_error));
    done.insert("ms".into(), json!(((now() - started) * 1000.0) as i64));
    log("result", done);
    let text = if is_error {
        data.get("error").and_then(|v| v.as_str()).unwrap_or("").to_string()
    } else {
        serde_json::to_string_pretty(&Value::Object(data.clone())).unwrap_or_default()
    };
    let mut out = Map::new();
    out.insert(
        "content".into(),
        json!([{"type": "text", "text": text}]),
    );
    out.insert("isError".into(), json!(is_error));
    if !is_error {
        out.insert("structuredContent".into(), Value::Object(data));
    }
    Ok(out)
}

fn error(msg_id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": msg_id, "error": {"code": code, "message": message}})
}

fn result(msg_id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": msg_id, "result": result})
}

/// One JSON-RPC message in, one response out (None for notifications).
pub fn handle(message: &Value) -> Option<Value> {
    let Some(map) = message.as_object() else {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        return Some(error(&id, INVALID_REQUEST, "Invalid Request"));
    };
    if map.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") || !map.get("method").map(|m| m.is_string()).unwrap_or(false) {
        return Some(error(&map.get("id").cloned().unwrap_or(Value::Null), INVALID_REQUEST, "Invalid Request"));
    }
    let method = map.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let msg_id = map.get("id").cloned().unwrap_or(Value::Null);
    let is_notification = !map.contains_key("id");
    let params = map.get("params").cloned().unwrap_or(Value::Object(Map::new()));
    if !params.is_object() {
        return if is_notification { None } else { Some(error(&msg_id, INVALID_PARAMS, "params must be an object")) };
    }
    if is_notification {
        return None;
    }
    let params = params.as_object().unwrap();
    match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
            let version = if PROTOCOL_VERSIONS.contains(&asked) { asked } else { PROTOCOL_VERSIONS[0] };
            let mut f = Map::new();
            f.insert("version".into(), json!(version));
            log("initialize", f);
            let mut capabilities = json!({"tools": {"listChanged": false}});
            if std::env::var("EVERETT_GATEWAY_EXACT_IDS").as_deref() == Ok("1") {
                capabilities["experimental"] = json!({"everettExactDestinationIds": {"enforced": true}});
            }
            Some(result(&msg_id, json!({
                "protocolVersion": version,
                "capabilities": capabilities,
                "serverInfo": {"name": "everett", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            })))
        }
        "ping" => Some(result(&msg_id, json!({}))),
        "tools/list" => Some(result(&msg_id, json!({"tools": tools()}))),
        "tools/call" => {
            // A panic in one tool must not kill the stdio server.
            let tool = params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call_tool(params)));
            match outcome {
                Ok(Ok(out)) => Some(result(&msg_id, Value::Object(out))),
                Ok(Err(e)) => Some(error(&msg_id, e["code"].as_i64().unwrap_or(INTERNAL_ERROR), e["message"].as_str().unwrap_or("error"))),
                Err(_) => {
                    let mut f = Map::new();
                    f.insert("tool".into(), json!(tool));
                    log("panic", f);
                    Some(error(&msg_id, INTERNAL_ERROR, "internal error"))
                }
            }
        }
        _ => Some(error(&msg_id, METHOD_NOT_FOUND, &format!("Method not found: {}", method))),
    }
}

pub fn serve() -> i32 {
    let mut f = Map::new();
    f.insert("pid".into(), json!(std::process::id()));
    f.insert(
        "cwd".into(),
        json!(std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()),
    );
    f.insert("hops".into(), json!(std::env::var("EVERETT_HOPS").unwrap_or_else(|_| "0".to_string())));
    log("start", f);
    let stdin = std::io::stdin();
    let reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in reader.lines() {
        let line = line.unwrap_or_default();
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(line) {
            Ok(m) => m,
            Err(_) => {
                let _ = writeln!(out, "{}", serde_json::to_string(&error(&Value::Null, PARSE_ERROR, "Parse error")).unwrap());
                let _ = out.flush();
                continue;
            }
        };
        if let Value::Array(batch) = &message {
            if batch.is_empty() {
                let _ = writeln!(out, "{}", serde_json::to_string(&json!([error(&Value::Null, INVALID_REQUEST, "Invalid Request")])).unwrap());
                let _ = out.flush();
                continue;
            }
            let responses: Vec<Value> = batch.iter().filter_map(handle).collect();
            if !responses.is_empty() {
                let _ = writeln!(out, "{}", serde_json::to_string(&responses).unwrap());
                let _ = out.flush();
            }
            continue;
        }
        if let Some(response) = handle(&message) {
            let _ = writeln!(out, "{}", serde_json::to_string(&response).unwrap());
            let _ = out.flush();
        }
    }
    log("stop", Map::new());
    0
}
