# Relay v2: short threat model

Scope: the Rust client and Cloudflare Worker/Durable Object backend, not harness/model
providers or SSH. See the [wire contract](../cloudflare/PROTOCOL.md) and
[Cloudflare adapter](../cloudflare/README.md).

## Trust and authorization

The native credential binds one immutable agent to a server-derived owner namespace.
Every hosted operation validates the credential and active membership. Same-owner
discovery uses that authenticated namespace and an exact-device lookup, pins verified
bundles on first use, and rejects changed keys. Cross-owner grants are not exposed by
the Cloudflare native adapter. Authenticated authors/session cards are opt-in advisory
metadata, not permission to run a session or download a transcript.

Messages and replies use independent Ed25519 signing and X25519 encryption keys,
HKDF-SHA256 and ChaCha20Poly1305. Signed routing binds owners, device IDs, fingerprints,
parent/hop lineage, expiry and optional grant ID. Verify signatures, routing, key pins
and hop limits before local inbox publication. Delivery observations are signed metadata.

The listener polls outbound, rechecks identity before processing pages and discards
cached ciphertext after authorization denial. Transient failures retain only encrypted
staging so retries recover uncertain committed sends without duplicate plaintext.
Revocation cannot recall data already decrypted on a device.

## Observable metadata and retention

E2EE hides message/reply bodies, not account/agent identity, public bundles, fingerprints,
message/idempotency IDs, recipient and sender, parent/hop lineage, timestamps, sizes,
signatures or delivery state. Operators can see network timing and IP addresses.
Opted-in authors cards also expose their published session metadata to the account.

Signed delivery deletes live ciphertext; undelivered rows have a one-hour client TTL
and backend cleanup. Replay/idempotency and revocation tombstones remain as metadata.
Deletion from live storage is not certified erasure of backups, logs or storage history.
Cloudflare stores metadata in Durable Objects; local plaintext inboxes remain on devices.

## Local safety and limits

External content stays untrusted across inbox pickup, replies, hooks, MCP and local
forwarding. Default ping dispatch is not task completion or proof of inbox consumption.
Receiver wake and webhook delivery require explicit local owner policy.
Session transcripts and shared-core data are not synchronized.

Protect the OS account and private keys; they are available to unattended processes.
There is no forward secrecy against later recipient static-key compromise, protection
from a compromised endpoint, anonymity, or guarantee against a malicious service
withholding messages/revocations. Key changes fail closed; never silently re-pin.
Cloudflare revocation does not prune SSH keys: use `peer revoke` independently.
Offline/unit tests do not prove live provider, physical-device or production acceptance.
