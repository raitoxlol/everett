# Optional owner-bound MCP gateway

This gateway lets an existing cloud agent call Everett's **Rust stdio backend**.
It does not create an OpenAI or xAI model instance. Normal Everett installs do
not import or require the gateway SDK.

Install the optional official MCP Python SDK integration in a separate environment:

```sh
python3 -m venv .gateway-venv
.gateway-venv/bin/python -m pip install '.[gateway]'
```

Use Python 3.10+ and an installed Rust Everett binary. Do not point
`backend_command` at the Python fallback or at the gateway itself. The executable
and its arguments are trusted owner configuration, never tool-call arguments.

## One process, one logical identity

Create a local, owner-controlled JSON configuration. Replace these example paths
and destination IDs with absolute paths and complete authorized session IDs:

```json
{
  "agent_id": "wright-dot",
  "harness": "openai-dot",
  "cwd": "/absolute/path/to/everett",
  "project": "everett",
  "destinations": ["complete-local-agent-id"],
  "allow_shared_core": true,
  "backend_command": ["/absolute/path/to/rust/everett", "mcp"],
  "max_workers": 2,
  "timeout": 20,
  "allowed_hosts": ["127.0.0.1:8788"],
  "allowed_origins": []
}
```

Keep the configuration and executable writable only by the trusted local owner.
`agent_id` must match `[A-Za-z0-9_-]{1,100}` and cannot be `human` or `live`
(case-insensitive); `harness` is `openai-dot` or
`grok-bot`. The fixed project slug is lowercase, begins with a letter or digit,
and contains only letters, digits, `_`, or `-`, up to 64 characters.
Destinations are exact, complete IDs, not titles, folder names, or prefixes.

`allow_shared_core: true` explicitly permits the **owner-wide global core plus
the named project core**. This is not project-only memory isolation. Setting it
to false removes the core tool entirely; this pass does not implement a
project-only reader. Shared core should contain no secrets.

The gateway uses `EVERETT_HOME`, or the current user's home, captured at startup.
Every tool call uses a new SDK-managed stdio worker with that home, the fixed
working directory, and immutable `EVERETT_SESSION_ID`/`EVERETT_HARNESS_NAME`.
It does not mutate `os.environ` or forward API keys or gateway bearer tokens to
the backend. The SDK additionally inherits its documented basic OS environment
(such as PATH and USER). The Rust worker is trusted local code with access to
the owner's stores; this is not an OS sandbox against a malicious executable.

Run a separate gateway process/configuration for each logical agent. A token or
tunnel connection identifies this owner-configured binding, **not a trusted
vendor Bot or conversation identity**. Other Bots authorized to use the same
connector may use its binding. The gateway does not manufacture per-Bot security
isolation from a shared owner account.

The external-session registration contract is:

```text
~/.everett/external/<agent_id>.json
{id, harness: "openai-dot" | "grok-bot", title, cwd, updated: epoch_seconds}
```

Use the native `everett external add`, `list`, and `remove` commands to manage
registrations (or `python -m everett.external` with the Python package). The
gateway does not write registrations, invent local CLI transcripts, or resume
either cloud Bot. Register the same ID before end-to-end
handoffs. External registration is durable, requires no periodic timestamp refresh,
and is not filtered by local session age. A connector alone does not make a session
discoverable to local agents.

## Allowed tools and delivery

- `everett_whoami`: reads this connection's logical identity.
- `everett_core`: reads the granted global and fixed project core.
- `everett_inbox`: reads only this binding's inbox. Existing backend semantics
  mark messages delivered unless `peek=true`; this is not a transactional work
  acknowledgement. A failure after consuming a message can require owner retry.
- `everett_card`: writes only this binding's card.
- `everett_send`: explicit destination or authorized `reply_to`, inbox-only,
  nonblocking, never spawn or headless resume. Idle recipients must check inboxes.

Foreign `session_id`, extra arguments, arbitrary working directories, router
selection, spawn flags, and resume modes are rejected. `ls`, `route`, learning,
events, and subscriptions are not exposed. No cloud-boundary text is routed to
Jev. Replies require one unique message addressed to this binding in its own
durable inbox, with the original sender in the destination allowlist.

**Direct sends require an enforced exact-ID backend capability.** Older Rust
`everett_send(to=...)` can fall back to prefix/title matching when an exact ID
disappears. To permit direct sends, the backend must advertise
`capabilities.experimental.everettExactDestinationIds: {"enforced": true}` on initialize and
actually enforce exact-ID-only delivery for gateway processes. The gateway sets
`EVERETT_GATEWAY_EXACT_IDS=1` for this integration. A preflight listing alone
cannot eliminate that disappearance race. Owned-message replies, memory, inbox,
identity, and cards do not require this capability.

## Private stdio / OpenAI Secure MCP Tunnel

```sh
.gateway-venv/bin/python -m everett.gateway --config /absolute/path/to/dot.json
```

This has no HTTP listener. The owner-authorized tunnel runs this command inside
the host that contains Everett's stores. Follow [OpenAI dots setup](openai-dots.md).
The tunnel's permissions and control-plane authentication authorize reachability;
the gateway then enforces its local binding. Restrict the tunnel to the intended
owner/workspace. This pass does not add end-user OAuth to stdio.

## Loopback Streamable HTTP / Grok Remote HTTPS

HTTP always requires a separately provisioned high-entropy bearer token in
`EVERETT_GATEWAY_TOKEN` (32–512 ASCII characters without whitespace). Generate and
store it securely outside the repository; never put it in command arguments, URLs,
configuration examples, or logs. There is no token issuance or OAuth service here.

```sh
# Set EVERETT_GATEWAY_TOKEN securely in this process's environment first.
.gateway-venv/bin/python -m everett.gateway \
  --config /absolute/path/to/grok.json --transport http --port 8788
```

The listener is always `127.0.0.1`; there is no public-bind override. `/mcp` uses the
official SDK's stateless Streamable HTTP transport and SDK bearer authentication
middleware. Missing or invalid authentication fails; there is no HTTP no-auth mode.
The gateway does not serve OAuth metadata, authorization, or token endpoints.

For Grok, the owner must supply a trusted HTTPS reverse proxy/tunnel, a valid TLS
certificate, and Remote HTTPS connector setup using `Authorization: Bearer …`.
Preserve Authorization and the configured Host. Add the exact public authority to
`allowed_hosts` if the proxy preserves it. Origins default to none: nonbrowser
clients may omit Origin; any present Origin must be explicitly allowlisted.
Do not disable Host/Origin checks or use wildcards. Reverse-proxy forwarded headers
are not trusted. Grok's ability to save a static credential must be confirmed in
the actual existing Bot's connector UI; Team Bot credential behavior is documented
by [xAI](https://docs.x.ai/grok-bot/team-bots).

Do not expose a local bearer token over plaintext internet HTTP. TLS/reachability,
connector authorization, network controls, and runtime approval prompts are owner
hookup, not deployments performed by this implementation. OpenAI direct public
HTTPS requiring OAuth is **not implemented**; use the Secure MCP Tunnel stdio path.

## Bounds and verification

Configuration is capped at 16 KiB; HTTP request bodies and tool arguments at
64 KiB; returned tool results at 256 KiB. HTTP body reads time out after five
seconds. Each backend operation including startup times out at 1–30 seconds,
and each process permits 1–8 workers (defaults: 20 seconds and two workers).
Excess work fails immediately rather than creating an unbounded queue. HTTP is
stateless and does not accumulate client sessions. Reply authorization scans only
this binding's inbox, capped at 8 MiB; oversized or malformed inboxes fail closed.
SDK backend transport assumes trusted local Rust output before the result cap.

The isolated tests use a synthetic subprocess backend and in-process HTTP client;
they invoke no model, live bot, tunnel, or production credentials:

```sh
python -m unittest discover -s tests -p test_gateway.py -v
```

SDK-specific cases skip without the optional extra; policy cases still run. Test
execution was deferred until the parent creates the integration PR. Tests do not
prove actual dot/Grok plan availability, TLS deployment, or runtime approval flow.

**No automatic dot wake-ups, MCP2 Events, webhook engine, Slack bridge, or external
Grok Bot messaging API is implemented.** The owner must start the bot or configure
a supported routine to check its inbox; connector access alone does not wake it.
