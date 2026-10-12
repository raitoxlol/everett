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

export const scopeFields = {
  fromOwner: v.string(), toOwner: v.string(), fromOrg: v.string(), toOrg: v.string(),
  fromAgent: v.string(), toAgent: v.string(),
};

export const agentEnvelopeFields = { ...envelopeFields, scope: v.object(scopeFields) };
const state = v.union(v.literal("queued"), v.literal("accepted"), v.literal("expired"));

export default defineSchema({
  memberships: defineTable({ owner: v.string(), org: v.string(), revoked: v.boolean() }).index("by_identity", ["org", "owner"]),
  rateLimits: defineTable({ owner: v.string(), org: v.string(), bucket: v.string(), key: v.string(), window: v.number(), count: v.number(), expiresAt: v.number() })
    .index("by_key", ["org", "bucket", "key"]).index("by_expiry", ["expiresAt"]),
  agents: defineTable({ owner: v.string(), org: v.string(), agentId: v.string(), device: v.object(deviceFields), revoked: v.boolean() })
    .index("by_agent", ["agentId"]).index("by_org", ["org"]),
  credentials: defineTable({ owner: v.string(), org: v.string(), agentId: v.string(), credentialId: v.string(), revoked: v.boolean() })
    .index("by_credential", ["credentialId"]).index("by_agent", ["agentId"]),
  grants: defineTable({ owner: v.string(), org: v.string(), leftAgent: v.string(), rightAgent: v.string(), rightOrg: v.string(),
    leftApproved: v.boolean(), rightApproved: v.boolean(), revoked: v.boolean() })
    .index("by_left", ["org"]).index("by_right", ["rightOrg"])
    .index("by_pair", ["leftAgent", "rightAgent"]),
  agentMessages: defineTable({ ...agentEnvelopeFields, org: v.string(), toOrg: v.string(), toOwner: v.string(),
    grantId: v.union(v.id("grants"), v.null()), credentialId: v.string() })
    .index("by_recipient", ["toOrg", "scope.toAgent"]).index("by_message", ["org", "messageId"])
    .index("by_expiry", ["expiresAt"]),
  agentAttempts: defineTable({ owner: v.string(), org: v.string(), toOrg: v.string(), toOwner: v.string(),
    fromAgent: v.string(), toAgent: v.string(), messageId: v.string(), idempotencyKey: v.string(), digest: v.string(),
    expiresAt: v.number(), retainUntil: v.number(), state })
    .index("by_key", ["fromAgent", "idempotencyKey"]).index("by_message", ["org", "messageId"])
    .index("by_recipient_message", ["toOrg", "toAgent", "messageId", "org"]).index("by_retention", ["retainUntil"]),
  devices: defineTable({ ...deviceFields, org: v.string() }).index("by_owner_device", ["owner", "deviceId"]),
  messages: defineTable({ ...envelopeFields, org: v.string() })
    .index("by_recipient", ["owner", "toDevice"])
    .index("by_message", ["owner", "messageId"])
    .index("by_expiry", ["expiresAt"]),
  attempts: defineTable({
    owner: v.string(), org: v.string(), fromDevice: v.string(), toDevice: v.string(),
    messageId: v.string(), idempotencyKey: v.string(), digest: v.string(),
    expiresAt: v.number(), retainUntil: v.number(),
    state: v.union(v.literal("queued"), v.literal("accepted"), v.literal("expired")),
  }).index("by_key", ["owner", "fromDevice", "idempotencyKey"])
    .index("by_message", ["owner", "messageId"])
    .index("by_retention", ["retainUntil"]),
});
