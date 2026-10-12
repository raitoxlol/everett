# Cross-device Everett

Everett supports two inbox-only transports per peer: manual SSH (phase 1) and an
end-to-end encrypted Convex relay (phase 2), from [issue #4](https://github.com/raitoxlol/everett/issues/4).
SSH pairing remains **one-way**, with replies through **status polling**. Convex peers
pin both directions and exchange messages, replies, and pickup receipts over a live WebSocket.

## Convex relay: owner deployment

The Everett client stays Rust-only. The deployable backend in `convex/` uses Convex's
TypeScript schema/function runtime. Node is needed only for deployment/backend tests,
not on machines running the Rust binary. No SSH reachability, port forwarding, or Tailscale
is needed; both machines make outbound HTTPS/WebSocket connections.

1. Create an OIDC **native/public** application supporting device-code login and refresh
   tokens, scopes `openid email profile offline_access`, with no client secret. Enable
   issuance of signed ID tokens both at login and refresh. Note the issuer (including
   its trailing slash, if any) and the public application/client ID. Convex must be able
   to reach the issuer's discovery/JWKS endpoints. See [auth setup](auth.md).
2. Install Node 24 and sign in to your Convex account. From this checkout's root:

   ```sh
   npm ci
   npm run typecheck
   npm test
   npx convex dev --configure new --once
   ```

   Choose a **cloud** development deployment. Initial deployment may fail until the
   following environment values are set; project selection still creates `.env.local`.
3. Configure the selected development deployment, then push schema/functions:

   ```sh
   npx convex env set OIDC_ISSUER 'https://YOUR-ISSUER/'
   npx convex env set OIDC_CLIENT_ID 'YOUR-PUBLIC-CLIENT-ID'
   npx convex dev --once
   ```

   For production, configure production **separately** and deploy:

   ```sh
   npx convex env set --prod OIDC_ISSUER 'https://YOUR-ISSUER/'
   npx convex env set --prod OIDC_CLIENT_ID 'YOUR-PUBLIC-CLIENT-ID'
   npx convex deploy
   ```

   Copy the selected deployment's HTTPS URL from the Convex dashboard (for example
   `https://YOUR-DEPLOYMENT.convex.cloud`). Do not mix dev and prod URLs. `.env.local`,
   generated types, deployment keys, and device credentials are ignored by git.

## Convex relay: two-machine trial

On **both** machines, build/install this branch and configure the **same** issuer, public
client ID, and Convex URL. Sign in as the **same provider user** (same JWT issuer + subject):

```sh
cargo install --path everett-rs --locked --force
export EVERETT_AUTH_ISSUER='https://YOUR-ISSUER/'
export EVERETT_AUTH_CLIENT_ID='YOUR-PUBLIC-CLIENT-ID'
export EVERETT_CONVEX_URL='https://YOUR-DEPLOYMENT.convex.cloud'
everett login --force
everett relay register
everett relay devices
```

Alternatively, persist configuration in `~/.everett/config.toml`:

```toml
[auth]
issuer = "https://YOUR-ISSUER/"
client_id = "YOUR-PUBLIC-CLIENT-ID"
[relay]
url = "https://YOUR-DEPLOYMENT.convex.cloud"
```

`relay register` prints this device's `deviceId` and a SHA256 fingerprint covering the
owner, Ed25519 key, and X25519 key. **Confirm those values on the other machine's terminal
or another trusted out-of-band channel**, never solely through the registry. The relay
fingerprint includes the encryption key and is distinct from the SSH fingerprint.
Private Ed25519 identity is reused from `peer identity`; X25519 private keys are created
locally. Do not copy `~/.everett` between machines: they must have different device keys.

On A, pin B. On B, pin A (substitute the confirmed IDs/fingerprints):

```sh
# A
everett peer add-relay machine-b --device B-DEVICE-ID --fingerprint SHA256:B-CONFIRMED-HEX
# B
everett peer add-relay machine-a --device A-DEVICE-ID --fingerprint SHA256:A-CONFIRMED-HEX
```

An alias selects exactly one transport; an existing SSH alias cannot become a relay alias
without explicit removal. `peer list` shows both transports. On both machines, leave
this foreground process running in a separate terminal (Ctrl-C stops it):

```sh
everett relay
```

Start a Claude session on A and a Codex session on B. On each machine, run `everett ls --json`
and note the **full** local session IDs. Convex does not publish session listings or cards;
remote `ls`/`card` remain SSH-only. From A (substitute the IDs):

```sh
EVERETT_SESSION_ID=A-CLAUDE-ID EVERETT_HARNESS_NAME=claude \
  everett send 'Hello from A' --to machine-b/codex/B-CODEX-ID \
  --idempotency-key two-machine-hello --json
```

Record the returned `message_id`. On B, the installed hooks or an agent's existing
`everett_inbox` MCP tool consume the local inbox; explicit terminal pickup also works:

```sh
everett inbox --session B-CODEX-ID --peek --json  # inspection only
everett inbox --session B-CODEX-ID --json         # actual pickup
EVERETT_SESSION_ID=B-CODEX-ID EVERETT_HARNESS_NAME=codex \
  everett reply MESSAGE-ID 'Hello back from B'
```

On A, the reply arrives live through its relay and enters the original sender inbox:

```sh
everett peer status machine-b MESSAGE-ID
everett inbox --session A-CLAUDE-ID --json
```

Status is `queued` after Convex acceptance, `delivered` only after A observes B's signed,
encrypted **inbox pickup receipt**, and `replied` only after A verifies/imports the correlated
encrypted reply. Arrival on B or a durable local inbox write alone is not `delivered`.
The backend calls that intermediate state `accepted` and deletes the ciphertext row;
the CLI still reports `queued` until it observes a receipt. A terminal-origin send returns
replies to the local `human` inbox. Neither transport wakes a harness session.

### Offline behavior, safety, and limits

- Stop B's relay, send with a **new** idempotency key, and check A reports `queued`.
  Restart B before expiry; the initial live subscription drains its pending queue.
  Repeat after expiry: no message is imported and status is `expired`.
- Each envelope (message, reply, receipt) expires **one hour after creation**. The backend
  enforces a maximum 24-hour TTL, filters expired rows, and deletes them with a one-minute
  cron plus bounded continuation batches. If B imported a message before expiry, its local
  inbox follows Everett's existing local retention policy; the relay TTL no longer applies.
- If A cannot contact Convex, the command errors: a mode-0600 encrypted local retry record
  exists, but server queueing is **unconfirmed**. `everett relay` retries before expiry.
  Retry the same `--idempotency-key` with the exact same sender, destination, and body to
  reuse the original ciphertext/message ID; changing it is rejected. Expired requests
  need a new key only when an intentionally new send is wanted.
- A's relay must run to receive replies/receipts. An `accepted` message can remain CLI
  `queued` if the pickup receipt expires before A reconnects. Do not infer online presence
  or successful pickup from server acceptance. Replies are single-shot per original
  message; retrying the same reply is idempotent, changing its body is rejected.
- Payload cap is **64 KiB for the serialized plaintext**, including message/card metadata
  and JSON escaping; ciphertext adds a 16-byte tag. Hops stay encrypted, inherit recent
  local pickup counts, and increment once per send/reply across all devices; a fourth hop
  exits 7. Receipts do not consume a message hop.
- X25519 ephemeral-static agreement, HKDF-SHA256, ChaCha20-Poly1305, and Ed25519 protect
  content, including sender/recipient **session** IDs, harness names, replies, and receipts.
  Convex sees ciphertext plus owner/device IDs, envelope IDs/idempotency keys, timestamps,
  signatures, sizes, and transient acceptance/expiry metadata. Idempotency tombstones
  contain no content/ciphertext and expire seven days after the envelope TTL.
- All public functions independently derive owner from the verified OIDC JWT. A claimed
  client owner is not authority. Unknown/wrong-account devices and changed signatures
  are rejected. The relay verifies its own pinned keys, AEAD, routing, TTL and hops before
  local writes. Unknown sessions/unpaired senders remain queued until TTL; the daemon
  retries cached pending rows when a session/pin becomes available.
- Device-key replacement is refused; remove a local pin with `peer remove ALIAS` and
  confirm a new device identity explicitly. For a lost device, remove its pins on every
  other machine and revoke its refresh tokens at the OIDC provider. Existing access/ID
  tokens may remain valid until provider expiry; removing local trust stops inbox imports
  immediately. Protect both devices' OS accounts: there is no forward secrecy against
  later compromise of a recipient's static key, and no trust in a compromised endpoint.
- The official Rust Convex client reconnects/resubscribes its WebSocket and refreshes
  OIDC tokens through a callback. Transient mutation failures retain retry records.
  An ended subscription, logout, changed owner, or failed authentication stops the daemon
  honestly; restart it after fixing login/configuration. Requests have 20-second deadlines.

### Offline verification

```sh
npm ci && npm run typecheck && npm test
cd everett-rs
cargo test --locked
cargo clippy --all-targets -- -D warnings
cargo build --release --locked
EVERETT_VERIFY_BINARY=target/release/everett cargo test --locked --test release_journey
```

`convex-test` runs the **actual backend functions** in memory, with no Convex login/network:
authentication and account isolation, signature tampering, duplicate idempotency keys,
expiry, signed acceptance, ciphertext deletion, and retention cleanup. Rust tests cover
AEAD round trips/tampering, wrong-account/device checks, caps/hops/TTL, Convex number
serialization, and two isolated device inboxes with pickup, receipts, replies, and replay
deduplication. CI runs Rust/release and backend checks on Linux and macOS. These fakes do
not prove production OIDC/JWKS configuration or real NAT/reconnect behavior; run the
two-machine trial above before rollout.

## SSH transport (phase 1)

Two devices can list sessions, read cards, send inbox messages, and poll delivery and replies
over manual SSH addresses. There is no registry, Everett sign-in dependency, Convex service,
Everett network listener, transcript sync, or automatic remote harness wake-up. Local listing
and routing remain local; remote targets are explicit.

OpenSSH must already be reachable on the receiving device. LAN, VPN, or Tailscale addresses work.
Each device needs its own Tailscale authentication if Tailscale is used; Everett login does not
authenticate a tailnet. Phase 1 supports Linux and macOS with `ssh` and `ssh-keygen` installed.
Only Ed25519 SSH host keys are supported.

## Pair once

On the receiving machine, locate a stable absolute Everett binary path and run:

```sh
command -v everett
everett peer identity
ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub -E sha256
```

On macOS, enable Remote Login; on Linux, enable the SSH service. Confirm the printed SSH host-key
and Everett device-key SHA256 fingerprints on the remote terminal or another trusted
out-of-band channel. Neither an SSH prompt nor a future registry row proves first-pair identity.
Also run `everett peer identity` locally and confirm that this is the caller key the receiver
intends to authorize. Private keys stay on their device.

From the sending machine:

```sh
everett peer add linux-box --ssh wright@linux-box.tailnet.ts.net \
  --binary /home/wright/.local/bin/everett \
  --host-key SHA256:<confirmed-host-fingerprint> \
  --device-key SHA256:<confirmed-everett-fingerprint>
everett peer list
```

Use actual fingerprint values, not the angle-bracket placeholders. Add `--port 2222` for a
nonstandard SSH port. Setup requires **existing noninteractive SSH access** to that account
(a standard login key, optionally unlocked in the SSH agent). Password/keyboard-interactive
prompts are disabled. Everett ignores SSH config files, so use a literal address rather than
a configured alias or ProxyJump. The remote login shell must permit the setup commands.

Pairing captures an SSH host key in a temporary known-hosts file, compares it with `--host-key`,
then saves the pin. It reads and compares the remote Everett fingerprint before installing a key.
A wrong fingerprint fails before authorization. A failed setup can leave a pin or installed
tagged line behind, but no usable local peer config: retry with the same confirmed fingerprints,
or revoke the tag on the receiver. Completion requires a successful restricted RPC handshake.
Existing pins and paired device identities are never replaced automatically.

### sshd enforces the restrictions

Everett writes a tagged line in the receiver's **`~/.ssh/authorized_keys`**, which OpenSSH reads
by default. It does not write a pretend authorization file under `~/.everett/peer/`.

```text
command="/home/wright/.local/bin/everett peer serve --client <caller-device-id>",no-pty,no-port-forwarding,no-agent-forwarding,no-X11-forwarding ssh-ed25519 <caller-public-key> # everett-peer:<caller-device-id>
```

The forced command contains the canonical absolute binary path resolved on the receiver, not
a PATH lookup. SSH ignores the caller's requested command and invokes one bounded Everett RPC;
there is no general shell, PTY, port forwarding, agent forwarding, or X11 forwarding through
the peer key. Keep that binary at its installed path, or repeat pairing after moving it.

Everett preserves unrelated key entries, backs up existing files, and replaces only its matching
tag on retry. Directories use mode 0700; keys, pins, configuration, authorized_keys and backups
use mode 0600. Do not modify sshd policy to accommodate Everett. If sshd uses a nonstandard
AuthorizedKeysFile or ignores this file, the restricted handshake fails and pairing reports an
error. Confirm the effective sshd configuration yourself; Everett does not inspect or edit it.

```sh
# On the sender: revoke remotely, then remove the local config and pin (peer must be online).
everett peer remove linux-box

# On the receiver: remove only this caller's tagged authorization; works while sender is offline.
everett peer revoke <caller-device-id>
```

`peer remove` leaves local configuration intact on connection failure. For a host-key rotation,
first revoke on the receiver, then remove the obsolete sender config and known-hosts pin manually,
verify the new fingerprints, and pair again. Backups may contain the revoked line; restoring a
backup can restore access. Revocation takes effect on new SSH connections, not an already-running RPC.

## Use the peer

```sh
everett ls --peer linux-box --json
everett peer ls linux-box --all
everett peer card linux-box/claude/<full-session-id>
everett send --to linux-box/claude/<full-session-id> \
  --idempotency-key task-42 --json 'Check the failing test and reply with what changed'
everett peer status linux-box <returned-message-id>
```

Remote IDs have the shape `peer/harness/session-id`. `ls --peer` prefixes IDs only in its output;
the receiver continues to store ordinary local IDs. Remote sends require the full exact ID,
never a prefix, title match, local router decision, or automatic spawn/resume. They are
inbox-only; `--wait`, `--mode resume`, and `--spawn` are rejected. Remote listing uses the same
recent-window and automated-session filters as local listing. Use `--hours` or `--all` as needed.

The request includes the sender identity, an idempotency key, and the end-to-end hop count.
Generate and retain an explicit `--idempotency-key` if you may need to retry an uncertain send.
Without it, Everett generates and prints a key after success. A repeated identical request with
the same key returns the original message ID; reuse for different content, target, sender, or
hops fails. Deduplication is scoped to the authorized caller and persists in receiver inbox data.

### Observed delivery, not promises

| State | Evidence on the receiver |
| --- | --- |
| `queued` | Message exists in the target's inbox; no matching done entry or reply. |
| `delivered` | Message ID appears in the target inbox's `.done` set. This is pickup, not task completion. |
| `replied` | A correlated reply exists, even if pickup was not separately observed. |

Nothing is silently queued on the sending device. An offline SSH peer exits 7 with a clear
connection error; SSH connection setup has a five-second connect timeout and one attempt.
Each subprocess/RPC has a 20-second deadline. An interrupted or timed-out send may have reached
the receiver without returning an acknowledgement: retry the **same** request/key to resolve
uncertainty rather than generating a new key. A local outgoing receipt is written only after
the receiver acknowledges the message; it is correlation metadata, not a retry queue.

### Replies are status-only

On the receiver, the agent uses its ordinary local inbox and reply commands:

```sh
everett inbox --session <full-session-id> --json
everett reply <message-id> 'The test is fixed'
```

The existing MCP `everett_send(reply_to=...)` also retains these replies. Everett retains the
reply locally in the peer-specific inbox and reports `RETAINED`, not remote delivery. It makes
no reverse SSH connection. The original sender calls `everett peer status` to fetch the reply;
that poll imports it once into the original local sender's inbox, or the human inbox when no
agent sender was detected. Repeated polls do not import duplicate replies, even after local
pickup. Offline periods do not discard the receiver's retained reply. Status rejects message
IDs belonging to a different paired caller. A second identical reply is idempotent; a changed
second reply is rejected.

Sending a new request in the reverse direction requires an independent `peer add` from that
device. **Replying does not.** This avoids incomplete reverse pairing and callback identity rules.
There is no remote `reply` RPC: the reply is posted on the device that owns the target session.

## Protocol and limits

`peer serve --client <device-id>` is the forced-command stdin/stdout endpoint, not a daemon.
It accepts one newline-terminated JSON frame and returns one JSON result/error. The client ID
comes from the installed command, not request data, and must still have a local authorization tag.
Allowed RPCs are `hello`, `ls`, `card`, `send`, `status`, and self-revocation. Unknown operations
and fields fail. Hidden `peer install` is a setup endpoint available only through the existing
SSH login, never the restricted peer key.

- Request frame cap: **64 KiB**, including JSON escaping and the final newline.
- Response cap: **1 MiB**; narrow a listing if it is too large.
- Deadline: **20 seconds** for each RPC/setup subprocess, including blocked input and lock waits.
- Hop limit: **three end-to-end**, inherited from `EVERETT_HOPS` and recent local inbox pickup,
  incremented once on send/reply, and never reset at a device boundary. A fourth hop exits 7.
  As with local delivery, an identified live session inherits its highest received hop count
  for 15 minutes; this conservative guard is not a per-conversation tracing system.
- RPC identity uses only Everett's local private key, strict pinned host checking, and no SSH
  agent fallback, multiplexed connection, or forwarding. Message text travels in JSON **stdin**,
  never in an SSH remote-command argument. The user-facing `send` command still accepts text
  as its ordinary CLI argument.

Peer authorization grants access to session summaries/cards and inbox message/status operations
within the same OS account. It is not multi-user isolation or per-project authorization. The
private key is unencrypted for unattended RPC; protect the device and its OS account. Session
content and peer keys stay on devices; no backend is contacted for peer operations.

## Verification and real-machine checks

`cargo test --locked --test peer_native` uses a fake `ssh` executable and two isolated homes;
there is no SSH daemon or real network connection in CI. It tests pairing, fingerprint mismatch,
host-pin changes, tagged restrictions/removal, oversized frames, idempotent send/reply recovery,
cross-device hops, queued-to-delivered pickup, offline errors, and 20-second timeouts.
The existing Rust and release-journey checks run on macOS and Linux.

A fake SSH fixture **cannot prove sshd enforces key restrictions**. Before rollout, use two real
machines and verify:

1. Pair, list, read a card, send, pick up, reply, and poll into the original sender inbox.
2. Use the peer key directly with `ssh ... 'id'`: it must invoke only `peer serve`, not a shell.
   Attempt `-tt`, `-L`, `-R`, agent forwarding, and X11 forwarding; confirm restrictions on sshd.
3. Revoke the tag and verify that the peer key can no longer connect, while normal login still works.
4. Change the host key in an isolated test account/sshd and confirm mismatch refusal.
5. Stop SSH and verify no local queued message, then recover using the same idempotency key.
6. Confirm effective AuthorizedKeysFile and the stable absolute Everett path on both OSes.

## Still out of scope

Neither a registry record nor a relay may silently replace an established trust pin.
Phone transports, a cross-device dashboard, remote shared-core merge, transcript
synchronization, automatic address discovery, and automatic remote wake-up remain out of scope.
