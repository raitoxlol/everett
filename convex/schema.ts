import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";

export const deviceFields = {
  owner: v.string(), deviceId: v.string(), signingKey: v.string(),
  encryptionKey: v.string(), signature: v.string(),
};
export const deviceValidator = v.object(deviceFields);
export const parentValidator = v.union(v.null(), v.object({ owner: v.string(), messageId: v.string() }));
export const envelopeFields = {
  owner: v.string(), recipientOwner: v.string(), grantId: v.union(v.null(), v.string()),
  fromDevice: v.string(), toDevice: v.string(), messageId: v.string(), idempotencyKey: v.string(),
  parent: parentValidator, hop: v.number(), createdAt: v.number(), expiresAt: v.number(),
  senderFingerprint: v.string(), recipientFingerprint: v.string(),
  ephemeralKey: v.string(), nonce: v.string(), ciphertext: v.string(), signature: v.string(),
};
export const envelopeValidator = v.object(envelopeFields);
export const observationValidator = v.union(v.literal("delivered"), v.literal("claimed"), v.literal("replied"));
export const stateValidator = v.union(observationValidator, v.literal("queued"), v.literal("expired"));
export const roleValidator = v.union(v.literal("admin"), v.literal("member"));
const status = v.union(v.literal("active"), v.literal("revoked"));

export default defineSchema({
  memberships: defineTable({
    owner: v.string(), issuer: v.string(), subject: v.string(), role: roleValidator, status,
  }).index("by_identity", ["owner", "issuer", "subject"]),
  // Agent IDs are issuer-global, immutable identity bindings, never recycled after revocation.
  credentials: defineTable({
    issuer: v.string(), agentId: v.string(), owner: v.string(), subject: v.string(), status,
  }).index("by_agent", ["issuer", "agentId"]).index("by_owner", ["owner"]),
  // revokedAt is optional so rows written before revocation existed stay valid (absent = active).
  // sshDeviceId is the device's SSH peer id, which differs from its relay agent id on default installs.
  // It is bound once, unique per owner, and proven by sshSignature from the key in sshPublicKey.
  devices: defineTable({
    ...deviceFields, generation: v.number(), revokedAt: v.optional(v.union(v.null(), v.number())),
    sshDeviceId: v.optional(v.string()), sshPublicKey: v.optional(v.string()), sshSignature: v.optional(v.string()),
  }).index("by_owner_device", ["owner", "deviceId"]).index("by_owner_revoked", ["owner", "revokedAt"])
    .index("by_owner_ssh", ["owner", "sshDeviceId"]),
  grants: defineTable({
    grantId: v.string(), aOwner: v.string(), aDevice: v.string(), bOwner: v.string(), bDevice: v.string(),
    aBundle: deviceValidator, bBundle: deviceValidator, aFingerprint: v.string(), bFingerprint: v.string(),
    aGeneration: v.number(), bGeneration: v.number(),
    state: v.union(v.literal("invited"), v.literal("active"), v.literal("reconfirming"), v.literal("revoked"), v.literal("expired")),
    confirmation: v.union(v.literal("a"), v.literal("b"), v.literal("both")),
    expiresAt: v.union(v.null(), v.number()), sweepAt: v.union(v.null(), v.number()), ratePerMinute: v.number(), bodyCapBytes: v.number(),
  }).index("by_grant", ["grantId"])
    .index("by_a", ["aOwner", "aDevice"]).index("by_b", ["bOwner", "bDevice"])
    .index("by_expiry", ["sweepAt"]),
  messages: defineTable(envelopeFields)
    .index("by_recipient", ["recipientOwner", "toDevice", "expiresAt"])
    .index("by_message", ["owner", "messageId"])
    .index("by_expiry", ["expiresAt"]),
  // Permanent replay tombstones: no ciphertext, plaintext, or token. Do not TTL these records.
  attempts: defineTable({
    owner: v.string(), recipientOwner: v.string(), fromDevice: v.string(), toDevice: v.string(),
    grantId: v.union(v.null(), v.string()), senderFingerprint: v.string(), recipientFingerprint: v.string(),
    messageId: v.string(), idempotencyKey: v.string(), digest: v.string(),
    parent: parentValidator, hop: v.number(), expiresAt: v.number(), state: stateValidator,
  }).index("by_key", ["owner", "fromDevice", "idempotencyKey"])
    .index("by_message", ["owner", "messageId"])
    .index("by_recipient_state_expiry", ["recipientOwner", "toDevice", "state", "expiresAt"]),
  rates: defineTable({ scope: v.string(), window: v.number(), count: v.number() }).index("by_scope", ["scope"]),
});
