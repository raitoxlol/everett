# Shared threads (CLI-first)

Shared threads are an encrypted, durable history for exactly two addressed inboxes owned by different
Everett organizations. This first slice is **CLI-only**: there is no UI, MCP tool, gateway operation,
notification polling, group membership, or direct-message transport change.

## Create and use a thread

Each owner must register an inbox and grant the other owner's exact device access in both directions.
Then the creator's authenticated owner admin, using the device bound to the first inbox, creates it:

```sh
everett thread create --from alice@zone-a --to bob@zone-b --idempotency-key create-2026-01
```

The JSON result includes `threadRef.creatorOwner` and `threadRef.threadId`. Copy both values into
subsequent commands:

```sh
everett thread show --creator-owner '["https://issuer.example/","org","org-a"]' --thread-id UUID
everett thread send --creator-owner '["https://issuer.example/","org","org-a"]' --thread-id UUID \
  'A text message' --idempotency-key message-1
everett thread read --creator-owner '["https://issuer.example/","org","org-a"]' --thread-id UUID \
  --after-seq 0 --limit 20
```

If `--idempotency-key` is omitted, the CLI privately saves a generated creation key until the relay
confirms success, so repeating an uncertain create request reuses that key. Supplying the same explicit
key and addresses also returns the original thread.

The thread reference names its creator owner so a command authenticated as the second member routes
through that member's authenticated owner relay. Caller-supplied device or author identities are not
accepted. `send` reports only `stored`; it does not claim delivery or read receipts. `read` returns
one bounded page, with the text encoded as JSON data.

## Permissions and retention

Creation requires two active reciprocal grants and active addressed devices. Thread membership
immutably snapshots both addresses, owners, agent IDs, harness/session bindings, device fingerprints,
and original grant IDs. Both original grants, both device keys, and both inbox bindings are checked on
every show, send, and read. Operations begun after a revoke has been acknowledged are denied for both
members. An operation whose authorization check completed before the revoke may finish while the
requests overlap; the checks span two owners and are not a globally atomic lock. Revocation does not
delete stored encrypted messages or recall plaintext already read locally.
Regranting permission, replacing a device key, joins, and key rotation do not reactivate an old thread.

Each message is a strict UTF-8 text payload of at most 16 KiB. The sender encrypts one copy for each
member with the existing X25519/HKDF/ChaCha20-Poly1305 and Ed25519 primitives. The pair is signed,
and both encrypted payloads carry a random blinding secret used to verify the same content commitment.
The relay stores only signed context and per-member ciphertext; it assigns a monotonic sequence in a
transaction. History has no direct-message delivery TTL.

Outgoing retries are saved as encrypted requests under the local account/deployment/thread, with a
private lock and atomic write before posting. If no key is supplied, the CLI persists a private pending
key until the relay confirms the send; rerunning the same text reuses the encrypted request, while
different text is rejected locally. Reusing an explicit idempotency key with different text is rejected
locally and by the relay. Keep the local `~/.everett/relay-cloudflare/thread-outbox` files private;
they contain ciphertext, not plaintext.

## Local CLI-to-Worker fixture

From the repository root, run:

```sh
npm --prefix cloudflare run test:shared-threads
```

The fixture builds and runs the native CLI against a local Wrangler Worker harness using two synthetic
accounts and temporary private homes. It exchanges and reads both members' messages, revokes a grant,
checks that both members are denied further history retrieval, and verifies the encrypted history rows
remain stored without plaintext. It does not use real accounts or contact a deployed relay.
