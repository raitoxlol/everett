# Grok Bot: owner-registered, inbox-only agents

Everett's `grok` harness reads **local Grok CLI sessions**. It is not a Grok Bot
cloud integration. `grok-bot` and `openai-dot` are separate external harnesses:
the owner registers a logical inbox identity, and the external agent polls
Everett through MCP. Registration does not sign in to a provider, create a Bot,
discover cloud conversations, or prove a vendor-issued identity.

## Register explicitly

Use the native Rust executable to register the inbox. If you installed only the
Python package, use `python -m everett.external` for the same commands and files.

```sh
everett external add \
  --id ext-grok-wright --harness grok-bot \
  --title 'Grok Bot: database backup reports' --cwd /work/backups

everett external add \
  --id ext-dot-wright --harness openai-dot --title 'Owner-authorized Dot'

everett external list
everett ls --json
everett send 'Report backup status' --to ext-grok-wright --mode inbox --json
```

The owner must authorize the Bot and its access before connecting it. The
registration is a local binding, not provider authentication. IDs use 1–100
ASCII letters, digits, underscores, or hyphens. `human` and `live` are reserved,
case-insensitively. Prefer an `ext-` namespace to avoid collisions with local
session IDs; use a distinct ID for each authorized agent/inbox binding.

Registrations are **durable**: `--hours` does not expire them. They also bypass
the normal 40-local-session listing cap. `updated` determines recency ordering,
not authorization lifetime or whether the Bot is running. A harness filter
still applies; MCP `everett_ls` accepts `grok-bot` and `openai-dot`. The module's
`list` operation returns registrations only.

## Shared Python/Rust storage contract

Each registration is `~/.everett/external/<id>.json` (or under `EVERETT_HOME`
when set). Example:

```json
{
  "id": "ext-grok-wright",
  "harness": "grok-bot",
  "title": "Grok Bot: database backup reports",
  "cwd": "/work/backups",
  "updated": 1791500000.0
}
```

Both readers validate the ID, supported harness, string `title`/`cwd`, finite
non-negative epoch `updated`, and filename matching the ID. Scans skip malformed,
symlinked, and non-regular records. Both registration commands write atomically
and refuse accidental replacement. External session listings have
`source: "external"`, no fabricated transcript path/start time, and `running:
false`; Everett has no provider liveness evidence.

Update an existing binding explicitly, or remove it before changing harness:

```sh
everett external add --id ext-grok-wright --harness grok-bot \
  --title 'Grok Bot: revised backup scope' --replace
everett external remove --id ext-grok-wright
```

Removal deletes the registration, **not its inbox**. Re-registering the same
ID can expose its pending messages again. Use a fresh ID for a different agent;
review retained inbox data before reusing one. Removing a record is not a
replacement for revoking connector credentials or deleting a provider routine.
The existing inbox TTL still applies: pending messages older than seven days
are not returned. Durable registration does not promise permanent pending work.

## Delivery is a queue, not a cloud wake-up

- External `auto` and `inbox` modes queue locally. Results include `mode:
  "inbox"`, `queued: true`, and `hooked: false`.
- `resume` is refused, even if process evidence contains the ID. Everett never
  runs `grok --resume`, OMP, or another provider CLI for these records.
- External agents cannot be spawned. `everett route` gives an `everett send
  ... --mode inbox` command, not a fabricated resume command.
- MCP's existing `delivered: true` field means accepted into Everett's inbox,
  **not** confirmed provider consumption. The external-send note explicitly
  says that no provider wake-up or consumption is confirmed.
- Optional reply waiting is only local inbox polling. It does not trigger the
  Bot; a timeout does not cancel the queued message.

The owner must connect a supported MCP endpoint and ask the Bot to poll. For a
local Grok Bot **Command** integration, launch `everett mcp` on the machine
holding these registrations/inboxes. Supply `EVERETT_SESSION_ID=ext-grok-wright`
and `EVERETT_HARNESS_NAME=grok-bot` in that process's environment. These are
logical identity labels, not secrets. The owner should test the endpoint in
their own conversation before scheduling unattended work.

Local stdio MCP intentionally permits explicit `session_id` arguments; these
labels are not an authentication boundary. A remote connection must use an
authenticated, identity-bound MCP gateway on the owner-controlled host. Never
publish the unrestricted stdio tools as an anonymous HTTP endpoint. Registration
alone does not provide Remote HTTPS transport, TLS, credential provisioning,
or cross-device synchronization.

Use the native [`everett gateway`](gateway.md) command for the restricted stdio
or authenticated loopback HTTP connection. It defaults to the same binary's MCP
backend and does not need Python. The owner must provide TLS and reachability
for a Remote HTTPS connector.

## Poll and reply through MCP

With the connection bound to `ext-grok-wright`, the Bot calls:

1. `everett_inbox` with `{ "peek": true }` to inspect pending messages without
   consuming them (optional).
2. `everett_inbox` with `{}` to take pending work. This marks messages done in
   the queue immediately; it is not a transactional work lease.
3. `everett_send` with `{ "text": "Backups complete", "reply_to": "<message id>" }`
   to reply to the original sender. Keep the message ID before acknowledging
   work and report failures explicitly.

The sender reads `everett_inbox` under their own identity for replies. Local
stdio can explicitly pass `session_id`; remote callers must use the identity
their authorized gateway connection supplies. These registration and queue
operations do not require or invoke live Grok Bot services.

## What xAI documents—and what Everett does not assume

Official documentation reviewed on 2026-10-09:

- [Team Bots](https://docs.x.ai/grok-bot/team-bots) describes custom MCP servers
  using **Remote HTTPS** or **Command**. Command servers run on the computer
  each conversation uses; commands needing environment variables are restricted
  to the owner's own chat. Do not put credentials in command arguments. A
  laptop-local Command endpoint is not guaranteed reachable by background work.
- [Skills and routines](https://docs.x.ai/grok-bot/skills-routines-and-automations)
  documents owner-configured workflows, Test run, and schedules at least
  **five minutes apart**. An owner-authorized polling routine can use an
  available connector; verify access and keep unsafe actions behind approval.
  Team routines remain personal to the person configuring them.

These pages do not establish an arbitrary cloud Bot/session resume or immediate
wake-up API. Everett does not invent one. Any polling cadence, provider routine,
connector availability, approval boundary, and provider usage cost remain the
owner's responsibility. Remote deployment and cross-device transport are
separate work, not part of this registration adapter.
