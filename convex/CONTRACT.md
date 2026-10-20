# Relay v2 backend and Rust integration contract

This backend intentionally breaks the owner-only v1 wire format. Do not send v1 envelopes or reuse v1 pins. `tests/relay-wire-v2.json` and the matching Rust fixture are deterministic, public, **test-only** interoperability vectors; both suites check all signed/encrypted bytes. The agent MCP tools and CLI share this one protocol and authorization model.

## Authorization decision and trust boundary

Two designs were considered: (1) issuer-controlled signed agent-specific OIDC claims plus an authoritative deployment control plane, and (2) an issuer-backed enrollment/credential-issuance service. We use (1): it needs no new enrollment HTTP service or broadly usable owner bearer credential. It does require a capable, trusted issuer and explicit deployment provisioning. There is deliberately no public self-provisioning function.

Configure `OIDC_ISSUER` and `OIDC_CLIENT_ID` in the Convex deployment. Convex verifies OIDC signature, exact issuer, audience and token expiry. Each agent/device has its own JWT with these **issuer-controlled, non-user-editable** top-level custom claims:

- `everett_agent`: required `[A-Za-z0-9_.-]{1,160}`. Issuer-global, unique, stable credential ID for exactly one device; equals `Device.deviceId`.
- `everett_org`: optional, nonempty string of at most 160 characters. Must represent authoritative membership, never user metadata. If absent, the account is isolated by issuer + subject.
- `everett_role`: `admin` required for admin functions; this alone does not make a caller admin. The authoritative membership row must also have role `admin`.
- Standard `iss`, `sub`, `aud`, `exp` and client refresh claims remain required by the issuer/OIDC configuration. Do not print JWTs or put them on command lines.

`owner = JSON.stringify([issuer, "org", everett_org])`; absent org uses `JSON.stringify([issuer, "account", subject])`. The marker prevents an org name from colliding with an account subject. Issuer is not normalized (trailing slash matters). Rust uses compact `serde_json::json!([...]).to_string()` and must derive namespace/device ID from verified server `relay:whoami` rather than trusting arbitrary local owner config.

The deployment operator (or a future verified issuer webhook calling an internal mutation) must provision `{issuer, subject, org: string|null, agentId, role: "admin"|"member"}` using **internal** `relay:provisionIdentity`. It returns `{owner, deviceId}`. This inserts `memberships` and `credentials` or updates an existing active membership role. It rejects revoked membership or credential tombstones and rejects rebinding an agent ID to another subject/org. Revocation tombstones are permanent: use a new credential ID after agent revocation, and a new issuer identity/subject after membership revocation. Never grant deployment/admin keys to agent clients.

Every public function reads the caller's authoritative membership and credential, binding org + issuer + subject + agent. Every routing operation additionally checks the registered agent and both endpoints' active credentials/memberships. Provisioned agents can query their identity and permitted administrative metadata before registration; routing still requires registered keys. A member's admin-looking JWT cannot elevate its stored role. An admin cannot use its JWT to send/register/read pending as another agent.

These authorization reads are Convex query dependencies: revocation updates invalidate existing WebSocket subscriptions, not merely future registrations. Clients must treat query/auth errors as denial, immediately stop delivery, and discard cached pending results. The offline tests re-execute actual functions using the same identity after revocation; they do not emulate a real WebSocket/JWKS server. Data already downloaded/decrypted cannot be recalled.

## Public JSON types (camelCase, no omitted nullable fields)

```
Device = { owner: string, deviceId: string, signingKey: hex32,
           encryptionKey: hex32, signature: hex64 }
Envelope = {
  owner: string, recipientOwner: string, grantId: string|null,
  fromDevice: string, toDevice: string, messageId: string, idempotencyKey: string,
  parent: null|{owner: string, messageId: string}, hop: integer,
  createdAt: integer, expiresAt: integer,
  senderFingerprint: hex32, recipientFingerprint: hex32,
  ephemeralKey: hex32, nonce: hex12, ciphertext: hex, signature: hex64
}
```

All hex is lowercase, even-length, no `0x`. IDs use the agent-ID character/length rule. Times are Unix milliseconds, finite safe integers (absolute value <= 2^53-1); native Convex numbers reach Rust as floating point, so deserialize with checked integral conversion. A new enqueue permits absolute sender-clock skew <= 300000 ms, TTL > 0 and <= 86400000 ms, and expiry strictly after server now. Old identical retries can return a tombstone beyond TTL; authorization/key/grant checks still run first.

### Signed bytes and encryption

Every array below is compact JSON encoded as UTF-8 (no whitespace, no trailing newline), with strings escaped by JSON, numbers as integers, and explicit JSON nulls. Do not sign an object's unordered serialized fields.

```
device.signed_bytes = ["everett-device-v2", owner, deviceId, signingKey, encryptionKey]
fingerprint = lowercase_hex(SHA256(UTF8(compact_JSON(device.signed_bytes))))

AAD = ["everett-relay-v2", owner, recipientOwner, grantId, fromDevice, toDevice,
       messageId, idempotencyKey,
       parent == null ? null : [parent.owner, parent.messageId],
       hop, createdAt, expiresAt, senderFingerprint, recipientFingerprint,
       ephemeralKey, nonce]

envelope.signed_bytes = [UTF8_string(compact_JSON(AAD)), ciphertext]
rotation.signed_bytes = ["everett-rotate-v2", owner, deviceId, oldFingerprint, newFingerprint]
observation.signed_bytes = ["everett-observe-v2", recipientOwner, recipientDeviceId,
                            senderOwner, messageId, state, at]
```

Device and envelope signatures use Ed25519. Device signature is by its new/current signing key. Rotation requires BOTH a valid self-signed new Device and a signature over rotation bytes by the **old** signing key (which authorizes the complete old/new key bundles). Observation signature is by the current recipient signing key.

Each device owns distinct independent random Ed25519 and X25519 private keys, stored locally with restrictive permissions. Encrypt serialized plaintext Body JSON bytes, not merely its text field. Generate a fresh random ephemeral X25519 private key and 12-byte nonce for every new envelope. Reject non-contributory/all-zero X25519 shared secrets at the client. Derive 32 key bytes with HKDF-SHA256: IKM = X25519 shared secret, salt = UTF8 `everett-relay-v2`, info = exact AAD bytes. ChaCha20Poly1305 uses these key bytes, nonce and exact AAD; ciphertext is ciphertext bytes followed by the 16-byte tag. Hex-encode that, then sign envelope bytes. Recipient checks registered/self-signed keys, fingerprints, signature, recipient org/device, grant and TTL before decrypting and checks plaintext serialized Body length and fields afterwards. Backend cannot validate plaintext or AEAD tags; it enforces `(ciphertext_bytes - 16) <= bodyCapBytes` and signature/AAD metadata binding.

## Function references, exact args and results

Errors reject the operation (there is no success-shaped denial). All functions below are public except the two explicitly marked internal. Empty args means `{}`.

| Reference | Kind | Args | Result |
| --- | --- | --- | --- |
| `relay:whoami` | query | `{}` | `{owner, deviceId, role: "admin"|"member"}` (allows provisioned agent before registration) |
| `relay:register` | mutation | `{device: Device, sshDeviceId?}` | `null`; idempotent same bundle only; JWT agent must equal deviceId; `sshDeviceId` (the device's SSH peer id) is recorded once and never replaced |
| `relay:devices` | query | `{}` | bounded presentation list of current registered active own-org devices, at most 256; never authoritative for pairing or recovery |
| `registry:publish` | mutation | `{deviceId, name, sshAddress: string|null, signature: hex64}` | `null`; caller's own registered active device only; unique name in owner namespace; verifies metadata signature and sets server `lastSeenAt` |
| `registry:devices` | query | `{cursor: string|null}` | `{devices: RegistryDevice[], isDone, continueCursor}`; 128 rows per page in caller's namespace, including historical/revoked devices; caller needs active membership/credential but need not have registered keys |
| `registry:resolve` | query | `{name}` | exact active own-namespace `RegistryDevice` or `null` if absent; rejects revoked target credentials/membership |
| `relay:device` | query | `{deviceId}` | exact current active `Device` in the caller's own organization; caller and target must both be active |
| `relay:credentials` | query | `{}` | `{credentials: [{agentId, subject, status}], truncated}`; admin's own org or member's own credential; at most 256, no tokens |
| `relay:memberships` | query | `{}` | `{memberships: [{subject, role, status}], truncated}`; admin's own org only; at most 256 |
| `relay:rotate` | mutation | `{device: newDevice, oldFingerprint, signature: oldKeyRotationSignature}` | `{fingerprint: newFingerprint}` |
| `relay:revokeAgent` | mutation | `{deviceId}` | `null`; self or current org admin; permanent credential tombstone |
| `relay:revokeDevice` | mutation | `{deviceId}` | `null`; same authorization and permanent credential denial as `revokeAgent`; stamps registered device `revokedAt` once, never clears it |
| `relay:revokedDevices` | query | `{cursor?: string\|null}` | `{devices: [{deviceId, sshDeviceId?, revokedAt: ms}], cursor: string\|null}`; registered active caller, own namespace only, 256 revoked rows per page, `cursor:null` on the last page; no public keys or metadata updates |
| `relay:revokeMember` | mutation | `{subject}` | `null`; current org admin; permanent membership tombstone |
| `relay:enqueue` | mutation | `{envelope: Envelope}` | `{messageId, state: "queued"|"delivered"|"claimed"|"replied"|"expired"}` |
| `relay:pending` | query/subscription | `{deviceId: own JWT agent, cursor?: string|null}` | `{serverNow: ms, envelopes: [{envelope: Envelope, trust: "same-org"|"external-untrusted"}], isDone: boolean, continueCursor: string}`; up to 32 inspected rows/page to keep maximum-body results below 8 MiB |
| `relay:accept` | mutation | `{deviceId, senderOwner, messageId, at, signature}` | `null`; signature is observation bytes with state `delivered` |
| `relay:observe` | mutation | `{deviceId, senderOwner, messageId, state: "delivered"|"claimed"|"replied", at, signature}` | `null` |
| `relay:status` | query/subscription | `{fromDevice: own JWT agent, messageId}` | `{state: "queued"|"delivered"|"claimed"|"replied"|"expired"}` |
| `grants:invite` | mutation | `{grantId, deviceId: named local agent, peerOwner, peerDeviceId, localFingerprint, peerFingerprint, expiresAt: ms|null, ratePerMinute: integer, bodyCapBytes: integer}` | `{grantId, state: "invited"}`; local org admin only |
| `grants:accept` | mutation | `{grantId, localFingerprint, peerFingerprint}` | `{grantId, state: "active"}`; invited org B admin only |
| `grants:reconfirm` | mutation | `{grantId, localFingerprint, peerFingerprint}` | `{grantId, state: "reconfirming"|"active"}`; either org admin; both sides must explicitly confirm identical current bundles |
| `grants:revoke` | mutation | `{grantId}` | `null`; either org admin; terminal revocation |
| `grants:get` | query/subscription | `{grantId}` | Grant view; either org admin or exactly a named endpoint agent |
| `grants:list` | query/subscription | `{}` | `{grants: GrantView[], truncated: boolean}`; admins see own-org grants; members see only their endpoint grants; max 128 per side |
| `relay:provisionIdentity` | INTERNAL mutation | `{issuer, subject, org: string|null, agentId, role: "admin"|"member"}` | `{owner, deviceId}`; deployment operator only |
| `relay:expire` | INTERNAL mutation | `{}` | `null`; cron every minute, bounded 64-row batches for messages and grants, follow-up scheduled if full |

Device rows also store `generation`, optional `revokedAt: number|null` (absent means active)
and optional `sshDeviceId`; these fields are not part of the signed `Device` bundle. Registration inserts `revokedAt:null`; rotation
preserves it. Both `revokeAgent` and `revokeDevice` permanently revoke the credential
and stamp any registered row once. An unregistered provisioned credential can also be
revoked, but has no device row to return from `revokedDevices`. Caller authentication
requires verified OIDC plus active membership and credential checks; revocation and
revocation listing also require an active registered caller. Repeated admin revocation
does not change the original timestamp. Revoked IDs cannot register or be reprovisioned.

The revocation query reads the `by_owner_revoked` index, so active rows never fill a page,
and pages 256 tombstones at a time; doctor follows the cursor. Its only SSH effect is removal
of matching authorization tags (by `sshDeviceId` when recorded, else `deviceId`), never adding
or re-pinning. Existing v2 rows need no backfill: a missing `revokedAt` reads as active.
Retained revoked credentials without a device timestamp still deny access through the
credential tombstone. Preserve all credential and device tombstones.

`GrantView = {grantId, aOwner, aDevice, bOwner, bDevice, aBundle: Device, bBundle: Device, aFingerprint, bFingerprint, state: "invited"|"active"|"reconfirming"|"revoked"|"expired", confirmation: "a"|"b"|"both", expiresAt: ms|null, ratePerMinute, bodyCapBytes, access: "invited"|"active"|"reconfirming"|"revoked"|"expired"|"key_changed"|"agent_revoked"}`. Views return the **pinned** bundles, not automatically refreshed bundles. Obtain rotated bundles/fingerprints from the other admin over an independently authenticated channel; never silently trust fresh keys just because the server lists them. Key mismatch denies queued/new traffic even before state is changed to `reconfirming`. One side repeating confirmation cannot stand in for the other. Another rotation during reconfirmation restarts confirmation for the newly pinned pair. Revoked grant IDs cannot be reused.

Grants are symmetric, one named agent in A and one in B, not wildcard org permission. Same-org messages require grantId null; cross-org messages require exactly the named active grant and explicit recipientOwner. Invitation and acceptance each pin BOTH actual bundle fingerprints. Limits: per grant shared fixed UTC-minute window, 1..120 new messages/minute total across both directions; bodyCapBytes 256..65536 serialized plaintext bytes. Each sending credential and each issuer/subject account within its organization has a 120-new-message/minute cap; the account budget is shared by all its credentials. Each organization has a 600-new-message/minute aggregate cap. Each recipient agent can hold at most 256 unexpired ciphertext rows, including currently blocked routes; delivery or TTL frees capacity. Fixed-window boundary bursts may be up to twice the quota. Mutations serialize/conflict on counters and recipient queues. Exact idempotent retries do not consume rate quota or queue capacity.

## SSH registry metadata

`RegistryDevice = {...Device, name, sshAddress: string|null, lastSeenAt: ms|null,
metadataSignature: hex64|null}`. If the devices table exposes `revokedAt: number|null`,
the view preserves it. This change introduces no revocation fields. Legacy rows without
metadata use their device ID as name, null address, null last-seen and null metadata signature.
The optional schema fields allow existing v2 rows to remain readable without a data reset.
New registrations reserve the device ID as a default name and initialize `lastSeenAt`.
Names use `[A-Za-z0-9_.-]{1,160}`, are case-sensitive and unique per owner, including
revoked/historical rows. Indexed mutation reads make concurrent name claims conflict.

The metadata signature uses the registered relay Ed25519 key over UTF-8 compact JSON:

```
["everett-registry-v2", owner, deviceId, name, sshAddress]
```

The existing `Device.signature`, fingerprints and relay/envelope bytes do not change.
`lastSeenAt` is set by the server at registration/publication and is not signed or a heartbeat.
Rotation preserves metadata names but clears the address and metadata signature until
publication with the new key. `registry:publish` only patches metadata on its own device;
it does not change credentials or bundles. `sshAddress` is a declared `user@host[:port]`,
at most 512 ASCII characters, with bracketed IPv6 and a port from 1 through 65535.

Rust directory/name queries use Convex's authenticated HTTP API at `/api/query` with
`{path, args, format:"json"}` and a bearer ID token. Responses use `{status:"success",value}`
or `{status:"error",errorMessage}`. Redirects are disabled. Remote origins require HTTPS;
HTTP is accepted only for literal loopback `127.0.0.1` or `[::1]` for local development/tests.
The existing relay SDK transport still requires HTTPS. Every connection verifies
`relay:whoami` against local claims before directory queries.

This directory is address discovery only. Clients verify metadata signatures before
using addresses and still require independent SSH host and device fingerprints for
first pairing. No row creates, replaces or refreshes an SSH pin. No sessions, cards,
messages or core are published to these registry functions.

## Replay, hops, delivery and local safety

- Message identity is `(sender owner, messageId)`; idempotency identity is `(sender owner, sender device, idempotencyKey)`. Store the original full envelope durably client-side and retry those exact bytes, not re-encryption. Different signed envelope digest under the same key is rejected. Both attempt identity indexes are permanent replay tombstones even after delivery/expiry; there is no seven-day replay reset.
- Roots require `parent=null, hop=1`. Forward/reply uses `{owner: previous.senderOwner, messageId: previous.messageId}` and a strictly greater hop, including any intervening local inbox forwards. Backend requires the parent to have been observed delivered/claimed/replied by the CURRENT credential's exact recipient device/org. This checks the chain across orgs/devices, not just a caller-supplied hop number. Max hop 3. Preserve these fields in local inbox entries and all hooks/MCP reply/forward paths; never reset a remote-derived automatic message to a new root. A malicious endpoint can manually start a genuinely new root: encrypted content cannot prove semantic lineage.
- `queued` means backend enqueue commit observed. Merely querying/decrypting does NOT mean delivered. Send `accept`/`observe(delivered)` only after authenticated ciphertext is durably staged locally; a successful observation authorizes durable inbox publication and deletes server ciphertext. Retain staging until inbox fsync completes, so crashes can finish publication. Claimed/replied require a later explicit signed observation; they are never inferred from WebSocket connection or process wake. Reply observations are durably retried after enqueue acknowledgement. States are monotonic queued -> delivered -> claimed -> replied (replied may directly follow delivered); stale lower-state retries do not regress. Observations require timestamp skew <= 300000 ms.
- Message TTL applies to **undelivered** rows. After local delivery, later claimed/replied observations are allowed after envelope expiry (as long as credential, membership, grant and keys remain valid). Undelivered expired rows are never returned and are deleted by the periodic bounded cron. Grant expiry/revocation and key changes also block old queued envelopes; ciphertext then survives only until its normal TTL cleanup.
- `pending` must be paged until `isDone=true`, EVEN IF `envelopes` is empty: a page of 32 queued rows may all have revoked credentials, changed pins, or inactive grants. Pass `continueCursor` as the next cursor and subscribe to the final page too, so later arrivals invalidate it. `pending` uses server `Date.now()`. Clock reads alone do NOT trigger a Convex subscription rerun; the deletion cron supplies database invalidation. Offline/reconnected clients must re-query, validate expiry before writing the local inbox, and evict locally cached envelopes on TTL/auth/grant failure. Do not claim real-time expiry solely from a cached subscription snapshot; cleanup and persisted grant expiration are usually within one minute plus scheduler delay. Use serverNow to account for clock skew.
- Derive external provenance from signed envelope owner != recipientOwner, not only from the convenience trust label. On cross-org ingress persist `external-untrusted` in local inbox, display an explicit external/untrusted prefix in CLI, formatted hooks and MCP, and treat its body only as untrusted agent data (never owner/system instructions). Do not strip provenance when forwarding. Internal transport receipts are not arbitrary owner text.
- A listener may receive live Convex WebSocket events behind NAT; it must only write/observe inbox state. It must NOT resume/wake harness sessions. SSH peer transport remains an independent per-peer decision; the registry stores declared addresses, never SSH trust pins or private keys.

## Deployment and operator pairing steps (backend commands are exact)

Prerequisite: use an OIDC issuer that supports issuer-controlled per-agent custom claims and refresh. This backend does NOT implement that issuer or enrollment UI. Provisioning from unverified user claims is unsafe. Deployment admin keys and JWTs stay in secret stores; examples contain no real secrets.

Hosted Convex:

```sh
npm ci
npx convex dev --once                  # select/create the project; first push may fail before env is set
npx convex env set OIDC_ISSUER https://issuer.example/
npx convex env set OIDC_CLIENT_ID everett-relay
npx convex dev --once                  # publish dev functions with the configured issuer
npx convex env set --prod OIDC_ISSUER https://issuer.example/
npx convex env set --prod OIDC_CLIENT_ID everett-relay
npx convex deploy                     # publish production functions
```

Set auth env BEFORE the successful push of auth.config.ts. On a new project, the first `convex dev --once` may exit nonzero after project/deployment selection because the issuer env is absent; once the CLI has written the local project selection, set both env vars and run it again. Do not use an empty issuer/audience in production. This initialization flow has not been exercised against a hosted deployment in this backend-only task. Commands shown with `--prod` target production; development and production data/credentials must be separately provisioned. A deployment administrator can run internal provisioning via CLI; add `--prod` consistently when targeting production:

```sh
npx convex run relay:provisionIdentity '{"issuer":"https://issuer.example/","subject":"alice","org":"org-a","agentId":"machine-a","role":"admin"}'
npx convex run relay:provisionIdentity '{"issuer":"https://issuer.example/","subject":"alice","org":"org-a","agentId":"machine-b","role":"admin"}'
# Separate org example (authoritatively verified B identity):
npx convex run relay:provisionIdentity '{"issuer":"https://issuer.example/","subject":"bob","org":"org-b","agentId":"machine-c","role":"admin"}'
```

For self hosting, start the official Convex backend following https://github.com/get-convex/convex-backend/blob/main/self-hosted/README.md, expose its client/WebSocket endpoint via TLS, and configure the operator's `CONVEX_SELF_HOSTED_URL` and `CONVEX_SELF_HOSTED_ADMIN_KEY` securely. The same `npx convex env set`, `npx convex deploy`, and internal `npx convex run` commands apply to that target; never send the self-hosted admin key to Rust agents. The Rust client endpoint is the public Convex backend URL, not the dashboard or `.convex.site` HTTP-actions URL.

Required Rust CLI sequence (command spelling must be implemented/documented by the parent; not yet present in this backend-only change):

1. Each machine obtains its own issuer token/refresh credential with its own everett_agent claim; configure endpoint + OIDC client, invoke `whoami`, generate separate local keys and register matching deviceId.
2. On two machines in org A: list/pin each other's Device bundles; start the existing inbox listener on each; send a same-org envelope with recipientOwner=A and grantId=null. A observes queued; B writes inbox and signs delivered; claim/reply observations are signed only when locally observed. No inbound NAT ports or SSH transport changes required.
3. Two orgs: each admin exchanges the named Device bundle/fingerprint over a trusted channel. A CLI invite calls `grants:invite` with both expected pins, optional expiry, named IDs, rate and body cap. B CLI acceptance calls `grants:accept` with **B local and A peer** fingerprints. Both list/get the grant and verify access=active before sending.
4. Rotation CLI saves old keys until `relay:rotate` commits, sends new self-signed bundle plus old-key rotation signature, then A and B explicitly reconfirm the changed pair using their corresponding local/peer fingerprints. Sending remains blocked after the first confirmation. Lost old signing key requires fresh credential/device provisioning and a new grant; never override keys with the JWT alone.
5. Either admin CLI revokes a grant via `grants:revoke`. Admin/self agent CLI revokes via `relay:revokeAgent`; member removal uses `relay:revokeMember`. Stop denied subscriptions, clear cached ingress, and refuse enrollment with old still-valid tokens.

## Short threat model and outstanding integration

The backend is a ciphertext-only relay, not a trusted instruction source. It learns org/device routing, timestamps, sizes, grant/key metadata and delivery state; ciphertext confidentiality relies on clients, key pinning and honest local key storage. Deployment operators control membership and issuer trust; a compromised deployment or issuer can deny service/change authorization, and first enrollment remains bearer-credential-based. Pinned existing keys cannot be rotated without the old signing key; changed keys require bilateral administrator confirmation. Compromised endpoints can disclose plaintext or lie about local observations. No system can recall prior downloaded data or infer semantic thread lineage from encrypted bodies.

There is no old-v1 data migration: existing v1 devices/messages/attempts do not satisfy v2 schema. Use a clean deployment for the v2 rollout or build an operator migration that preserves revoked/replay tombstones; do not delete active production data or revocation history blindly. Backend tests execute actual functions offline; Rust tests cover the SDK fake transport, crypto, durable inbox and MCP paths. These are not issuer/JWKS, hosted deployment, real WebSocket invalidation or physical two-machine checks. The owner must perform the live runbook before rollout.

Official API references read before implementation: https://docs.convex.dev/auth/advanced/custom-jwt, https://docs.convex.dev/auth/functions-auth, https://docs.convex.dev/functions/internal-functions, https://docs.convex.dev/self-hosting.
