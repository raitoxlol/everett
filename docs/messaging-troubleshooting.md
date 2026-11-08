# Troubleshoot message delivery

Use this guide when a session is missing, a send fails, or a queued request has
no reply. Keep the message ID from the first attempt until its outcome is known.

## Diagnose the failed step

Start with `everett relay diagnose` for a JSON snapshot that makes no network
calls. MCP clients can call `everett_agent_relay` with `action="diagnose"`.
No account sign-in or running receiver is required.

The snapshot reports the receiver's held local leases, cached credential and
key-file presence, external registration counts, webhook configuration and retry
counts, and cached outgoing and encrypted ingress counts. `cached` or `present`
does not prove current server registration or authorization. `running` means
the local receiver holds its leases with a matching cached identity; it does not
prove that polling succeeds. Webhook configuration does not prove endpoint
acceptance. Bearer files are never opened by diagnostics.

Each cache or registration scan examines at most 256 directory entries. Private
JSON reads are capped at 256 KiB per file; receiver capability status has its
own smaller bound. Unsafe, oversized, or malformed records count as unreadable
and remain untouched. `truncated: true` means counts are partial. Cached queue
counts include all stored accounts. Expired encrypted ingress can still be
recoverable after an earlier delivery, so diagnosis does not delete it.

Output excludes message text, identifiers, webhook URLs, bearer paths, and
credential values. CLI diagnostics do not create or modify state. MCP uses its
normal call-metadata log; the diagnostic handler does not change relay state.
Use the checks below when you deliberately need a launcher probe or a remote
observation.

1. Run `everett --version` on each computer involved.
2. Run `everett doctor` on the sender and recipient. Check the reported executable,
   session stores, hooks, and MCP registrations. Doctor probes local launchers;
   it does not prove a provider received a prompt.
3. Run `everett ls --json` to find the exact target. For older sessions, run
   `everett --hours 168 ls --json`.
4. Preview a local send with
   `everett send "delivery check" --to SESSION --dry-run --json`.
5. If a request already exists, inspect the recipient's inbox with
   `everett inbox --session SESSION --peek --json`. A peek leaves messages unread.

For a remote destination, use `everett peer status PEER MESSAGE_ID` with the
original peer and message ID. SSH status can import a correlated reply into the
sender's inbox. Hosted relay status queries signed observations. These commands
contact the selected peer or service, so they are not offline diagnostics.

| Observation | Meaning and next action |
| --- | --- |
| Default ping refuses with nothing queued | Check the target's supported transport. Choose explicit inbox delivery only if delayed pickup is acceptable. |
| Dry-run has `command: null` | T3, OMP extension, and Claude background pings use a nudge. Check `transport`; do not substitute a provider resume command. |
| `queued: true`, `delivered: false` locally | The inbox has accepted the request. Check enabled hooks or have the target read its inbox. |
| `pinged: true` | Dispatch was accepted. Check pickup and a correlated reply separately. |
| OMP reports `extension_pending` and `pinged: false` | A live extension transport accepted the inbox item. A nudge and reply remain unconfirmed. |
| Remote state is `unconfirmed` | A local retry record exists, but server queueing is unconfirmed. Do not create a replacement request blindly. |
| `posted: false` | No enqueue acknowledgement was recorded locally. This does not prove the server never accepted the request. |
| Hosted state is `delivered` or `claimed` | Respectively, authorized device ingress or inbox pickup was observed. Task completion remains unconfirmed. |
| A send error names a retained message ID | Inspect that original message before retrying. The recipient may already have read it. |
| State is `expired` | The transport retention period ended. Check for an earlier pickup or reply before intentionally sending a new request. |

A saved authenticated reply keeps the status `replied` after request expiry.
An expired unconfirmed request no longer carries a retry instruction. Status
still checks the current account and peer; it does not rewrite the receipt.

See [message delivery policy](message-delivery.md) for receipt semantics and
[cross-device peers](cross-device.md) for SSH status and retry behavior.

## Check transport compatibility

Use the [agent support table](agent-support.md) for the current capability of each
harness. A process being alive does not establish a supported wake transport.

T3 pings require the owning thread and the T3 CLI. Resume the thread through T3;
a provider CLI resume would fork its context. OMP requires a fresh extension lease
bound to the live process. Attached Claude pings require a full session UUID, an
existing project directory, and a verified background session. The preview checks
target metadata; `everett proof wake --to SESSION --dry-run --json` also performs
the proof command's bounded transport preflight.

Registered external sessions use explicit inbox delivery. Remote-derived callers
remain inbox-only. For Cloudflare, both computers need the selected account,
registered device keys, the required peer trust, and a running receiver. Use
`everett relay whoami` to verify the selected hosted identity. This contacts the
service. Follow [Cloudflare setup](cloudflare-native.md) and [account repair](auth.md)
before changing issuer or relay settings.

## Migrate an older registration

Inspect registrations before applying a repair. Run `everett install-hooks` and
`everett install-mcp` without `--apply`. Keep other servers and unrelated settings.

If a registration still calls the retired Python package, use
`everett install-hooks --apply` and `everett install-mcp --apply`. For an existing
Claude MCP launcher, use `everett install-mcp --claude --repair --apply` to save a
backup and repair its entry. Select the corresponding harness flag for other
clients. Restart the affected client, then run `everett doctor` again.

In a local interactive wizard, press `l` at the account screen to continue with
local setup. Applying local hooks and MCP registrations does not sign into a
harness provider. Restart the selected agents and verify the loaded Everett tools.

Do not copy credential files into a report or another computer. Credential
revocation, key rotation, account migration, and production receiver restarts need
their own deliberate operation. See [accounts](auth.md) for their distinct effects.

## Verify pickup and a correlated reply

For one explicit local target, run
`everett proof wake --to SESSION --timeout 120 --json`. This sends a canary and
asks the target for a nonce-bearing reply. The command can invoke the target's
provider. Its JSON records dispatch, pickup, and the matching reply separately.
Exit zero with `dry_run: false` proves the matching reply arrived within the budget;
read `picked_up` separately because a reply after a peek can pass without a pickup
observation. See [wake proof exit codes](message-delivery.md#wake-proofs).

For a deliberately queued path, send once with
`everett send "reply with delivery-check" --to SESSION --mode inbox --json`.
Have the recipient read that message and run
`everett reply MESSAGE_ID "delivery-check"` from its bound session. Read the
sender's inbox and confirm both the expected text and `reply_to` equal to the
original message ID. For a remote target, poll its original peer status first.
An enqueue receipt, HTTP success, process heartbeat, or provider acceptance alone
does not pass this check.

Record the binary versions, target, transport, original message ID, receipt state,
and correlated reply. Exclude tokens, credential contents, and private message
bodies from shared diagnostics.
