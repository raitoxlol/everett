import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";
import { authTables } from "@convex-dev/auth/server";

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
  ...authTables,
  enrollments: defineTable({
    code: v.string(), pollHash: v.string(), tokenHash: v.string(), agentId: v.string(),
    name: v.string(), expiresAt: v.number(), clientKey: v.optional(v.string()),
    state: v.union(v.literal("pending"), v.literal("approved"), v.literal("denied")),
    owner: v.optional(v.string()),
  }).index("by_code", ["code"]).index("by_poll", ["pollHash"]).index("by_expiry", ["expiresAt"])
    .index("by_client", ["clientKey", "expiresAt"]),
  enrollmentRates: defineTable({ clientKey: v.string(), window: v.number(), count: v.number() })
    .index("by_client", ["clientKey"]).index("by_window", ["window"]),
  memberships: defineTable({
    owner: v.string(), issuer: v.string(), subject: v.string(), role: roleValidator, status,
  }).index("by_identity", ["owner", "issuer", "subject"]),
  // Agent IDs are issuer-global, immutable identity bindings, never recycled after revocation.
  credentials: defineTable({
    issuer: v.string(), agentId: v.string(), owner: v.string(), subject: v.string(), status,
    authUserId: v.optional(v.id("users")), tokenHash: v.optional(v.string()), name: v.optional(v.string()),
    kind: v.optional(v.literal("oauth")), clientId: v.optional(v.string()),
  }).index("by_agent", ["issuer", "agentId"]).index("by_owner", ["owner"])
    .index("by_auth_user", ["authUserId"]).index("by_token", ["tokenHash"]),
  devices: defineTable({ ...deviceFields, generation: v.number() }).index("by_owner_device", ["owner", "deviceId"]),
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
  oauthClients: defineTable({ clientId: v.string(), name: v.string(), redirectUris: v.array(v.string()), createdAt: v.number(),
    expiresAt: v.optional(v.number()), usedAt: v.optional(v.number()) })
    .index("by_client", ["clientId"]).index("by_expiry", ["expiresAt"]),
  oauthRequests: defineTable({
    requestId: v.string(), clientId: v.string(), clientName: v.string(), redirectUri: v.string(), codeChallenge: v.string(),
    state: v.union(v.null(), v.string()), scope: v.string(), resource: v.string(), expiresAt: v.number(),
    bindingHash: v.optional(v.string()),
  }).index("by_request", ["requestId"]).index("by_expiry", ["expiresAt"]).index("by_client", ["clientId", "expiresAt"]),
  oauthCodes: defineTable({
    codeHash: v.string(), credential: v.id("credentials"), clientId: v.string(), redirectUri: v.string(), codeChallenge: v.string(),
    resource: v.string(), scope: v.string(), expiresAt: v.number(), used: v.boolean(),
  }).index("by_hash", ["codeHash"]).index("by_expiry", ["expiresAt"]),
  // One family per authorization; replaying a used code or rotated refresh token revokes the whole family.
  oauthTokens: defineTable({
    tokenHash: v.string(), kind: v.union(v.literal("access"), v.literal("refresh")), family: v.string(), credential: v.id("credentials"),
    clientId: v.string(), resource: v.string(), scope: v.string(), issuedAt: v.number(), expiresAt: v.number(), used: v.boolean(), revoked: v.boolean(),
  }).index("by_hash", ["tokenHash"]).index("by_family", ["family"]).index("by_expiry", ["expiresAt"]),
  rates: defineTable({ scope: v.string(), window: v.number(), count: v.number() }).index("by_scope", ["scope"]),
});
