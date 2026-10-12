# Relay threat model

The relay protects message content from Convex, network observers, other accounts,
and unapproved organizations. It does not protect a message from its authorized
recipient, a compromised endpoint, or a malicious organization administrator.

## Identity and authority

Convex validates the OIDC token signature, issuer, audience, and expiry. Every public
function also requires the configured `OIDC_ISSUER`. The server derives the account
from `[issuer, subject]`. The organization is the SHA256 of the issuer and either
`["org", org_id]` or `["personal", subject]`. Personal and organization namespaces
cannot collide. Human identities without `org_id` remain private to their subject.

Agent identities require issuer-signed `org_id`, `agent_id`, and `jti`. The agent
ID is SHA256 of `[issuer, subject, agent_id]`. Its credential handle is SHA256 of
`[issuer, subject, jti]`. A handle is not a bearer secret. Sender fields in envelopes
are checked against this identity; target IDs only select a recipient. All rows
carry server-stamped account and organization scope. Recipient scope comes from
the registered agent, never a claimed destination organization.

The OIDC provider is a security boundary: these claims must not be self-editable
profile fields. Only a human identity with issuer-signed `org_role: "admin"` can
approve grants or revoke memberships/agents. An agent token is never an admin,
even if it contains that role. Membership records are created from verified
claims on registration. Existing revocation records override still-valid tokens.
The issuer remains the source of membership and administrator assignment; Everett
does not add a second client-managed membership directory.

Agent credentials are registered with one fixed signed device-key bundle. Neither
re-registration nor token renewal replaces that bundle. Revoking a credential
blocks that `jti` immediately, including queued delivery and acceptance. It does
not prevent the issuer from issuing a new credential for the same agent. Revoking
the agent blocks all its current and future credentials. Membership revocation
blocks all that account's agents in that org. These revocations are permanent in
this protocol; restore access with a new issuer-controlled identity. Owner-only
device functions reject agent tokens, so they cannot bypass grants through v1.

## Named-agent cross-org grants

Cross-org communication is denied unless both org admins approve the exact pair
of registered agent IDs. Proposing is not approval. Neither admin can approve for
the other organization. Either can revoke, and a revoked grant cannot be revived.
A new grant does not authorize messages queued under the old grant.

Same-org agents can exchange messages without cross-org grants, subject to valid
membership, registered credentials, and device pins. Agent grant listings expose
only pairs containing that caller; human identities see their org's pairs. Remote
keys are returned only for active bilateral grants. The same-org directory omits
revoked agents and memberships. It contains public routing identities and keys,
not session lists or cards.

Inbound cross-org messages carry `external: true` and `trust: "external-untrusted"`.
The Rust importer verifies that label against pinned org scope and renders a
warning in text inbox output. JSON inbox clients must preserve and honor these
fields. Authentication proves origin, not the safety of the text. External content
is data, not instructions, privilege, or permission to execute tools. Everett does
not provide a prompt-injection sandbox for a harness that ignores this distinction.

## End-to-end content protection

The sender encrypts on-device with ephemeral-static X25519, HKDF-SHA256, and
ChaCha20-Poly1305. Ed25519 signs the envelope. Agent v2 binds both account IDs,
org IDs, agent IDs, devices, message/idempotency IDs, timestamps, nonce, and
ephemeral key into AEAD additional data and the signature. Owner v1 remains
wire-compatible and is rejected by the agent importer.

Agent keys are private, separate for each account/org/agent/deployment. Both
directions require out-of-band confirmation of a fingerprint covering org, named
agent, and signed device keys. A server directory entry alone is not trusted.
Changed keys require explicit unpairing and reconfirmation. The daemon binds one
named agent to one exact local session and harness. It never routes to another
session or wakes a harness automatically. Jev/local routing and SSH remain separate.

Static recipient-key compromise can decrypt recorded ciphertext: this protocol
does not provide forward secrecy against future compromise of that key. A
compromised sender can sign malicious content; a compromised recipient can leak
plaintext. Local OS accounts, key files, pins, inboxes, and the issuer are trusted.
A malicious Convex operator can drop, replay, delay, correlate, or falsify server
queue state, but cannot forge a pinned sender's signature or a valid AEAD payload.
Server `accepted` is not proof of pickup. `delivered` and `replied` require verified
encrypted receipt/reply observations on the original sender.

## What Convex still sees

- Verified issuer/subject account IDs and organization membership associations.
- Hashed org, named-agent, and credential handles; administrator approvals and
  revocations; device IDs and signed public-key bundles.
- Who sends to whom, envelope IDs, idempotency keys, nonce/ephemeral keys,
  signatures, ciphertext lengths, timestamps, and requested expiry.
- Queue volume, connection/subscription timing, acceptance/expiry metadata,
  content-free ciphertext digests, and rate counters. Network infrastructure may
  also observe IP addresses and OIDC authentication traffic.

Convex does not receive plaintext message text, cards, session IDs, harness names,
hop counts, reply text/correlation payloads, or pickup receipts in decryptable form.
Ciphertext length and traffic patterns can still reveal likely activity. There is
no padding or anonymity guarantee. OIDC tokens reach Convex for authentication;
application rows never store bearer/refresh tokens or private keys.

## Retention and abuse limits

Ciphertext is deleted after a signed acknowledgement of durable local import.
Unread ciphertext is filtered at expiry and removed by a one-minute cron with
bounded continuation batches. Everett creates one-hour envelopes; the backend
allows at most 24 hours. Cleanup is eventual during outages, not an exact erasure
deadline. Seven-day post-expiry idempotency tombstones contain no ciphertext.
Memberships, public keys, credential revocation records, and grants persist so
revocation survives token renewal. Convex backups/operator logs may retain deleted
metadata or ciphertext beyond application TTL; backups are not cryptographic erasure.

New sends are limited to 60 per account (owner v1) or named agent (v2) per minute,
plus 600 per org per minute per transport. Retries reuse the original ciphertext
and do not consume new-send capacity. Both queues cap at 256 envelopes per
recipient. Agent registration caps at 512 identities per org and 16 new identities
per account per minute. Credential registration caps at 16 per account per minute;
revoked handles must remain recorded. Public functions reject bodies above
64 KiB serialized plaintext plus the AEAD tag. Read abuse and valid-token floods
still need deployment-level Convex/IdP quotas and monitoring.

Local retry records contain ciphertext and metadata, with private file permissions.
Imported plaintext inboxes use Everett's existing local retention, not server TTL.
Revocation cannot recall plaintext already imported. The daemon reauthorizes cached
subscription rows before import, but an authorization can change between that
query and a local write; distributed revocation is not retroactive erasure. Signed
acceptance also rechecks current grant/membership/credential authorization.

## Verification boundary

Fake-Convex tests exercise authentication, scope spoofing, bilateral grants,
revocation, external labels, signature/key replacement, idempotency, queues,
rates, and deletion/TTL. Rust tests exercise scoped cryptography and local inbox
behavior; the release journey calls the four new MCP tools without credentials
and verifies fail-closed responses. These tests do not validate production JWKS,
audience, claim provisioning, two real NATed machines, or network reconnection.
Run the owner deployment and agent trial before relying on a live relay.

No production deployment, schema migration, token provisioning, or security-policy
change is performed by this PR. The new required `org` fields need an explicit
owner-reviewed migration if a deployment already contains v1 rows. Do not disable
schema validation or guess orgs from client-provided data to make a deploy pass.
