import { ed25519 } from '@noble/curves/ed25519';
import { sha256 } from '@noble/hashes/sha256';
import { z } from 'zod';

export const DEVICE_LABEL_LIMIT = 2048;

export const identifier = z.string().regex(/^[A-Za-z0-9_.-]{1,160}$/);
const hex = (bytes: number) => z.string().regex(new RegExp(`^[0-9a-f]{${bytes * 2}}$`));
const safeInteger = z.number().int().safe();
export const identitySchema = z.strictObject({
  issuer: z.url().max(2048), subject: z.string().min(1).max(256),
  org: z.string().min(1).max(160).nullable(),
});
export const principalSchema = identitySchema.extend({
  agentId: identifier, role: z.enum(['admin', 'member']),
  expiresAt: safeInteger.nonnegative(),
});
export type Identity = z.infer<typeof identitySchema>;
export type Principal = z.infer<typeof principalSchema>;
export const namespace = (p: Identity) => JSON.stringify([p.issuer, p.org === null ? 'account' : 'org', p.org ?? p.subject]);
export const deviceSchema = z.strictObject({
  owner: z.string().max(4096), deviceId: identifier, signingKey: hex(32),
  encryptionKey: hex(32), signature: hex(64),
});
export type Device = z.infer<typeof deviceSchema>;
export const envelopeSchema = z.strictObject({
  owner: z.string().max(4096), recipientOwner: z.string().max(4096),
  grantId: identifier.nullable(), fromDevice: identifier, toDevice: identifier,
  messageId: identifier, idempotencyKey: identifier,
  parent: z.strictObject({ owner: z.string().max(4096), messageId: identifier }).nullable(),
  hop: safeInteger.min(1).max(3), createdAt: safeInteger.nonnegative(), expiresAt: safeInteger.nonnegative(),
  senderFingerprint: hex(32), recipientFingerprint: hex(32), ephemeralKey: hex(32), nonce: hex(12),
  ciphertext: z.string().min(32).max((65536 + 16) * 2).regex(/^(?:[0-9a-f]{2})+$/),
  signature: hex(64),
});
export type Envelope = z.infer<typeof envelopeSchema>;
export const observationSchema = z.strictObject({
  deviceId: identifier, senderOwner: z.string().max(4096), messageId: identifier,
  at: safeInteger.nonnegative(), signature: hex(64), state: z.enum(['delivered', 'claimed', 'replied']),
});
export type Observation = z.infer<typeof observationSchema>;
export type MessageState = 'queued' | 'delivered' | 'claimed' | 'replied' | 'expired';
export const encode = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));
export const hexString = (value: Uint8Array) => Array.from(value, b => b.toString(16).padStart(2, '0')).join('');
export const bytes = (value: string) => Uint8Array.from(value.match(/../g)!.map(b => parseInt(b, 16)));
export const digest = (value: Uint8Array) => hexString(sha256(value));
export const deviceBytes = (d: Device) => encode(['everett-device-v2', d.owner, d.deviceId, d.signingKey, d.encryptionKey]);
export const fingerprint = (d: Device) => digest(deviceBytes(d));
export const aadBytes = (e: Envelope) => encode([
  'everett-relay-v2', e.owner, e.recipientOwner, e.grantId, e.fromDevice, e.toDevice,
  e.messageId, e.idempotencyKey, e.parent === null ? null : [e.parent.owner, e.parent.messageId],
  e.hop, e.createdAt, e.expiresAt, e.senderFingerprint, e.recipientFingerprint, e.ephemeralKey, e.nonce,
]);
export const envelopeBytes = (e: Envelope) => encode([new TextDecoder().decode(aadBytes(e)), e.ciphertext]);
export const rotationBytes = (d: Device, oldFingerprint: string) => encode(['everett-rotate-v2', d.owner, d.deviceId, oldFingerprint, fingerprint(d)]);
export const observationBytes = (owner: string, o: Observation) => encode([
  'everett-observe-v2', owner, o.deviceId, o.senderOwner, o.messageId, o.state, o.at,
]);
export function verify(key: string, signature: string, message: Uint8Array) {
  if (!ed25519.verify(bytes(signature), message, bytes(key), { zip215: false })) throw new RelayError('invalid_signature', 400);
}
export function validateDevice(d: Device) {
  if (d.encryptionKey === '00'.repeat(32)) throw new RelayError('invalid_encryption_key', 400);
  verify(d.signingKey, d.signature, deviceBytes(d));
}
export class RelayError extends Error {
  constructor(public code: string, public status = 403) { super(code); }
}
