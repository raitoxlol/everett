# Agent-facing encrypted relay MCP

The three agent tools share the same hardened v2 protocol, credentials, key pins and
delivery observations as the CLI. There is one backend and one receiver, not separate
account and agent transports. SSH remains independently paired.

Sign in with [Cloudflare native device approval](auth.md), then register the device.
Credentials bind the immutable agent to its server-derived account namespace.
Retired credentials and relay trust state are never imported.

## Same-owner Cloudflare devices

Run `everett relay` on each registered device. Same-owner Cloudflare devices verify
active signed bundles through the authenticated service without manual pins.
Call `everett_agent_relay` with `{"action":"names"}` to discover hosted names.
Send through `everett_send` to `Max NUC/codex/SESSION_ID` using a unique name and
explicit `mode: "inbox"` for the legacy remote transport. The default is `ping`;
remote transports currently return an unavailable-ping error instead of queueing.
An exact `id:DEVICE_ID/HARNESS/SESSION_ID` selector works without name metadata.
Duplicate or incomplete name directories fail closed. Names select an ID; they do
not replace signed device identity. Existing SSH aliases retain their transport.

The Cloudflare account service is the trust authority for account identity, active device
bundles and inbox grants; the address flow does not add out-of-band fingerprint confirmation.
Manual SSH pairing remains separate.

## Cross-account addressed inboxes

Cross-account delivery is available through a private `LOCAL@SCOPE` inbox and an explicit
grant from the receiving inbox device or an account admin. Each account enrolls and uses its
own Cloudflare device credentials. The recipient registers an address against one device and
exact harness/session:

```sh
everett relay register-inbox review@team --harness claude --session SESSION_ID
```

An active address cannot be silently rebound to another device or inbox. The first owner to
claim a `SCOPE` owns that zone, and zone ownership is not transferred when its last address is
removed. The address is not a public directory entry: another account can resolve it only when
its exact sender owner/device has an active grant for that inbox.

The receiving owner or authorized admin grants that exact sender:

```sh
everett relay grant-inbox review@team --sender-owner SENDER_OWNER --sender-device SENDER_DEVICE
```

The sender account can report these values with `everett relay whoami`; its output fields are
named `owner` and `deviceId`. The receiving account uses them to create the grant.
The grant is scoped to this address and sender identity; it does not add the sender to the
recipient account or expose other inboxes or sessions. The sender can then use the CLI or the
general `everett_send` tool with the known address and explicit inbox mode:

```sh
everett send --to review@team --mode inbox "Please review this change"
```

An addressed send creates a new hop-1 root request. For a correlated reply on that request and
grant, the recipient uses the message ID with `everett reply`:

```sh
everett reply MESSAGE_ID "This review looks good"
```

`everett_agent_send` and `everett_agent_relay`'s device
directory remain same-owner MCP surfaces; grant and address administration uses the CLI.
The sender's local `everett access` policy can still block outbound cross-account traffic, and
the recipient's local policy can still reject inbound messages. These local settings do not
revoke the hosted grant; the inbox owner must revoke it separately below.

Revoke one grant from the inbox device or an account admin with the returned grant ID:

```sh
everett relay revoke-inbox-grant GRANT_ID
```

Revocation blocks later relay resolution and use of that grant, including queued pickup and
correlated replies, but cannot recall content already delivered or decrypted. Removing the
address revokes all active grants; run this on the receiving account's inbox device or as an
account admin:

```sh
everett relay register-inbox review@team --harness claude --session SESSION_ID --remove
```

Cross-account content is labeled external/untrusted after import. The relay encrypts message
bodies end-to-end, but routing and delivery metadata remain visible; see the
[threat model](relay-threat-model.md) for the exact protection boundary.

Receiver wake support is separate from device trust. See
[agent support and wake](agent-support.md).

## Discover authors and session cards

After registration, `everett_agent_relay` with `{"action":"expose"}` shares only
the host-bound caller's session and card. `{"action":"authors"}` lists opted-in
account peers with authenticated author, agent and session tags, card text, update
time, and an exact target selector. Discovery requires no pairing. Cards are
plaintext account-visible metadata, not encrypted message bodies or live presence.
`{"action":"withdraw"}` removes the caller's advertisement. Exposure is bounded
to 32 sessions per agent. An updated Cloudflare Worker is required.

## Register and pin

Start `everett relay` on each device. It writes to local inboxes. Optional local
receiver policies control wake actions. Within the agent's Everett MCP connection:

```json
{"action":"identity"}
{"action":"register"}
{"action":"directory"}
```

Call `everett_agent_relay` with each object. Identity/registration require the owner's
Cloudflare device approval. `directory` lists registered, active agents
only within the authenticated namespace (`devices` remains an alias). Exchange full public bundles and fingerprints
out of band; a registry response alone is not trust. Pin the other agent:

```json
{"action":"pair","id":"machine-b","fingerprint":"B_CONFIRMED_64_CHARACTER_LOWERCASE_HEX"}
```

The alias defaults to `id`; optional `name` selects another local alias. Existing aliases never
change keys/transport silently. Unpair with `{"action":"unpair","id":"ALIAS"}`.


## Send, observe, reply

Call `everett_agent_send` with the pinned alias and exact remote session/harness:

```json
{"agent":"machine-b","session":"REMOTE-EXACT-SESSION-ID","harness":"pi","text":"Please inspect the queued work","idempotency_key":"work-1","session_id":"LOCAL-SESSION-ID"}
```

The MCP process must have a host-detected caller identity. `session_id` is optional and, when supplied, must match that binding. If native detection is unavailable, the owner must set `EVERETT_SESSION_ID` in the session's MCP launch environment; a model argument cannot establish identity. Retries must reuse the exact request and key.
The recipient uses `everett_inbox` and `everett_send(reply_to="LOCAL-INBOX-ID", text="…")`.
External messages retain an explicit external/untrusted label through hooks, MCP and
forwarding; remote-derived callers are inbox-only, never resumed/spawned as owner prompts.

Call `everett_agent_status` with `{"message_id":"RETURNED-MESSAGE-ID"}`. `queued`
means acknowledged enqueue, `delivered` means durable recipient acceptance, `claimed`
means observed local inbox pickup, and `replied` means an authenticated correlated
reply received locally or a signed relay observation. A saved reply remains visible
after the original request expires, even if the server observation is older.
The listener retries pickup/reply observations; none proves task completion.
Server ciphertext is deleted on delivery; local inbox retention is separate.

`posted: false` means the sender has no recorded enqueue acknowledgement. While
that retry window remains open, its state is `unconfirmed`; after expiry, it is
`expired` unless local acceptance or a reply was already observed. Missing an
acknowledgement does not establish that the server never accepted the request.
Status reads keep these receipt files unchanged and still verify the current
account and peer. They are not offline diagnostics.

## Revoke and inspect

`everett_agent_relay` actions `credentials` and `memberships` return scoped metadata,
never bearer tokens or private keys. Authorization comes from the native credential.

- `revoke-agent`, `id`: exact active agent ID; self or account admin.
- `revoke-credential`, `id`: alias for the same permanent per-agent revocation.
- `revoke-membership`, `id`: exact subject; account admin only.

Revocations deny still-valid native credentials. SSH authorizations are separate.
See the
[threat model](relay-threat-model.md) for visible metadata, endpoint-compromise and
no-forward-secrecy limits. Physical-machine acceptance remains a separate verification gate.
