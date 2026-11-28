mod conversation;
pub mod diagnostics;
mod identities;
mod event_http;
mod native_start;
pub mod permissions;
mod worker;
pub use worker::run as run_internal_worker;
pub(crate) use worker::{mcp_call as call_scoped_worker, mcp_enabled as scoped_worker_enabled};
#[cfg(test)]
mod ownership_shadow;

use std::{
    collections::BTreeSet,
    fs::File,
    io::{self, Cursor, Read},
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use rmcp::{
    model::*,
    service::RequestContext,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ErrorData, RoleServer, ServerHandler, ServiceExt,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
    process::Command,
    sync::Semaphore,
    time::timeout,
};

const MAX_CONFIG: usize = 16 * 1024;
const MAX_BODY: usize = 64 * 1024;
const MAX_RESULT: usize = 256 * 1024;
const MAX_INBOX: usize = 8 * 1024 * 1024;
pub(crate) const WORKER_CAPABILITY: &str = "everettGatewayWorker";
pub(crate) const WORKER_PROTOCOL: u64 = 1;
pub(crate) const WORKER_METHOD: &str = "everett/gateway/call";
const SECURITY_HEADERS: &[&str] = &[
    "host",
    "origin",
    "authorization",
    "content-length",
    "transfer-encoding",
    conversation::HANDLE_HEADER,
    conversation::ADMIN_HEADER,
];
const BACKEND_ERROR: &str = "backend unavailable, incompatible, or request timed out";
const INSTRUCTIONS: &str = "This connection authorizes owner-scoped gateway access. Read the selected inbox for handoffs; reply using reply_to. Send only to authorized full IDs. everett_directory labels those IDs with their session, T3 thread or conversation names and state; call it to resolve a named existing session or T3 Code thread instead of guessing an ID. Delivery is inbox-only; no automatic routing or bot wake-up. Only when everett_start is advertised has the owner separately granted a fixed-project T3 start; it requires a private identity_handle and durable request_id, confirms dispatch only, starts a NEW run (never reaches an existing thread), and grants no additional send/read/control access. Shared core, when enabled, includes the owner's global core. To register this chat separately, call everett_identity(action=create, registration_key=<fresh private random key>, name=<chat name>, kind=chat|dot|codex). Keep identity_handle private and pass it on subsequent calls; omit session_id. A name or host conversation metadata never authenticates an inbox.";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct OAuth {
    issuer: String,
    resource: String,
    #[serde(default = "default_scope")]
    scope: String,
    #[serde(default)]
    introspection_endpoint: Option<String>,
    #[serde(default, deserialize_with = "configured_owner_account")]
    owner_account: Option<String>,
}

fn configured_owner_account<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Option<String>, D::Error> {
    String::deserialize(value).map(Some)
}

fn default_scope() -> String {
    "everett".into()
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    #[serde(default)]
    events: bool,
    #[serde(default)]
    require_identity_handle: bool,
    #[serde(default)]
    conversation_identity: bool,
    #[serde(skip)]
    conversation_worker: Option<worker::PinnedWorker>,
    agent_id: String,
    harness: String,
    cwd: PathBuf,
    project: String,
    destinations: BTreeSet<String>,
    allow_shared_core: bool,
    #[serde(default)]
    agent_tool_permissions: Option<PathBuf>,
    #[serde(default = "default_backend_command")]
    backend_command: Vec<String>,
    #[serde(skip)]
    home: PathBuf,
    #[serde(skip)]
    identity_owner: Option<String>,
    #[serde(default = "default_workers")]
    max_workers: usize,
    #[serde(default = "default_timeout")]
    timeout: f64,
    #[serde(default = "default_hosts")]
    allowed_hosts: Vec<String>,
    #[serde(default)]
    allowed_origins: Vec<String>,
    #[serde(default)]
    oauth: Option<OAuth>,
    #[serde(default)]
    native_start: Option<native_start::Policy>,
}

fn default_workers() -> usize {
    2
}
fn default_timeout() -> f64 {
    20.0
}
fn default_hosts() -> Vec<String> {
    vec!["127.0.0.1:8788".into()]
}
fn default_backend_command() -> Vec<String> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.into_os_string().into_string().ok())
        .map(|path| vec![path, "mcp".into()])
        .unwrap_or_default()
}

fn bounded_file(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let mut raw = Vec::new();
    File::open(path)
        .and_then(|file| file.take((limit + 1) as u64).read_to_end(&mut raw))
        .map_err(|_| "cannot read boundary file".to_string())?;
    if raw.len() > limit {
        return Err("boundary file exceeds size limit".into());
    }
    Ok(raw)
}

pub(crate) fn valid_destination(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn valid_authority(authority: &str) -> bool {
    if authority.is_empty()
        || !authority.is_ascii()
        || authority
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control() || b"*@/?#\\".contains(&b))
    {
        return false;
    }
    authority.parse::<axum::http::uri::Authority>().is_ok()
        && url::Url::parse(&format!("https://{authority}")).is_ok()
}

/// https, or plain http only to a loopback host (local tests and tunnels).
fn valid_oauth_url(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|u| {
        let loopback = matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        (u.scheme() == "https" || (u.scheme() == "http" && loopback))
            && u.host_str().is_some()
            && u.fragment().is_none()
            && u.username().is_empty()
            && u.password().is_none()
    })
}

fn strong_secret(secret: &str) -> bool {
    (32..=512).contains(&secret.len())
        && secret.is_ascii()
        && !secret
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
}

fn valid_origin(origin: &str) -> bool {
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    matches!(scheme, "http" | "https") && valid_authority(authority)
}

impl Binding {
    fn load(path: &Path) -> Result<Self, String> {
        let raw = bounded_file(path, MAX_CONFIG)?;
        let mut binding: Self = serde_json::from_slice(&raw)
            .map_err(|_| "invalid binding configuration fields".to_string())?;
        binding.home = crate::session::home();
        if !crate::adapters::external::valid_id(&binding.agent_id)
            || !crate::adapters::external::is_external(&binding.harness)
        {
            return Err("invalid external agent identity or harness".into());
        }
        if binding.project.is_empty()
            || binding.project.len() > 64
            || !binding.project.as_bytes()[0].is_ascii_alphanumeric()
            || !binding
                .project
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
        {
            return Err("project must be a fixed lowercase project slug".into());
        }
        if !binding.cwd.is_absolute() || !binding.cwd.is_dir() || !binding.home.is_absolute() {
            return Err("cwd must be an existing absolute directory; home must be absolute".into());
        }
        binding.home = binding
            .home
            .canonicalize()
            .map_err(|_| "home must exist".to_string())?;
        if binding.destinations.len() > 64
            || binding
                .destinations
                .iter()
                .any(|id| !valid_destination(id) || id == &binding.agent_id)
        {
            return Err("destinations must be full valid IDs excluding this agent".into());
        }
        if binding.backend_command.is_empty()
            || binding.backend_command.len() > 8
            || !Path::new(&binding.backend_command[0]).is_absolute()
            || binding
                .backend_command
                .iter()
                .any(|s| s.is_empty() || s.contains('\0'))
        {
            return Err(
                "backend_command must be a trusted absolute executable and arguments".into(),
            );
        }
        if !(1..=8).contains(&binding.max_workers)
            || !binding.timeout.is_finite()
            || !(1.0..=30.0).contains(&binding.timeout)
        {
            return Err("max_workers must be 1..8 and timeout must be 1..30 seconds".into());
        }
        if binding.allowed_hosts.is_empty()
            || !binding.allowed_hosts.iter().all(|s| valid_authority(s))
            || !binding.allowed_origins.iter().all(|s| valid_origin(s))
        {
            return Err("HTTP hosts and origins must be exact authorities and http(s) origins without wildcards".into());
        }
        if binding.oauth.as_ref().is_some_and(|o| {
            !valid_oauth_url(&o.issuer)
                || !valid_oauth_url(&o.resource)
                || o.introspection_endpoint.as_deref().is_some_and(|e| !valid_oauth_url(e))
                || o.scope.is_empty()
                || o.scope.len() > 64
                || !o.scope.bytes().all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
                || o.owner_account.as_ref().is_some_and(|account| {
                    account.is_empty() || account.len() > 256 || !account.bytes().all(|b| b.is_ascii_graphic())
                })
        }) {
            return Err("oauth needs https issuer, resource and introspection_endpoint URLs (http only on loopback), one scope token and a nonempty printable owner_account when configured".into());
        }
        if binding.conversation_identity
            && (binding.events
                || binding.require_identity_handle
                || binding.allow_shared_core
                || binding.native_start.is_some()
                || binding.oauth.as_ref().is_some_and(|oauth| oauth.owner_account.is_none()))
        {
            return Err("conversation_identity requires an owner_account for OAuth and no legacy handles, shared core, events, or native start".into());
        }
        if binding.conversation_identity {
            binding.conversation_worker = Some(worker::PinnedWorker::new(&binding.backend_command)
                .ok_or("conversation_identity requires the built-in backend")?);
        }
        if let Some(path) = &binding.agent_tool_permissions {
            if !path.is_absolute() || binding.oauth.is_some() || binding.events
                || binding.require_identity_handle || binding.native_start.is_some()
                || !worker::is_builtin(&binding.backend_command)
            {
                return Err("agent_tool_permissions requires an absolute path, the built-in backend and a separate bearer binding without gateway-only grants".into());
            }
            permissions::Grant::load(path)?;
        }
        if let Some(policy) = &binding.native_start {
            policy.validate(&binding)?;
        }
        Ok(binding)
    }

    fn owner_agent_permissions(&self) -> Option<&Path> {
        self.agent_tool_permissions.as_deref().filter(|_| !self.conversation_identity)
    }

    fn reply_message(&self, message_id: &str) -> Result<Value, String> {
        if message_id.is_empty() || message_id.chars().count() > 128 {
            return Err("invalid reply message ID".into());
        }
        let path = crate::inbox::path_in(&self.home, &self.agent_id);
        let raw = bounded_file(&path, MAX_INBOX)?;
        let mut matches = Vec::new();
        for line in raw.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
            let message: Value = serde_json::from_slice(line)
                .map_err(|_| "cannot authorize reply from malformed inbox")?;
            if message["id"] == message_id {
                matches.push(message);
            }
        }
        if matches.len() != 1 || matches[0]["to"] != self.agent_id {
            return Err("reply message is not uniquely addressed to this agent".into());
        }
        let sender = matches[0]["from"]
            .as_str()
            .ok_or("reply destination is not authorized")?;
        if !self.destinations.contains(sender) {
            return Err("reply destination is not authorized".into());
        }
        if matches[0].get("relay_device").is_some() || matches[0].get("peer_client").is_some() {
            return Err("gateway replies require a local inbox message".into());
        }
        Ok(matches.pop().expect("one authorized message"))
    }

    fn authorize(
        &self,
        name: &str,
        mut args: Map<String, Value>,
    ) -> Result<Map<String, Value>, String> {
        let allowed: &[&str] = match name {
            "everett_whoami" => &[],
            "everett_core" if self.allow_shared_core => &["project"],
            "everett_inbox" => &["session_id", "peek", "message_id"],
            "everett_card" => &["session_id", "what", "state", "next"],
            "everett_send" => &["session_id", "text", "to", "reply_to"],
            "everett_directory" => &[],
            _ => return Err("tool is not allowed".into()),
        };
        if serde_json::to_vec(&args)
            .map_err(|_| "invalid tool arguments")?
            .len()
            > MAX_BODY
        {
            return Err("tool arguments exceed boundary limits".into());
        }
        if args.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(
                "unexpected argument; spawning, resuming, routing and cwd changes are not allowed"
                    .into(),
            );
        }
        if args
            .get("session_id")
            .is_some_and(|id| id != &self.agent_id)
        {
            return Err("session_id differs from the immutable binding".into());
        }
        match name {
            "everett_core" => {
                if args
                    .get("project")
                    .is_some_and(|project| project != &self.project)
                {
                    return Err("project differs from the fixed binding".into());
                }
                args.insert("project".into(), json!(self.project));
            }
            "everett_whoami" => {}
            _ => {
                args.insert("session_id".into(), json!(self.agent_id));
            }
        }
        if name == "everett_inbox" {
            if args.get("peek").is_some_and(|peek| !peek.is_boolean()) {
                return Err("peek must be a boolean".into());
            }
            if args.get("message_id").is_some_and(|id| !id.as_str().is_some_and(crate::inbox::valid_id)) {
                return Err("message_id must be a valid Everett message ID".into());
            }
        }
        if name == "everett_card" {
            for key in ["what", "state", "next"] {
                require_text(&args, key, 2000)?;
            }
        }
        if name == "everett_send" {
            require_text(&args, "text", 16000)?;
            match (args.get("to"), args.get("reply_to")) {
                (Some(to), None)
                    if to.as_str().is_some_and(|to| self.destinations.contains(to)) => {}
                (None, Some(reply)) => {
                    self.reply_message(reply.as_str().ok_or("invalid reply message ID")?)?;
                }
                _ => {
                    return Err(
                        "provide exactly one authorized destination ID from everett_directory or an owned reply_to; names and titles are not accepted"
                            .into(),
                    )
                }
            }
            args.extend([
                ("mode".into(), json!("inbox")),
                ("wait".into(), json!(0)),
                ("spawn".into(), json!(false)),
                ("hours".into(), json!(crate::directory::SCAN_HOURS)),
            ]);
        }
        if name == "everett_directory" {
            args.insert("ids".into(), json!(self.destinations));
        }
        Ok(args)
    }

    fn requires_handle(&self, name: &str) -> bool {
        self.require_identity_handle
            && matches!(name, "everett_inbox" | "everett_send" | "everett_card")
    }

    fn tools(&self) -> Vec<Tool> {
        let own = json!({"type":"string", "description":"Must match the identity selected by identity_handle; omit to use that binding."});
        let short = json!({"type":"string", "minLength":1, "maxLength":2000});
        let specs = [
            ("everett_identity", "Register a separate chat inbox under this connection. create requires a fresh private registration_key, name and kind; reuse the key only for a retry. list returns only this connection's registered conversation inboxes -- it is not a session or T3 thread directory; use everett_directory for send destinations. revoke requires the private identity_handle. Names and kinds are self-declared.", json!({"action":{"type":"string","enum":["create","list","revoke"]},"registration_key":{"type":"string","minLength":32,"maxLength":128},"name":{"type":"string","minLength":1,"maxLength":100},"kind":{"type":"string","enum":["chat","dot","codex"]}}), vec!["action"]),
            ("everett_whoami", "Read this connection's logical owner-scoped identity, not vendor-attested Bot identity. authorized_destinations lists the exact IDs this connection may send to; everett_directory labels them.", json!({}), vec![]),
            ("everett_core", "Read owner-wide GLOBAL shared core plus this connection's fixed project core.", json!({"project":{"type":"string","enum":[self.project]}}), vec![]),
            ("everett_inbox", "Read only this agent's inbox. Use message_id from an MCP event to take only that message. Use peek=true to leave it pending.", json!({"session_id":own,"peek":{"type":"boolean"},"message_id":{"type":"string","minLength":1,"maxLength":128,"description":"Exact message ID in this inbox."}}), vec![]),
            ("everett_card", "Update only this agent's session card.", json!({"session_id":own,"what":short,"state":short,"next":short}), vec!["what","state","next"]),
            ("everett_send", "Queue an inbox-only message to an authorized existing session, T3 Code thread or conversation inbox, or reply to an owned message. No automatic bot wake-up.", json!({"session_id":own,"text":{"type":"string","minLength":1,"maxLength":16000},"to":{"type":"string","minLength":1,"maxLength":128,"description":"Exact authorized ID from everett_directory or everett_whoami.authorized_destinations; names and titles are rejected. The gateway checks the owner allowlist on every send."},"reply_to":{"type":"string","minLength":1,"maxLength":128}}), vec!["text"]),
            ("everett_directory", "List the destinations this connection may reach through everett_send, with each authorized ID's kind (session, T3 Code thread, registered conversation, or inbox), project, title, host, availability and permitted actions. Use it to resolve a named existing session or T3 Code thread to the exact ID instead of guessing. Only owner-authorized IDs appear.", json!({}), vec![]),
        ];
        specs.into_iter().filter(|(name, ..)| {
                if self.conversation_identity {
                    conversation::is_tool(name)
                } else {
                    *name != "everett_core" || self.allow_shared_core
                }
            })
            .map(|(name, description, mut properties, mut required)| {
                if self.conversation_identity {
                    properties.as_object_mut().expect("tool properties").remove("session_id");
                } else {
                    if self.requires_handle(name) {
                        required.push("identity_handle");
                    }
                    properties["identity_handle"] = json!({"type":"string","minLength":64,"maxLength":64,"description":"Private handle for this registered conversation. For everett_identity, supply it only when revoking."});
                }
                serde_json::from_value(json!({
                "name":name,"description":description,
                "inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
                "annotations":{"readOnlyHint":matches!(name,"everett_core"|"everett_whoami"|"everett_directory"),"openWorldHint":false}
            })).expect("gateway tool definitions are valid") }).collect()
    }
}

fn require_text(args: &Map<String, Value>, key: &str, max: usize) -> Result<(), String> {
    match args.get(key).and_then(Value::as_str) {
        Some(text) if !text.trim().is_empty() && text.chars().count() <= max => Ok(()),
        _ => Err(format!(
            "{key} must be a nonempty string of at most {max} characters"
        )),
    }
}

// Cap frames before the SDK buffers an untrusted or broken peer's entire line.
struct BoundedRead<R> {
    inner: R,
    bytes: usize,
    limit: usize,
}
impl<R: AsyncRead + Unpin> AsyncRead for BoundedRead<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = result {
            for byte in &buf.filled()[before..] {
                self.bytes += 1;
                if self.bytes > self.limit {
                    buf.set_filled(before);
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "MCP frame exceeds boundary limit",
                    )));
                }
                if *byte == b'\n' {
                    self.bytes = 0;
                }
            }
        }
        result
    }
}

/// The dot identity an OAuth access token was issued to, attached to the HTTP request.
#[derive(Clone)]
struct OAuthAgent {
    expires_at: u64,
    agent_id: String,
    name: String,
    client_id: String,
    account: Option<String>,
}

#[derive(Clone)]
struct Gateway {
    binding: Arc<Binding>,
    workers: Arc<Semaphore>,
    oauth: bool,
}

impl Binding {
    fn for_oauth(&self, agent: &OAuthAgent) -> Self {
        let mut binding = self.clone();
        binding.agent_id = agent.agent_id.clone();
        binding.identity_owner = agent.account.as_ref().map(|account|
            serde_json::to_string(&(account, &agent.client_id)).expect("identity owner is serializable"));
        binding
    }
}

impl Gateway {
    /// In OAuth mode the token's agent replaces the fixed `agent_id` from dot.json.
    fn bound(&self, ctx: &RequestContext<RoleServer>) -> Result<(Arc<Binding>, Option<OAuthAgent>), String> {
        if !self.oauth {
            return Ok((self.binding.clone(), None));
        }
        let agent = ctx
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<OAuthAgent>())
            .cloned()
            .ok_or("OAuth identity missing from request; reconnect and authorize again")?;
        let binding = self.binding.for_oauth(&agent);
        Ok((Arc::new(binding), Some(agent)))
    }

    async fn call_owner_agent(
        &self,
        binding: &Binding,
        path: &Path,
        request: CallToolRequestParams,
    ) -> Result<CallToolResult, String> {
        if !permissions::Grant::load(path)?.permits(&request.name) {
            return Err("tool is not granted to this connection".into());
        }
        let permit = self.workers.clone().try_acquire_owned()
            .map_err(|_| "gateway workers busy; retry later")?;
        let mut command = backend_command(binding, Some(Path::new(&binding.backend_command[0])));
        command.env("EVERETT_GATEWAY_OWNER_AGENT", "1");
        command.env_remove("EVERETT_GATEWAY_EXACT_IDS");
        command.env_remove("EVERETT_SEND");
        command.env_remove("EVERETT_ROUTER");
        for key in ["EVERETT_T3_BIN", "T3CODE_HOME"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let mut child = command.spawn().map_err(|_| BACKEND_ERROR)?;
        let call = worker::Call::OwnerAgent {
            name: request.name.into_owned(),
            arguments: request.arguments.unwrap_or_default(),
        };
        let task = tokio::spawn(async move {
            let _permit = permit;
            let result = worker::dispatch(call, &mut child).await;
            let _ = child.kill().await;
            let _ = child.wait().await;
            result
        });
        match timeout(Duration::from_secs_f64(binding.timeout), task).await {
            Ok(Ok(Ok(data))) => serde_json::from_value(data).map_err(|_| BACKEND_ERROR.into()),
            Ok(Ok(Err(error))) => Err(format!("{error}. The tool may have run; check its state before retrying.")),
            Ok(Err(_)) => Err("Worker ended with an unconfirmed outcome; check state before retrying.".into()),
            Err(_) => {
                let mut result = CallToolResult::structured(json!({
                    "completion_confirmed": false,
                    "retry_safe": false,
                    "message": "The request exceeded the HTTP response deadline. It may still be running or have dispatched. Check its state before retrying."
                }));
                result.is_error = Some(true);
                Ok(result)
            }
        }
    }

    async fn call(
        &self,
        binding: &Binding,
        agent: Option<&OAuthAgent>,
        name: &str,
        mut arguments: Map<String, Value>,
        conversation_handle: Option<&str>,
    ) -> Result<Value, String> {
        if binding.conversation_identity {
            conversation::authorize_tool(binding, name)?;
            if arguments.contains_key("identity_handle") || arguments.contains_key("session_id") {
                return Err("conversation identity comes from the authenticated HTTP request".into());
            }
        }
        if name == "everett_identity" {
            let action = arguments.get("action").and_then(Value::as_str).ok_or("action is required")?;
            let allowed: &[&str] = match action {
                "create" => &["action", "registration_key", "name", "kind"],
                "list" => &["action", "kind"],
                "revoke" => &["action", "identity_handle"],
                _ => return Err("unknown identity action".into()),
            };
            if arguments.keys().any(|key| !allowed.contains(&key.as_str())) { return Err("unexpected identity argument".into()); }
            let text = |key: &str| arguments.get(key).and_then(Value::as_str).ok_or_else(|| format!("{key} is required"));
            return match action {
                "create" => identities::create(binding, text("registration_key")?, text("name")?, text("kind")?),
                "list" => {
                    let kind = arguments.get("kind").map(|v| v.as_str().ok_or("kind must be a string")).transpose()?;
                    identities::list(binding, kind)
                }
                "revoke" => identities::revoke(binding, text("identity_handle")?),
                _ => unreachable!(),
            };
        }
        let mut selected = binding.clone();
        let argument_handle = arguments.remove("identity_handle");
        let handle = if binding.conversation_identity {
            Some(conversation_handle.ok_or("private conversation handle required")?)
        } else {
            argument_handle.as_ref().map(|value| value.as_str().ok_or("identity_handle must be a string")).transpose()?
        };
        let private_identity = handle.is_some();
        if binding.requires_handle(name) && !private_identity {
            return Err("identity_handle is required; register a private inbox with everett_identity".into());
        }
        if let Some(handle) = handle {
            selected.agent_id = identities::resolve(binding, handle)?;
        }
        let binding = &selected;
        if matches!(name, "everett_start" | "everett_start_status") {
            if !private_identity { return Err("native start requires a private identity_handle for durable retry isolation".into()); }
            let policy = binding.native_start.as_ref().filter(|p| p.permits(agent))
                .ok_or("native start is not granted to this OAuth account and client")?;
            let _permit = self.workers.try_acquire().map_err(|_| "gateway workers busy; retry later")?;
            return if name == "everett_start_status" { native_start::status(binding, policy, arguments).await } else { native_start::start(binding, policy, arguments).await };
        }
        let args = binding.authorize(name, arguments)?;
        let _permit = self
            .workers
            .try_acquire()
            .map_err(|_| "gateway workers busy; retry later")?;
        let worker = if binding.conversation_identity {
            Some(binding.conversation_worker.as_ref()
                .and_then(|worker| worker.path_for(&binding.backend_command))
                .ok_or("conversation identity requires the original built-in worker")?)
        } else {
            worker::is_builtin(&binding.backend_command).then(|| Path::new(&binding.backend_command[0]))
        };
        let mut child = backend_command(binding, worker).spawn().map_err(|_| BACKEND_ERROR)?;
        let operation = async {
            if worker.is_some() {
                let request = worker::Call::authorized(binding, name, &args)?;
                return worker::dispatch(request, &mut child).await;
            }
            let stdout = child.stdout.take().ok_or(BACKEND_ERROR)?;
            let stdin = child.stdin.take().ok_or(BACKEND_ERROR)?;
            let peer = ()
                .serve((
                    BoundedRead {
                        inner: stdout,
                        bytes: 0,
                        limit: MAX_RESULT + 1024,
                    },
                    stdin,
                ))
                .await
                .map_err(|_| BACKEND_ERROR)?;
            let supports_worker = peer.peer_info().is_some_and(|info| {
                info.capabilities.experimental.as_ref()
                    .and_then(|caps| caps.get(WORKER_CAPABILITY))
                    .and_then(|cap| cap.get("version"))
                    .and_then(Value::as_u64) == Some(WORKER_PROTOCOL)
            });
            if supports_worker {
                let result = async {
                    let call = worker::Call::authorized(binding, name, &args)?;
                    let request = CustomRequest::new(WORKER_METHOD, Some(json!({"request": call})));
                    let response = peer.send_request(ClientRequest::CustomRequest(request)).await
                        .map_err(|_| BACKEND_ERROR)?;
                    let ServerResult::CustomResult(CustomResult(mut value)) = response else {
                        return Err(BACKEND_ERROR.into());
                    };
                    value.get_mut("everettGatewayResult").filter(|data| data.is_object())
                        .map(Value::take).ok_or_else(|| BACKEND_ERROR.into())
                }.await;
                peer.cancellation_token().cancel();
                return result;
            }
            let result = async {
                if name == "everett_send" && args.contains_key("to") {
                    let info = peer.peer_info().ok_or(BACKEND_ERROR)?;
                    let enforced = info.capabilities.experimental.as_ref()
                        .and_then(|caps| caps.get(crate::mcp::EXACT_DESTINATION_IDS))
                        .and_then(|cap| cap.get("enforced"));
                    if enforced != Some(&Value::Bool(true)) { return Err("backend lacks strict exact-ID delivery; owned-message replies remain available".into()); }
                }
                if name == "everett_inbox" && args.contains_key("message_id") {
                    let info = peer.peer_info().ok_or(BACKEND_ERROR)?;
                    let enforced = info.capabilities.experimental.as_ref()
                        .and_then(|caps| caps.get(crate::mcp::EXACT_INBOX_PICKUP))
                        .and_then(|cap| cap.get("enforced"));
                    if enforced != Some(&Value::Bool(true)) {
                        return Err("backend lacks exact inbox pickup; update the Everett backend to use message_id".into());
                    }
                }
                if let Some(reply) = args.get("reply_to").and_then(Value::as_str) { binding.reply_message(reply)?; }
                let response = peer.call_tool_once(CallToolRequestParams::new(name.to_owned()).with_arguments(args))
                    .await.map_err(|_| BACKEND_ERROR)?;
                let CallToolResponse::Complete(result) = response else { return Err(BACKEND_ERROR.into()); };
                if result.is_error == Some(true) { return Err("backend rejected the tool call".into()); }
                if serde_json::to_vec(&result).map_err(|_| BACKEND_ERROR)?.len() > MAX_RESULT { return Err("backend result exceeds 256 KiB".into()); }
                result.structured_content.filter(Value::is_object).ok_or_else(|| "backend must return structured tool results".into())
            }.await;
            peer.cancellation_token().cancel();
            result
        };
        let result = timeout(Duration::from_secs_f64(binding.timeout), operation)
            .await
            .unwrap_or_else(|_| Err(BACKEND_ERROR.into()));
        let _ = child.kill().await;
        let _ = child.wait().await;
        let mut data = result?;
        if name == "everett_whoami" && (data["session_id"] != binding.agent_id || data["harness"] != binding.harness) { return Err("backend identity differs from binding".into()); }
        if name == "everett_whoami" {
            let account_verified = agent.and_then(|a| a.account.as_ref()).is_some();
            let client_verified = agent.is_some_and(|a| !a.client_id.is_empty());
            data["identity"] = json!({
                "account": {"status": if account_verified { "verified" } else { "unknown" },
                    "issuer": if account_verified { binding.oauth.as_ref().map(|o| &o.issuer) } else { None },
                    "subject": null, "subject_redacted": account_verified},
                "client": {"kind": if agent.is_some() { "oauth_client" } else { "fixed_gateway" },
                    "status": if client_verified { "verified" } else { "unknown" },
                    "id": agent.map(|a| &a.client_id), "name": agent.map(|a| &a.name)},
                "session": {"id": binding.agent_id, "harness": binding.harness,
                    "label": format!("{} [{}]", if private_identity { "Registered conversation" } else if binding.require_identity_handle { "Gateway connection" } else { "Legacy client inbox" }, binding.agent_id),
                    "provenance": if private_identity { "owner_registered_logical" } else { "legacy_client_binding" },
                    "ownership": if private_identity && account_verified && client_verified { "verified_account_client" } else { "unmapped" }}
            });
            data.as_object_mut().ok_or("invalid whoami response")?.remove("native_start_scope");
            if private_identity {
                if let Some(policy) = binding.native_start.as_ref().filter(|p| p.permits(agent)) {
                    data["native_start_scope"] = policy.scope_info();
                }
            }
            data["gateway_runtime"] = crate::build_info::value();
            data["authorized_destinations"] = json!(binding.destinations);
            let directory_granted = !binding.conversation_identity
                || conversation::grant(binding)
                    .ok()
                    .flatten()
                    .is_none_or(|grant| grant.permits("everett_directory"));
            data["capabilities"] = json!({"native_session_start": binding.native_start.as_ref().is_some_and(|p| p.permits(agent)), "automatic_wake": false, "delivery": "inbox", "destination_directory": directory_granted});
            if binding.require_identity_handle || binding.conversation_identity {
                data["capabilities"]["private_inbox_required"] = json!(true);
            }
            if binding.conversation_identity {
                data["capabilities"]["private_inbox_transport"] = json!("http_header");
            }
            if let Some(oauth) = &binding.oauth {
                data["gateway_resource"] = json!(oauth.resource);
            }
        }
        if let (true, Some(agent)) = (name == "everett_whoami", agent) {
            let mut data = data;
            data["gateway_auth"] = json!({"mode": "oauth", "agent_name": agent.name, "client_id": agent.client_id});
            return Ok(data);
        }
        if name == "everett_core" {
            return Ok(json!({"core":data["core"].as_str().ok_or("backend must return shared core text")?}));
        }
        Ok(data)
    }
}

fn backend_command(binding: &Binding, worker: Option<&Path>) -> Command {
    let mut command = Command::new(worker.unwrap_or_else(|| Path::new(&binding.backend_command[0])));
    if worker.is_some() {
        command.arg("gateway-worker");
    } else {
        command.args(&binding.backend_command[1..]);
    }
    command
        .env_clear()
        .current_dir(&binding.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in ["PATH", "USER", "LOGNAME", "SHELL", "TMPDIR", "TMP", "TEMP", "LANG"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("HOME", &binding.home)
        .env("EVERETT_HOME", &binding.home)
        .env("EVERETT_SESSION_ID", &binding.agent_id)
        .env("EVERETT_HARNESS_NAME", &binding.harness)
        .env("EVERETT_NOTIFY", "none")
        .env("EVERETT_ROUTER", "local")
        .env("EVERETT_SEND", "inbox")
        .env("EVERETT_HOPS", "0")
        .env("EVERETT_GATEWAY_WORKER", "1")
        .env("EVERETT_GATEWAY_EXACT_IDS", "1");
    command
}

impl ServerHandler for Gateway {
    fn get_info(&self) -> ServerConfig {
        if self.binding.conversation_identity {
            return ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
                .with_server_info(Implementation::new("everett-conversation-gateway", env!("CARGO_PKG_VERSION")))
                .with_instructions("Use the Everett tools for this conversation. Your HTTP client must supply the private handle. Omit identity arguments.");
        }
        if self.binding.owner_agent_permissions().is_some() {
            return ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
                .with_server_info(Implementation::new("everett-owner-agent", env!("CARGO_PKG_VERSION")))
                .with_instructions("Use the Everett tools granted to this connection by the owner.");
        }
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "everett-owner-gateway",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(format!("{INSTRUCTIONS} {}", if self.binding.require_identity_handle {
                "Mailbox tools and Event subscriptions require a private identity_handle."
            } else {
                "Without a private identity handle, mailbox tools use the legacy connection inbox."
            }))
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let (binding, agent) = self.bound(&ctx).map_err(|e| ErrorData::invalid_request(e, None))?;
        if let Some(path) = binding.owner_agent_permissions() {
            let grant = permissions::Grant::load(path).map_err(|e| ErrorData::invalid_request(e, None))?;
            let tools = crate::mcp::tools().iter().filter(|tool| {
                tool["name"].as_str().is_some_and(|name| grant.permits(name))
            }).map(|tool| {
                serde_json::from_value(tool.clone())
                    .map_err(|_| ErrorData::internal_error("cannot encode Everett tool", None))
            }).collect::<Result<Vec<Tool>, _>>()?;
            return Ok(ListToolsResult::with_all_items(tools));
        }
        let mut tools = binding.tools();
        if binding.conversation_identity {
            let grant = conversation::grant(&binding).map_err(|e| ErrorData::invalid_request(e, None))?;
            tools.retain(|tool| grant.as_ref().is_none_or(|grant| grant.permits(&tool.name)));
        }
        if let Some(policy) = binding.native_start.as_ref().filter(|p| p.permits(agent.as_ref())) {
            tools.push(native_start::tool(policy));
            tools.push(native_start::status_tool());
        }
        Ok(ListToolsResult::with_all_items(tools))
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let mut result = match self.bound(&ctx) {
            Ok((binding, agent)) => {
                let outcome = if let Some(path) = binding.owner_agent_permissions() {
                    self.call_owner_agent(&binding, path, request).await
                } else {
                    let handle = ctx.extensions.get::<axum::http::request::Parts>()
                        .and_then(|parts| parts.headers.get(conversation::HANDLE_HEADER))
                        .and_then(|value| value.to_str().ok());
                    self.call(&binding, agent.as_ref(), &request.name, request.arguments.unwrap_or_default(), handle)
                        .await
                        .map(|data| {
                            let unconfirmed = request.name == "everett_start" && data["dispatch_confirmed"] != true;
                            let mut result = CallToolResult::structured(data);
                            if unconfirmed { result.is_error = Some(true); }
                            result
                        })
                };
                outcome.unwrap_or_else(|error| CallToolResult::error(vec![ContentBlock::text(error)]))
            }
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
        };
        if serde_json::to_vec(&result)
            .map_err(|_| ErrorData::internal_error("cannot encode tool result", None))?
            .len()
            > MAX_RESULT
        {
            result =
                CallToolResult::error(vec![ContentBlock::text("gateway result exceeds 256 KiB")]);
        }
        Ok(result.into())
    }
}

#[derive(Clone)]
enum HttpAuth {
    Bearer([u8; 32]),
    OAuth(Arc<OAuthVerifier>),
}

#[derive(Clone)]
struct HttpBoundary {
    events: Option<crate::gateway_events::Events>,
    event_scope: String,
    gateway: Gateway,
    auth: HttpAuth,
    identity_admin: Option<[u8; 32]>,
    requests: Arc<Semaphore>,
    event_requests: Arc<Semaphore>,
}

const PROTECTED_RESOURCE: &str = "/.well-known/oauth-protected-resource";

struct OAuthVerifier {
    config: OAuth,
    endpoint: String,
    secret: String,
    metadata_url: String,
    agent: ureq::Agent,
}

type Rejection = (StatusCode, &'static str);

impl OAuthVerifier {
    fn new(config: OAuth, secret: String) -> Result<Self, String> {
        let resource = url::Url::parse(&config.resource).map_err(|_| "invalid oauth resource")?;
        let path = resource.path().trim_end_matches('/');
        let metadata_url = format!("{}{PROTECTED_RESOURCE}{path}", resource.origin().ascii_serialization());
        let endpoint = config.introspection_endpoint.clone().unwrap_or_else(|| {
            format!("{}/oauth/introspect", config.issuer.trim_end_matches('/'))
        });
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into();
        Ok(Self { config, endpoint, secret, metadata_url, agent })
    }

    /// RFC 9728 protected resource metadata.
    fn metadata(&self, harness: &str) -> Response {
        let body = json!({
            "resource": self.config.resource,
            "authorization_servers": [self.config.issuer],
            "scopes_supported": [self.config.scope],
            "bearer_methods_supported": ["header"],
            "resource_name": format!("Everett gateway ({harness})"),
        });
        (
            [("content-type", "application/json"), ("cache-control", "max-age=300")],
            body.to_string(),
        )
            .into_response()
    }

    fn challenge(&self, error: Option<&str>) -> Response {
        let mut value = format!(
            "Bearer resource_metadata=\"{}\", scope=\"{}\"",
            self.metadata_url, self.config.scope
        );
        if let Some(description) = error {
            value += &format!(", error=\"invalid_token\", error_description=\"{description}\"");
        }
        let body = format!("OAuth access token required: {}", error.unwrap_or("missing bearer token"));
        (StatusCode::UNAUTHORIZED, [("www-authenticate", value)], body).into_response()
    }

    /// Validates on every request so owner revocation applies to tokens already issued.
    async fn verify(self: &Arc<Self>, token: String) -> Result<OAuthAgent, Rejection> {
        let malformed = (StatusCode::UNAUTHORIZED, "malformed access token");
        if token.len() != 68 || !token.starts_with("eva_") || !token[4..].bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(malformed);
        }
        let verifier = self.clone();
        let data = tokio::task::spawn_blocking(move || verifier.introspect(&token))
            .await
            .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "authorization server unavailable"))??;
        let denied = |reason| Err((StatusCode::UNAUTHORIZED, reason));
        if data["active"] != true {
            return denied("token inactive, expired or revoked");
        }
        if self.config.owner_account.as_ref().is_some_and(|owner| data["everett_account"].as_str() != Some(owner.as_str())) {
            return denied("token account is not this gateway owner");
        }
        if data["iss"] != self.config.issuer.as_str() {
            return denied("token issuer mismatch");
        }
        let audience = match &data["aud"] {
            Value::String(aud) => aud == &self.config.resource,
            Value::Array(auds) => auds.iter().any(|a| a == self.config.resource.as_str()),
            _ => false,
        };
        if !audience {
            return denied("token audience is not this gateway");
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(u64::MAX, |d| d.as_secs());
        if data["exp"].as_u64().is_none_or(|exp| exp <= now) {
            return denied("token expired");
        }
        if !data["scope"]
            .as_str()
            .is_some_and(|scope| scope.split_whitespace().any(|s| s == self.config.scope))
        {
            return denied("token lacks the required scope");
        }
        match (data["sub"].as_str(), data["everett_agent"].as_str(), data["client_id"].as_str()) {
            (Some(sub), Some(agent), Some(client)) if sub == agent && crate::adapters::external::valid_id(agent) => Ok(OAuthAgent {
                expires_at: data["exp"].as_u64().expect("expiry verified above"),
                agent_id: agent.to_owned(),
                account: data["everett_account"].as_str().filter(|s| !s.is_empty() && s.len() <= 256).map(str::to_owned),
                name: data["everett_agent_name"].as_str().unwrap_or_default().chars().take(100).collect(),
                client_id: client.to_owned(),
            }),
            _ => denied("token carries no valid Everett agent identity"),
        }
    }

    fn introspect(&self, token: &str) -> Result<Value, Rejection> {
        let unavailable = (StatusCode::SERVICE_UNAVAILABLE, "authorization server unavailable");
        let mut response = self
            .agent
            .post(&self.endpoint)
            .header("Authorization", &format!("Bearer {}", self.secret))
            .send_form([("token", token)])
            .map_err(|_| unavailable)?;
        if response.status() != 200 {
            return Err(unavailable);
        }
        response
            .body_mut()
            .with_config()
            .limit(16 * 1024)
            .read_json()
            .map_err(|_| unavailable)
    }
}

fn host_and_origin(headers: &axum::http::HeaderMap, binding: &Binding) -> Option<Response> {
    if !headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| binding.allowed_hosts.iter().any(|allowed| allowed == h))
    {
        return Some((StatusCode::MISDIRECTED_REQUEST, "Invalid Host").into_response());
    }
    if headers.get("origin").is_some_and(|h| {
        !h.to_str().ok().is_some_and(|origin| {
            binding
                .allowed_origins
                .iter()
                .any(|allowed| allowed == origin)
        })
    }) {
        return Some((StatusCode::FORBIDDEN, "Invalid Origin").into_response());
    }
    None
}

async fn http_boundary(
    State(state): State<HttpBoundary>,
    mut request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    for key in SECURITY_HEADERS {
        if headers.get_all(*key).iter().count() > 1 {
            return (StatusCode::BAD_REQUEST, "Duplicate security header").into_response();
        }
    }
    let binding = &state.gateway.binding;
    let bearer = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split_once(' '))
        .filter(|(scheme, token)| scheme.eq_ignore_ascii_case("bearer") && token.len() <= 512)
        .map(|(_, token)| token.to_owned());
    let event_token = bearer.clone();
    let agent = match &state.auth {
        HttpAuth::Bearer(digest) => {
            if !bearer.is_some_and(|token| bool::from(digest.ct_eq(&Sha256::digest(token.as_bytes())))) {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            None
        }
        HttpAuth::OAuth(verifier) => {
            if let Some(rejected) = host_and_origin(headers, binding) {
                return rejected;
            }
            if request.uri().path().starts_with(PROTECTED_RESOURCE) {
                return if request.method() == Method::GET {
                    verifier.metadata(&binding.harness)
                } else {
                    StatusCode::METHOD_NOT_ALLOWED.into_response()
                };
            }
            let Some(token) = bearer else {
                return verifier.challenge(None);
            };
            match verifier.verify(token).await {
                Ok(agent) if binding.conversation_identity && agent.client_id.is_empty() => {
                    return verifier.challenge(Some("token carries no OAuth client identity"));
                }
                Ok(agent) if !binding.destinations.contains(&agent.agent_id) => Some(agent),
                Ok(_) => return verifier.challenge(Some("token agent is also a configured destination")),
                Err((StatusCode::UNAUTHORIZED, reason)) => return verifier.challenge(Some(reason)),
                Err((status, reason)) => return (status, reason).into_response(),
            }
        }
    };
    let headers = request.headers();
    if let Some(rejected) = host_and_origin(headers, binding) {
        return rejected;
    }
    if !binding.conversation_identity && headers.contains_key(conversation::HANDLE_HEADER) {
        return (StatusCode::BAD_REQUEST, "Private conversation transport is disabled").into_response();
    }
    if !matches!(
        *request.method(),
        Method::POST | Method::GET | Method::DELETE
    ) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let _permit = match state.requests.try_acquire() {
        Ok(permit) => permit,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if headers.get("content-length").is_some_and(|h| {
        h.to_str()
            .ok()
            .and_then(|h| h.parse::<usize>().ok()).is_none_or(|size| size > MAX_BODY)
    }) {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let body = std::mem::replace(request.body_mut(), Body::empty());
    let bytes = match timeout(Duration::from_secs(5), to_bytes(body, MAX_BODY)).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        Err(_) => return StatusCode::GATEWAY_TIMEOUT.into_response(),
    };
    if request.uri().path() == conversation::LIFECYCLE_PATH {
        return conversation::manage(&state, request.method(), request.headers(), &bytes, agent.as_ref()).await;
    }
    if let Some(response) = event_http::dispatch(&state, request.method(), request.uri(), request.headers(), &bytes, agent.as_ref(), event_token).await {
        return response;
    }
    let advertise_events = state.events.is_some()
        && request.method() == Method::POST
        && request.uri().path() == "/mcp"
        && serde_json::from_slice::<Value>(&bytes).is_ok_and(|v| {
            matches!(v["method"].as_str(), Some("initialize" | "server/discover"))
        });
    *request.body_mut() = Body::from(bytes);
    if let Some(agent) = agent {
        request.extensions_mut().insert(agent);
    }
    match timeout(
        Duration::from_secs_f64(binding.timeout + 5.0),
        next.run(request),
    )
    .await
    {
        Ok(response) if advertise_events => event_http::advertise(response).await,
        Ok(response) => response,
        Err(_) => StatusCode::GATEWAY_TIMEOUT.into_response(),
    }
}

async fn serve(binding: Binding, transport: &str, port: u16) -> Result<(), String> {
    let mut gateway = Gateway {
        workers: Arc::new(Semaphore::new(binding.max_workers)),
        binding: Arc::new(binding),
        oauth: false,
    };
    if transport == "stdio" && gateway.binding.agent_tool_permissions.is_some() {
        return Err("agent_tool_permissions requires authenticated HTTP".into());
    }
    if transport == "stdio" && gateway.binding.conversation_identity {
        return Err("conversation_identity requires authenticated HTTP".into());
    }
    if transport == "stdio" && gateway.binding.events {
        return Err("Events require authenticated HTTP".into());
    }
    if transport == "stdio" {
        gateway
            .serve((
                BoundedRead {
                    inner: tokio::io::stdin(),
                    bytes: 0,
                    limit: MAX_BODY,
                },
                tokio::io::stdout(),
            ))
            .await
            .map_err(|_| "gateway stdio initialization failed")?
            .waiting()
            .await
            .map_err(|_| "gateway stdio transport failed")?;
        return Ok(());
    }
    let auth = if let Some(oauth) = gateway.binding.oauth.clone() {
        if std::env::var_os("EVERETT_GATEWAY_TOKEN").is_some() {
            return Err("oauth in dot.json replaces EVERETT_GATEWAY_TOKEN; unset one of them".into());
        }
        let secret = std::env::var("EVERETT_OAUTH_INTROSPECTION_SECRET")
            .ok()
            .filter(|s| strong_secret(s))
            .ok_or("OAuth HTTP requires EVERETT_OAUTH_INTROSPECTION_SECRET of 32..512 printable ASCII characters")?;
        gateway.oauth = true;
        HttpAuth::OAuth(Arc::new(OAuthVerifier::new(oauth, secret)?))
    } else {
        let token = std::env::var("EVERETT_GATEWAY_TOKEN")
            .map_err(|_| "HTTP requires EVERETT_GATEWAY_TOKEN, or oauth in dot.json")?;
        if !strong_secret(&token) {
            return Err(
                "HTTP requires a bearer token of 32..512 printable ASCII characters without whitespace"
                    .into(),
            );
        }
        HttpAuth::Bearer(Sha256::digest(token.as_bytes()).into())
    };
    let identity_admin = if gateway.binding.conversation_identity {
        let digest = conversation::admin_digest()?;
        if let HttpAuth::Bearer(bearer_digest) = &auth {
            if bool::from(digest.ct_eq(bearer_digest)) {
                return Err("conversation identity admin credential must differ from the connection bearer".into());
            }
            let binding = Arc::make_mut(&mut gateway.binding);
            binding.identity_owner = Some(format!("{}:{}", binding.agent_id, hex::encode(bearer_digest)));
        }
        Some(digest)
    } else {
        None
    };
    let connection_limit = gateway.binding.max_workers * 2;
    let event_scope = match &auth {
        HttpAuth::Bearer(digest) => hex::encode(digest),
        HttpAuth::OAuth(v) => serde_json::to_string(&(&v.config.issuer, &v.config.resource)).unwrap(),
    };
    let events = if gateway.binding.events {
        Some(crate::gateway_events::Events::open(&gateway.binding.home, &event_scope)?)
    } else { None };
    let state = HttpBoundary {
        events,
        event_scope,
        auth,
        identity_admin,
        requests: Arc::new(Semaphore::new(gateway.binding.max_workers * 2)),
        event_requests: Arc::new(Semaphore::new(gateway.binding.max_workers)),
        gateway: gateway.clone(),
    };
    if state.events.is_some() { tokio::spawn(event_http::pump(state.clone())); }
    let mut config = StreamableHttpServerConfig::default();
    config.legacy_session_mode = false;
    config.json_response = true;
    config.max_request_body_bytes = MAX_BODY;
    config.allowed_hosts = gateway.binding.allowed_hosts.clone();
    config.allowed_origins = gateway.binding.allowed_origins.clone();
    config = config.enforce_origin_validation();
    let service = StreamableHttpService::new(
        move || Ok(gateway.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let metadata = || async { StatusCode::NOT_FOUND };
    let app = Router::new()
        .route(PROTECTED_RESOURCE, get(metadata))
        .route(&format!("{PROTECTED_RESOURCE}/{{*path}}"), get(metadata))
        .route(conversation::LIFECYCLE_PATH, axum::routing::any(metadata))
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, http_boundary));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|_| "cannot bind loopback gateway listener")?;
    eprintln!(
        "everett.gateway: listening on 127.0.0.1:{}",
        listener
            .local_addr()
            .map_err(|_| "cannot read listener address")?
            .port()
    );
    let connections = Arc::new(Semaphore::new(connection_limit));
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|_| "gateway HTTP accept failed")?;
        let permit = match connections.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let _ = timeout(Duration::from_secs(1), stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")).await;
                continue;
            }
        };
        let app = app.clone();
        tokio::spawn(async move {
            let _permit = permit;
            match read_http_head(&mut stream).await {
                Ok(prefix) => {
                    let io = hyper_util::rt::TokioIo::new(PrefixedStream {
                        prefix: Cursor::new(prefix),
                        inner: stream,
                    });
                    let service = hyper_util::service::TowerToHyperService::new(app);
                    let mut builder = hyper::server::conn::http1::Builder::new();
                    builder.keep_alive(false);
                    let _ = timeout(
                        Duration::from_secs(45),
                        builder.serve_connection(io, service),
                    )
                    .await;
                }
                Err(status) => {
                    let response = format!(
                        "HTTP/1.1 {} {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("Rejected")
                    );
                    let _ = timeout(
                        Duration::from_secs(1),
                        stream.write_all(response.as_bytes()),
                    )
                    .await;
                }
            }
        });
    }
}

struct PrefixedStream {
    prefix: Cursor<Vec<u8>>,
    inner: tokio::net::TcpStream,
}

impl AsyncRead for PrefixedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.prefix.position() < self.prefix.get_ref().len() as u64 {
            Pin::new(&mut self.prefix).poll_read(cx, buf)
        } else {
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }
}

impl AsyncWrite for PrefixedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

async fn read_http_head(stream: &mut tokio::net::TcpStream) -> Result<Vec<u8>, StatusCode> {
    // Hyper combines duplicate Content-Length headers before application validation.
    let read = async {
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let count = stream
                .read(&mut chunk)
                .await
                .map_err(|_| StatusCode::BAD_REQUEST)?;
            if count == 0 {
                return Err(StatusCode::BAD_REQUEST);
            }
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                if end + 4 > MAX_CONFIG {
                    return Err(StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);
                }
                let mut headers = [httparse::EMPTY_HEADER; 64];
                let mut request = httparse::Request::new(&mut headers);
                request
                    .parse(&bytes[..end + 4])
                    .map_err(|_| StatusCode::BAD_REQUEST)?;
                for key in SECURITY_HEADERS {
                    if request
                        .headers
                        .iter()
                        .filter(|header| header.name.eq_ignore_ascii_case(key))
                        .count()
                        > 1
                    {
                        return Err(StatusCode::BAD_REQUEST);
                    }
                }
                if request
                    .headers
                    .iter()
                    .any(|header| header.name.eq_ignore_ascii_case("content-length"))
                    && request
                        .headers
                        .iter()
                        .any(|header| header.name.eq_ignore_ascii_case("transfer-encoding"))
                {
                    return Err(StatusCode::BAD_REQUEST);
                }
                return Ok(bytes);
            }
            if bytes.len() > MAX_CONFIG {
                return Err(StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);
            }
        }
    };
    timeout(Duration::from_secs(5), read)
        .await
        .unwrap_or(Err(StatusCode::GATEWAY_TIMEOUT))
}

pub fn run(config: &Path, transport: &str, port: u16) -> i32 {
    let result = Binding::load(config).and_then(|binding| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|_| "cannot start gateway runtime".to_string())?
            .block_on(serve(binding, transport, port))
    });
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("everett.gateway: {error}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(home: &Path) -> Binding {
        Binding {
            events: false,
            require_identity_handle: false,
            conversation_identity: false,
            conversation_worker: None,
            agent_id: "dot-one".into(),
            identity_owner: None,
            harness: "openai-dot".into(),
            cwd: home.into(),
            home: home.into(),
            project: "everett".into(),
            destinations: BTreeSet::from(["agent-two".into()]),
            allow_shared_core: true,
            agent_tool_permissions: None,
            backend_command: vec!["/bin/false".into()],
            max_workers: 2,
            timeout: 1.0,
            allowed_hosts: default_hosts(),
            allowed_origins: vec![],
            oauth: None,
            native_start: None,
        }
    }

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn authorizes_only_bound_tools_and_identity() {
        let home = tempfile::tempdir().unwrap();
        let mut b = binding(home.path());
        assert_eq!(
            b.authorize("everett_core", Map::new()).unwrap()["project"],
            "everett"
        );
        for name in [
            "everett_ls",
            "everett_route",
            "everett_learn",
            "everett_recall",
            "everett_event",
            "everett_subscribe",
            "everett_search",
            "everett_peers",
            "everett_broadcast",
            "everett_tasks",
        ] {
            assert!(b.authorize(name, Map::new()).is_err());
        }
        for name in ["everett_inbox", "everett_card", "everett_send"] {
            assert!(b
                .authorize(name, args(json!({"session_id":"other"})))
                .is_err());
        }
        assert!(b
            .authorize("everett_core", args(json!({"project":"other"})))
            .is_err());
        b.allow_shared_core = false;
        assert!(b.authorize("everett_core", Map::new()).is_err());
        assert_eq!(b.tools().len(), 6);
        assert!(b
            .tools()
            .iter()
            .any(|tool| tool.name == "everett_directory"));
        let ids = b
            .authorize("everett_directory", Map::new())
            .unwrap()["ids"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(ids, vec![json!("agent-two")]);
        assert!(b
            .authorize("everett_directory", args(json!({"ids":["other"]})))
            .is_err());
    }

    #[test]
    fn send_is_explicit_allowlisted_and_inbox_only() {
        let home = tempfile::tempdir().unwrap();
        let b = binding(home.path());
        let safe = b
            .authorize(
                "everett_send",
                args(json!({"text":"task","to":"agent-two"})),
            )
            .unwrap();
        assert_eq!(safe["mode"], "inbox");
        assert_eq!(safe["spawn"], false);
        assert_eq!(safe["wait"], 0);
        for value in [
            json!({"text":"task"}),
            json!({"text":"task","to":"agent"}),
            json!({"text":"task","to":"dot-one"}),
            json!({"text":"task","to":"agent-two","reply_to":"message-one"}),
        ] {
            assert!(b.authorize("everett_send", args(value)).is_err());
        }
        for key in [
            "spawn", "dir", "harness", "mode", "wait", "timeout", "router",
        ] {
            let mut value = args(json!({"text":"task","to":"agent-two"}));
            value.insert(key.into(), json!(false));
            assert!(b.authorize("everett_send", value).is_err());
        }
    }

    #[test]
    fn replies_require_unique_owned_message_and_authorized_sender() {
        let home = tempfile::tempdir().unwrap();
        let b = binding(home.path());
        let folder = home.path().join(".everett/inbox");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("dot-one.jsonl");
        let own = json!({"id":"one","to":"dot-one","from":"agent-two"}).to_string() + "\n";
        std::fs::write(&path, &own).unwrap();
        assert_eq!(b.reply_message("one").unwrap()["from"], "agent-two");
        for invalid in [
            own.clone() + &own,
            json!({"id":"one","to":"dot-one","from":"agent-two","relay_device":"remote"}).to_string(),
            json!({"id":"one","to":"dot-one","from":"agent-two","peer_client":"remote"}).to_string(),
            "not json\n".into(),
            json!({"id":"one","to":"foreign","from":"agent-two"}).to_string(),
            json!({"id":"one","to":"dot-one","from":"foreign"}).to_string(),
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(b.reply_message("one").is_err());
        }
        std::fs::write(&path, vec![b' '; MAX_INBOX + 1]).unwrap();
        assert!(b.reply_message("one").is_err());
    }

    #[test]
    fn text_and_argument_types_are_bounded() {
        let home = tempfile::tempdir().unwrap();
        let b = binding(home.path());
        for text in [json!(" "), json!(true), json!("x".repeat(16001))] {
            assert!(b
                .authorize("everett_send", args(json!({"text":text,"to":"agent-two"})))
                .is_err());
        }
        assert!(b
            .authorize("everett_inbox", args(json!({"peek":1})))
            .is_err());
        assert!(b
            .authorize(
                "everett_card",
                args(json!({"what":"x","state":" ","next":"x"}))
            )
            .is_err());
    }

    #[test]
    fn host_and_origin_allowlists_are_exact() {
        for host in [
            "",
            "*.example.com",
            "example.com/path",
            "user@example.com",
            "example.com:99999",
            "example.com?x",
            "bad host",
            "é.com",
        ] {
            assert!(!valid_authority(host), "{host}");
        }
        for host in ["127.0.0.1:8788", "example.com", "[::1]:8788"] {
            assert!(valid_authority(host), "{host}");
        }
        for origin in [
            "https://example.com/path",
            "https://example.com/",
            "https://user@example.com",
            "null",
            "ftp://example.com",
        ] {
            assert!(!valid_origin(origin), "{origin}");
        }
        assert!(valid_origin("https://example.com:8443"));
    }

    #[tokio::test]
    async fn framing_limit_resets_only_at_newlines() {
        use tokio::io::AsyncReadExt;
        let mut bounded = BoundedRead {
            inner: &b"1234\n1234\n"[..],
            bytes: 0,
            limit: 5,
        };
        let mut output = Vec::new();
        bounded.read_to_end(&mut output).await.unwrap();
        let mut oversized = BoundedRead {
            inner: &b"123456"[..],
            bytes: 0,
            limit: 5,
        };
        assert!(oversized.read_to_end(&mut Vec::new()).await.is_err());
    }
}
