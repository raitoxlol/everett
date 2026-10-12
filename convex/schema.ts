import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";

export const deviceFields = {
  owner: v.string(), deviceId: v.string(), signingKey: v.string(),
  encryptionKey: v.string(), signature: v.string(),
};

export const envelopeFields = {
  owner: v.string(), fromDevice: v.string(), toDevice: v.string(),
  messageId: v.string(), idempotencyKey: v.string(),
  createdAt: v.number(), expiresAt: v.number(),
  ephemeralKey: v.string(), nonce: v.string(),
  ciphertext: v.string(), signature: v.string(),
};

export default defineSchema({
  devices: defineTable(deviceFields).index("by_owner_device", ["owner", "deviceId"]),
  messages: defineTable(envelopeFields)
    .index("by_recipient", ["owner", "toDevice"])
    .index("by_message", ["owner", "messageId"])
    .index("by_expiry", ["expiresAt"]),
  attempts: defineTable({
    owner: v.string(), fromDevice: v.string(), toDevice: v.string(),
    messageId: v.string(), idempotencyKey: v.string(), digest: v.string(),
    expiresAt: v.number(), retainUntil: v.number(),
    state: v.union(v.literal("queued"), v.literal("accepted"), v.literal("expired")),
  }).index("by_key", ["owner", "fromDevice", "idempotencyKey"])
    .index("by_message", ["owner", "messageId"])
    .index("by_retention", ["retainUntil"]),
});
