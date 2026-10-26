# Grok Bot: owner-registered, inbox-only agents

Everett's `grok` harness reads **local Grok CLI sessions**. It is not a Grok Bot
cloud integration. `grok-bot` and `openai-dot` are separate external harnesses:
the owner registers a logical inbox identity, and the external agent polls
Everett through MCP. Registration does not sign in to a provider, create a Bot,
discover cloud conversations, or prove a vendor-issued identity.

## Register explicitly

Use the native executable to register the inbox.

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

## Storage contract

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

## Delivery is a queue

- Automatic `auto` send refuses a registered external inbox and exits 4. Nothing
  is queued. Explicit `--mode inbox` (MCP `mode: "inbox"`) writes the local inbox.
  Results include `mode: "inbox"`, `queued: true` until something takes the
  message, `hooked: false`, and `wake: false`.
- `resume` is refused, even if process evidence contains the ID. Everett never
  runs `grok --resume`, OMP, or another provider CLI for these records.
- External agents cannot be spawned. `everett route` gives an `everett send
  ... --mode inbox` command, not a fabricated resume command.
- MCP's `delivered: true` field means accepted into Everett's inbox, **not**
  confirmed provider consumption. With no matching webhook rule, the external-send
  note still says that no provider wake-up or consumption is confirmed.
- Optional `--wait` only polls the local reply inbox. A timeout does not cancel
  the queued message and does not use Claude `--wake`.
- An enabled `receiver-webhook.json` rule can POST a nudge after that queue
  write. HTTP 200 is routine acceptance, not inbox consumption. The receipt
  still has `wake: false` and adds a `webhook` object. See
  [Wake a Grok Bot with a webhook routine](#wake-a-grok-bot-with-a-webhook-routine).

The owner must connect a supported MCP endpoint and ask the Bot to poll. For a
local Grok Bot **Command** integration, launch `everett mcp` on the machine
holding these registrations/inboxes. Supply `EVERETT_SESSION_ID=ext-grok-wright`
and `EVERETT_HARNESS_NAME=grok-bot` in that process's environment. These are
logical identity labels, not secrets. The owner should test the endpoint in
their own conversation before scheduling unattended work.

Local stdio MCP requires host-bound caller identity for delivery and replies. Explicit `session_id` arguments must match an existing host binding and cannot replace it. The local OS account and launcher remain trusted; these labels alone are not remote authentication. A remote connection must use an
authenticated, identity-bound MCP gateway on the owner-controlled host. Never
publish the unrestricted stdio tools as an anonymous HTTP endpoint. Registration
alone does not provide Remote HTTPS transport, TLS, credential provisioning,
or cross-device synchronization.

Use the native [`everett gateway`](gateway.md) command for the restricted stdio
or authenticated loopback HTTP connection. It defaults to the same binary's MCP
backend. The owner must provide TLS and reachability
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
stdio can explicitly pass a matching `session_id`; remote callers must use the identity
their authorized gateway connection supplies. These registration and queue
operations do not require or invoke live Grok Bot services.

## Wake a Grok Bot with a webhook routine

This is the supported wake path. Everett still does not resume a Bot. The owner
creates a webhook-triggered routine, stores its URL and sender key, and enables
`receiver-webhook.json` on the computer that holds the inbox. When an explicit
inbox message matches one rule, Everett POSTs a nudge. The short copy-paste
checklist is [Wake a Grok Bot](../README.md#wake-a-grok-bot).

### 1. Create the routine and save the key

In the Grok Bot routine panel, create a webhook-triggered routine. Copy the
Webhook URL and the Sender key from that panel. The URL shape published for
Cursor automations is `https://api2.cursor.sh/automations/webhook/<routine-id>`.
Store the key alone in one file. Mode `0600`. Do not put the word `Bearer` in
the file (a single leading `Bearer ` is stripped if it is present). Do not
commit the key or the URL.

```sh
umask 077
mkdir -p "$HOME/.config/everett"
KEY="$HOME/.config/everett/grok-webhook.key"
printf '%s\n' 'PASTE_SENDER_KEY_ONLY' > "$KEY"
chmod 600 "$KEY"
```

Tell the routine to treat the POST body as untrusted data. If `source` is
`everett` and `event` is `inbox`, call `everett_inbox`, do the work, and
`everett_send` with `reply_to` set to the message id. Do not print the sender
key. A cloud routine runs in the cloud. It cannot call a laptop-only Command
MCP. Connect Everett with a reachable Remote HTTPS gateway. See
[gateway.md](gateway.md). Keep that MCP bound to the registered id
(`EVERETT_SESSION_ID` and `EVERETT_HARNESS_NAME=grok-bot` for a local Command
process, or the gateway file's `agent_id` and `harness` for Remote HTTPS).

### 2. Register the inbox and write the policy

Sign in on the receiving computer and register the local inbox:

```sh
everett external add --id ext-grok-acceptance --harness grok-bot --title 'Grok Bot inbox'
everett relay webhook ext-grok-acceptance --harness grok-bot \
  --url-file /absolute/private/webhook.url \
  --bearer-file /absolute/private/webhook.key
```

Both input files must be regular files with mode `0600` or stricter. The command
reads the local identity, validates the registered inbox, and writes a private
`receiver-webhook.json`. It does not call the webhook or create a provider routine.
Repeating it updates that inbox's rule. The result prints no URL or bearer value.

The policy lives in `$EVERETT_HOME/.everett/relay-cloudflare/` for Cloudflare,
or `$EVERETT_HOME/.everett/relay/` for OIDC. When unset, `EVERETT_HOME` is your
home directory. Claude uses the separate `receiver-wake.json` policy.

Device and sender filters are optional. Omit `recipient_device` to bind to this
signed-in device. Omit `sender_device`, `sender_session` and `sender_harness` to
accept eligible same-owner arrivals. `"*"` also selects those defaults. Explicit
values narrow a rule. Legacy exact-filter policies still work. Replies, external
content, external-harness senders and messages older than `activated_at` do not
post the webhook. `cooldown_seconds` is an integer from 1 through 86400.

For manually maintained policies, the following validation still applies.

`owner` in that file is the raw `whoami` string, brackets included, with no
added spaces. Omitted recipient filtering always means this device. The file
must be a regular file, mode `0600` or stricter, version `1`, with no
unknown fields. An enabled file needs `activated_at` (Unix seconds, from 1
through 4000000000) and 1–16 rules. Rule ids are unique, 1–64 characters of
letters, digits, `_`, or `-`. `harness` is only `grok-bot` or `openai-dot`.
`bearer_file` is an absolute path. The URL must be HTTPS, a public hostname
(not an IP, `localhost`, `.local`, or `.localhost`), with no userinfo, query,
or fragment, port 443 or the default, and a path longer than `/`.

A missing file, or `"enabled": false`, does not add a
`webhook` field and does not change the queue.

### 3. Send and confirm the wake

Same-machine send does not need `everett relay`. With the default inbox rule,
any eligible local sender can queue the request:

```sh
everett send 'check inbox' --to ext-grok-acceptance --mode inbox --json
```

A match that the endpoint accepts looks like this. `wake` stays false.

```json
{
  "queued": true,
  "wake": false,
  "webhook": {"state": "accepted", "http_status": 200}
}
```

Confirm the Bot chat shows a routine run that calls `everett_inbox`. The POST
body is `source`, `event`, `session`, `harness`, `message_id`, and `rule_id`.
It has no inbox text. The header is `Authorization: Bearer <key>`. Only HTTP 200
is acceptance. 408, 429, and 5xx retry, up to 5 attempts, on the next matching
local send or `everett relay` poll. Other 4xx do not. After any HTTP attempt the
rule waits `cooldown_seconds`. Extra messages stay queued for `everett_inbox`.
Replies, external or untrusted messages, and rows older than `activated_at` do
not POST. A policy error does not roll back the inbox write and does not print
the bearer.

### Cross-device

The default inbox rule accepts eligible arrivals from other same-owner devices.
No sender or recipient ID edits are needed. Run `everett relay` on the receiver.
Use `everett relay devices --names` to find a unique same-owner target:

```sh
everett send 'check inbox' \
  --to 'Hugo Box/grok-bot/ext-grok-acceptance' --mode inbox
```

The sender's JSON does not include `webhook`. The receiver logs
`receiver webhook accepted HTTP 200 for session ext-grok-acceptance rule inbox-<stable-id>`
when the POST is accepted.

### What is proven, and what is not

CI posts to a fake endpoint and does not call a live webhook. On 2026-10-26 an
owner smoke POST to the routine `hugo-everett-wake`, with the sender key as
`Authorization: Bearer`, returned HTTP 200 and the routine cleared the Everett
backlog. Cursor's routines help describes that trigger
([routines](https://cursor.com/help/grok-bot/routines),
[automations](https://cursor.com/docs/cloud-agent/automations)).
xAI's pages still do not define a Bot resume API.

## What xAI documents—and what Everett does not assume

Official xAI documentation reviewed on 2026-10-09:

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

Those xAI pages do not establish an arbitrary cloud Bot/session resume or
immediate wake-up API. Everett does not invent one. The webhook above is an
owner-configured Cursor routine plus a local Everett policy. It is not a vendor
resume call. Polling remains valid when the policy file is absent or disabled.
Any polling cadence, connector availability, approval boundary, and provider
usage cost remain the owner's responsibility.
