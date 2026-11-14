# Owner-bound Rust MCP gateway

The native `everett gateway` command lets an existing cloud agent call Everett's
Rust stdio backend. The official Rust MCP SDK is compiled into the binary.
The gateway does not
create an OpenAI or xAI model instance.

Build the native binary from this branch:

```sh
cargo build --locked --manifest-path everett-rs/Cargo.toml
everett-rs/target/debug/everett gateway --help
```

Use an installed Rust Everett binary that supports `gateway`. If `backend_command`
is omitted, the gateway starts the same binary with `mcp`. An explicit override
must use an absolute executable path. Do not point it at the gateway itself. The executable and its arguments are trusted owner
configuration, never tool-call arguments.

## One process, fixed access grants

Create a local, owner-controlled JSON configuration. Replace these example paths
and destination IDs with absolute paths and complete authorized session IDs:

```json
{
  "agent_id": "ext-dot-wright",
  "harness": "openai-dot",
  "cwd": "/absolute/path/to/everett",
  "project": "everett",
  "destinations": ["ext-grok-wright"],
  "allow_shared_core": true,
  "backend_command": ["/absolute/path/to/rust/everett", "mcp"],
  "max_workers": 2,
  "timeout": 20,
  "allowed_hosts": ["127.0.0.1:8788"],
  "allowed_origins": []
}
```

Keep the configuration and executable writable only by the trusted local owner.
A suggested path is `~/.config/everett/dot.json`. `agent_id` must be the same
string as `everett external add --id` for that binding.
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
Backend tool calls use a new SDK-managed stdio worker with that home, the fixed
working directory, and immutable `EVERETT_SESSION_ID`/`EVERETT_HARNESS_NAME`.
It does not mutate `os.environ` or forward API keys or gateway bearer tokens to
the backend. The SDK additionally inherits its documented basic OS environment
(such as PATH and USER). The Rust worker is trusted local code with access to
the owner's stores; this is not an OS sandbox against a malicious executable.

A gateway configuration grants access to fixed destinations, a directory and
shared core. The connection is authorized by its bearer token or OAuth approval.
It is not a vendor-attested Bot or conversation identity. Existing clients may
keep using the original connection identity. New chats can register separate
logical inboxes without another account sign-in.

### Register this chat

1. Generate a fresh private random `registration_key` of 32 to 128 URL-safe
   characters. Treat this key as a secret: retrying it returns the same handle.
2. Call `everett_identity(action="create", registration_key=<key>, name=<chat name>, kind="chat")`.
   Kind may also be `dot` or `codex`; it is self-declared, not a native provider ID.
3. Keep the returned `identity_handle` private in this chat. Pass it on later
   gateway calls, including `everett_whoami` and `everett_inbox`. Omit `session_id`;
   if supplied, it must match the selected identity. Handles never grant more
   destinations, memory, directory access or tool permissions.
4. The server registers the returned `session_id` in the local external directory.
   Other local clients can discover it with `everett ls` or `everett external list`.
   The local owner must still authorize exact destination IDs before chats send
   to one another. Registration does not grant new destinations or relay exposure.
5. `everett_identity(action="list")` lists this authorization's identities without
   handles. `state="registered"` means the local discovery record matches the
   identity. `state="registration_unavailable"` means that record is missing,
   invalid, or conflicts with the identity. `availability="unknown"` makes clear
   that neither state proves a chat is online or polling. Reuse the original
   registration key to repair a missing record. A corrupt or conflicting record
   fails clearly and must be repaired by the local owner; a retry never overwrites
   it or silently changes its relay destination.
   `everett_identity(action="revoke", identity_handle=<handle>)` disables one
   identity and removes its discovery registration, retaining its inbox. Retry the
   same handle to finish interrupted registration cleanup; revocation is idempotent.

Names may repeat; the server issues distinct logical IDs. Creation retries with
the same private key and parameters return the same identity. A revoked key cannot
create it again. Up to 256 identities, including revoked entries, are retained per
authorization scope. The private store is `~/.everett/gateway-identities.sqlite`.
It must be a regular file accessible only to its owner. Back it up privately with
the profile; restarting the gateway preserves handles. Losing the store loses
handle bindings. Do not share handles or registration keys in message bodies,
public cards, screenshots or logs.

OAuth identity scope uses the verified issuer, resource, account and client ID.
A fresh approval for that same account and client preserves handles, while each
request still needs a valid, unrevoked OAuth token. An older issuer without the
`everett_account` introspection claim falls back to the token's original agent
binding; fresh approvals on that older issuer do not preserve chat handles.
Fixed bearer and stdio configurations retain their configured binding scope.

The external-session registration contract is:

```text
~/.everett/external/<agent_id>.json
{id, harness: "openai-dot" | "grok-bot", title, cwd, updated: epoch_seconds}
```

Use the native `everett external add`, `list`, and `remove` commands to manage
registrations. The gateway writes registrations only for explicitly created chat
identities. It does not invent native provider transcripts or resume cloud Bots.
Register the legacy connection ID manually before using its inbox for handoffs. External registration is durable, requires no periodic timestamp refresh,
and is not filtered by local session age. A connector alone does not make a session
discoverable to local agents.

## Allowed tools and delivery

- `everett_identity`: creates, lists or revokes logical chat identities under this authorization.
- `everett_whoami`: reads the identity selected by the private handle, or the legacy connection identity.
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

Backend responses must follow MCP's schema. The SDK rejects malformed initialize
responses, including non-object experimental capability values, before any tool
call. Missing or false exact-ID enforcement in a valid capability object blocks
direct sends but still permits owned replies.

## Private stdio / OpenAI Secure MCP Tunnel

```sh
/absolute/path/to/everett gateway --config /absolute/path/to/dot.json
```

This has no HTTP listener. The owner-authorized tunnel runs this command inside
the host that contains Everett's stores. Follow [OpenAI dots setup](openai-dots.md),
including `everett tunnel watch` so a stuck control-plane poll is restarted.
`/readyz` on the tunnel client is not proof that calls are still being forwarded.
The tunnel's permissions and control-plane authentication authorize reachability;
the gateway then enforces its local binding. Restrict the tunnel to the intended
owner/workspace. This pass does not add end-user OAuth to stdio.

## Loopback Streamable HTTP / Grok Remote HTTPS

Without an `oauth` block, HTTP requires a separately provisioned high-entropy bearer token in
`EVERETT_GATEWAY_TOKEN` (32–512 ASCII characters without whitespace). Generate and
store it securely outside the repository; never put it in command arguments, URLs,
configuration examples, or logs. For OpenAI/ChatGPT, use [OAuth mode](#oauth-resource-server-mode-openai-chatgpt) instead.

```sh
# Set EVERETT_GATEWAY_TOKEN securely in this process's environment first.
/absolute/path/to/everett gateway \
  --config /absolute/path/to/grok.json --transport http --port 8788
```

The listener is always `127.0.0.1`; there is no public-bind override. `/mcp` uses the
official Rust SDK's stateless Streamable HTTP transport. The gateway verifies a
SHA-256 digest of the bearer token with a constant-time comparison before SDK
dispatch. Missing or invalid authentication fails; there is no HTTP no-auth mode.
In bearer mode the gateway serves no OAuth metadata.

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
hookup, not deployments performed by this implementation.

## OAuth resource-server mode (OpenAI / ChatGPT)

Adding an `oauth` block to the binding switches HTTP mode from the static bearer
token to OAuth 2.1 access tokens issued by the owner's hosted Everett deployment
(see [OpenAI dots setup](openai-dots.md#oauth-http-gateway-with-the-cloudflare-candidate)). stdio
ignores the block and keeps the fixed `agent_id`.

```json
{
  "agent_id": "wright-dot",
  "harness": "openai-dot",
  "cwd": "/absolute/path/to/everett",
  "project": "everett",
  "destinations": ["complete-local-agent-id"],
  "allow_shared_core": true,
  "allowed_hosts": ["127.0.0.1:8788"],
  "oauth": {
    "issuer": "https://<your-worker>.workers.dev",
    "resource": "https://<public gateway URL>/mcp"
  }
}
```

- `issuer` must equal the authorization server's configured `EVERETT_ISSUER` exactly; `resource` must
  be listed in the deployment's `EVERETT_OAUTH_RESOURCES`. Both are https (plain
  http only to a loopback host). Optional: `scope` (default `everett`) and
  `introspection_endpoint` (default `<issuer>/oauth/introspect`).
- Set `EVERETT_OAUTH_INTROSPECTION_SECRET` (32–512 printable ASCII, same value as
  the deployment's) in the gateway's environment, and **unset**
  `EVERETT_GATEWAY_TOKEN`; startup fails if both or neither are present.
- `GET /.well-known/oauth-protected-resource` (and the RFC 9728 path-suffixed
  `/.well-known/oauth-protected-resource/mcp`) returns `resource`,
  `authorization_servers: [issuer]` and `scopes_supported`, without auth but with
  the same Host/Origin checks.
- `/mcp` without a token returns `401` with
  `WWW-Authenticate: Bearer resource_metadata="<resource origin>/.well-known/oauth-protected-resource/mcp", scope="everett"`;
  a bad token adds `error="invalid_token"` and a reason (`token audience is not
  this gateway`, `token inactive, expired or revoked`, …). An unreachable
  authorization server returns `503`.
- Every request is introspected (no cache), so owner revocation applies to tokens
  already issued. The gateway checks `active`, `iss`, `aud` (= `resource`), `exp`,
  `scope`, and that `sub` equals `everett_agent` and is a valid agent ID that is not
  one of the binding's destinations. The introspection secret is never passed to
  workers.
- The token's agent ID replaces the configured legacy `agent_id`. A private
  `identity_handle` can select a chat registered under that authorization. Workers
  receive only the selected `EVERETT_SESSION_ID`, never the handle or registration
  key. `everett_whoami` retains `gateway_auth: {mode: "oauth", agent_name, client_id}`;
  its legacy `agent_name` field labels the plugin connection, not the chat.
  Project, cwd, destinations and core grant still come from the binding.
- Consent authorizes the plugin connection. Its credential label comes from the
  reviewed client metadata; it does not ask the account owner to name an agent.
  Existing credentials, their labels and revocation behavior are preserved.

The Cloudflare candidate in PR #43 implements the authorization server contract;
this gateway port does not deploy it or configure the live issuer. Its OAuth
settings include exact client IDs, redirect URIs and resources plus the private
introspection secret. Dynamic client registration is not implemented by that
Cloudflare candidate. Native CLI credentials are separate from OAuth access
credentials and cannot be used as gateway access tokens. The development issuer
and actual provider/client connection must be verified before claiming hosted
support.

## Bounds and verification

Configuration is capped at 16 KiB; HTTP request bodies, tool arguments, and stdio
request frames at 64 KiB; returned tool results at 256 KiB. HTTP headers are capped
at 16 KiB and 64 fields. Header and body reads each time out after five seconds.
Each backend operation including startup times out at 1–30 seconds,
and each process permits 1–8 workers (defaults: 20 seconds and two workers).
Excess work fails immediately rather than creating an unbounded queue. HTTP is
stateless and does not accumulate client sessions. The listener allows twice the
worker count in concurrent connections, with one request per connection and a
45-second total deadline. It checks duplicate security headers before the HTTP
parser can combine them. Reply authorization scans only
this binding's inbox, capped at 8 MiB; oversized or malformed inboxes fail closed.
Backend frames are bounded before SDK buffering, and tool results are checked
again after SDK encoding.

`everett-rs/tests/gateway_native.rs` drives the native CLI and real loopback HTTP
sockets with `PATH=/nonexistent` for the gateway process. Adversarial worker tests
use the test binary itself as a synthetic MCP backend; OAuth tests use a synthetic
loopback introspection endpoint. No test invokes a model, live bot, tunnel,
ChatGPT, or production credentials:

```sh
cargo test --locked --manifest-path everett-rs/Cargo.toml --test gateway_native
```

Tests do not
prove actual dot/Grok plan availability, TLS deployment, or runtime approval flow.

### Local Cloudflare gateway acceptance

`scripts/verify-cloudflare-gateway.py` also drives the actual gateway and native
MCP backend against the local Worker fixture from [PR #43](https://github.com/raitoxlol/everett/pull/43).
Use a separate checkout of that PR with its pinned Node dependencies installed.
The test uses fixed loopback ports 18839 and 18840. It refreshes and owner-revokes
the first test credential, so coordinate exclusive fixture use before you run it.
Use fresh test directories. Never use real account credentials with this fixture.

1. Start the Worker in one terminal. Set `cf_checkout` to that checkout's absolute
   path and `fixture_home` to a fresh absolute private directory:

   ```sh
   node "$cf_checkout/cloudflare/test/oauth-fixture.mjs" serve "$fixture_home" 18839 http://127.0.0.1:18840/mcp
   ```

   Wait for the fixture's ready message. Keep this process alive through the test.
   It creates the private directory and configuration with modes 0700 and 0600.
2. In a second terminal, set the same paths and set `second_home` to another fresh
   absolute directory. Create a second independent grant on the same local issuer:

   ```sh
   node "$cf_checkout/cloudflare/test/oauth-fixture.mjs" issue "$fixture_home"
   python3 - "$fixture_home" "$second_home" <<'PY'
   from pathlib import Path
   import shutil
   import sys
   source, destination = map(Path, sys.argv[1:])
   destination.mkdir(mode=0o700)
   for name in ("fixture.json", "introspection.secret"):
       shutil.copyfile(source / name, destination / name)
       (destination / name).chmod(0o600)
   PY
   node "$cf_checkout/cloudflare/test/oauth-fixture.mjs" issue "$second_home"
   ```

   Only the fixture configuration and shared test introspection secret are copied.
   Each `issue` command performs its own consent and PKCE exchange. Never display
   the private files in a terminal, log or chat.
3. Build this branch, then pass the five absolute paths to the verification script.
   Set `node_binary` to the Node executable used by the fixture:

   ```sh
   cargo build --locked --manifest-path everett-rs/Cargo.toml
   python3 scripts/verify-cloudflare-gateway.py "$fixture_home" \
     "$cf_checkout/cloudflare/test/oauth-fixture.mjs" "$node_binary" \
     "$PWD/everett-rs/target/debug/everett" "$second_home"
   ```

The script prints status fields without credentials. It checks metadata, missing
access, two distinct worker identities, refresh, rejection after owner revocation,
and continued access for the independent agent. It stops its gateway process and
leaves the Worker and private test files with their owner. Stop the fixture with
Ctrl-C after the test.

This journey passed locally with the production Worker routes and SQLite storage.
Only outbound GitHub responses and the exact synthetic CIMD document were mocked.
The test does not prove live provider consent, public TLS, an actual cloud client,
or physical-device delivery.

**The gateway does not resume a bot**, and it does not implement MCP2 events, a
Slack bridge, or an xAI or OpenAI messaging API. Owner webhook wake is a
separate policy: [Grok Bot](grok-bot.md#wake-a-grok-bot-with-a-webhook-routine).
