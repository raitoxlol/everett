# Message delivery policy

Agents are Everett's message recipients. An inbox append does not ping an agent,
wake its provider, or prove that it read the request.

See [agent support and wake](agent-support.md) for the per-agent capability table.

## Default local sends

`everett send` and `everett_send` default to `ping`. An idle local target runs
headless immediately and returns its reply, without waiting for a busy target to
go quiet. A verified attached local Claude background session receives a fixed
native nudge to read its durable inbox. The original message stays in the inbox;
it is never interpolated into the native nudge.

`pinged: true` confirms dispatch, not inbox consumption or task completion.
The native path retains `queued` and `delivered` as independent inbox-pickup
observations. If native attachment fails after the inbox write, the error names
the retained message. The request may already have been read; do not blindly retry.

T3 Code, external agents, attached sessions without a supported wake transport, and remote transports
currently have no synchronous ping path. They fail explicitly without queueing.
Remote-derived callers cannot resume, spawn, or wake local providers. A process
being alive does not prove it can receive a prompt. Legacy queue delivery requires
an explicit `--mode inbox` or MCP `mode: "inbox"`; `auto` and `resume` retain their
previous behavior only when explicitly selected.

## Attached OMP sessions

On OMP 18.12.2, the running Everett extension watches its session's inbox and
polls every five seconds as a fallback. A fresh capability marker must match the
live OMP session, PID, and kernel process start and boot identity. The heartbeat
refreshes only that existing binding's liveness timestamp, preserving activity
state and inbox receipts even after more than 24 hours idle. Unknown process
identity fails closed. Local CLI and MCP `ping` or `auto` sends then return
`mode: "ping"`, `transport: "omp-extension"`, and
`wake: {"state": "extension_pending"}`. This confirms inbox acceptance by the
watcher transport. It does not confirm a nudge, pickup, or reply; `pinged` remains
false. Missing, expired, malformed, or mismatched capabilities retain the refusal
for attached OMP sessions, without queueing a request.

The idle watcher calls `pi.sendMessage` with `triggerTurn: true`,
`deliverAs: "nextTurn"`, and agent attribution. Its fixed text names only the
message ID and the commands `everett inbox --session <sid> --message <id>` and
`everett reply <id> '<your concise result>'`. Both commands use the resolved
absolute executable with shell quoting and explicit `EVERETT_HOME` and
`EVERETT_AUTH_PROVIDER` values. The reply also carries the destination session's
OMP identity. Sender text never enters the nudge.
A durable exclusive reservation allows at most one nudge attempt per message ID,
including overlapping watchers and extension reloads. The optional journal is
best effort; journal failure cannot prevent the claimed nudge attempt. Reserving does not mark the
message consumed. Nudged messages bypass passive injection so the explicit inbox
read records the receipt. An ambiguous or failed send is never retried
automatically; the message remains available to an explicit reader.

Busy sessions receive no watcher nudge. Existing tool-result steering handles
their pending inbox data. The watcher closes on session shutdown and is replaced
on session switch or branch. Initial lease publication precedes watcher and
timer allocation, so a publication failure leaves no unmanaged watcher.
Capability leases expire after 20 seconds without a heartbeat.

The trust boundary remains local. Remote-derived callers and external harnesses
cannot use this ping transport. Wake candidates exclude external/untrusted
messages, relay provenance, and senders with durable relay context. Those
messages retain passive hook or explicit inbox pickup with existing provenance
handling. A local capability file is evidence from a cooperating extension, not
authentication against another process with write access to the same home.

The API is defined by OMP's [v18.12.2 extension types](https://github.com/can1357/oh-my-pi/blob/v18.12.2/packages/coding-agent/src/extensibility/extensions/types.ts#L1836)
and [message delivery guide](https://github.com/can1357/oh-my-pi/blob/v18.12.2/docs/extensions.md#message-delivery-semantics).
The [session implementation](https://github.com/can1357/oh-my-pi/blob/v18.12.2/packages/coding-agent/src/session/agent-session.ts#L9203)
starts an idle turn for `nextTurn` plus `triggerTurn`. ACP clients may defer
agent-initiated turns; this transport does not override that policy.

## Authenticated author cards

After sign-in and `everett relay register`, run `everett relay expose SESSION`
with an exact local session ID. Use `--harness H` if IDs overlap. This publishes
only its harness, session ID, and up to 640 characters of its card. The backend
derives the author and agent from active credentials. It does not accept author
claims from the caller. Cards are plaintext account-visible metadata; do not put
secrets in an exposed card. Exposure is opt-in, with a limit of 32 cards per agent.

`everett relay authors` lists exposed cards across all pages without pairing.
The dashboard shows the same authenticated cards separately from local stores,
with author, agent, and session tags. Only active account peers are visible;
revoked credentials or memberships are filtered on every read. Cross-owner grants
still govern remote transport and do not publish cross-owner cards.

MCP `everett_agent_relay` actions `expose` and `withdraw` affect only the
host-bound caller's session. `authors` returns author cards; `directory` retains
the old key-bundle listing (`devices` is an alias). To update a shared card, expose the session again.
To withdraw it, run `everett relay expose SESSION --remove`. Existing exposure
persists until withdrawal or revocation; its update time is not live presence.
Cloudflare requires the new backend APIs to be deployed. An old
backend produces an explicit unavailable-directory notice, not a fabricated list.

## Explicit queue exceptions

These routes deliberately retain messages for later pickup.

| Explicit action | Retention and pickup |
| --- | --- |
| `--mode inbox` or MCP `mode: "inbox"` | Opts into the local inbox. Hooks or `everett_inbox` must read it. T3 and registered external agents use this path. `wait` waits for a reply and does not use Claude `--wake`. A matching `receiver-webhook.json` rule may POST a nudge; the receipt keeps `wake: false`. A timeout leaves the request recorded. |
| `send --to <full-Claude-UUID> --wake` | A local interactive human opts into a queue plus a verified same-session Claude background wake. Native prompt receipt and a correlated Everett reply are separate observations. Attach failure, cancellation, or timeout can leave the request recorded because the recipient may already have read it. |
| An exact remote destination with `mode=inbox`, or the legacy `everett_agent_send` tool | Explicit durable remote transport, including approved offline-device retention. Naming a remote target alone does not opt into a queue. Preserve the transport's signed status meanings and configured receiver wake policy. |
| `reply_to` or `everett reply` | Queues or retains a correlated reply for its sender. The sender polls, uses its hooks, or waits for that reply. Enqueue does not prove sender pickup. |
| An event subscription or owner-authorized external gateway | Explicitly accepts durable inbox pickup. A gateway remains owner-bound and inbox-only; provider scheduling and wake are separate acceptance paths. |

Native Claude ping requires a full UUID, Claude 2.1.285 or newer, an existing
absolute working directory, and verified background state. CLI and host-bound
local MCP sends can use it. The older `--wake` compatibility command retains its
exact-target, interactive-human restrictions. Neither path allows remote-derived
callers to promote agent data into owner prompts.

Default ping does not require an interactive stdin: CLI automation and local MCP
callers use the same private PTY connector. After target validation, the request is
recorded before native preflight/attach. A preflight or attach failure returns an
explicit error with the retained message ID, not a successful ping; do not blindly
retry. Invalid or unsupported targets are rejected before an inbox item is created.

## Local send receipts

An explicit local inbox send reports `wake: false`. Until a reader marks the
message id in the recipient's `.done` file, it reports `queued: true` and
`delivered: false`. If pickup is observed before the send returns, those flags
become `queued: false` and `delivered: true`. `hooked` describes harness support;
it does not prove that a hook is enabled or has run. Task completion still needs
the recipient's result.

Replies and explicit remote MCP sends report `delivered: false` at enqueue.
Transport statuses remain distinct. In encrypted relay observations, `delivered`
means authorized device ingress and `claimed` means inbox pickup. Neither proves
task completion.

`everett_inbox(peek=true)` leaves messages unread. A normal `everett_inbox` call
or an accepting hook marks the returned message ids consumed so they do not
appear again. Records remain available for `reply_to` lookup until expiry.

## Verification

`everett-rs/tests/delivery_policy.rs` drives the real CLI and stdio MCP with an
isolated home. It checks refusal without a leftover request, truthful explicit
queue receipts, and read-once inbox consumption. `tests/live.rs`,
`tests/external.rs`, `tests/t3code.rs`, and `tests/release_journey.rs` cover hooks,
explicit external polling, synthetic T3 request/reply, and the CLI/MCP journey.
These fixtures do not prove that a real provider UI wakes or accepts a prompt.
Webhook tests post to a fake endpoint. They do not prove a live Grok Bot run.
