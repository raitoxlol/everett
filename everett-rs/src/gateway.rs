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
const SECURITY_HEADERS: &[&str] = &[
    "host",
    "origin",
    "authorization",
    "content-length",
    "transfer-encoding",
];
const BACKEND_ERROR: &str = "backend unavailable, incompatible, or request timed out";
const INSTRUCTIONS: &str = "This connection is bound to one logical agent. Read its inbox for handoffs; reply using reply_to. Send only to authorized full IDs. No spawn/resume, automatic routing, or automatic bot wake-up. Shared core, when enabled, includes the owner's global core.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    agent_id: String,
    harness: String,
    cwd: PathBuf,
    project: String,
    destinations: BTreeSet<String>,
    allow_shared_core: bool,
    #[serde(default = "default_backend_command")]
    backend_command: Vec<String>,
    #[serde(skip)]
    home: PathBuf,
    #[serde(default = "default_workers")]
    max_workers: usize,
    #[serde(default = "default_timeout")]
    timeout: f64,
    #[serde(default = "default_hosts")]
    allowed_hosts: Vec<String>,
    #[serde(default)]
    allowed_origins: Vec<String>,
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

fn valid_destination(id: &str) -> bool {
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
        Ok(binding)
    }

    fn reply_destination(&self, message_id: &str) -> Result<String, String> {
        if message_id.is_empty() || message_id.chars().count() > 128 {
            return Err("invalid reply message ID".into());
        }
        let path = self
            .home
            .join(".everett/inbox")
            .join(format!("{}.jsonl", self.agent_id));
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
        Ok(sender.into())
    }

    fn authorize(
        &self,
        name: &str,
        mut args: Map<String, Value>,
    ) -> Result<Map<String, Value>, String> {
        let allowed: &[&str] = match name {
            "everett_whoami" => &[],
            "everett_core" if self.allow_shared_core => &["project"],
            "everett_inbox" => &["session_id", "peek"],
            "everett_card" => &["session_id", "what", "state", "next"],
            "everett_send" => &["session_id", "text", "to", "reply_to"],
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
        if name == "everett_inbox" && args.get("peek").is_some_and(|peek| !peek.is_boolean()) {
            return Err("peek must be a boolean".into());
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
                    self.reply_destination(reply.as_str().ok_or("invalid reply message ID")?)?;
                }
                _ => {
                    return Err(
                        "provide exactly one authorized full destination ID or owned reply_to"
                            .into(),
                    )
                }
            }
            args.extend([
                ("mode".into(), json!("inbox")),
                ("wait".into(), json!(0)),
                ("spawn".into(), json!(false)),
            ]);
        }
        Ok(args)
    }

    fn tools(&self) -> Vec<Tool> {
        let own = json!({"type":"string", "enum":[self.agent_id]});
        let short = json!({"type":"string", "minLength":1, "maxLength":2000});
        let specs = [
            ("everett_whoami", "Read this connection's logical owner-scoped identity, not vendor-attested Bot identity.", json!({}), vec![]),
            ("everett_core", "Read owner-wide GLOBAL shared core plus this connection's fixed project core.", json!({"project":{"type":"string","enum":[self.project]}}), vec![]),
            ("everett_inbox", "Read only this agent's inbox. By default mark messages delivered; use peek=true to retain them.", json!({"session_id":own,"peek":{"type":"boolean"}}), vec![]),
            ("everett_card", "Update only this agent's session card.", json!({"session_id":own,"what":short,"state":short,"next":short}), vec!["what","state","next"]),
            ("everett_send", "Queue an inbox-only message to an authorized full ID, or reply to an owned message. No automatic bot wake-up.", json!({"session_id":own,"text":{"type":"string","minLength":1,"maxLength":16000},"to":{"type":"string","enum":self.destinations},"reply_to":{"type":"string","minLength":1,"maxLength":128}}), vec!["text"]),
        ];
        specs.into_iter().filter(|(name, ..)| *name != "everett_core" || self.allow_shared_core)
            .map(|(name, description, properties, required)| serde_json::from_value(json!({
                "name":name,"description":description,
                "inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
                "annotations":{"readOnlyHint":matches!(name,"everett_core"|"everett_whoami"),"openWorldHint":false}
            })).expect("gateway tool definitions are valid")).collect()
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

#[derive(Clone)]
struct Gateway {
    binding: Arc<Binding>,
    workers: Arc<Semaphore>,
}

impl Gateway {
    async fn call(&self, name: &str, arguments: Map<String, Value>) -> Result<Value, String> {
        let args = self.binding.authorize(name, arguments)?;
        let _permit = self
            .workers
            .try_acquire()
            .map_err(|_| "gateway workers busy; retry later")?;
        let binding = &self.binding;
        let mut command = Command::new(&binding.backend_command[0]);
        command
            .args(&binding.backend_command[1..])
            .env_clear()
            .current_dir(&binding.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for key in [
            "PATH", "USER", "LOGNAME", "SHELL", "TMPDIR", "TMP", "TEMP", "LANG",
        ] {
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
            .env("EVERETT_GATEWAY_EXACT_IDS", "1");
        let mut child = command.spawn().map_err(|_| BACKEND_ERROR)?;
        let stdout = child.stdout.take().ok_or(BACKEND_ERROR)?;
        let stdin = child.stdin.take().ok_or(BACKEND_ERROR)?;
        let operation = async {
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
            let result = async {
                if name == "everett_send" && args.contains_key("to") {
                    let info = peer.peer_info().ok_or(BACKEND_ERROR)?;
                    let enforced = info.capabilities.experimental.as_ref()
                        .and_then(|caps| caps.get("everettExactDestinationIds"))
                        .and_then(|cap| cap.get("enforced"));
                    if enforced != Some(&Value::Bool(true)) { return Err("backend lacks strict exact-ID delivery; owned-message replies remain available".into()); }
                }
                if let Some(reply) = args.get("reply_to").and_then(Value::as_str) { binding.reply_destination(reply)?; }
                let response = peer.call_tool_once(CallToolRequestParams::new(name.to_owned()).with_arguments(args))
                    .await.map_err(|_| BACKEND_ERROR)?;
                let CallToolResponse::Complete(result) = response else { return Err(BACKEND_ERROR.into()); };
                if result.is_error == Some(true) { return Err("backend rejected the tool call".into()); }
                if serde_json::to_vec(&result).map_err(|_| BACKEND_ERROR)?.len() > MAX_RESULT { return Err("backend result exceeds 256 KiB".into()); }
                let data = result.structured_content.filter(Value::is_object).ok_or("backend must return structured tool results")?;
                if name == "everett_whoami" && (data["session_id"] != binding.agent_id || data["harness"] != binding.harness) { return Err("backend identity differs from binding".into()); }
                if name == "everett_core" {
                    return Ok(json!({"core":data["core"].as_str().ok_or("backend must return shared core text")?}));
                }
                Ok(data)
            }.await;
            peer.cancellation_token().cancel();
            result
        };
        let result = timeout(Duration::from_secs_f64(binding.timeout), operation)
            .await
            .unwrap_or_else(|_| Err(BACKEND_ERROR.into()));
        let _ = child.kill().await;
        let _ = child.wait().await;
        result
    }
}

impl ServerHandler for Gateway {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "everett-owner-gateway",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.binding.tools()))
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let mut result = match self
            .call(&request.name, request.arguments.unwrap_or_default())
            .await
        {
            Ok(data) => CallToolResult::structured(data),
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
struct HttpBoundary {
    gateway: Gateway,
    digest: [u8; 32],
    requests: Arc<Semaphore>,
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
    let Some((scheme, token)) = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split_once(' '))
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.len() > 512
        || !bool::from(state.digest.ct_eq(&Sha256::digest(token.as_bytes())))
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let binding = &state.gateway.binding;
    if !headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| binding.allowed_hosts.iter().any(|allowed| allowed == h))
    {
        return (StatusCode::MISDIRECTED_REQUEST, "Invalid Host").into_response();
    }
    if headers.get("origin").is_some_and(|h| {
        !h.to_str().ok().is_some_and(|origin| {
            binding
                .allowed_origins
                .iter()
                .any(|allowed| allowed == origin)
        })
    }) {
        return (StatusCode::FORBIDDEN, "Invalid Origin").into_response();
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
    *request.body_mut() = Body::from(bytes);
    match timeout(
        Duration::from_secs_f64(binding.timeout + 5.0),
        next.run(request),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => StatusCode::GATEWAY_TIMEOUT.into_response(),
    }
}

async fn serve(binding: Binding, transport: &str, port: u16) -> Result<(), String> {
    let gateway = Gateway {
        workers: Arc::new(Semaphore::new(binding.max_workers)),
        binding: Arc::new(binding),
    };
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
    let token = std::env::var("EVERETT_GATEWAY_TOKEN")
        .map_err(|_| "HTTP requires EVERETT_GATEWAY_TOKEN")?;
    if !(32..=512).contains(&token.len())
        || !token.is_ascii()
        || token
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err(
            "HTTP requires a bearer token of 32..512 printable ASCII characters without whitespace"
                .into(),
        );
    }
    let connection_limit = gateway.binding.max_workers * 2;
    let state = HttpBoundary {
        digest: Sha256::digest(token.as_bytes()).into(),
        requests: Arc::new(Semaphore::new(gateway.binding.max_workers * 2)),
        gateway: gateway.clone(),
    };
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
    let app = Router::new()
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
            agent_id: "dot-one".into(),
            harness: "openai-dot".into(),
            cwd: home.into(),
            home: home.into(),
            project: "everett".into(),
            destinations: BTreeSet::from(["agent-two".into()]),
            allow_shared_core: true,
            backend_command: vec!["/bin/false".into()],
            max_workers: 2,
            timeout: 1.0,
            allowed_hosts: default_hosts(),
            allowed_origins: vec![],
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
            "everett_event",
            "everett_subscribe",
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
        assert_eq!(b.tools().len(), 4);
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
        assert_eq!(b.reply_destination("one").unwrap(), "agent-two");
        for invalid in [
            own.clone() + &own,
            "not json\n".into(),
            json!({"id":"one","to":"foreign","from":"agent-two"}).to_string(),
            json!({"id":"one","to":"dot-one","from":"foreign"}).to_string(),
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(b.reply_destination("one").is_err());
        }
        std::fs::write(&path, vec![b' '; MAX_INBOX + 1]).unwrap();
        assert!(b.reply_destination("one").is_err());
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
