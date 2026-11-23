# ChatGPT connections and the local plugin

Use **Everett** as your hosted plugin for ChatGPT and Dots. The [hosted Everett
bundle](../plugins/everett/README.md) points to one owner-authorized app. Use that
same app for your chats and Dots, with a private inbox handle for each
conversation. Use the private test connection only for route diagnostics.

Everett has two connection paths. A local plugin starts the installed binary over
stdio. A ChatGPT custom MCP connection reaches the restricted gateway through
HTTPS or Secure MCP Tunnel. They have different installation and account checks.

| Client or mode | Connection | Current evidence |
| --- | --- | --- |
| Codex with local plugin support | Everett Local, stdio | Installer, launcher, identity and listing verified with isolated fixtures. |
| ChatGPT desktop, Codex mode | Everett Local, stdio | Verified 2026-11-10: fresh chat called `mcp__everett_local__everett_whoami` (bound session id) and `mcp__everett_local__everett_ls`. Requires an app restart after install and no root `plugin.json`. |
| ChatGPT desktop, ChatGPT mode | — | Local plugins do not run here; use Codex mode. |
| ChatGPT custom MCP connection | Existing HTTPS gateway or Secure MCP Tunnel | OAuth compatibility tests pass. A fresh ChatGPT identity/listing call remains required. |
| ChatGPT web/mobile | Remote MCP connection | The local installer does not deploy a remote server or install into these clients. |

## Install Everett Local

Install Everett 2.2.1 or newer and Python 3.9 or newer. Everett Local is not
published as a release asset. From a checkout of this repository, follow the
[installer instructions](../plugins/everett-local/README.md). The plugin source
contains no runtime path or credentials. The installer validates your binary and
generates the Codex/ChatGPT desktop manifest in a machine-local marketplace.
Register that marketplace, install Everett Local, and run the installed `check.py`
([layout check](../plugins/everett-local/README.md#check-startup)). Then **quit and
reopen ChatGPT**, start a new chat in **Codex mode** and confirm the tool calls are
named `mcp__everett_local__*`; `check.py` passing does not prove the app loaded the
server. See the [step-by-step check](../plugins/everett-local/README.md#use-it-in-the-chatgpt-desktop-app).

The official [desktop-only plugin guide](https://developers.openai.com/plugins/deploy/desktop-only/)
says local MCP plugins require a local desktop thread. Public directory submission
of local MCP plugins is limited to approved partners with an allowlisted
organization. Do not submit or advertise a public ChatGPT plugin until eligibility, clean
installation, and actual tool calls are verified.

## Test an existing ChatGPT MCP connection

The official [connection guide](https://developers.openai.com/plugins/deploy/connect-chatgpt/)
supports a public HTTPS MCP endpoint or Secure MCP Tunnel for custom connections.
Use the existing owner-configured connection. Its URL is the gateway's MCP URL
or tunnel, not the Cloudflare relay URL. The relay does not serve the gateway's
MCP tools. See [gateway setup](gateway.md) and [OpenAI dots](openai-dots.md).

1. Open ChatGPT Plugins and inspect the existing connection. For a tunnel,
   confirm its client is running and associated with the intended workspace.
   `tunnel_client_not_seen` identifies a tunnel transport failure.
2. Reconnect an expired OAuth connection using its normal consent flow. Everett
   accepts ChatGPT's optional `ui_locales` hint and a client metadata document
   that supports public-client `none` authentication even when its default is
   `private_key_jwt`. PKCE and configured client, redirect and resource checks
   still apply. Do not replace credentials or create another connector to mask
   an unexplained failure.
3. Start a fresh conversation with the intended plugin selected. Inspect its
   advertised tools and call `everett_whoami`. The hosted gateway does not expose
   `everett_ls` or `everett_control`; use `everett_ls` only through the local
   integration. Require actual tool results, not a model's statement that a plugin
   is available. Identity must match the local conversation or the gateway's
   explicit external binding. An identity error needs setup, not a fabricated ID.
4. Record the connection, time, tool names, results and errors without tokens or
   message bodies. Test each existing connector independently. Discovery and
   identity do not establish message pickup, wake, reply or task completion.

On November 9, 2026, a ChatGPT-mode test reported Everett Local unavailable and
executed neither identity nor listing. A separate hosted reconnect reached consent
after the OAuth repair but still showed an expired connection after callback.
These are open app verification gates, tracked in
[issue 132](https://github.com/raitoxlol/everett/issues/132) and
[issue 114](https://github.com/raitoxlol/everett/issues/114). Everett Local 0.3.2
adds `check.py` and skill guidance that names the cause when no local tools appear;
issue 132 stays open until a live client check passes.

Public remote plugin submission requires a stable HTTPS MCP endpoint. Secure MCP
Tunnel alone does not meet the public submission requirements in the official
[MCP server guide](https://developers.openai.com/plugins/build/mcp-server/).

## Receive Everett events in a Dot

1. Select **Everett** on the existing Events-enabled HTTP gateway. Verify that
   it advertises `events` during initialization and lists `message.created`.
   Refresh the existing app's tools in ChatGPT and check that the registered
   `everett_inbox` schema accepts `message_id` and `identity_handle`.
   [Gateway event setup](mcp-events.md) and [hosted readiness check](../plugins/everett/README.md#check-the-actual-connection).
2. Register the Dot once with `everett_identity(action=create, kind=dot)`.
   Retain its private `identity_handle`. Set `arguments.identity_handle` to that
   value on `events/subscribe` for `message.created`.
3. When an event arrives, pass `data.message_id` from the event and the same
   handle to `everett_inbox`. Use that handle again to reply with
   `everett_send(reply_to=...)` after processing a new request. Treat received
   replies as results. If the handle is unavailable, pause and recover the Dot's
   private inbox binding.

## Messaging, session starts, and conversation identity

The hosted gateway accepts `everett_send` only to owner-configured exact IDs or
an authorized `reply_to`. `everett_directory` labels those authorized IDs with
their session, T3 Code thread, or conversation names and state, so a named
existing thread resolves to its exact ID instead of a guess; a T3 thread ID
delivers to the thread's provider session inbox. It forces inbox-only delivery with `spawn=false` and
`wait=0`. A queued message does not start a T3 turn. The recipient must read its
inbox and send a correlated reply. The default gateway cannot start sessions, wake T3,
list arbitrary sessions, or read their transcripts. See [gateway permissions](gateway.md#allowed-tools-and-delivery).

Source also includes an opt-in [owner-authorized T3 start](gateway.md#optional-owner-authorized-native-start).
It requires a fixed local policy and exact verified OAuth account/client; it is
not enabled by registering an identity or installing a plugin. The separate
`everett_start` tool requires a private handle and durable request ID, validates
dispatch receipts, and preserves uncertainty across reconnects/crashes without
launching twice. It grants no extra send/read/control access. No deployed hosted
start or paid-model pickup/reply acceptance is established by the isolated tests.

The local stdio tool `everett_control` can start T3 threads and send turns through
`t3 thread start` and `t3 thread send`. It refuses remote-derived callers; hosting
that tool behind a ChatGPT connection would change the authorization boundary.
This is a capability difference, not an authentication repair. See
[T3 delivery](t3code.md#polling-and-replying).

For [issue 148](https://github.com/raitoxlol/everett/issues/148), these are separate identity sources:

| Route | Identity source | Discovery limitation |
| --- | --- | --- |
| Local Codex, including T3-driven Codex | Native provider ID from host environment or Codex request metadata | Requires the matching local store and host binding. |
| Fixed hosted gateway | Owner-configured `agent_id` | Legacy calls share its inbox; explicit chat handles select separate inboxes. |
| OAuth hosted gateway | Agent returned by token introspection | Approval authorizes the plugin; explicit chat handles identify individual participants. |

The official [OpenAI metadata reference](https://developers.openai.com/plugins/reference#_meta-fields-the-client-provides)
documents `_meta["openai/session"]` as an anonymized conversation correlation ID,
plus subject and organization metadata. Everett's gateway does not currently use
these fields to register separate conversations. They must not replace OAuth
owner authorization or let a caller select another inbox. Actual metadata
availability and stability across reconnects, Dots, and client modes remain
unverified here. No separate inbox, automatic registration, or automatic return
message into a ChatGPT conversation is established by that documentation.

An external registration is durable and separate from a connector or OAuth
approval. Call `everett_identity` to register this chat and retain its private
handle. See [registration and isolation](gateway.md#register-this-chat). Consent
no longer asks you to name the agent or defaults to Argos. Existing connection
credentials remain valid. An empty local `openai-dot` listing
means no matching local registration was discovered; an empty `relay authors`
result means no exposed authors were returned for that query. Neither proves
that every ChatGPT conversation is offline or that account authentication failed.
Local gateway tests cover two chats, a Dot and a logical Codex identity, creation
retries, restart, reapproval, cross-account rejection, revocation, discovery and a
correlated request/reply through the real Rust backend. Actual ChatGPT metadata,
handle retention and client-to-client live acceptance in #148 remain unverified.
No production deployment is implied by these local checks.

### Recovery and deployment evidence

A `tunnel_client_not_seen` response establishes that the selected route has no
recently seen client. It does not establish which binary is deployed or whether
its credentials, host, or service manager are operational. Use the owner-managed
`everett tunnel watch` service described in [the tunnel runbook](openai-dots.md#keep-the-tunnel-forwarding).
Its scheduled renewal bounds silent connection wedges; `/readyz` and a running
PID alone are not end-to-end health evidence. Do not install or restart a live
service as part of a read-only verification.

New builds report source revision and tracked-tree dirty state with `everett
--version`. A successful `everett_whoami` reports backend `runtime`, and the
owner gateway adds its own `gateway_runtime` and explicit inbox-only
`capabilities`. Compare both with the intended release. Build metadata is a
self-report, not a signed artifact attestation. Unknown or inaccessible metadata
must remain unconfirmed; an installed local executable does not prove the
remote deployment revision.

The Cloudflare relay HTTP worker bounds requests at ten seconds and returns
HTTP 504 `request_timeout` with outcome `unconfirmed`. A timed-out mutation may
still finish, so reconcile by its existing idempotency key or message state
before retrying. This deadline applies after a request reaches this worker;
it cannot fix a connector or tunnel timeout before the worker receives it.

Native session start and automatic wake remain unavailable on the registered
hosted inbox connections. A disposable logical identity is an inbox, not a model
session. Local fixtures do not establish ChatGPT acceptance or live model pickup.
