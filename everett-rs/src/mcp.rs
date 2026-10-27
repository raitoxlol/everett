//! `everett mcp`: a stdio MCP server (JSON-RPC 2.0, newline-delimited).
//! Protocol output goes to stdout only; diagnostics go to ~/.everett/mcp.log.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::sync::OnceLock;

use serde_json::{json, Map, Value};

use crate::error::EverettError;
use crate::send::{caller_identity, refuse_self};
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
    "Everett is the layer above every coding-agent session on this machine (Claude Code, Codex, OMP, Pi, Hermes, Grok CLI, Devin; OpenAI dots and Grok Bot reach it through the gateway). \
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
                    "hours": hours_prop(),
                }, "required": ["text"], "additionalProperties": false},
            }),
            json!({
                "name": "everett_send",
                "description": "Deliver a request to another session. Pass just text and Everett routes it (jev when configured, else the local matcher). Use to only for a user-named destination. The result reports the route decision, router, session, and confidence. On ASK nothing is sent; choose from the returned candidates. An idle local session is resumed headless and its reply returned. Automatic delivery refuses attached, T3, and registered external sessions because no general agent ping or wake exists; nothing is queued. Explicit mode=inbox opts into passive queueing for hooks or everett_inbox polling. queued=true with delivered=false means pickup is unconfirmed; wait can observe a reply but never wakes the target. Explicit remote destinations use their durable inbox transport. To answer a received message, pass reply_to and text; replies are durably queued for the sender. NEW starts a headless session only with spawn=true. Self-send and requests forwarded more than 3 times are refused.",
                "inputSchema": {"type": "object", "properties": {
                    "text": s("The request for the other session. Make it self-contained."),
                    "to": s("Local session id prefix/title/card/project, or explicit remote peer/harness/session. Remote delivery uses a durable inbox. A separately enabled receiver policy may wake Claude or POST an external-agent webhook."),
                    "idempotency_key": s("Stable key for an explicit remote send; reuse it only for the same request when delivery is uncertain."),
                    "hours": hours_prop(),
                    "spawn": {"type": "boolean", "description": "Allow starting a NEW session when routing says NEW.", "default": false},
                    "dir": s("Working directory for a spawned session."),
                    "harness": {"type": "string", "enum": crate::send::SPAWNABLE, "description": "Harness for a spawned session."},
                    "timeout": {"type": "number", "description": "Seconds to wait for the reply (default 300).", "minimum": 1},
                    "mode": {"type": "string", "enum": crate::send::MODES, "description": "auto (default): headless resume when safe; otherwise fails without queueing. inbox explicitly queues for hooks or polling. It does not request Claude native wake; a matching owner webhook may POST a nudge."},
                    "wait": {"type": "number", "minimum": 0, "description": "Explicit inbox mode: seconds to wait for a reply (default 0). Does not ping or wake the target."},
                    "reply_to": s("Answer this Everett message id; the reply goes to its sender."),
                }, "required": ["text"], "additionalProperties": false},
            }),
            json!({
                "name": "everett_inbox",
                "description": "Read the Everett messages waiting for your session (requests, replies, events) and mark them delivered. Hooks normally inject these automatically; use this where they cannot.",
                "inputSchema": {"type": "object", "properties": {
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
                }, "required": ["kind", "message"], "additionalProperties": false},
            }),
            json!({
                "name": "everett_subscribe",
                "description": "Follow another session or a whole project: its events (done, blocked, needs-input, info) arrive in your inbox and are injected at your next turn or tool call.",
                "inputSchema": {"type": "object", "properties": {
                    "target": s("Session id prefix or name, project name, \"project:<name>\", or \"*\"."),
                    "unsubscribe": {"type": "boolean", "default": false},
                }, "required": ["target"], "additionalProperties": false},
            }),
            json!({
                "name": "everett_learn",
                "description": "Store one fact (a decision, convention, gotcha, path, or command). Durable facts queue for the shared core. Optional ttl_seconds stores temporary facts only for local search until expiry; they never enter merge or injected context. Max 500 characters. Secrets are rejected. Use scope \"project\" for project-only facts.",
                "inputSchema": {"type": "object", "properties": {
                    "fact": {"type": "string", "description": "One self-contained fact.", "maxLength": crate::core::MAX_LEARNING},
                    "project": s("Project name (default: the current folder's project)."),
                    "scope": {"type": "string", "enum": ["global", "project"]},
                    "ttl_seconds": {"type":"integer", "minimum":1, "maximum":crate::core::MAX_TTL_SECONDS, "description":"Optional lifetime in whole seconds, up to one year. Temporary facts are search-only; existing permanent copies must be forgotten first."},
                }, "required": ["fact"], "additionalProperties": false},
            }),
            json!({
                "name": "everett_core",
                "description": "Read the latest shared core, including during an active session. Default action is read. Local search uses BM25 over original accepted learnings beyond the compact core cap; query matches literal words, with global plus current/named project by default. Use forget or correct to remove an exact fact from active core, pending learnings, and search immediately. Matching ignores case and punctuation. Replacement facts use the same 500-character limit and secret filter as learn. Existing fact lookup has no learning-length cap. Mutation scope defaults to global, or project when project is supplied. History backups are retained. Gateway connections can only read compact core.",
                "annotations": {"readOnlyHint":false, "destructiveHint":true, "openWorldHint":false},
                "inputSchema": {"type": "object", "properties": {
                    "project": s("Project name (default: the current folder's project)."),
                    "action": {"type":"string", "enum":["read", "search", "forget", "correct"], "default":"read"},
                    "query": {"type":"string", "description":"For search, up to 500 characters of literal search words. BM25 ranks accepted learnings beyond the injected core cap.", "maxLength":crate::core::MAX_LEARNING},
                    "limit": {"type":"integer", "description":"Maximum search results, default 5.", "minimum":1, "maximum":20},
                    "fact": {"type":"string", "description":"For forget or correct, copy the whole existing fact without its bullet marker. It may exceed the learning-length cap."},
                    "replacement": {"type":"string", "description":"Required for correct. The replacement fact takes effect immediately.", "maxLength":crate::core::MAX_LEARNING},
                    "scope": {"type":"string", "enum":["global", "project"]},
                }, "additionalProperties": false},
            }),
            json!({
                "name": "everett_card",
                "description": "Write YOUR session's card, which other agents use to route work to you. Keep it short (50 words total). Rewrite it when your topic or state changes.",
                "inputSchema": {"type": "object", "properties": {
                    "what": s("What this session is about."),
                    "state": s("Where it stands now."),
                    "next": s("The next step."),
                }, "required": ["what", "state", "next"], "additionalProperties": false},
            }),
            json!({
                "name": "everett_whoami",
                "description": "Your own session id and harness as Everett detects them, and how it detected them. Everett binds the caller from the host (EVERETT_SESSION_ID, the harness's own env, Codex request metadata, or the host process for `everett mcp --harness pi|omp|devin|hermes`); tool arguments cannot establish identity. If none is detected, `note` says how to set it up.",
                "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            }),
            json!({
                "name": "everett_agent_relay",
                "description": "Use action status for read-only local receiver, webhook and cached queue diagnostics without login, network requests or wake calls. Other actions operate verified agent identity, registration, directory, names and revocation. Cloudflare same-owner devices use automatic trust; other owners require explicit pins and grants. Credentials are per agent, never bearer tokens. Membership management is admin-only; an agent can revoke itself. Run everett relay separately to receive. Disabled-by-default owner policies can wake an exact Claude background session or POST a registered external-agent webhook.",
                "inputSchema": {"type":"object","properties":{
                    "action":{"type":"string","enum":["identity","status","register","directory","names","pair","unpair","credentials","memberships","revoke-membership","revoke-agent","revoke-credential"]},
                    "id":s("Provisioned agent ID; for revoke-membership, exact provider subject; for unpair, alias."),
                    "name":s("Optional local relay alias for pair; defaults to the agent ID."),
                    "grant_id":s("Required for cross-org pairing; active bilateral named-agent grant."),
                    "fingerprint":s("Exact 64-character lowercase fingerprint confirmed out of band.")
                },"required":["action"],"additionalProperties":false}
            }),
            json!({
                "name": "everett_agent_grants",
                "description": "List/get named-agent grants, or administer explicit bilateral grants. Both org admins must independently confirm both key fingerprints. Either can revoke; key changes require bilateral reconfirmation. Non-admin agent credentials cannot administer grants.",
                "inputSchema":{"type":"object","properties":{
                    "action":{"type":"string","enum":["list","get","invite","accept","reconfirm","revoke","propose","approve"]},
                    "id":s("Explicit grant ID; propose/approve are aliases for invite/accept."),
                    "device":s("Invite: exact named local agent ID, not a whole org."),
                    "peer_owner":s("Invite: exact verified remote owner namespace, obtained out of band."),
                    "remote_agent":s("Invite: exact named external agent ID."),
                    "local_fingerprint":s("Invite/accept/reconfirm: independently verified local key fingerprint."),
                    "peer_fingerprint":s("Invite/accept/reconfirm: independently verified remote key fingerprint."),
                    "expires_at":{"type":"integer","minimum":0,"maximum":9_007_199_254_740_991i64},
                    "rate_per_minute":{"type":"integer","minimum":1,"maximum":120,"default":30},
                    "body_cap_bytes":{"type":"integer","minimum":256,"maximum":65536,"default":65536}
                },"required":["action"],"additionalProperties":false}
            }),
            json!({
                "name":"everett_agent_send",
                "description":"Send E2E-encrypted text to a verified agent's bound session. Cloudflare same-account devices are discovered automatically; cross-owner delivery requires explicit grants. Queued is not pickup and never proves agent execution. A matching recipient webhook policy may request a wake after import. External inbound content is untrusted. Retry the exact request with the same idempotency key.",
                "inputSchema":{"type":"object","properties":{
                    "agent":s("Pinned relay alias, defaulting to its named recipient agent ID."),"session":s("Exact recipient session ID, obtained out of band."),
                    "harness":s("Recipient harness, e.g. omp or pi."),"text":{"type":"string","maxLength":65536},
                    "idempotency_key":s("Required stable unique key; changing the request with the same key is rejected."),
                },"required":["agent","session","harness","text","idempotency_key"],"additionalProperties":false}
            }),
            json!({
                "name":"everett_agent_status",
                "description":"Sender-only status: queued after enqueue acknowledgement; delivered after durable recipient acceptance; claimed after inbox pickup; replied after correlated reply enqueue and signed observation. Never inferred from a connection or wake-up.",
                "inputSchema":{"type":"object","properties":{"message_id":s("Outgoing message ID.")},"required":["message_id"],"additionalProperties":false}
            }),
        ]
    })
}

fn tool_names() -> Vec<&'static str> {
    tools()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect()
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
    doc.insert(
        "ts".into(),
        json!(crate::timefmt::strftime_local("%Y-%m-%dT%H:%M:%S", now())),
    );
    doc.insert("event".into(), json!(event));
    for (k, v) in fields {
        let v = match v.as_str() {
            Some(s) if s.chars().count() > 200 => {
                json!(format!("{}…", s.chars().take(200).collect::<String>()))
            }
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
    b.insert(
        "card_source".into(),
        s.card_source
            .clone()
            .map(|v| json!(v))
            .unwrap_or(Value::Null),
    );
    b.insert("title".into(), json!(s.title));
    b.insert(
        "first_request".into(),
        json!(s.first_user.chars().take(160).collect::<String>()),
    );
    b.insert(
        "last_request".into(),
        json!(s.last_user.chars().take(160).collect::<String>()),
    );
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

fn str_arg(
    args: &Map<String, Value>,
    key: &str,
    required: bool,
) -> std::result::Result<String, ToolFailure> {
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
                Err(ToolFailure::Params(format!(
                    "\"{}\" must not be empty",
                    key
                )))
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
            let ok = v
                .as_f64()
                .map(|h| h.is_finite() && h >= 0.0)
                .unwrap_or(false)
                && !v.is_boolean();
            if ok {
                Ok(v.as_f64().unwrap())
            } else {
                Err(ToolFailure::Params(
                    "\"hours\" must be a finite non-negative number".to_string(),
                ))
            }
        }
    }
}

fn err2(code: i32, msg: impl Into<String>) -> ToolFailure {
    ToolFailure::Tool(EverettError::new(code, msg.into()))
}

/// A stdio MCP request is model-controlled. When the host identifies its
/// harness session, that binding is authoritative for safety and lineage.
/// Model-supplied arguments cannot establish a missing host binding.
fn call_identity(params: &Map<String, Value>) -> Map<String, Value> {
    let launch = crate::host::launch_harness();
    let identity = match launch {
        Some(h) => crate::send::caller_identity_for(h),
        None => caller_identity(),
    };
    if identity
        .get("session_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
    {
        return identity;
    }
    if launch.is_none_or(|h| h == "codex") {
        if let Some(thread_id) = params
            .get("_meta")
            .and_then(Value::as_object)
            .and_then(|meta| meta.get("threadId"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            let mut identity = Map::new();
            identity.insert("session_id".into(), json!(thread_id));
            identity.insert("harness".into(), json!("codex"));
            identity.insert("source".into(), json!("request _meta.threadId"));
            return identity;
        }
    }
    match crate::host::identify(launch) {
        Ok(Some(found)) => found,
        Ok(None) => identity,
        Err(reason) => {
            let mut identity = identity;
            identity.insert("harness".into(), json!(launch.unwrap_or("")));
            identity.insert("unbound".into(), json!(reason));
            identity
        }
    }
}

/// Why this caller has no session, and how to fix it, in one readable sentence.
fn unbound_message(identity: &Map<String, Value>) -> String {
    let harness = identity.get("harness").and_then(Value::as_str).unwrap_or("");
    let reason = identity
        .get("unbound")
        .and_then(Value::as_str)
        .map(|r| format!(" ({})", r))
        .unwrap_or_default();
    format!(
        "Cannot tell which session you are{}. {}",
        reason,
        crate::host::setup_hint(harness)
    )
}

fn caller_session(
    args: &Map<String, Value>,
    identity: &Map<String, Value>,
) -> std::result::Result<(String, bool), ToolFailure> {
    let requested = str_arg(args, "session_id", false)?;
    let bound = identity
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if !bound.is_empty() {
        if !requested.is_empty() && requested != bound {
            return Err(ToolFailure::Tool(EverettError::new(
                2,
                "\"session_id\" conflicts with the host-detected caller session; omit it, Everett binds the caller itself.",
            )));
        }
        return Ok((bound, true));
    }
    if !requested.is_empty() {
        return Err(ToolFailure::Tool(EverettError::new(
            2,
            format!("\"session_id\" cannot establish identity. {}", unbound_message(identity)),
        )));
    }
    Ok((String::new(), false))
}

fn tool_ls(
    args: &Map<String, Value>,
    identity: &Map<String, Value>,
) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let hours = hours_arg(args)?;
    let harness = str_arg(args, "harness", false)?;
    let sessions = crate::registry::scan(hours, false, Some(crate::registry::CAP), &harness);
    let me = identity
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

fn tool_route(
    args: &Map<String, Value>,
    identity: &Map<String, Value>,
) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let text = str_arg(args, "text", true)?;
    let router = str_arg(args, "router", false)?;
    let (caller, _) = caller_session(args, identity)?;
    let sessions = crate::registry::scan(hours_arg(args)?, false, Some(crate::registry::CAP), "");
    let r = crate::route::route(
        &text,
        &sessions,
        if router.is_empty() {
            None
        } else {
            Some(&router)
        },
        if caller.is_empty() {
            None
        } else {
            Some(&caller)
        },
    )
    .map_err(ToolFailure::Tool)?;
    let mut out = Map::new();
    for k in [
        "decision",
        "confidence",
        "router",
        "suggested",
        "candidates",
        "command",
        "fallback_from",
        "warning",
    ] {
        if let Some(v) = r.get(k) {
            if !v.is_null() {
                out.insert(k.to_string(), v.clone());
            }
        }
    }
    if let Some(sess) = r.get("session").and_then(|v| v.as_object()) {
        out.insert(
            "session".into(),
            Value::Object(brief(&Session::from_dict(sess))),
        );
    }
    Ok(out)
}

fn tool_send(
    args: &Map<String, Value>,
    identity: &Map<String, Value>,
) -> std::result::Result<Map<String, Value>, ToolFailure> {
    use crate::send::{send_for, send_inbox_as, spawn_for};
    let text = str_arg(args, "text", true)?;
    let to = str_arg(args, "to", false)?;
    let hours = hours_arg(args)?;
    let spawn_ok = args
        .get("spawn")
        .map(|v| v.as_bool())
        .unwrap_or(Some(false))
        .unwrap_or(false);
    if args.get("spawn").map(|v| !v.is_boolean()).unwrap_or(false) {
        return Err(ToolFailure::Params(
            "\"spawn\" must be a boolean".to_string(),
        ));
    }
    let timeout = args
        .get("timeout")
        .and_then(|v| v.as_f64())
        .unwrap_or(300.0);
    if args
        .get("timeout")
        .map(|v| v.is_boolean() || v.as_f64().map(|t| t <= 0.0).unwrap_or(true))
        .unwrap_or(false)
    {
        return Err(ToolFailure::Params(
            "\"timeout\" must be a positive number".to_string(),
        ));
    }
    let (caller, caller_bound) = caller_session(args, identity)?;
    let caller_harness = identity.get("harness").and_then(Value::as_str);
    let mode = str_arg(args, "mode", false)?;
    let mode = if mode.is_empty() {
        "auto".to_string()
    } else {
        mode
    };
    if !crate::send::MODES.contains(&mode.as_str()) {
        return Err(ToolFailure::Params(format!(
            "\"mode\" must be one of {}",
            crate::send::MODES.join(", ")
        )));
    }
    if !caller_bound && (spawn_ok || mode == "resume") {
        return Err(err2(
            2,
            "Caller identity is not host-detected; spawn and resume are unavailable.",
        ));
    }
    let wait = args.get("wait").and_then(|v| v.as_f64()).unwrap_or(0.0);
    if args
        .get("wait")
        .map(|v| v.is_boolean() || v.as_f64().map(|w| w < 0.0).unwrap_or(true))
        .unwrap_or(false)
    {
        return Err(ToolFailure::Params(
            "\"wait\" must be a non-negative number".to_string(),
        ));
    }
    let reply_to = str_arg(args, "reply_to", false)?;
    let exact_ids = std::env::var("EVERETT_GATEWAY_EXACT_IDS").as_deref() == Ok("1");
    if exact_ids && (spawn_ok || mode != "inbox" || (to.is_empty() && reply_to.is_empty())) {
        return Err(err2(
            2,
            "Gateway delivery requires inbox mode and an explicit exact destination or reply.",
        ));
    }
    if !reply_to.is_empty() {
        if !caller_bound {
            return Err(err2(
                2,
                "Cannot authenticate the calling session for delivery; use a host-bound MCP session.",
            ));
        }
        let result = crate::send::reply_as(
            &reply_to,
            &text,
            if caller.is_empty() {
                None
            } else {
                Some(&caller)
            },
            caller_harness,
        )
        .map_err(ToolFailure::Tool)?;
        let mut out = Map::new();
        out.insert("delivered".into(), json!(false));
        out.insert("queued".into(), json!(true));
        out.insert("wake".into(), json!(false));
        for (k, v) in result {
            out.insert(k, v);
        }
        return Ok(out);
    }
    let idempotency_key = str_arg(args, "idempotency_key", false)?;
    if crate::peer::target(&to).is_ok() {
        if !caller_bound {
            return Err(err2(
                2,
                "Cannot authenticate the calling session for relay forwarding; use a host-bound MCP session.",
            ));
        }
        if exact_ids {
            return Err(err2(2, "Gateway remote destinations are not allowed; use an authorized exact local session id."));
        }
        if spawn_ok || mode == "resume" || wait != 0.0 {
            return Err(err2(
                2,
                "Remote sends are inbox-only; no spawn, resume, or wait. Poll everett peer status.",
            ));
        }
        let value = crate::peer::send_as(
            &to,
            &text,
            if idempotency_key.is_empty() {
                None
            } else {
                Some(&idempotency_key)
            },
            if caller.is_empty() {
                None
            } else {
                Some(&caller)
            },
            caller_harness,
        )
        .map_err(ToolFailure::Tool)?;
        let mut out = value
            .as_object()
            .cloned()
            .ok_or_else(|| err2(6, "invalid remote send result"))?;
        out.insert("decision".into(), json!("SESSION"));
        out.insert("router".into(), json!("direct-remote"));
        out.insert("mode".into(), json!("inbox"));
        out.insert("delivered".into(), json!(false));
        out.insert("queued".into(), json!(true));
        out.insert("wake".into(), json!(false));
        return Ok(out);
    }
    let caller = if caller.is_empty() {
        None
    } else {
        Some(caller.as_str())
    };
    crate::send::guard_delivery(&mode, spawn_ok, caller).map_err(ToolFailure::Tool)?;
    let env = crate::send::hop_env_for(caller).map_err(ToolFailure::Tool)?;
    let mut decision = Map::new();
    let session;
    if !to.is_empty() {
        if spawn_ok {
            return Err(ToolFailure::Params(
                "\"to\" and \"spawn\" are exclusive".to_string(),
            ));
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
        let r = crate::route::route(&text, &sessions, None, caller).map_err(ToolFailure::Tool)?;
        decision.insert(
            "decision".into(),
            r.get("decision").cloned().unwrap_or(Value::Null),
        );
        decision.insert(
            "confidence".into(),
            r.get("confidence").cloned().unwrap_or(Value::Null),
        );
        decision.insert(
            "router".into(),
            r.get("router").cloned().unwrap_or(Value::Null),
        );
        for key in ["fallback_from", "warning"] {
            if let Some(v) = r.get(key) {
                decision.insert(key.into(), v.clone());
            }
        }
        if r.get("decision").and_then(|v| v.as_str()) == Some("NEW") {
            if !spawn_ok
                || r.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0)
                    < crate::route::MIN_CONFIDENCE
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
                if h.is_empty() {
                    crate::route::default_harness()
                } else {
                    h
                }
            });
            let cwd = match args.get("dir").and_then(|v| v.as_str()) {
                Some(d) => crate::paths::realpath(&crate::paths::expanduser(d))
                    .to_string_lossy()
                    .to_string(),
                None => {
                    let bd = crate::route::best_dir(&text, &sessions);
                    if !bd.is_empty() {
                        bd
                    } else {
                        std::env::current_dir()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default()
                    }
                }
            };
            let result = spawn_for(&harness, &text, &cwd, timeout, Some(&env), caller)
                .map_err(ToolFailure::Tool)?;
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
            out.insert(
                "suggested".into(),
                r.get("suggested").cloned().unwrap_or(Value::Null),
            );
            out.insert(
                "candidates".into(),
                r.get("candidates").cloned().unwrap_or(json!([])),
            );
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
    refuse_self(&session, caller).map_err(ToolFailure::Tool)?;
    let delivery_mode = crate::send::delivery_mode_for(&session, &mode, None, caller)?;
    if !caller_bound {
        return Err(err2(
            2,
            "Cannot authenticate the calling session for delivery; use a host-bound MCP session.",
        ));
    }
    if delivery_mode == "inbox" {
        let result = send_inbox_as(
            &session,
            &text,
            wait,
            caller,
            caller_harness,
            1.0,
        )
        .map_err(ToolFailure::Tool)?;
        let note = if result.get("delivered").and_then(Value::as_bool) == Some(true) {
            "Inbox pickup was observed. No ping or wake was sent. Read replies with everett_inbox."
                .to_string()
        } else if crate::adapters::external::is_external(&session.harness) {
            match result.get("webhook").and_then(|value| value.get("state")).and_then(Value::as_str) {
                Some("accepted") => "Queued for the external agent. The owner webhook returned HTTP 200, so a routine run was accepted. That is not inbox consumption. Read replies with everett_inbox.".to_string(),
                Some(_) => "Queued for the external agent to poll through MCP. The owner webhook did not accept a routine run. No provider wake-up or consumption is confirmed. Read replies with everett_inbox.".to_string(),
                None => "Queued for the external agent to poll through MCP. No provider wake-up or consumption is confirmed. Read replies with everett_inbox.".to_string(),
            }
        } else {
            result.get("pickup_note").and_then(Value::as_str).map(str::to_string).unwrap_or_else(||
                "Explicit inbox send. Hooks or everett_inbox must pick it up; no ping or wake was sent. \
                 The delivered field reports observed inbox pickup. Read replies with everett_inbox."
                    .to_string()
            )
        };
        let mut out = decision.clone();
        out.insert("session".into(), Value::Object(brief(&session)));
        for (k, v) in &result {
            out.insert(k.clone(), v.clone());
        }
        out.insert("note".into(), json!(note));
        return Ok(out);
    }
    crate::send::command_for(&session, &text).map_err(ToolFailure::Tool)?;
    let result = send_for(
        &session,
        &text,
        timeout,
        crate::send::IDLE_WAIT,
        Some(&env),
        caller,
    )
    .map_err(ToolFailure::Tool)?;
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
    let ttl_seconds = match args.get("ttl_seconds") {
        None => None,
        Some(value) => Some(value.as_u64().filter(|ttl| (1..=crate::core::MAX_TTL_SECONDS).contains(ttl))
            .ok_or_else(|| ToolFailure::Params("ttl_seconds must be an integer between 1 and 31536000".into()))?),
    };
    let entry = crate::core::learn_with_ttl(
        &fact,
        if project.is_empty() {
            None
        } else {
            Some(&project)
        },
        if scope.is_empty() { None } else { Some(&scope) },
        None,
        ttl_seconds,
    )
    .map_err(ToolFailure::Tool)?;
    let mut out = Map::new();
    out.insert("learned".into(), json!(true));
    out.insert("stored".into(), json!(true));
    out.insert("pending".into(), json!(ttl_seconds.is_none()));
    out.insert("expires_at".into(), entry.get("expires_at").cloned().unwrap_or(Value::Null));
    out.insert("id".into(), entry.get("id").cloned().unwrap_or(Value::Null));
    out.insert(
        "scope".into(),
        entry.get("scope").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "project".into(),
        entry.get("project").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "note".into(),
        json!(if ttl_seconds.is_some() { "Stored for local search until expires_at; excluded from merge and injected core." }
            else { "Stored for search and queued in the inbox; the next `everett trunk merge` updates the shared core." }),
    );
    Ok(out)
}

fn tool_core(args: &Map<String, Value>) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let project = str_arg(args, "project", false)?;
    let action = str_arg(args, "action", false)?;
    if action == "search" {
        if args.contains_key("fact") || args.contains_key("replacement") {
            return Err(ToolFailure::Params("fact and replacement are not search arguments".into()));
        }
        let query = str_arg(args, "query", true)?;
        let scope = str_arg(args, "scope", false)?;
        let limit = match args.get("limit") {
            None => 5,
            Some(value) => value.as_u64().filter(|limit| (1..=20).contains(limit))
                .ok_or_else(|| ToolFailure::Params("limit must be an integer between 1 and 20".into()))? as usize,
        };
        return crate::core::search(
            &query, if project.is_empty() { None } else { Some(&project) },
            if scope.is_empty() { None } else { Some(&scope) },
            limit,
        ).map_err(ToolFailure::Tool);
    }
    if args.contains_key("query") || args.contains_key("limit") {
        return Err(ToolFailure::Params("query and limit require action search".into()));
    }
    if action == "forget" || action == "correct" {
        let fact = str_arg(args, "fact", true)?;
        let replacement = str_arg(args, "replacement", action == "correct")?;
        if action == "forget" && args.contains_key("replacement") {
            return Err(ToolFailure::Params("replacement is only allowed for correct".into()));
        }
        let scope = str_arg(args, "scope", false)?;
        return crate::core::change(
            if action == "forget" { crate::core::CoreChange::Forget { fact: &fact } }
            else { crate::core::CoreChange::Correct { fact: &fact, replacement: &replacement } },
            if project.is_empty() { None } else { Some(&project) },
            if scope.is_empty() { None } else { Some(&scope) },
        ).map_err(ToolFailure::Tool);
    }
    if args.contains_key("fact") || args.contains_key("replacement") || args.contains_key("scope") {
        return Err(ToolFailure::Params("fact, replacement and scope require action forget or correct".into()));
    }
    let text = if !project.is_empty() {
        [
            crate::core::global_path(),
            crate::core::project_path(&project),
        ]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
    } else {
        crate::core::context(
            &std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
        )
    };
    let mut out = Map::new();
    out.insert(
        "core".into(),
        json!(if text.is_empty() {
            "(the shared core is empty)".to_string()
        } else {
            text
        }),
    );
    out.insert(
        "pending_learnings".into(),
        json!(crate::core::read_inbox().len()),
    );
    Ok(out)
}

fn tool_card(
    args: &Map<String, Value>,
    identity: &Map<String, Value>,
) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let mut parts = Map::new();
    for k in ["what", "state", "next"] {
        let v = str_arg(args, k, true)?;
        parts.insert(
            k.to_string(),
            json!(v.split_whitespace().collect::<Vec<_>>().join(" ")),
        );
    }
    let (sid, _) = caller_session(args, identity)?;
    if sid.is_empty() {
        return Err(ToolFailure::Tool(EverettError::new(2, unbound_message(identity))));
    }
    if !crate::inbox::valid_id(&sid) {
        return Err(ToolFailure::Params(
            "\"session_id\" is not a valid session id".to_string(),
        ));
    }
    let body = format!(
        "What: {}\nState: {}\nNext: {}\n",
        crate::hooks_common::word_limit(parts["what"].as_str().unwrap_or(""), 16),
        crate::hooks_common::word_limit(parts["state"].as_str().unwrap_or(""), 14),
        crate::hooks_common::word_limit(parts["next"].as_str().unwrap_or(""), 14)
    );
    let Some(path) = crate::cards::card_path(&sid) else {
        return Err(err2(
            2,
            format!("not a valid session id for a card: {sid:?}"),
        ));
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    crate::peer::save(&path, body.as_bytes()).map_err(ToolFailure::Tool)?;
    let mut out = Map::new();
    out.insert("written".into(), json!(path.to_string_lossy()));
    out.insert("card".into(), json!(body.trim()));
    Ok(out)
}

fn tool_inbox(
    args: &Map<String, Value>,
    identity: &Map<String, Value>,
) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let (sid, _) = caller_session(args, identity)?;
    if sid.is_empty() {
        return Err(ToolFailure::Tool(EverettError::new(2, unbound_message(identity))));
    }
    if !crate::inbox::valid_id(&sid) {
        return Err(ToolFailure::Params(
            "\"session_id\" is not a valid session id".to_string(),
        ));
    }
    let peek = args
        .get("peek")
        .map(|v| v.as_bool())
        .unwrap_or(Some(false))
        .unwrap_or(false);
    if args.get("peek").map(|v| !v.is_boolean()).unwrap_or(false) {
        return Err(ToolFailure::Params(
            "\"peek\" must be a boolean".to_string(),
        ));
    }
    let items = crate::inbox::pending(&sid, None);
    let mut messages: Vec<Value> = Vec::new();
    let mut out = Map::new();
    out.insert("session".into(), json!(sid));
    out.insert("messages".into(), json!(messages));
    let size_of = |out: &Map<String, Value>| {
        serde_json::to_vec(&out)
            .map(|v| v.len())
            .unwrap_or(usize::MAX)
    };
    for m in &items {
        let mut msg = Map::new();
        for k in [
            "id",
            "kind",
            "from",
            "from_harness",
            "from_card",
            "text",
            "reply_to",
            "hops",
            "ts",
        ] {
            msg.insert(k.to_string(), m.get(k).cloned().unwrap_or(Value::Null));
        }
        if let Some(depth) = m.get("reply_depth") {
            msg.insert("reply_depth".into(), depth.clone());
        }
        if m.contains_key("relay_device") {
            for k in [
                "external",
                "untrusted",
                "trust",
                "instruction_source",
                "relay_message_id",
                "relay_sender_owner",
                "relay_recipient_owner",
                "relay_grant_id",
                "relay_device",
                "relay_recipient_device",
                "relay_parent",
            ] {
                msg.insert(k.into(), m.get(k).cloned().unwrap_or(Value::Null));
            }
        }
        if crate::inbox::external(m) {
            let safe = crate::inbox::consumption_message(m.clone());
            for k in [
                "external",
                "untrusted",
                "trust",
                "instruction_source",
                "relay_lineage",
            ] {
                if let Some(value) = safe.get(k) {
                    msg.insert(k.into(), value.clone());
                }
            }
            msg.insert("text".into(), safe["text"].clone());
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
                let text = msg
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut keep = text.chars().count();
                loop {
                    msg.insert(
                        "text".into(),
                        json!(text.chars().take(keep).collect::<String>()),
                    );
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
    let ids: Vec<&str> = messages
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .collect();
    // Peek still exposes remote data: preserve its provenance/lineage before returning,
    // without recording a claimed observation or marking the inbox consumed.
    crate::inbox::persist_relay_context(&sid, &ids)?;
    if !peek {
        crate::inbox::mark_done(&sid, &ids);
        if let Some(hops) = items
            .iter()
            .filter(|m| {
                m.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| ids.contains(&id))
            })
            .filter_map(|m| m.get("hops").and_then(Value::as_i64))
            .max()
        {
            crate::inbox::record_hops(&sid, hops);
        }
    }
    Ok(out)
}

fn tool_event(
    args: &Map<String, Value>,
    identity: &Map<String, Value>,
) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let kind = str_arg(args, "kind", true)?;
    let message = str_arg(args, "message", true)?;
    let project = str_arg(args, "project", false)?;
    let (sid, _) = caller_session(args, identity)?;
    if !sid.is_empty() && !crate::inbox::valid_id(&sid) {
        return Err(ToolFailure::Params(
            "\"session_id\" is not a valid session id".to_string(),
        ));
    }
    let event = crate::events::record(
        &kind,
        &message,
        if sid.is_empty() { None } else { Some(&sid) },
        if project.is_empty() {
            None
        } else {
            Some(&project)
        },
        "",
        None,
        "manual",
        None,
    )
    .map_err(ToolFailure::Tool)?;
    let mut out = Map::new();
    out.insert("recorded".into(), json!(true));
    out.insert(
        "event".into(),
        event.map(Value::Object).unwrap_or(Value::Null),
    );
    if sid.is_empty() {
        out.insert(
            "note".into(),
            json!(format!("The event is logged but not attached to a card. {}", unbound_message(identity))),
        );
    }
    Ok(out)
}

fn tool_subscribe(
    args: &Map<String, Value>,
    identity: &Map<String, Value>,
) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let (sid, _) = caller_session(args, identity)?;
    let remove = args
        .get("unsubscribe")
        .map(|v| v.as_bool())
        .unwrap_or(Some(false))
        .unwrap_or(false);
    if args
        .get("unsubscribe")
        .map(|v| !v.is_boolean())
        .unwrap_or(false)
    {
        return Err(ToolFailure::Params(
            "\"unsubscribe\" must be a boolean".to_string(),
        ));
    }
    let target = crate::events::resolve_target(&str_arg(args, "target", true)?);
    let current = crate::events::subscribe(&sid, &target, remove).map_err(ToolFailure::Tool)?;
    let mut out = Map::new();
    out.insert("subscriber".into(), json!(sid));
    out.insert("following".into(), json!(current));
    Ok(out)
}

fn tool_whoami(
    identity: &Map<String, Value>,
) -> std::result::Result<Map<String, Value>, ToolFailure> {
    let ident = identity;
    let mut out = ident.clone();
    let hops = std::env::var("EVERETT_HOPS").unwrap_or_else(|_| "0".to_string());
    out.insert("hops".into(), json!(hops.parse::<i64>().unwrap_or(0)));
    out.insert(
        "cwd".into(),
        json!(std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()),
    );
    let has_session = ident
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    out.insert(
        "note".into(),
        json!(if has_session { String::new() } else { unbound_message(ident) }),
    );
    Ok(out)
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl Pipe for String {}

fn grant_args(
    args: &Map<String, Value>,
) -> std::result::Result<(&'static str, Value, bool), ToolFailure> {
    let action = str_arg(args, "action", true)?;
    if action == "list" {
        return Ok(("grants:list", json!({}), true));
    }
    let id = str_arg(args, "id", true)?;
    let fingerprints = || -> std::result::Result<Value, ToolFailure> {
        Ok(
            json!({"grantId":id,"localFingerprint":str_arg(args,"local_fingerprint",true)?,
            "peerFingerprint":str_arg(args,"peer_fingerprint",true)?}),
        )
    };
    let integer = |key: &str, default: Option<i64>, min: i64, max: i64| {
        let value = args
            .get(key)
            .filter(|v| !v.is_null())
            .map(Value::as_i64)
            .unwrap_or(default);
        value.filter(|v| *v >= min && *v <= max).ok_or_else(|| {
            ToolFailure::Params(format!("\"{key}\" must be an integer from {min} to {max}"))
        })
    };
    match action.as_str() {
        "get" => Ok(("grants:get", json!({"grantId":id}), true)),
        "revoke" => Ok(("grants:revoke", json!({"grantId":id}), false)),
        "accept" | "approve" => Ok(("grants:accept", fingerprints()?, false)),
        "reconfirm" => Ok(("grants:reconfirm", fingerprints()?, false)),
        "invite" | "propose" => {
            let mut value = fingerprints()?;
            value["deviceId"] = json!(str_arg(args, "device", true)?);
            value["peerOwner"] = json!(str_arg(args, "peer_owner", true)?);
            value["peerDeviceId"] = json!(str_arg(args, "remote_agent", true)?);
            value["expiresAt"] = match args.get("expires_at") {
                None | Some(Value::Null) => Value::Null,
                _ => json!(integer("expires_at", None, 0, 9_007_199_254_740_991)?),
            };
            value["ratePerMinute"] = json!(integer("rate_per_minute", Some(30), 1, 120)?);
            value["bodyCapBytes"] = json!(integer("body_cap_bytes", Some(65536), 256, 65536)?);
            Ok(("grants:invite", value, false))
        }
        _ => Err(ToolFailure::Params("unknown grant action".into())),
    }
}

fn call_handler(
    name: &str,
    args: &Map<String, Value>,
    identity: &Map<String, Value>,
) -> std::result::Result<Map<String, Value>, ToolFailure> {
    if name.starts_with("everett_agent_") {
        let value = match name {
            "everett_agent_relay" => {
                let action = str_arg(args, "action", true)?;
                let requires_id = matches!(
                    action.as_str(),
                    "pair" | "unpair" | "revoke-agent" | "revoke-credential" | "revoke-membership"
                );
                let id = str_arg(args, "id", requires_id)?;
                match action.as_str() {
                    "identity" => crate::relay::admin_call("relay:whoami", json!({}), true)?,
                    "status" => crate::relay::receiver_status()?,
                    "register" => crate::relay::register()?,
                    "directory" => crate::relay::devices()?,
                    "names" => json!({"text":crate::relay::device_names()?}),
                    "credentials" => {
                        crate::relay::admin_call("relay:credentials", json!({}), true)?
                    }
                    "memberships" => {
                        crate::relay::admin_call("relay:memberships", json!({}), true)?
                    }
                    "pair" => {
                        let alias = str_arg(args, "name", false)?;
                        let grant = str_arg(args, "grant_id", false)?;
                        crate::relay::add(
                            if alias.is_empty() { &id } else { &alias },
                            &id,
                            &str_arg(args, "fingerprint", true)?,
                            if grant.is_empty() { None } else { Some(&grant) },
                        )?
                    }
                    "unpair" => crate::relay::remove(&id)?,
                    "revoke-membership" => crate::relay::admin_call(
                        "relay:revokeMember",
                        json!({"subject":id}),
                        false,
                    )?,
                    "revoke-agent" | "revoke-credential" => crate::relay::admin_call(
                        "relay:revokeAgent",
                        json!({"deviceId":id}),
                        false,
                    )?,
                    _ => return Err(ToolFailure::Params("unknown relay action".into())),
                }
            }
            "everett_agent_grants" => {
                let (function, value, query) = grant_args(args)?;
                crate::relay::admin_call(function, value, query)?
            }
            "everett_agent_send" => {
                let target = format!(
                    "{}/{}/{}",
                    str_arg(args, "agent", true)?,
                    str_arg(args, "harness", true)?,
                    str_arg(args, "session", true)?
                );
                let (caller, caller_bound) = caller_session(args, identity)?;
                if !caller_bound {
                    return Err(ToolFailure::Tool(EverettError::new(
                        2,
                        "Cannot authenticate the calling session for relay forwarding; use a host-bound MCP session.",
                    )));
                }
                crate::relay::send_as(
                    &target,
                    &str_arg(args, "text", true)?,
                    Some(&str_arg(args, "idempotency_key", true)?),
                    Some(&caller),
                    identity.get("harness").and_then(Value::as_str),
                )?
            }
            "everett_agent_status" => {
                crate::relay::agent_status(&str_arg(args, "message_id", true)?)?
            }
            _ => return Err(ToolFailure::Params("unknown agent tool".into())),
        };
        return serde_json::from_value(if value.is_object() {
            value
        } else {
            json!({"result":value})
        })
        .map_err(|_| ToolFailure::Params("invalid agent response".into()));
    }
    match name {
        "everett_ls" => tool_ls(args, identity),
        "everett_route" => tool_route(args, identity),
        "everett_send" => tool_send(args, identity),
        "everett_learn" => tool_learn(args),
        "everett_core" => tool_core(args),
        "everett_card" => tool_card(args, identity),
        "everett_whoami" => tool_whoami(identity),
        "everett_inbox" => tool_inbox(args, identity),
        "everett_event" => tool_event(args, identity),
        "everett_subscribe" => tool_subscribe(args, identity),
        _ => Err(ToolFailure::Params(format!("unknown tool: {}", name))),
    }
}

fn call_tool(params: &Map<String, Value>) -> std::result::Result<Map<String, Value>, Value> {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if std::env::var("EVERETT_MCP_PANIC_TEST").as_deref() == Ok("1") && name == "everett_ls" {
        panic!("injected test panic"); // tests: a panicking tool must not kill the server
    }
    if name.is_empty() || !tool_names().contains(&name) {
        return Err(
            json!({"code": INVALID_PARAMS, "message": format!("unknown tool: {}", if name.is_empty() { Value::Null.to_string() } else { name.to_string() })}),
        );
    }
    let args = match params.get("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(a)) => a.clone(),
        _ => {
            return Err(
                json!({"code": INVALID_PARAMS, "message": "\"arguments\" must be an object"}),
            )
        }
    };
    let schema = tools()
        .iter()
        .find(|t| t["name"].as_str() == Some(name))
        .and_then(|t| t["inputSchema"].as_object())
        .cloned()
        .unwrap_or_default();
    for key in schema
        .get("required")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default()
    {
        if let Some(k) = key.as_str() {
            if !args.contains_key(k) {
                return Err(
                    json!({"code": INVALID_PARAMS, "message": format!("\"{}\" is required", k)}),
                );
            }
        }
    }
    let properties = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .cloned()
        .unwrap_or_default();
    for (key, value) in &args {
        if key == "session_id" && !properties.contains_key(key) {
            // Deprecated: identity comes from the host. Still accepted when it matches the binding.
            if !value.is_string() {
                return Err(json!({"code": INVALID_PARAMS, "message": "\"session_id\" must be a string"}));
            }
            continue;
        }
        let Some(spec) = properties.get(key).and_then(|v| v.as_object()) else {
            return Err(
                json!({"code": INVALID_PARAMS, "message": "unexpected argument; use the tool inputSchema"}),
            );
        };
        match spec.get("type").and_then(|v| v.as_str()) {
            Some("string") => {
                if !value.is_string() {
                    return Err(
                        json!({"code": INVALID_PARAMS, "message": format!("\"{}\" must be a string", key)}),
                    );
                }
            }
            Some("boolean") => {
                if !value.is_boolean() {
                    return Err(
                        json!({"code": INVALID_PARAMS, "message": format!("\"{}\" must be a boolean", key)}),
                    );
                }
            }
            Some("number")
                if (value.as_f64().map(|n| !n.is_finite()).unwrap_or(true)
                    || value.is_boolean()) =>
            {
                return Err(
                    json!({"code": INVALID_PARAMS, "message": format!("\"{}\" must be a finite number", key)}),
                );
            }
            Some("integer") if value.as_i64().is_none() => {
                return Err(
                    json!({"code": INVALID_PARAMS, "message": format!("\"{}\" must be an integer", key)}),
                );
            }
            _ => {}
        }
        if let Some(values) = spec.get("enum").and_then(|e| e.as_array()) {
            if !values.contains(value) {
                let opts = values
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(
                    json!({"code": INVALID_PARAMS, "message": format!("\"{}\" must be one of {}", key, opts)}),
                );
            }
        }
        if let Some(min) = spec.get("minimum").and_then(|m| m.as_f64()) {
            if value.as_f64().map(|v| v < min).unwrap_or(false) {
                return Err(
                    json!({"code": INVALID_PARAMS, "message": format!("\"{}\" must be at least {}", key, crate::fmt::g(min))}),
                );
            }
        }
        if let Some(max) = spec.get("maximum").and_then(|m| m.as_f64()) {
            if value.as_f64().is_some_and(|v| v > max) {
                return Err(
                    json!({"code": INVALID_PARAMS, "message": format!("\"{}\" must be at most {}", key, crate::fmt::g(max))}),
                );
            }
        }
        if let Some(max) = spec.get("maxLength").and_then(|m| m.as_i64()) {
            if value
                .as_str()
                .map(|s| s.chars().count() > max as usize)
                .unwrap_or(false)
            {
                return Err(
                    json!({"code": INVALID_PARAMS, "message": format!("\"{}\" exceeds {} characters", key, max)}),
                );
            }
        }
    }
    let identity = call_identity(params);
    let started = now();
    let mut log_fields = Map::new();
    log_fields.insert("tool".into(), json!(name));
    let mut keys: Vec<&String> = args.keys().collect();
    keys.sort();
    log_fields.insert("args".into(), json!(format!("{:?}", keys)));
    log_fields.insert(
        "caller".into(),
        json!(identity
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")),
    );
    log("call", log_fields);
    let (data, is_error) = match call_handler(name, &args, &identity) {
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
        data.get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        serde_json::to_string_pretty(&Value::Object(data.clone())).unwrap_or_default()
    };
    let mut out = Map::new();
    out.insert("content".into(), json!([{"type": "text", "text": text}]));
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
    if map.get("id").is_some_and(|id| !id.is_null() && !id.is_string() && !id.is_number()) {
        return Some(error(&Value::Null, INVALID_REQUEST, "Invalid Request"));
    }
    if map.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0")
        || !map.get("method").map(|m| m.is_string()).unwrap_or(false)
    {
        return Some(error(
            &map.get("id").cloned().unwrap_or(Value::Null),
            INVALID_REQUEST,
            "Invalid Request",
        ));
    }
    let method = map.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let msg_id = map.get("id").cloned().unwrap_or(Value::Null);
    let is_notification = !map.contains_key("id");
    let params = map
        .get("params")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    if !params.is_object() {
        return if is_notification {
            None
        } else {
            Some(error(&msg_id, INVALID_PARAMS, "params must be an object"))
        };
    }
    if is_notification {
        return None;
    }
    let params = params.as_object().unwrap();
    match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let version = if PROTOCOL_VERSIONS.contains(&asked) {
                asked
            } else {
                PROTOCOL_VERSIONS[0]
            };
            let mut f = Map::new();
            f.insert("version".into(), json!(version));
            log("initialize", f);
            let mut capabilities = json!({"tools": {"listChanged": false}});
            if std::env::var("EVERETT_GATEWAY_EXACT_IDS").as_deref() == Ok("1") {
                capabilities["experimental"] =
                    json!({"everettExactDestinationIds": {"enforced": true}});
            }
            Some(result(
                &msg_id,
                json!({
                    "protocolVersion": version,
                    "capabilities": capabilities,
                    "serverInfo": {"name": "everett", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                }),
            ))
        }
        "ping" => Some(result(&msg_id, json!({}))),
        "tools/list" => Some(result(&msg_id, json!({"tools": tools()}))),
        "tools/call" => {
            // A panic in one tool must not kill the stdio server.
            let tool = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call_tool(params)));
            match outcome {
                Ok(Ok(out)) => Some(result(&msg_id, Value::Object(out))),
                Ok(Err(e)) => Some(error(
                    &msg_id,
                    e["code"].as_i64().unwrap_or(INTERNAL_ERROR),
                    e["message"].as_str().unwrap_or("error"),
                )),
                Err(_) => {
                    let mut f = Map::new();
                    f.insert("tool".into(), json!(tool));
                    log("panic", f);
                    Some(error(&msg_id, INTERNAL_ERROR, "internal error"))
                }
            }
        }
        _ => Some(error(
            &msg_id,
            METHOD_NOT_FOUND,
            &format!("Method not found: {}", method),
        )),
    }
}

pub fn serve() -> i32 {
    let mut f = Map::new();
    f.insert("pid".into(), json!(std::process::id()));
    f.insert(
        "cwd".into(),
        json!(std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()),
    );
    f.insert(
        "hops".into(),
        json!(std::env::var("EVERETT_HOPS").unwrap_or_else(|_| "0".to_string())),
    );
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
                let _ = writeln!(
                    out,
                    "{}",
                    serde_json::to_string(&error(&Value::Null, PARSE_ERROR, "Parse error"))
                        .unwrap()
                );
                let _ = out.flush();
                continue;
            }
        };
        if let Value::Array(batch) = &message {
            if batch.is_empty() {
                let _ = writeln!(
                    out,
                    "{}",
                    serde_json::to_string(&json!([error(
                        &Value::Null,
                        INVALID_REQUEST,
                        "Invalid Request"
                    )]))
                    .unwrap()
                );
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
