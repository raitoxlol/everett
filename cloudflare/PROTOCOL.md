# Cloudflare relay v2 wire contract

Cloudflare is the only hosted relay. Native operations use authenticated
`POST /v2/relay/<operation>` with `{result:...}` or non-2xx `{error:code}`.
See [adapter support](README.md) for supported operations and enrollment.
The server derives owner/agent from the native credential, never client-selected identity.
The deterministic public test-only vector lives in `test/relay-wire-v2.json`
and `../everett-rs/tests/fixtures/relay-wire.json`.

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

All hex is lowercase, even-length, no `0x`. IDs use the agent-ID character/length rule. Times are Unix milliseconds, finite safe integers (absolute value <= 2^53-1); deserialize with checked integral conversion. A new enqueue permits absolute sender-clock skew <= 300000 ms, TTL > 0 and <= 86400000 ms, and expiry strictly after server now. Old identical retries can return a tombstone beyond TTL; authorization and key checks still run first.

Cloudflare accepts same-owner delivery only: `recipientOwner` must equal `owner` and `grantId` must be null. The field remains in signed bytes to preserve the v2 wire contract, not to offer cross-account grants.

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
