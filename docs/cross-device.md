# Cross-device Everett

Everett supports two **independent, inbox-only transports per peer**: manual SSH
and an end-to-end encrypted Convex relay. SSH pairing remains one-way, with replies
through status polling; its setup below is unchanged. Relay peers pin both directions
and exchange encrypted messages/replies through live Convex WebSocket subscriptions,
with separate signed delivery observations. Neither transport wakes/resumes/spawns sessions.

Relay v2 supports the same owner/account or org across harnesses/devices behind NAT;
cross-namespace chat requires **both org admins' explicit consent for named agents**.
Here an agent is one provisioned device credential, not an individual harness session.
Local harnesses under that OS account share it. An alias chooses exactly one transport;
there is no relay-to-SSH fallback or backend SSH configuration.

## Convex relay: owner deployment

The client stays Rust-only. Node 24 is needed on the **operator/development machine**
for the TypeScript backend in `convex/`, not on machines running the installed binary.
Agents need outbound HTTPS/WebSocket access only. First configure an OIDC public/native
application with device-code login, refresh and signed per-agent ID tokens. Follow the
[Keycloak/native-provider recipe](auth.md#practical-keycloak-device-code-setup); a normal
shared user token is insufficient. `everett_agent`, `everett_org`, and `everett_role`
must be issuer-controlled, not client-editable. There is no public enrollment API.

Commands below run from the checkout root unless stated otherwise. Replace example
URLs, subject UUIDs and fingerprint placeholders. Never put actual tokens/admin keys
in shell arguments, docs, logs, git, or agent MCP output. These are operator runbooks,
**not a claim that a cloud/self-hosted deployment or live OIDC journey was executed**.

### Convex cloud

Use a clean v2 deployment for a first trial. v1 envelopes/pins/schema are incompatible;
an existing deployment requires a reviewed migration preserving revocation/replay
records, not deletion/re-enrollment of old identities.

```sh
npm ci
npm run typecheck
npx convex login
npx convex dev --configure new --once
# Select a CLOUD development deployment, not a local deployment.
```

On a new project the first push can fail because auth environment is not yet set.
Once project selection has written `.env.local`, configure the selected development
backend and retry:

```sh
npx convex env set OIDC_ISSUER 'https://login.example.com/realms/everett'
npx convex env set OIDC_CLIENT_ID 'everett-relay'
npx convex dev --once
```

For production configure **production separately** and deploy:

```sh
npx convex env set --prod OIDC_ISSUER 'https://login.example.com/realms/everett'
npx convex env set --prod OIDC_CLIENT_ID 'everett-relay'
npx convex deploy
```

Copy the target HTTPS backend URL from the dashboard, e.g.
`https://YOUR-DEPLOYMENT.convex.cloud`. Do not use the dashboard or `.convex.site`
HTTP-actions URL. Dev and prod have separate data/provisioning: add `--prod` to
**every** operator `convex run` below when using production. Never copy development
credentials or deployment keys onto agents. See the official
[Convex OIDC configuration](https://docs.convex.dev/auth/advanced/custom-auth).

### Self-hosted Convex

Read the official [self-hosting guide](https://docs.convex.dev/self-hosting),
[Docker runbook](https://github.com/get-convex/convex-backend/blob/main/self-hosted/README.md)
and [own-infrastructure configuration](https://github.com/get-convex/convex-backend/blob/main/self-hosted/advanced/hosting_on_own_infra.md).
Everett uses custom OIDC, **not the separate Convex Auth package** (whose self-hosted
setup has different limitations). No frontend app is needed for this CLI relay.

On the operator's Linux server, install Docker Engine/Compose and a TLS reverse proxy
(e.g. Caddy), configure DNS for the three example domains, and allow public ports
80/443. **Block direct external access to ports 3210, 3211 and 6791**, including Docker's
published-port firewall behavior; alternatively bind the compose ports to loopback.
Protect the dashboard with your operator network/access policy. Then in a dedicated
service directory (not the Everett checkout):

```sh
mkdir -p "$HOME/convex-relay-service"
cd "$HOME/convex-relay-service"
curl -fsSLo docker-compose.yml \
  https://raw.githubusercontent.com/get-convex/convex-backend/main/self-hosted/docker/docker-compose.yml
```

Upstream `main`/`:latest` are moving targets. For production, download this file from a
reviewed immutable upstream commit instead, and archive it; the image-digest pinning
steps below make that exact chosen configuration reusable. No specific backend release
has been compatibility-tested here. Create `.env` alongside it with these **non-secret** origins:

```dotenv
CONVEX_CLOUD_ORIGIN=https://relay.example.com
CONVEX_SITE_ORIGIN=https://relay-actions.example.com
NEXT_PUBLIC_DEPLOYMENT_URL=https://relay.example.com
```

For a Caddy proxy **on that same server**, the corresponding Caddyfile blocks are:

```caddyfile
relay.example.com {
    reverse_proxy 127.0.0.1:3210
}
relay-actions.example.com {
    reverse_proxy 127.0.0.1:3211
}
relay-admin.example.com {
    reverse_proxy 127.0.0.1:6791
}
```

Caddy forwards WebSocket upgrades; another proxy must explicitly support long-lived
WebSockets and TLS. Apply the proxy configuration using your service manager. Start
Convex and generate an operator key in a private terminal:

```sh
# Resolve moving image tags once, then retain/reuse the pinned file (Compose v2).
umask 077
docker compose pull
docker compose config --resolve-image-digests > docker-compose.pinned.yml
sha256sum docker-compose.yml docker-compose.pinned.yml > compose.sha256
docker compose -f docker-compose.pinned.yml up -d
# Secret output: put only in the operator's protected secret store/.env.local.
docker compose -f docker-compose.pinned.yml exec backend ./generate_admin_key.sh
```

Archive the compose files/checksums, upstream commit, resolved image digests and proxy
configuration with the deployment record; do not resolve `latest` again on restart.
The rendered YAML can contain interpolated secrets if you add any: protect it, do not
commit it. Digest pinning follows Docker's official
[`compose config` reference](https://docs.docker.com/reference/cli/docker/compose/config/).

The compose volume contains the database; arrange persistent storage/backups and test
restores. Do not run `down -v` or reset data as a way to recover revoked identities.
On the operator machine, use a **separate Everett checkout** for this self-hosted target.
In an ignored mode-0600 `.env.local`, populated through a secret editor/store, set:

```dotenv
CONVEX_SELF_HOSTED_URL=https://relay.example.com
CONVEX_SELF_HOSTED_ADMIN_KEY=OPERATOR_KEY_FROM_SECRET_STORE
```

Do not leave cloud `CONVEX_DEPLOYMENT`/`CONVEX_DEPLOY_KEY` selections in that checkout
or environment. The repo pins its Convex CLI dependency; use `npm ci` rather than
silently upgrading it. Push to this target:

```sh
npm ci
npx convex env set OIDC_ISSUER 'https://login.example.com/realms/everett'
npx convex env set OIDC_CLIENT_ID 'everett-relay'
npx convex dev --once
# Subsequent non-development pushes may use: npx convex deploy
```

Internal provisioning below uses the same self-hosted selection, **without `--prod`**.
Agents set only `EVERETT_CONVEX_URL=https://relay.example.com`, never the operator key.
The Rust client requires an **HTTPS origin without a path**, so the guide's bare
`http://127.0.0.1:3210` cannot be used directly by Everett. Ensure the server can reach
OIDC discovery/JWKS and verify `/version` plus a real authenticated subscription before
rollout. These infrastructure instructions have not been deployment-tested here.

### Provision authoritative identities

Use the [auth recipe](auth.md) to create three independently controlled identities:
A = `machine-a`, org A admin; B = `machine-b`, org A member; C = `machine-c`, org B admin.
Get immutable provider subject UUIDs from the **trusted issuer Admin Console**, not
from arbitrary client enrollment requests. Replace `A-SUBJECT-UUID`, etc. below.
Only the deployment operator runs these internal mutations:

```sh
npx convex run relay:provisionIdentity '{"issuer":"https://login.example.com/realms/everett","subject":"A-SUBJECT-UUID","org":"org-a","agentId":"machine-a","role":"admin"}'
npx convex run relay:provisionIdentity '{"issuer":"https://login.example.com/realms/everett","subject":"B-SUBJECT-UUID","org":"org-a","agentId":"machine-b","role":"member"}'
npx convex run relay:provisionIdentity '{"issuer":"https://login.example.com/realms/everett","subject":"C-SUBJECT-UUID","org":"org-b","agentId":"machine-c","role":"admin"}'
```

For a cloud production journey, insert `--prod` after `convex run` on each command.
For account fallback provision `org:null`, omit the JWT org claim, and obtain distinct
issuer-bound agents with the **same subject**; the simple Keycloak per-user recipe does
not supply that enrollment mechanism. `relay register` never provisions membership.
Agent IDs, grant IDs, alias names and idempotency keys use `[A-Za-z0-9_.-]{1,160}`.

## Convex relay: two-machine trial

On **A and B**, install this branch (or the resulting release binary), set the same
issuer/public client/Convex URL, and approve login as the distinct provisioned users
`alice-a` / `alice-b` from the auth recipe. Use the self-hosted URL instead if applicable:

```sh
cargo install --path everett-rs --locked --force
export EVERETT_AUTH_ISSUER='https://login.example.com/realms/everett'
export EVERETT_AUTH_CLIENT_ID='everett-relay'
export EVERETT_CONVEX_URL='https://YOUR-DEPLOYMENT.convex.cloud'
everett login --force
everett whoami --json
everett relay whoami
everett relay register
everett relay devices
```

Configuration can instead be persisted using the [auth config example](auth.md#native-sign-in).
`relay whoami` must report the same owner namespace for A/B but different `deviceId`s.
Registration prints `device` and `fingerprint`; the relay fingerprint is **64 lowercase
hex characters, with no `SHA256:` prefix**. It covers the protocol version, owner,
agent ID and **both** independent Ed25519/X25519 public keys. It differs from the SSH
fingerprint/identity; v2 does not reuse SSH private signing keys.

Exchange the full printed public bundles, IDs and fingerprints via an authenticated
out-of-band channel (other machine's terminal, in person, or a previously verified
channel), never solely from a registry/grant response. Do not copy private keys or
`~/.everett` between machines. After confirming the values, pin both directions:

```sh
# A: replace the placeholder with B's exact 64-character fingerprint.
everett peer add-relay machine-b --device machine-b --fingerprint B_CONFIRMED_HEX
# B: replace the placeholder with A's exact fingerprint.
everett peer add-relay machine-a --device machine-a --fingerprint A_CONFIRMED_HEX
# Both
everett peer list
```

An alias already selecting SSH cannot be silently converted. On each machine keep a
separate terminal running the listener; there is **no `relay run` subcommand**:

```sh
everett relay  # foreground; Ctrl-C stops it, not the harness
```

Start any supported harness session on each device; this example uses Claude on A,
Codex on B. Run `everett ls --json` locally and exchange **full exact session IDs**.
Relay sends do not publish/list remote sessions or cards; `ls --peer` / `peer card`
remain SSH-only. In command terminals with the same environment/configuration:

```sh
# A: substitute the exact local A and remote B session IDs.
EVERETT_SESSION_ID=A-CLAUDE-ID EVERETT_HARNESS_NAME=claude \
  everett send 'Hello from A' --to machine-b/codex/B-CODEX-ID \
  --idempotency-key two-machine-hello --json
# A: substitute the send result's message_id.
everett peer status machine-b A_MESSAGE_ID
# B: inspection does not claim the message; actual pickup does.
everett inbox --session B-CODEX-ID --peek --json
everett inbox --session B-CODEX-ID --json
# B: use the recipient inbox entry's id, NOT A's wire message_id.
EVERETT_SESSION_ID=B-CODEX-ID EVERETT_HARNESS_NAME=codex \
  everett reply B_LOCAL_INBOX_ID 'Hello back from B'
# A: status queries state from signed observations; replies arrive via A's listener.
everett peer status machine-b A_MESSAGE_ID
everett inbox --session A-CLAUDE-ID --json
```

B's `id` is a local `relay-…` ID; `relay_message_id` is the originating wire ID used
for A's status. Existing hooks or MCP `everett_inbox` can do the pickup instead;
`everett_send(reply_to="B_LOCAL_INBOX_ID", text="…", session_id="B-CODEX-ID")`
replies inside the receiving session. A terminal-origin send returns replies to the
local `human` inbox. Receiving only writes inbox state; it never starts a harness.

### Observed states (relay v2, not SSH pickup semantics)

| State | Evidence, not a promise |
| --- | --- |
| `unconfirmed` (local only) | Encrypted retry record exists; server enqueue acknowledgment was not observed. |
| `queued` | Convex enqueue commit acknowledged; no recipient delivery observation. |
| `delivered` | Recipient validated/persisted encrypted private ingress and committed its signed delivery observation, authorizing local inbox publication; server ciphertext row is deleted. Not session pickup or task completion. Crash recovery can finish publication later. |
| `claimed` | Listener observed the local inbox's `.done` pickup entry and committed a signed claim. `--peek` alone never claims. |
| `replied` | Recipient's encrypted correlated reply enqueue was acknowledged and its signed reply observation committed. Does not prove the original sender has imported/read that reply. |
| `expired` | Envelope TTL elapsed while still undelivered. |

Observations are plaintext **signed state metadata**, not encrypted receipt messages.
States are monotonic; a connection, query or daemon start is not delivery/pickup/reply.
The listener reconciles actual pickup with the server; if it is offline, `claimed` can
lag. Status after grant/agent revocation or key mismatch can **error**, not return a
historical success state. None of these states means work completion.

## Convex relay: two-org trial

Keep the working A/B org-A trial; add **machine C / `bob-c` / org B admin** provisioned
above. Both orgs must use this **same Convex deployment and configured issuer**.
This is not federation between independent deployments. A/C are admins and also the
named endpoints here; admins could instead authorize other registered named agents
in their own org. Repeat the install/config/login/whoami/register steps on C with its
own provider user/agent and start a local session (this example: Claude).

1. A admin and C admin independently exchange and verify **both public bundles,
   fingerprints, exact owner namespaces and agent IDs** out of band. Verify C reports
   owner `["https://login.example.com/realms/everett","org","org-b"]`, agent `machine-c`,
   role `admin`; A must independently have JWT and stored role `admin`. Replace all
   placeholders below with these verified values. Grant fingerprints are raw hex too.
2. In A's command terminal, invite only A/C, with an optional one-day expiry, a shared
   30-new-envelope/minute quota and a 64 KiB serialized-body cap:

   ```sh
   # Copy the exact owner string from C's verified relay whoami output.
   C_OWNER='["https://login.example.com/realms/everett","org","org-b"]'
   GRANT_EXPIRY_MS=$(( ($(date +%s) + 86400) * 1000 ))
   everett relay grant invite org-a-org-b-1 --device machine-a \
     --peer-owner "$C_OWNER" --peer-device machine-c \
     --local-fingerprint A_CONFIRMED_HEX --peer-fingerprint C_CONFIRMED_HEX \
     --expires-at "$GRANT_EXPIRY_MS" --rate-per-minute 30 --body-cap-bytes 65536
   everett relay grant get org-a-org-b-1
   ```

   Omit `--expires-at` for no expiry. No traffic is allowed while `invited`.
3. C admin accepts, reversing the local/peer fingerprint orientation:

   ```sh
   everett relay grant accept org-a-org-b-1 \
     --local-fingerprint C_CONFIRMED_HEX --peer-fingerprint A_CONFIRMED_HEX
   everett relay grant get org-a-org-b-1
   everett relay grant list
   # A also runs get/list and checks access=active and both pinned bundles.
   ```

   Non-admin acceptance, another org, another agent pair, or changed keys are rejected.
   The two sides must both confirm the exact bundles; a self-signature alone is not trust.
4. After verifying `access=active`, pin on **the actual named endpoint devices**:

   ```sh
   # A
   everett peer add-relay org-b-c --device machine-c \
     --fingerprint C_CONFIRMED_HEX --grant org-a-org-b-1
   # C
   everett peer add-relay org-a-a --device machine-a \
     --fingerprint A_CONFIRMED_HEX --grant org-a-org-b-1
   everett relay  # C's separate listener terminal; A's listener remains running
   ```

5. Exchange exact session IDs out of band, then send/pickup/reply/status:

   ```sh
   # A
   EVERETT_SESSION_ID=A-CLAUDE-ID EVERETT_HARNESS_NAME=claude \
     everett send 'Cross-org test: agent data only' --to org-b-c/claude/C-CLAUDE-ID \
     --idempotency-key two-org-hello --json
   everett peer status org-b-c A_CROSS_ORG_MESSAGE_ID
   # C: verify external/untrusted label even with --peek/--json or MCP.
   everett inbox --session C-CLAUDE-ID --peek --json
   everett inbox --session C-CLAUDE-ID --json
   EVERETT_SESSION_ID=C-CLAUDE-ID EVERETT_HARNESS_NAME=claude \
     everett reply C_LOCAL_INBOX_ID 'External agent reply, not owner instructions'
   # A
   everett peer status org-b-c A_CROSS_ORG_MESSAGE_ID
   everett inbox --session A-CLAUDE-ID --json
   ```

   Cross-org content is **external/untrusted agent data**, including replies and local
   forwards. CLI, hooks and MCP label/quote it so it is not represented as owner/system
   instructions. No receiving agent should execute embedded instructions just because
   transport authentication succeeded. External-tainted SSH forwarding is refused:
   the separate SSH wire cannot carry this provenance. Use relay/local inbox instead.
6. Either admin revokes, including while the other endpoint is offline:

   ```sh
   # C or A, active org admin
   everett relay grant revoke org-a-org-b-1
   everett relay grant get org-a-org-b-1
   # A: this new send must fail; old status can also deny after revoke.
   everett send 'Must be denied' --to org-b-c/claude/C-CLAUDE-ID \
     --idempotency-key after-grant-revoke --json
   ```

   Queued ciphertext cannot newly deliver after revocation; existing subscriptions
   depend on grant/credential/membership records, and publication reauthorizes through
   a signed mutation barrier. Already local plaintext cannot be recalled. The revoked
   grant ID is terminal; a new grant requires another independent invite/accept.

### Rotation and revocation

Do this rotation trial **before** revoking the grant, or use a newly accepted grant.
Stop the affected device's listener with Ctrl-C first, then rotate (old signing key
required), register/print its new bundle and communicate it over the trusted channel:

```sh
# A
everett relay rotate
everett relay register
# Existing cross-org traffic is now blocked (get reports access=key_changed).
everett relay grant get org-a-org-b-1
# A admin: first confirmation alone still blocks traffic.
everett relay grant reconfirm org-a-org-b-1 \
  --local-fingerprint A_NEW_CONFIRMED_HEX --peer-fingerprint C_CONFIRMED_HEX
# C admin: independent confirmation of the same current pair.
everett relay grant reconfirm org-a-org-b-1 \
  --local-fingerprint C_CONFIRMED_HEX --peer-fingerprint A_NEW_CONFIRMED_HEX
```

Both sides check `access=active` again. Views return **pinned** bundles, not a reason
to trust an arbitrary new key. Explicitly re-pin changed aliases; C removes/re-adds A:

```sh
# C
everett peer remove org-a-a
everett peer add-relay org-a-a --device machine-a \
  --fingerprint A_NEW_CONFIRMED_HEX --grant org-a-org-b-1
# B (same org, no grant): re-pin A's changed key too.
everett peer remove machine-a
everett peer add-relay machine-a --device machine-a --fingerprint A_NEW_CONFIRMED_HEX
# A: restart listener in its separate terminal.
everett relay
```

Old queued envelopes still bind the **old** fingerprints and remain blocked until TTL;
reconfirmation never silently decrypts/re-encrypts them. Intentionally new requests need
new keys/IDs. Same-org peers need explicit out-of-band re-pins too, but no bilateral grant.
If old signing keys were lost/compromised, revoke/provision a fresh agent instead of
attempting JWT-only replacement. Revoking an agent blocks both its signing/encryption
bundle and JWT credential in backend routing. For the lost-B trial, from still-active A:

```sh
# A command terminal
everett relay revoke-agent machine-b     # alias: revoke-device
# B command terminal: its already subscribed listener must also deny/stop.
everett relay whoami                    # denied with still-valid B JWT
everett relay register                  # denied; cannot re-enroll old agent ID
# A command terminal: optional permanent removal uses the trusted subject UUID.
everett relay revoke-member B-SUBJECT-UUID
```

Use B's terminal for the two denial commands, not A's. Also revoke issuer sessions/
refresh/offline tokens; deleting local pins or `logout` is not sufficient. A revoked
credential requires a new ID; revoked membership requires a new subject. Never erase
revocation tombstones to work around denial. See [auth revocation](auth.md#private-storage-and-revocation).

### Offline behavior, safety, and limits

- Before running the permanent revocation trial, stop B's relay, send from A with a
  **new** idempotency key, and check `queued`. Restart B before expiry to drain pending
  pages. Repeat with B offline beyond TTL: no new inbox publication and `expired`
  (provided the route/credentials/grant are still valid).
- Rust envelopes (messages/replies) expire **one hour after creation**; backend maximum
  TTL is 24 hours. Undelivered expired rows are filtered on fresh reads and deleted by
  minute cron in batches of 64 with bounded continuation. Clock time alone does not
  invalidate a subscription: expiry invalidation is normally within a minute **plus
  scheduler delay**, not an exact-time event. Clients re-query/recheck server time and
  expiry before new delivery. Grant expiry is similarly enforced and persisted by cron.
- Once signed delivery committed, relay TTL no longer applies to the delivered local
  copy or later `claimed`/`replied` observations. Private staging can recover an uncertain
  committed delivery after TTL only if the backend again authorizes it. Ordinary local
  undelivered inbox retention is seven days; revocation cannot erase downloaded plaintext.
- When A cannot confirm enqueue, it errors and retains a mode-0600 **encrypted** local
  retry record; `peer status` can report `unconfirmed`. `everett relay` retries before
  expiry. Reuse the same `--idempotency-key` with the exact sender, target, text and
  lineage to reuse the **same ciphertext/message ID**. Changing any of it is rejected.
  Backend replay/idempotency metadata is permanent, even after ciphertext deletion/TTL.
  After expiry, only an intentionally new send should use a new key. Replies are
  single-shot per original request; an identical retry is idempotent, changed text denied.
- A's listener must run to import replies; status separately queries backend state
  resulting from signed observation commits (not a signed status response). Do not infer
  presence or pickup from enqueue, a WebSocket connection, `delivered`, or even `replied`.
  An undelivered reply can expire independently.
- Global body cap: **65536 serialized plaintext bytes**, including routing/session/card
  fields and JSON escaping, not just visible text; AEAD adds a 16-byte tag. A grant
  may lower the cap to 256..65536 bytes. Backend checks ciphertext length minus tag;
  clients verify AEAD and plaintext fields. Grant rate is 1..120 new envelopes per
  fixed UTC minute **shared across both directions** (default 30); each sender agent
  also has 120/minute, shared with its issuer/subject account's other credentials
  in the same organization. Organization sends are capped at 600/minute total;
  each recipient holds at most 256 unexpired ciphertext rows. Delivery or TTL
  frees queue capacity. Identical retries do not consume quota or capacity. Boundary bursts can
  approach twice a window's limit; these are abuse controls, not guaranteed bandwidth.
- Signed visible parent/hop lineage is checked against the permanent observed parent
  and exact receiving credential. Roots start at 1; replies/forwards increment across
  devices/orgs; maximum **3 end to end**. Local pickup persists lineage/external taint
  for later sends, not merely a transient environment variable. Fourth hops fail.
  A malicious endpoint can manually invent a new root; encryption cannot prove semantic
  ancestry. Hop limits are not a sandbox or general loop-proofing against adversaries.
- X25519 ephemeral-static agreement, HKDF-SHA256, ChaCha20Poly1305 and Ed25519 protect
  serialized message/reply content, session IDs, harness names and card snippets. Sender/
  recipient namespaces/devices, grant, parent/hops, both fingerprints and timing are
  **visible signed metadata**; observations are visible too. See the complete
  [metadata/retention inventory and crypto limits](relay-threat-model.md).
- Public functions verify JWT **and** stored membership **and** their own revocable
  agent credential. Existing query dependencies invalidate on revocation; ingress
  publication also requires a signed authoritative observation, rejecting stale cached
  rows. Unknown sessions or unpinned senders are refused locally and remain queued until
  TTL; fresh pending queries retry when a session/pin becomes available. The client
  pages even through empty filtered pages, holding a final-page live subscription.
- The official Rust Convex client handles reconnect/resubscribe and uses an OIDC refresh
  callback. Revoked credentials, denied/ended subscriptions, logout or changed owner/
  agent stop the daemon. Fix provisioning/auth/config and restart; never bypass denial
  using a cached row. Transient failures retain private encrypted staging/retries, not
  visible unauthenticated messages. Requests have 20-second deadlines. No service manager
  or automatic session wake-up is installed by `everett relay`.

### Offline verification

```sh
npm ci && npm run typecheck && npm test
cd everett-rs
cargo test --locked
cargo clippy --all-targets -- -D warnings
cargo build --release --locked
EVERETT_VERIFY_BINARY=target/release/everett cargo test --locked --test release_journey
```

These are **verification commands to run**, not results claimed by this documentation
update. `convex-test` executes actual functions offline with fake identities: wrong org,
missing/revoked grants, agent revocation/re-registration, key changes/reconfirmation,
external labels, quotas/caps, durable duplicate keys, lineage/hops, TTL and ciphertext
row deletion. Its crypto helper covers public interoperability bytes and tampered
bodies; Rust unit/native fixtures cover encryption and isolated same-/cross-org inbox
journeys. Check the current test results before claiming these pass. Linux/macOS CI
workflows are configured for backend checks, Rust tests/clippy and the release journey.

Offline fakes do **not** validate real OIDC/JWKS, native-provider refresh, live WebSocket
revocation/reconnect/cache behavior, TLS/self-hosted infrastructure, or two real NATed
machines. Owner/operator release gate: run both journeys above, including already-open
subscriptions during agent/grant revocation, empty/final-cursor pending pages, rotation
and bilateral reconfirmation, offline TTL/recovery, idempotent retry and local external
CLI/hooks/MCP formatting. Record exact environments/results; do not represent an
unexecuted deployment or broad test suite as passed.

## SSH transport (phase 1)

Two devices can list sessions, read cards, send inbox messages, and poll delivery and replies
over explicit SSH addresses or addresses discovered from the signed-in Convex registry.
Manual SSH pairing needs no sign-in or Convex service. There is no Everett network listener,
transcript sync, or automatic remote harness wake-up. Local listing and routing remain local;
remote targets are explicit.

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
out-of-band channel. Neither an SSH prompt nor a registry row proves first-pair identity.
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

### Discover an SSH address by device name

On the receiving device, sign in with its provisioned agent credential and a unique display
name, for example `everett login --device linux-box`. Names are case-sensitive and use
`[A-Za-z0-9_.-]{1,160}`. Configure the SSH address that the sender can reach. Everett does
not detect addresses, create an SSH listener, or configure a tailnet.

```toml
[peer]
address = "wright@linux-box.tailnet.ts.net:2222"
```

Run `everett relay register` to register the public relay bundle and publish this device's
display name, declared address and signed metadata. Registration publishes a null address
when `[peer] address` is unset. Login only stores credentials locally because it does not
imply Convex provisioning. Each authorized `peer serve` startup also attempts publication
when signed in with a declared address. `peer serve --address user@host[:port] --client ID`
overrides the config for that invocation. The installed forced command uses the config.
This is a one-request endpoint invoked by sshd, not a daemon to start by hand.

Startup publication waits at most three seconds. Failures go to stderr and leave the SSH
request available. Retry `everett relay register` to confirm publication. `lastSeenAt` is
the server's last registration/publication time, not a heartbeat or proof of reachability.
Relay key rotation clears the published address; register again to sign it with the new key.

On the sender, sign in to the same account or org namespace and run:

```sh
everett devices
everett devices --json
everett peer add linux-box \
  --binary /home/wright/.local/bin/everett \
  --host-key SHA256:<confirmed-host-fingerprint> \
  --device-key SHA256:<confirmed-everett-fingerprint>
```

The listing drains registry pages and includes historical devices. Columns are name, device
ID, last seen in UTC, SSH address and revoked. The revoked column is `-` when the backend
does not expose `revokedAt`, `no` for null, and `yes` for a timestamp. JSON preserves that
field when present. `everett relay devices` keeps its original bounded active key-bundle list.
Legacy rows use the device ID as their name and have no address until they publish one.

Name resolution is an exact owner-scoped query, independent of listing pagination. It
rejects revoked credentials and verifies the address metadata's device signature. The
registry supplies only the address. Both out-of-band fingerprints and the remote binary
path remain required. A registry can still hide rows, replay metadata or advertise an
unfamiliar device key; its signature is not proof of ownership.

An explicit `--ssh` remains the selected address. When signed in with a configured registry,
pairing checks that name for conflicts and fails closed if the address or port disagrees,
or if the registry query fails. Without sign-in/registry configuration, manual `--ssh`
works offline as before. `--port` defaults to 22 for explicit addresses. Discovery uses the
published port and rejects a conflicting `--port`. Use `[user@]host` for manual `--ssh`;
published addresses require `user@host[:port]`, with brackets around IPv6 hosts.

If a discovered address changes for an already-paired alias, pairing reports that the peer
is already pinned. It preserves the host key and device pin. Verify the change out of band,
then explicitly remove and pair again. No registry operation refreshes trust. Only public
device metadata goes to the registry; sessions, cards, messages and shared-core content do not.

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
content and SSH private keys stay on devices. Established peer RPCs use the local pins;
registry discovery and optional startup metadata publication are separate from RPC content.

## Verification and real-machine checks

`cargo test --locked --test peer_native` uses a fake `ssh` executable and two isolated homes;
there is no SSH daemon or real network connection in CI. It tests pairing, fingerprint mismatch,
host-pin changes, tagged restrictions/removal, oversized frames, idempotent send/reply recovery,
cross-device hops, queued-to-delivered pickup, offline errors, and 20-second timeouts.
A loopback Convex HTTP stub also checks discovery, explicit-address conflicts, paged JSON
listing, revoked fields, signed metadata and unchanged pins after an address change.
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
synchronization, automatic re-pinning, and automatic remote wake-up remain out of scope.
