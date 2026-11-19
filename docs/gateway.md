# Owner-bound Rust MCP gateway

The native `everett gateway` command lets an existing cloud agent call Everett's
scoped tools. The official Rust MCP SDK is compiled into the binary.
The gateway does not
create an OpenAI or xAI model instance.
An optional owner grant can dispatch a native T3 run through an existing local
T3 server. This is separate from the default inbox-only permissions.

Build the native binary from this branch:

```sh
cargo build --locked --manifest-path everett-rs/Cargo.toml
everett-rs/target/debug/everett gateway --help
```

Use an installed Rust Everett binary that supports `gateway`. If `backend_command`
is omitted, the gateway starts its built-in worker in the same binary. Existing
`backend_command` entries may also specify an Everett executable followed by
`mcp`, including through a launcher. If the Everett child supports scoped calls,
the gateway detects that during initialization and uses the same stdio connection
for those calls. Older Everett and custom backends continue to use their MCP
tools. Upgrade the gateway
before a separately installed Everett backend so it can recognize scoped worker
support. The owner controls the absolute executable path and arguments.

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
(case-insensitive). Use `openai-dot`, `grok-bot`, or `devin-bot` for `harness`.
The fixed project slug is lowercase, begins with a letter or digit,
and contains only letters, digits, `_`, or `-`, up to 64 characters.
Destinations are exact, complete IDs, not titles, folder names, or prefixes.

`allow_shared_core: true` explicitly permits the **owner-wide global core plus
the named project core**. This is not project-only memory isolation. Setting it
to false removes the core tool entirely; this pass does not implement a
project-only reader. Shared core should contain no secrets.

The gateway uses `EVERETT_HOME`, or the current user's home, captured at startup.
Each authorized tool call starts a one-shot worker with that home, the fixed
working directory, and the selected session and harness. The built-in worker
handles only gateway operations and uses local inbox delivery. It requires no
native `everett login` credential. Custom backends continue to use MCP over
stdio. Backend processes receive basic OS environment variables such as `PATH`
and `USER`. API keys and gateway tokens stay in the calling environment. Use
only trusted backends because they have access to the owner's local stores.

The hidden worker and the gateway-scoped stdio method trust their local launcher.
A local process with the gateway service account's filesystem permissions can
call them under a chosen external session ID and access that account's Everett
stores. Run the gateway under a dedicated OS
account with a private Everett home if untrusted local processes share the host.
Give remote clients access only through the authenticated gateway transport.

A gateway configuration grants access to fixed destinations, a directory and
shared core. The connection is authorized by its bearer token or OAuth approval.
One account sign-in can authorize multiple logical inboxes. Each ChatGPT
conversation, hosted Codex conversation, and Dot registers its own Everett
identity and uses its own private handle. Clients that can keep credentials
outside MCP tool calls can use [HTTP conversation identity](#http-conversation-identity).

### Require a private inbox

Add the top-level JSON field `"require_identity_handle": true` to the owner's
gateway binding file (the file passed to `everett gateway --config`) and restart
the gateway. The default is `false`, which allows existing clients to use the
legacy connection inbox without a handle. In strict mode:

- `everett_inbox`, `everett_send`, and `everett_card` require a valid
  `identity_handle` in the tool arguments. Event subscriptions require it in
  `params.arguments`. Use the same handle to unsubscribe a private inbox.
- Authenticated clients can still list tools and events, register an identity,
  and call `everett_whoami` without a handle for connection details.
  `everett_whoami` reports `capabilities.private_inbox_required: true`. Supply
  a handle to see the corresponding registered identity.
- The event pump skips legacy subscriptions that lack a handle. These leases
  still count toward the gateway quota until they expire or you unsubscribe.
  To free a slot, the original connection can unsubscribe its legacy lease
  using the original callback URL and no handle. The strict gateway still
  requires a handle to subscribe again.

Before enabling strict mode, register each conversation using the steps below
and confirm the client can supply its handle on later calls. Have the owner
update senders to use the new full inbox IDs and recreate Event subscriptions
with the corresponding handles. Existing private handles continue to work.
Legacy messages stay in the original mailbox. Drain it before the switch or
retrieve retained messages through the owner's local Everett profile. Set the
field back to `false` and restart to restore handle-free gateway access.

Everett issues logical mailbox IDs. Authorization comes from the connection
credential and the private handle. A `session_id` or host-provided `_meta`
cannot authenticate a mailbox. Handle retention across turns or restarts
depends on the client. The gateway does not verify native conversation IDs or
store a private handle on the client's behalf.

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
   handles. Add `kind="chat"`, `"dot"`, or `"codex"` to list only that kind.
   `state="registered"` means the local discovery record matches the
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
{id, harness: "openai-dot" | "grok-bot" | "devin-bot", title, cwd, updated: epoch_seconds}
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

`everett_whoami.authorized_destinations` lists the binding's current exact owner
allowlist. Read it before sending. Destination grants are live policy, not a tool
schema enum: clients cache schemas, so an enum would reject a newly authorized
target before the gateway can check it. The gateway still rejects every full ID
outside its allowlist before calling the backend. Existing clients with an old
destination enum need one normal tool refresh to adopt this stable schema;
subsequent owner grant changes do not change the send schema.

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

## Optional owner-authorized native start

`native_start` is disabled by default. Enabling it is an explicit permission to
promote remote text to a model prompt under the owner's account; it can consume
model allowance or incur provider charges. Deploying the feature, configuring
the grant, refreshing client tool registrations, and running a live model test
require separate owner authorization. Source and fixture tests do not establish
that an existing hosted connector supports it.

The only native operation is `everett_start`. The owner fixes the T3 project
and provider instance and sets the default model and optional reasoning effort.
Existing remote-derived callers remain refused by local `everett_control`.

The optional block in the local gateway configuration is:

```json
{
  "native_start": {
    "owner_account": "github:OWNER_ID",
    "client_id": "https://chatgpt.com/oauth/client.json",
    "command": "/absolute/path/to/t3",
    "project_id": "EXACT_T3_PROJECT_ID",
    "provider": "EXACT_PROVIDER_INSTANCE_ID",
    "model": "EXACT_MODEL_SLUG"
  }
}
```

Use the actual verified account and client ID. The existing OAuth block is
required; stdio and fixed-bearer connections cannot use this grant. A token must
still pass issuer, audience, scope, expiry and revocation checks on every request.
Only tokens with the exact configured account/client can discover or call the
tool. This is account/client authorization, not vendor-attested conversation
identity. The configured CLI must support `thread start --json` and `-` for
stdin, and must already reach the intended T3 server. No server is installed or
started by this feature.

To require verifiable model and effort selection, also configure
`"effort": "xhigh"` and `"mcp_endpoint": "http://127.0.0.1:PORT/mcp"` in
`native_start`. Supply the existing scoped T3 MCP bearer through
`EVERETT_T3_MCP_TOKEN` in the gateway process environment. The gateway uses
the literal loopback address, checks the model and its `reasoningEffort` option
against the T3 catalog, and launches through `t3_thread_launch`. It requires
that receipt to report the same provider instance, model, and effort. Some T3
CLI versions can submit model options through `--option`. Their start receipts
only carry project, thread and run IDs.

Register a chat with `everett_identity` first. Then call:

```text
everett_start(identity_handle=<private handle>,
              request_id=<fresh 32..128 character URL-safe ID>, text=<request>,
              model="gpt-6.2-sol", effort="xhigh")
```

When MCP transport is configured, omitted `model` and `effort` fields use the
owner's defaults. Owners can optionally configure up to 32 additional pairs
in `allowed_selections`, for example
`[{"model":"gpt-6.1-sol","effort":"high"}]`. The resolved pair must match the
default or an allowed pair and be available on the fixed provider. For a
legacy CLI grant, omit both arguments. Keep the private handle and reuse the
same request ID and inputs after a timeout or reconnect. A private identity is
required so retry scope survives
normal account/client reapproval. Another chat cannot read or retry that chat's
receipt. While a request has a journal reservation, changing the text, resolved
pair, or owner policy under the same ID is rejected.

Before dispatch, the gateway commits a reservation in the private
`~/.everett/gateway-starts.sqlite` journal. A confirmed MCP preflight failure
releases the reservation and returns `dispatch_not_started` with
`t3_mcp_unavailable` or `t3_model_or_effort_unavailable`. Resolve the issue
and retry with the same request ID and inputs. Once a launch may have been
sent, retries return the recorded result. If the gateway crashes after the
reservation, the result remains `dispatch_unconfirmed`, even if T3 actually
started the run. The owner must reconcile T3 before attempting a new request ID.
Back up this journal with the gateway state; deleting it loses duplicate-start
protection. It stores request fingerprints and bounded receipts, not prompts,
tokens or handles. It retains at most 256 requests per issuer/resource/account/
client scope; reaching the limit requires owner reconciliation.

`state=dispatch_confirmed` requires a valid T3 receipt with the exact project
ID and nonempty thread/run IDs. `requested` contains the resolved model and
effort. `effective` contains the verified selection for MCP dispatch and is
null for legacy CLI or uncertain receipts. The receipt establishes T3 dispatch.
`native_session_id=null` means the gateway has not established the native
provider's session identity. Pickup, completion and correlated reply remain
explicitly unconfirmed. The returned thread ID is not an Everett destination;
the owner must separately verify and authorize a full native ID for messaging.

Uncertain results set MCP `isError=true`, retain the durable reservation and
include a bounded diagnostic code: `cli_unavailable`, `cli_io_error`, `cli_exit`,
`cli_timeout`, `cli_output_too_large`, `invalid_receipt`,
`destination_mismatch_or_missing_run`, `interrupted_or_pending`,
`t3_mcp_launch_unconfirmed`, or `t3_effective_selection_mismatch`. The
legacy command receives the prompt over stdin and runs with a limited
environment. MCP sends the prompt over the configured loopback connection. The trusted CLI can still access the owner's stores;
this is not an OS sandbox. Cancellation kills only the gateway-owned CLI process
group; it cannot cancel a model run already accepted by the independent T3 server.

`everett_whoami.capabilities.native_session_start` reports the configured grant
for this caller, not CLI or model health. `gateway_resource` identifies this
gateway's advertised OAuth resource. Real acceptance still requires a client
call through that resource, intended-target verification, pickup and a matching
reply. `tests/gateway_native.rs` exercises the real HTTP/SDK/journal boundary
with a synthetic issuer and T3 subprocess; it makes no model calls.

## Private stdio / OpenAI Secure MCP Tunnel

```sh
/absolute/path/to/everett gateway --config /absolute/path/to/dot.json
```

This has no HTTP listener. The owner-authorized tunnel runs this command inside
the host that contains Everett's stores. Follow [OpenAI dots setup](openai-dots.md),
including `everett tunnel watch` so a stuck control-plane poll is restarted.
`/readyz` on the tunnel client is not proof that calls are still being forwarded.
Configure `everett tunnel watch --health-url http://127.0.0.1:8080/health?details=true`
with the actual tunnel-client health port. This requires its structured JSON logs
and detailed operator health, supported by tunnel-client0.0.16. The supervisor
matches the health snapshot to the child instance from its logs and checks recent
successful polling and the current poll deadline. Fresh polling defers scheduled
rotation, so a healthy client is not killed every four minutes while forwarding
requests. Stale, malformed, unavailable or mismatched health retains the bounded
renewal fallback. Health reads have a two-second deadline and64KiB limit, use
loopback HTTP only and follow no redirects or proxy environment settings.
Without this option, the existing age-based renewal remains. A client disconnect
can strand a request upstream even after the client recovers. Bound reads at the
caller where supported; retry only non-consuming peeks after reconciliation.
Never retry sends or starts under a new ID solely because a connection recovered.
Neither local health nor supervisor renewal proves that any request was completed.
For a read-only poll diagnosis, pass a detailed operator-health JSON snapshot and
the supervised child's expected instance ID to
`python3 scripts/diagnose-tunnel-host.py --poll-health INSTANCE < snapshot.json`.
The command reads at most64KiB of input, makes no network requests or process
changes, and exits nonzero for invalid, stale, mismatched or unconfirmed polling.
It accepts variable RFC3339 fractional seconds on macOS Python3.9, distinguishes
timestamp parse errors from missing successful polls, and always leaves request
completion unconfirmed. Preserve intermediate observations during recovery;
silently swallowing timestamp errors can turn healthy polling into an apparent
timeout. A later passing run does not erase an earlier missed recovery deadline.
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
    "resource": "https://<public gateway URL>/mcp",
    "owner_account": "github:OWNER_ID"
  }
}
```

- `issuer` must equal the authorization server's configured `EVERETT_ISSUER` exactly; `resource` must
  be listed in the deployment's `EVERETT_OAUTH_RESOURCES`. Both are https (plain
  http only to a loopback host). Optional: `scope` (default `everett`) and
  `introspection_endpoint` (default `<issuer>/oauth/introspect`).
- Set `owner_account` to the exact introspected account for a gateway that serves
  one owner's local sessions, especially when the issuer permits public enrollment.
  Every authenticated request must carry that exact `everett_account`; missing,
  malformed and different accounts are rejected before tools or workers run.
  Omitting it preserves the issuer's existing account policy. This restriction is
  separate from the exact account/client/project/model native-start grant.
  Legacy deployment-only keys `events`, `owner_local_sessions`, `chat_identities`
  and `oauth.owner` are unsupported and fail startup. Migrate owner metadata to
  `oauth.owner_account`; do not remove an owner restriction just to pass parsing.
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

## Owner agent access over HTTPS

A separate bearer connection can opt into the same MCP tool catalog as a local
Everett agent. Add `"agent_tool_permissions": "/absolute/path/to/client-tools.json"`
to its gateway binding. Use the built-in backend, a dedicated external agent ID,
and a dedicated bearer key. Run the gateway in HTTP mode from a signed-in
Everett profile. The gateway pins the caller to that binding and the native
tools still verify the device account and enforce their normal rules.

Keep the configuration and permission file in an owner-controlled directory.
Use one file per client. Inspect or replace its grant on the gateway host:

```sh
everett gateway-permissions --file /absolute/path/to/client-tools.json
everett gateway-permissions --file /absolute/path/to/client-tools.json --allow everett_whoami,everett_inbox
everett gateway-permissions --file /absolute/path/to/client-tools.json --all
everett gateway-permissions --file /absolute/path/to/client-tools.json --deny-all
```

The file contains the JSON string `"all"` or an array of exact native tool
names. A manager should use the same file and replace it atomically with mode
0600. The gateway reads it before discovery and each new call. Permission changes
do not interrupt work already running. If the client caches discovery, refresh
its tool list after a grant changes.

Each tool grant allows its native operations. For example, `everett_core` can
read, search, forget and correct facts. The restricted gateway's
`allow_shared_core` and `destinations` fields do not narrow owner-mode calls.
Full access includes session control, shared memory changes and relay
administration. Use it for clients trusted with that access. If a bearer key
leaks, deny all tools and rotate the key. Reuse neither a demo key nor another
connector's key. Owner mode uses the gateway host's configured T3 runtime,
including `EVERETT_T3_BIN` and `T3CODE_HOME` when set, and is separate from
OAuth bindings, legacy private identity handles, gateway-native start and gateway
event subscriptions. You can reuse the grant file format on a dedicated bearer
binding with HTTP conversation identity. Deploy remote connections behind HTTPS.

Prefer dispatch without waiting for long T3 turns. If an owner tool exceeds
the HTTP response deadline, it can continue under the native operation's
deadline and retains its worker slot until it exits. Check the session or
request's state before retrying an unconfirmed call.


## HTTP conversation identity

Set `"conversation_identity": true` in a dedicated gateway binding and run
it over HTTP. The default is `false`. Use the built-in worker and set
`"allow_shared_core": false`. Leave Events, native start, and
`require_identity_handle` off. Use `"harness": "devin-bot"` for a Devin Bot
binding and `"harness": "openai-dot"` for a ChatGPT plugin binding. These
values label Everett's external directory. For OAuth, set
`oauth.owner_account` and use a dedicated resource. The trusted issuer must
supply the account and client ID in the token's introspection response.

The owner supplies a separate, high-entropy
`EVERETT_GATEWAY_IDENTITY_ADMIN_TOKEN` in the gateway environment. The existing
[permission CLI](#owner-agent-access-over-https) manages the optional private
`agent_tool_permissions` file for a bearer binding. Use a separate file per
binding. Its permissions narrow the four conversation tools in discovery and
execution. Even `"all"` allows only `everett_whoami`, `everett_inbox`,
`everett_card`, and `everett_send`. An OAuth conversation binding exposes
these four tools. Owners can grant session inspection, global memory, control,
and relay administration through a separate fixed owner binding and credential.
Those privileges can reach other sessions. Keep them outside the conversation client.

### Provision, use and revoke an inbox

The owner or a trusted process acting for the owner calls `POST /mcp/identity`
with `Content-Type: application/json`, the connection's
`Authorization: Bearer …` header, and the separate
`Everett-Identity-Admin` credential header. Keep the endpoint, credential and
response out of model-accessible tools, logs and traces.

- Create with `{"action":"create","registration_key":"<fresh private random key>","name":"Conversation label","kind":"chat"}`.
  Persist the 32–128 character URL-safe random key before the request. Retrying
  with the same key and fields returns the same `session_id` and
  `identity_handle`. Keep the key as private as the handle.
- The client sends the normal connection authorization and one
  `Everett-Identity-Handle` header on each `tools/call` to `/mcp`. The handle
  is a 64-character hexadecimal secret. The client can initialize and list
  tools before it has a handle. Omit `session_id` and `identity_handle` in tool
  arguments. `everett_whoami` returns the selected public Everett ID and reports
  `capabilities.private_inbox_transport: "http_header"`.
- `everett_send` queues a local inbox message to an exact owner-granted
  destination, or replies to a message addressed to this conversation from an
  allowed sender. It omits the destination card and transcript summaries from
  direct send receipts. A conversation send cannot invoke a receiver webhook,
  start a session or send through the relay.
- Revoke with `{"action":"revoke","identity_handle":"<private handle>"}` on the
  owner endpoint. Retrying revocation is safe. It removes the external
  registration and blocks future tool authorizations with that handle. Other
  conversations keep their access. An operation already authorized may finish.
  The owner retains the inbox and card files.

Lifecycle success returns uncached JSON and HTTP 200. Invalid input or an
unusable handle returns 400. Invalid connection authorization returns 401,
missing or incorrect owner authorization returns 403, and the endpoint
returns 404 when the mode is off. Use POST with JSON. Other methods and
content types return 405 and 415 respectively. MCP returns tool errors for
denied calls. A legacy gateway rejects the new handle header, so a
misconfigured client fails instead of falling back to a shared inbox.

The private SQLite store retains up to 256 registrations per scope, including
revoked keys. The owner can inspect registrations with `everett external list`
and back up the store privately. Bearer scope includes the bearer credential,
configured `agent_id`, and harness. Rotating the connection bearer invalidates
that binding's existing handles. Revoke active registrations before rotation
when possible, then provision fresh inboxes through the new credential. The
owner can remove obsolete local directory entries with `everett external remove`.
OAuth scope includes the verified issuer, resource, account, client ID, and
configured harness. A fresh token for those same values keeps existing handles.
Each request still requires an active token. Handles are scoped to their
gateway mode.

### Devin Bot and ChatGPT clients

A trusted client or broker must identify its conversation from state it
controls, keep the registration key and handle in private per-conversation
storage, and inject the handle after removing any caller-supplied copy. Fail
when the conversation binding is missing or ambiguous. A vendor conversation
ID, MCP transport ID, tool argument or request metadata cannot select an inbox.
Keep the broker's store and credentials outside files, environment variables,
prompts and shared memory that the model can access.

- **Devin Bot.** Bots inherit their configured connections. Devin documents an
  MCP header script on the session machine that can return custom headers. A
  deployment could use it to call a trusted broker if the script can
  authenticate its specific session to that broker without exposing secrets
  to the model. Verify that binding, header delivery and private retention
  across reconnects before using it. [Devin Bot connections](https://docs.devin.ai/work-with-devin/bots/connections) · [Devin header scripts](https://docs.devin.ai/work-with-devin/mcp#header-script)
- **ChatGPT plugin.** Plugin OAuth authorizes the connection. The documented
  `_meta["openai/session"]` value correlates calls. A trusted host integration
  would have to retain and attach the handle privately. The HTTPS plugin and
  the existing stdio Secure MCP Tunnel setup have no verified path for that
  here. Tool results, including hidden `_meta`, are unsuitable for credentials.
  [Plugin OAuth](https://developers.openai.com/plugins/build/auth) · [Client metadata](https://developers.openai.com/plugins/reference) · [Tool results](https://developers.openai.com/plugins/build/mcp-server)

The native acceptance test uses synthetic HTTP clients and a loopback OAuth
issuer. Vendor header delivery, vendor-side secret persistence, and production
deployment remain unverified. Legacy OAuth and stdio clients can keep their
existing binding until the new client path is verified.

## Bounds and verification

Configuration is capped at 16 KiB; HTTP request bodies, tool arguments, and stdio
request frames at 64 KiB; returned tool results at 256 KiB. HTTP headers are capped
at 16 KiB and 64 fields. Header and body reads each time out after five seconds.
The configured timeout is 1–30 seconds (default 20). For restricted bindings
it limits backend operations. For owner mode it limits the HTTP tool response,
and native work can continue under its own deadline. Each process permits 1–8
workers (default two).
Excess work fails immediately rather than creating an unbounded queue. HTTP is
stateless and does not accumulate client sessions. The listener allows twice the
worker count in concurrent connections, with one request per connection and a
45-second total deadline. It checks duplicate security headers before the HTTP
parser can combine them. Reply authorization scans only
this binding's inbox, capped at 8 MiB; oversized or malformed inboxes fail closed.
Backend input and output are bounded. Custom MCP backend frames are bounded
before SDK buffering, and their tool results are checked after SDK encoding.

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
