import { createPrivateKey, createPublicKey, diffieHellman, hkdfSync, createCipheriv, createDecipheriv } from "node:crypto";
import nacl from "tweetnacl";
import { deviceBytes, aadBytes, envelopeBytes, fingerprint, hexString, type Device, type Envelope } from "../protocol";

// Deterministic keys for OFFLINE TESTS ONLY. Production must generate independent random keys/nonces.
export function xKey(seed: number) {
  const secret = createPrivateKey({
    key: Buffer.concat([Buffer.from("302e020100300506032b656e04220420", "hex"), Buffer.alloc(32, seed)]),
    format: "der", type: "pkcs8",
  });
  const publicKey = createPublicKey(secret).export({ format: "der", type: "spki" }).subarray(-32);
  return { secret, publicKey };
}
export function key(deviceId: string, owner: string, seed: number) {
  const pair = nacl.sign.keyPair.fromSeed(new Uint8Array(32).fill(seed));
  const encryption = xKey(seed + 1);
  const device: Device = { owner, deviceId, signingKey: hexString(pair.publicKey), encryptionKey: hexString(encryption.publicKey), signature: "" };
  device.signature = hexString(nacl.sign.detached(deviceBytes(device), pair.secretKey));
  return { device, pair, encryption };
}
export type Keys = ReturnType<typeof key>;
function publicX(hex: string) {
  return createPublicKey({ key: Buffer.concat([Buffer.from("302a300506032b656e032100", "hex"), Buffer.from(hex, "hex")]), format: "der", type: "spki" });
}
export function resign(e: Envelope, sender: Keys) {
  e.signature = hexString(nacl.sign.detached(envelopeBytes(e), sender.pair.secretKey));
}
export function seal(sender: Keys, recipient: Keys, overrides: Partial<Envelope> = {}, body = JSON.stringify({ text: "hello", kind: "message" })) {
  const ephemeral = xKey(7);
  const e: Envelope = {
    owner: sender.device.owner, recipientOwner: recipient.device.owner, grantId: null,
    fromDevice: sender.device.deviceId, toDevice: recipient.device.deviceId,
    messageId: "message-1", idempotencyKey: "request-1", parent: null, hop: 1,
    createdAt: Date.now(), expiresAt: Date.now() + 3600000,
    senderFingerprint: fingerprint(sender.device), recipientFingerprint: fingerprint(recipient.device),
    ephemeralKey: hexString(ephemeral.publicKey), nonce: "22".repeat(12), ciphertext: "", signature: "", ...overrides,
  };
  const shared = diffieHellman({ privateKey: ephemeral.secret, publicKey: publicX(recipient.device.encryptionKey) });
  const cipher = createCipheriv("chacha20-poly1305", Buffer.from(hkdfSync("sha256", shared, Buffer.from("everett-relay-v2"), aadBytes(e), 32)), Buffer.from(e.nonce, "hex"), { authTagLength: 16 });
  cipher.setAAD(aadBytes(e), { plaintextLength: Buffer.byteLength(body) });
  e.ciphertext = hexString(Buffer.concat([cipher.update(body, "utf8"), cipher.final(), cipher.getAuthTag()]));
  resign(e, sender);
  return e;
}
export function open(e: Envelope, recipient: Keys) {
  const shared = diffieHellman({ privateKey: recipient.encryption.secret, publicKey: publicX(e.ephemeralKey) });
  const bytes = Buffer.from(e.ciphertext, "hex");
  const cipher = createDecipheriv("chacha20-poly1305", Buffer.from(hkdfSync("sha256", shared, Buffer.from("everett-relay-v2"), aadBytes(e), 32)), Buffer.from(e.nonce, "hex"), { authTagLength: 16 });
  cipher.setAAD(aadBytes(e), { plaintextLength: bytes.length - 16 });
  cipher.setAuthTag(bytes.subarray(-16));
  return Buffer.concat([cipher.update(bytes.subarray(0, -16)), cipher.final()]).toString("utf8");
}
