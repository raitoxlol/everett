# Messaging: Find The Failed Stage

An accepted relay request is not an awake agent. Diagnose the specific stage
before retrying, changing authorization, or asking someone to poll.

## Start On The Receiving Computer

```sh
everett relay status --json
```

This reads local state only. It does not refresh credentials, contact the
service, consume an inbox, or call a webhook. MCP clients can request the same
report using `everett_agent_relay` with `{"action":"status"}`.

The report omits webhook URLs, bearer-file paths, keys, ciphertext and message
text. `home` is important: an inbox registered under an isolated acceptance home
does not exist under the receiver's real home.

| Report | Meaning | Next Action |
| --- | --- | --- |
| `receiver.state: stopped` | No process holds this provider's daemon lock. | Start the intended receiver under the correct account and home. |
| `receiver.state: unknown` | A process is running, but current capability evidence is absent or invalid. | Check and update the actual running binary; do not infer compatibility from `everett --version`. |
| `receiver.state: invalid` | Local receiver evidence is unsafe or unreadable. | Repair the local status/lock files privately; other diagnostic sections remain available, but policy migration is blocked. |
| `receiver.state: identity-mismatch` | The receiver and CLI are using different account/device identities. | Reconcile the home, selected provider and intended process before changing policy. |
| `receiver.connection: retrying` | The last receive cycle encountered a temporary error. | Inspect the receiver's sanitized log; preserve encrypted staging. |
| `receiver.connection: stale` | The process holds its lease, but has not refreshed local health for 90 seconds. | Investigate a stalled poll or unavailable local status storage. This is not proof the agent is offline. |
| `webhook.state: invalid` | The receiving binary cannot use the policy. | Fix policy or binary compatibility before sending more tests. |
| Rule issue `inbox-not-registered-under-this-home` | The actual external inbox is absent here. | Register the exact inbox on this computer under the receiver's home. |
| Rule issue `sender-allowlist-restricts-account-delivery` | One or more device/session/harness filters remain. | Preview an account-trust migration if same-account wake is intended. |
| Rule issue `webhook-credential-unavailable-or-unsafe` | The private credential cannot be safely read. | Repair its provisioning or permissions privately; never paste it into chat. |

The capability lease is released when the receiver exits. An old PID file or
JSON status record is not accepted as evidence of a running compatible process.
The binary path in a valid report belongs to the running receiver, not merely
the CLI currently found on `PATH`.
The path is recorded at startup; overwriting that executable later does not
update a process that is already running.

## Register The Real Inbox

For an external Grok Bot inbox, run on the receiving computer:

```sh
everett external add --id ext-grok-acceptance --harness grok-bot
```

Use the actual session ID and harness. A device name is not a session ID.
Registering `hugo` on the sender's Mac does not register
`ext-grok-acceptance` on Hugo's computer.

## Same-Account Wake Without Sender Allowlists

First update and restart the actual receiver with a build that supports
`same-owner-webhook-v1`. Two different builds can share the same version string.
The CLI refuses to rewrite wake policy while an active receiver cannot prove
that capability.

Preview an existing rule:

```sh
everett relay trust-account ext-grok-acceptance --harness grok-bot --dry-run
```

A preview writes nothing, including no backup or ledger lock. It validates
the signed-in identity, registration, enabled policy and unique rule match.

Apply only when the preview is correct:

```sh
everett relay trust-account ext-grok-acceptance --harness grok-bot
```

The change removes sender device/session/harness filters for that one inbox and
binds its recipient to this device. Owner, recipient session/harness, endpoint,
credential, cooldown, ledger and unrelated rules remain protected. This does
not enable disabled policies, accept other accounts or call the webhook.

A byte-for-byte private backup is saved next to the policy as
`receiver-webhook.before-account-<binding-hash>.json` before the first change.
The account/inbox-bound backup is not overwritten by subsequent runs. Unsafe
or malformed existing backups block a new change. Repeating an already-applied
migration does not rewrite policy or replace the backup.

For recovery, inspect that backup privately and restore only the affected
fields, preserving any unrelated changes made since. Do not blindly replace a
current shared policy with an old whole-file backup.

## Send And Verify One Real Request

```sh
everett send 'Please reply with your current status.' \
  --to 'Hugo/grok-bot/ext-grok-acceptance' \
  --idempotency-key status-check-unique-request --json
```

Device names are advisory, account-scoped labels. Ambiguous names fail; obtain
the exact device ID if necessary. Use the exact remote session. There is no
silent fallback to a local inbox.

A trusted local CLI caller should have its real session/harness identity.
The harness environment variable is `EVERETT_HARNESS_NAME`, not
`EVERETT_HARNESS`. MCP clients should use their host-bound identity rather
than inventing sender IDs.

Retry an uncertain enqueue using the same key and identical request, not a new
key on every poll. Changing request contents with the same key is rejected.

Inspect a specific request:

```sh
everett peer status Hugo MESSAGE_ID --json
```

This command does contact the relay and may import replies. `relay status`
instead shows bounded, account/device-scoped cached observations:

- `unposted`: ciphertext is staged locally without enqueue acknowledgement.
- `queued`: enqueue was observed, but not recipient pickup.
- `expired`: the cached queued request passed its expiry; stop calling it pending.
- `delivered`: durable recipient acceptance was observed, not an agent wake.
- `replied`: a correlated reply observation exists, not proof of automatic wake.

The local summary scans at most 256 directory entries and returns up to 20
messages. Check `complete` before treating counts as exhaustive. It neither
decrypts ciphertext nor outputs message bodies.

Webhook ledger `accepted` means HTTP acceptance only. Confirm the actual routine
run, inbox consumption and correlated reply separately. A reply produced after
manually prompting the bot is useful delivery evidence, but is not automatic
wake evidence.
