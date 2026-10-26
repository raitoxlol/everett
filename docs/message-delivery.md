# Message delivery policy

Agents are Everett's message recipients. An inbox append does not ping an agent,
wake its provider, or prove that it read the request.

## Default local sends

`everett send` and `everett_send` default to `auto`. An idle local target is
resumed headless and its reply is returned. If the target requires passive inbox
delivery, automatic delivery fails before publishing a request. The error names
the target and the explicit queue option. The CLI exits with code 4.

This refusal covers attached sessions, T3 Code threads, registered external
agents, and local forwarding by a caller with remote provenance. A process being
alive does not prove it can receive a new prompt. Everett does not automatically
resume an attached session or turn remote agent data into an owner prompt.

There is no general provider wake mechanism in the local CLI or MCP send route.
Blocked and needs-input event notifications tell the human about those events;
ordinary inbox sends do not trigger those notifications. Owner webhook wake for
`grok-bot` and `openai-dot` is [Grok Bot](grok-bot.md#wake-a-grok-bot-with-a-webhook-routine).

## Explicit queue exceptions

These routes deliberately retain messages for later pickup.

| Explicit action | Retention and pickup |
| --- | --- |
| `--mode inbox` or MCP `mode: "inbox"` | Opts into the local inbox. Hooks or `everett_inbox` must read it. T3 and registered external agents use this path. `wait` waits for a reply and does not use Claude `--wake`. A matching `receiver-webhook.json` rule may POST a nudge; the receipt keeps `wake: false`. A timeout leaves the request recorded. |
| `send --to <full-Claude-UUID> --wake` | A local interactive human opts into a queue plus a verified same-session Claude background wake. Native prompt receipt and a correlated Everett reply are separate observations. Attach failure, cancellation, or timeout can leave the request recorded because the recipient may already have read it. |
| An exact SSH peer, encrypted relay, or named-agent destination | Explicit durable remote transport, including approved offline-device retention. There is no fallback to a silent local queue. Preserve the transport's signed status meanings and configured receiver wake policy. |
| `reply_to` or `everett reply` | Queues or retains a correlated reply for its sender. The sender polls, uses its hooks, or waits for that reply. Enqueue does not prove sender pickup. |
| An event subscription or owner-authorized external gateway | Explicitly accepts durable inbox pickup. A gateway remains owner-bound and inbox-only; provider scheduling and wake are separate acceptance paths. |

The native Claude wake requires an exact UUID, a supported version, and verified
background state. It rejects routed, remote, T3, relay-derived, non-Claude, and
noninteractive callers. Automatic and MCP sends do not inherit that permission.

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
