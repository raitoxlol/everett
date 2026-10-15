# Relay v2: short threat model

Scope: the Rust client and deployable TypeScript Convex backend, not SSH, harness/model
sandboxing, or an issuer enrollment service. See [auth](auth.md), [operator journeys](cross-device.md)
and the exact [wire/function contract](../convex/CONTRACT.md). No live deployment,
OIDC/JWKS/WebSocket revocation test, or security audit is claimed by this document.

## Boundaries and controls

- Trust the installed client, both endpoint OS accounts, the issuer's claim/enrollment
  policy, and deployment operators for authorization. Treat Convex/network operators as
  able to observe metadata, retain traffic and deny service, but not as holders of
  endpoint decryption/signing keys. E2EE does not make a malicious control plane's
  authorization decisions trustworthy; independently authenticate initial/changed pins.
- Every public backend function requires Convex-verified OIDC JWT **plus** active stored
  membership **plus** an immutable per-agent credential binding. A signed admin claim
  also needs a stored admin role. There is no public self-enrollment or owner-wide bearer
  credential capable of acting as every agent. Relay admins can manage their own org's
  agents/grants, not send/register/read as them. Issuer/deployment administrators remain
  privileged: their compromise can subvert enrollment/authorization.
- Same namespace means issuer+org, or issuer+subject in optional account fallback.
  Cross-namespace chat needs a symmetric grant naming exactly two agents: A admin invites,
  B admin accepts, both pin **both complete bundles/fingerprints**, optional expiry,
  either-side terminal revocation, serialized-body cap and shared fixed-minute rate.
  A changed bundle blocks old/new traffic until independent bilateral reconfirmation;
  local peer pins must also be explicitly changed. No wildcard org trust or federation
  across independent deployments is implemented.
- Fresh ephemeral X25519 sender keys agree with the recipient's static X25519 key;
  HKDF-SHA256 derives the ChaCha20Poly1305 key. Ed25519 signatures and AEAD AAD bind
  owners, agents, grant, both fingerprints, message/idempotency IDs, parent/hop, creation/
  expiry times, ephemeral public key and nonce to ciphertext. Clients verify pins,
  signatures, AEAD, routing and body fields; backend validates signatures/metadata/size
  but cannot validate encrypted semantics or AEAD tags. Ed25519 and X25519 private keys
  are independent, stored locally, not shared with SSH or the relay.
- Backend revocation reads are query dependencies, invalidating already subscribed
  queries. Client ingress is durably staged as encrypted private data, then a signed
  mutation reauthorizes both endpoints, keys, membership, grant and undelivered TTL
  **before** publication to hooks/MCP/local inbox. Denied staging is discarded; uncertain
  network commits can recover after another successful authorization. No stale query
  cache is authority. Authorization is ordered at the server transaction commit; it is
  not retroactive erasure of an earlier authorized delivery.
- Cross-org ingress derives `external-untrusted` from signed owner mismatch and persists
  it through replies/local forwarding. CLI/JSON, formatted hooks and MCP label/quote
  external text as **untrusted agent data, never owner/system instructions**. External
  taint and parent/hop context survive local pickup; provenance-less SSH forwarding is
  refused. These labels are not an LLM prompt-injection proof or permission to execute
  content. The owner must still control agent tools and human approval policies.
- No remote chat starts/resumes/spawns a harness. `queued`, `delivered`, `claimed` and
  `replied` reflect acknowledged enqueue and explicit signed observations, not presence
  or task completion. `delivered` is durable private device ingress/authorization, not
  pickup; `claimed` observes `.done`; `replied` observes accepted reply enqueue, not
  the original sender reading it. See the [state table](cross-device.md#observed-states-relay-v2-not-ssh-pickup-semantics).

## Observable metadata and retention

“Ciphertext only” describes **content**, not anonymity or zero metadata. The backend
can observe all function names/arguments/responses, authenticated connections and
these fields (source: `convex/schema.ts`, `relay.ts`, `grants.ts`):

| Surface | Visible data and application retention |
| --- | --- |
| Authentication/network | JWT and all its claims, including issuer, subject, audience, agent/org/role, expiry and any provider-added profile/email/session claims; Convex identity/token identifier; connection/IP, timing, frequency, TLS endpoints and sizes. Functions do not store tokens in relay tables; provider/platform/proxy logging and retention are separate operator policies. |
| Memberships/credentials | Namespace/owner, issuer, subject, role and active/revoked status; issuer-global agent ID, immutable owner/subject binding and credential status. Rows, including **revocation tombstones, are permanent**. |
| Devices | Owner, device ID, current signing/encryption public keys, self-signature, monotonic rotation generation and nullable revocation timestamp. Registry rows retained on revoke; rotation replaces the current bundle, while grants can retain pinned historical bundles. Active and revoked own-namespace listings are each capped at 256. |
| Grants | Grant ID, both owners/named device IDs, both complete pinned public bundles/self-signatures/fingerprints and rotation generations, state, which side confirmed (`a`/`b`/`both`), optional expiry and cron sweep time, rate/minute and body-byte cap. Views add computed access status (e.g. key changed/agent revoked); stored grant records, including revoked/expired grants, are **not pruned**. List is capped at 128 per side and reports truncation. |
| Undelivered envelopes | Sender/recipient owner, sender/recipient device ID, nullable grant ID, message ID, idempotency key, nullable parent owner/message ID, hop, creation/expiry Unix milliseconds, both fingerprints, ephemeral X25519 public key, nonce, ciphertext/tag (and its exact length), Ed25519 signature. These reveal routing, timing and conversation graph, not session IDs/text. |
| Replay/idempotency attempts | Sender/recipient owners and devices, grant ID, both fingerprints, message ID, idempotency key, digest of signed envelope bytes, parent, hop, expiry and current observed state. **Permanent**, no ciphertext, keys, nonce, signature, explicit `createdAt` field or plaintext in these attempt rows (Convex's implicit `_creationTime` remains). Replays remain blocked after TTL/delivery; no seven-day reset. |
| Rates | Scope (agent owner/ID, account owner/issuer/subject, organization owner or grant ID), fixed UTC-minute window and count; the existing scope row is overwritten each window, not pruned. Agent and account quotas are 120/minute, organization quota is 600/minute, and grant quota is shared across directions. Each recipient queue is capped at 256 unexpired ciphertext rows. Exact retries consume neither quotas nor queue capacity. |
| Observations/query output | Recipient owner/device, sender owner/message ID, observed state, observation timestamp and Ed25519 signature are mutation arguments; only resulting state is retained in attempt rows. Pending outputs include server time, trust label, pagination/cursor and envelopes; status outputs state. Convex documents additionally have `_id` and `_creationTime`, though public views strip them. |

Session/harness IDs, text, sender card snippets, reply correlation and forwarded
external taint are inside the encrypted serialized body, not backend plaintext. Visible
parent/hop and shared grant IDs still correlate replies/forwards. Do not use secrets or
sensitive free text as org/agent/grant/alias/idempotency identifiers.

Successful signed delivery immediately **deletes the live `messages` ciphertext row**;
metadata attempts remain. Undelivered Rust envelopes have one-hour TTL (backend maximum
24 hours). Fresh reads reject expiry; minute cron deletes in bounded batches and persists
grant expiry, with scheduling delay. Revoked/changed-key/inactive-grant queued rows may
remain physically present until normal TTL cleanup, but cannot newly deliver. Expiry
of undelivered mail does not erase a delivered local inbox; local retention is separate.

Deletion is **not secure erasure**: operators may have captured traffic, backups,
exports, logs and Convex storage history. Official self-hosted compose currently sets
`DOCUMENT_RETENTION_DELAY` to 172800 seconds by default (two days); that is a storage
history policy, not Everett's TTL or a promise about cloud/backups. Inventory and
minimize your own platform/proxy/backups. Permanent metadata grows with use; deleting
replay/revocation records or restoring a pre-revocation backup can reopen identities
or duplicate keys. Plan storage/quota monitoring and tombstone-preserving migrations.

## Limits and compromise response

- Registry revocations can only remove tagged SSH authorizations during doctor. They never
  install keys, modify addresses, or replace pins. A compromised registry can hide a
  revocation and delay SSH enforcement indefinitely; this is an accepted limitation.
  Offline peers retain SSH access until a successful doctor fetch. Propagation requires
  the revoked registry device ID to equal the SSH UUID in the authorization tag. Existing
  SSH identities are preserved, with no automatic binding or migration. Doctor shows a
  local mismatch. The 256-row revocation bound can omit further tombstones; doctor warns
  at the bound. Use receiver-side `peer revoke` when propagation is insufficient.
- **No forward secrecy for past recipient static-key compromise.** Anyone with captured
  ciphertext and a later stolen recipient X25519 private key can decrypt those past
  envelopes. Fresh ephemeral sender keys/rotation are not a ratchet. Delete old keys
  responsibly; rotate/re-pin/reconfirm or revoke/provision fresh credentials after loss.
- A stolen OS account/device can read local plaintext/private keys/refresh tokens, act as
  that agent, and read any already downloaded data. If its credential is an org admin,
  the attacker also gets that admin's management powers. A JWT stolen **before first
  registration** can race registration with an attacker key; provisioning is not a
  hardware-bound enrollment attestation. Authenticate initial fingerprints independently
  and protect browser approval, tokens, local files and issuer recovery/MFA. Revocation
  cannot erase copies or undo actions already authorized.
- Backend `revoke-agent`/`revoke-member` denies a still-valid JWT, including queries and
  re-registration. Also revoke issuer refresh/offline sessions, remove local trust and
  remediate the device. Provider logout alone does not immediately deny an existing ID
  token. Agent tombstones require new agent IDs; membership tombstones require new
  subjects. A lost old signing key cannot authorize ordinary rotation via JWT alone.
- The relay can drop/delay/reorder traffic, report misleading availability/state if its
  implementation is malicious, capture metadata, or exhaust storage/quota. E2EE has
  no anonymity/padding and cannot guarantee uptime, delivery or honest server status.
  TTL/rate/body caps are bounded-abuse policies (fixed-window boundary bursts can double
  a quota), not DoS protection against all principals or a malicious host.
- Durable signed parent lineage caps compliant relay paths at three hops across devices/
  orgs and checks the exact observed recipient parent. Malicious endpoints can create a
  new root or rewrite semantic content; the backend cannot prove ancestry of encrypted
  prose. Hop guards and external labels do not replace tool-level authorization.

Release gate still needs a real issuer refresh/JWKS and cloud/self-hosted TLS deployment,
live empty-page/final-cursor subscription/reconnect, already-open revocation/key-change
races, crash/recovery, and two physical-machine/two-org trials. Offline `convex-test`
identities and Rust fixtures exercise actual function/crypto paths but cannot establish
these operational properties. Keep unverified deployment/test claims explicit.
