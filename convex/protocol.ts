import { ConvexError, type Infer } from "convex/values";
import nacl from "tweetnacl";
import { sha256 } from "@noble/hashes/sha256";
import { deviceValidator, envelopeValidator } from "./schema";

export type Device = Infer<typeof deviceValidator>;
export type Envelope = Infer<typeof envelopeValidator>;
export const encode = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));
export const hexString = (value: Uint8Array) => Array.from(value, (b) => b.toString(16).padStart(2, "0")).join("");
export const digest = (value: Uint8Array) => hexString(sha256(value));

export function deviceBytes(d: Device) {
  return encode(["everett-device-v2", d.owner, d.deviceId, d.signingKey, d.encryptionKey]);
}
export function fingerprint(d: Device) {
  return digest(deviceBytes(d));
}
export function aadBytes(e: Envelope) {
  return encode([
    "everett-relay-v2", e.owner, e.recipientOwner, e.grantId, e.fromDevice, e.toDevice,
    e.messageId, e.idempotencyKey, e.parent === null ? null : [e.parent.owner, e.parent.messageId],
    e.hop, e.createdAt, e.expiresAt, e.senderFingerprint, e.recipientFingerprint, e.ephemeralKey, e.nonce,
  ]);
}
export function envelopeBytes(e: Envelope) {
  return encode([new TextDecoder().decode(aadBytes(e)), e.ciphertext]);
}
export function rotationBytes(d: Device, oldFingerprint: string) {
  return encode(["everett-rotate-v2", d.owner, d.deviceId, oldFingerprint, fingerprint(d)]);
}
export function observationBytes(a: {
  owner: string; deviceId: string; senderOwner: string; messageId: string;
  state: "delivered" | "claimed" | "replied"; at: number;
}) {
  return encode(["everett-observe-v2", a.owner, a.deviceId, a.senderOwner, a.messageId, a.state, a.at]);
}
export function sshBindingBytes(owner: string, deviceId: string, sshDeviceId: string, sshPublicKey: string) {
  return encode(["everett-ssh-bind-v1", owner, deviceId, sshDeviceId, sshPublicKey]);
}
const SSH_ED25519_PREFIX = [0, 0, 0, 11, ...new TextEncoder().encode("ssh-ed25519"), 0, 0, 0, 32];
/** Raw Ed25519 key from the base64 field of an `ssh-ed25519` public key line. */
export function sshEd25519Key(blob: string) {
  const bytes = /^[A-Za-z0-9+/]{68}$/.test(blob) ? Uint8Array.from(atob(blob), (c) => c.charCodeAt(0)) : new Uint8Array();
  if (bytes.length !== 51 || SSH_ED25519_PREFIX.some((b, i) => bytes[i] !== b)) throw new Error("Invalid SSH public key");
  return bytes.slice(SSH_ED25519_PREFIX.length);
}
export function hex(value: string, bytes?: number) {
  if (!/^(?:[0-9a-f]{2})+$/.test(value) || (bytes !== undefined && value.length !== bytes * 2)) {
    throw new Error("Invalid cryptographic field");
  }
  return Uint8Array.from(value.match(/../g)!.map((b) => parseInt(b, 16)));
}
export function verify(key: string, signature: string, bytes: Uint8Array) {
  if (!nacl.sign.detached.verify(bytes, hex(signature, 64), hex(key, 32))) {
    throw new Error("Invalid device signature");
  }
}
export function identifier(value: string) {
  if (!/^[A-Za-z0-9_.-]{1,160}$/.test(value)) throw new Error("Invalid identifier");
}
export function validateDevice(d: Device) {
  identifier(d.deviceId);
  hex(d.encryptionKey, 32);
  if (d.encryptionKey === "00".repeat(32)) throw new Error("Invalid encryption key");
  verify(d.signingKey, d.signature, deviceBytes(d));
}

// User-facing rejections carry a stable code so clients never parse message text or function names.
export type UserErrorCode = "invalid_ssh_address" | "name_taken" | "name_collision" | "device_revoked";
export function userError(code: UserErrorCode, message: string) {
  return new ConvexError({ code, message });
}
