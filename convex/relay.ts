import {
  queryGeneric as query, mutationGeneric as mutation, internalMutationGeneric as internalMutation,
  makeFunctionReference,
} from "convex/server";
import { v } from "convex/values";
import { deviceValidator, envelopeValidator, observationValidator, roleValidator } from "./schema";
import {
  authorize, ownAgent, requireAdmin, activeDevice, bundle, namespace,
  type Context, type WriteContext,
} from "./authorization";
import {
  deviceBytes, envelopeBytes, fingerprint, rotationBytes, observationBytes,
  hex, identifier, verify, validateDevice, digest, type Envelope,
} from "./protocol";
import { allowedGrant } from "./grants";
export { deviceBytes, envelopeBytes } from "./protocol";

// Deployment-privileged control plane, NOT a public JWT-callable function. Provision only after
// authoritative issuer verification; the issuer must never source these claims from user metadata.
export const provisionIdentity = internalMutation({
  args: { issuer: v.string(), subject: v.string(), org: v.union(v.null(), v.string()), agentId: v.string(), role: roleValidator },
  handler: async (ctx: WriteContext, args) => {
    identifier(args.agentId);
    if (!args.issuer || args.issuer.length > 2048 || !args.subject || args.subject.length > 256 ||
        (args.org !== null && (!args.org || args.org.length > 160))) throw new Error("Invalid provisioned identity");
    const owner = namespace(args.issuer, args.subject, args.org);
    const member = await ctx.db.query("memberships").withIndex("by_identity", (q) =>
      q.eq("owner", owner).eq("issuer", args.issuer).eq("subject", args.subject)).unique();
    if (member?.status === "revoked") throw new Error("Membership tombstone cannot be reused");
    const credential = await ctx.db.query("credentials").withIndex("by_agent", (q) =>
      q.eq("issuer", args.issuer).eq("agentId", args.agentId)).unique();
    if (credential && (credential.status === "revoked" || credential.owner !== owner || credential.subject !== args.subject)) {
      throw new Error("Agent credential tombstone or identity binding cannot be reused");
    }
    if (member) await ctx.db.patch(member._id, { role: args.role });
    else await ctx.db.insert("memberships", { owner, issuer: args.issuer, subject: args.subject, role: args.role, status: "active" });
    if (!credential) await ctx.db.insert("credentials", { issuer: args.issuer, agentId: args.agentId, owner, subject: args.subject, status: "active" });
    return { owner, deviceId: args.agentId };
  },
});
export const register = mutation({
  args: { device: deviceValidator },
  handler: async (ctx: WriteContext, { device: d }) => {
    const p = await authorize(ctx); ownAgent(p, d.deviceId);
    if (d.owner !== p.owner) throw new Error("Wrong organization");
    validateDevice(d);
    const existing = await ctx.db.query("devices").withIndex("by_owner_device", (q) => q.eq("owner", p.owner).eq("deviceId", d.deviceId)).unique();
    if (existing) {
      if (fingerprint(existing) !== fingerprint(d)) throw new Error("Device keys cannot be replaced; use signed rotation");
    } else await ctx.db.insert("devices", { ...d, generation: 0 });
    return null;
  },
});
export const credentials = query({
  args: {}, handler: async (ctx: Context) => {
    const p = await authorize(ctx);
    const rows = p.admin
      ? await ctx.db.query("credentials").withIndex("by_owner", q => q.eq("owner", p.owner)).take(257)
      : [p.credential];
    return { credentials: rows.slice(0, 256).map(row => ({ agentId: row.agentId, subject: row.subject, status: row.status })), truncated: rows.length > 256 };
  },
});
export const memberships = query({
  args: {}, handler: async (ctx: Context) => {
    const p = await authorize(ctx); requireAdmin(p);
    const rows = await ctx.db.query("memberships").withIndex("by_identity", q => q.eq("owner", p.owner)).take(257);
    return { memberships: rows.slice(0, 256).map(row => ({ subject: row.subject, role: row.role, status: row.status })), truncated: rows.length > 256 };
  },
});
export const rotate = mutation({
  args: { device: deviceValidator, oldFingerprint: v.string(), signature: v.string() },
  handler: async (ctx: WriteContext, args) => {
    const p = await authorize(ctx); ownAgent(p, args.device.deviceId);
    if (args.device.owner !== p.owner) throw new Error("Wrong organization");
    const old = await activeDevice(ctx, p.owner, p.agentId);
    validateDevice(args.device);
    if (fingerprint(old) !== args.oldFingerprint) throw new Error("Old fingerprint mismatch");
    verify(old.signingKey, args.signature, rotationBytes(args.device, args.oldFingerprint));
    if (!Number.isSafeInteger(old.generation + 1)) throw new Error("Device rotation limit reached");
    await ctx.db.replace(old._id, { ...args.device, generation: old.generation + 1 });
    return { fingerprint: fingerprint(args.device) };
  },
});
export const revokeAgent = mutation({
  args: { deviceId: v.string() },
  handler: async (ctx: WriteContext, args) => {
    const p = await authorize(ctx); await activeDevice(ctx, p.owner, p.agentId);
    if (args.deviceId !== p.agentId) requireAdmin(p);
    const credential = await ctx.db.query("credentials").withIndex("by_agent", (q) => q.eq("issuer", p.credential.issuer).eq("agentId", args.deviceId)).unique();
    if (!credential || credential.owner !== p.owner) throw new Error("Wrong organization or unknown agent");
    await ctx.db.patch(credential._id, { status: "revoked" });
    return null;
  },
});
export const revokeMember = mutation({
  args: { subject: v.string() },
  handler: async (ctx: WriteContext, args) => {
    const p = await authorize(ctx); await activeDevice(ctx, p.owner, p.agentId); requireAdmin(p);
    const member = await ctx.db.query("memberships").withIndex("by_identity", (q) => q.eq("owner", p.owner).eq("issuer", p.credential.issuer).eq("subject", args.subject)).unique();
    if (!member) throw new Error("Unknown organization member");
    await ctx.db.patch(member._id, { status: "revoked" });
    return null;
  },
});
export const whoami = query({
  args: {},
  handler: async (ctx: Context) => {
    const p = await authorize(ctx);
    return { owner: p.owner, deviceId: p.agentId, role: p.admin ? "admin" as const : "member" as const };
  },
});
export const devices = query({
  args: {},
  handler: async (ctx: Context) => {
    const p = await authorize(ctx); await activeDevice(ctx, p.owner, p.agentId);
    const rows = await ctx.db.query("devices").withIndex("by_owner_device", (q) => q.eq("owner", p.owner)).take(256);
    const active = [];
    for (const row of rows) {
      try { await activeDevice(ctx, row.owner, row.deviceId); active.push(bundle(row)); } catch { /* revoked peers are not routable */ }
    }
    return active;
  },
});
// This is an exact, authoritative lookup. `devices` is a bounded presentation list and
// must not be used for pairing or staged rotation recovery.
export const device = query({
  args: { deviceId: v.string() },
  handler: async (ctx: Context, args) => {
    const p = await authorize(ctx);
    await activeDevice(ctx, p.owner, p.agentId);
    return bundle(await activeDevice(ctx, p.owner, args.deviceId));
  },
});
async function attempt(ctx: Context, owner: string, messageId: string) {
  identifier(messageId);
  return ctx.db.query("attempts").withIndex("by_message", (q) => q.eq("owner", owner).eq("messageId", messageId)).unique();
}
async function route(ctx: Context, e: {
  owner: string; recipientOwner: string; fromDevice: string; toDevice: string; grantId: string | null;
  senderFingerprint: string; recipientFingerprint: string;
}, now: number) {
  const sender = await activeDevice(ctx, e.owner, e.fromDevice);
  const recipient = await activeDevice(ctx, e.recipientOwner, e.toDevice);
  if (fingerprint(sender) !== e.senderFingerprint || fingerprint(recipient) !== e.recipientFingerprint) {
    throw new Error("Envelope key changed");
  }
  const grant = await allowedGrant(ctx, e, now);
  return { sender, recipient, grant };
}
async function rate(ctx: WriteContext, scope: string, limit: number, now: number) {
  const window = Math.floor(now / 60000);
  const row = await ctx.db.query("rates").withIndex("by_scope", (q) => q.eq("scope", scope)).unique();
  const count = row?.window === window ? row.count : 0;
  if (count >= limit) throw new Error("Relay rate exceeded");
  if (row) await ctx.db.patch(row._id, { window, count: count + 1 });
  else await ctx.db.insert("rates", { scope, window, count: 1 });
}
const observedState = (a: { state: string; expiresAt: number }, now: number) => a.state === "queued" && a.expiresAt <= now ? "expired" : a.state;
// Attempts are permanent replay tombstones but deliberately exclude ciphertext and other
// large envelope fields. This bounded read keeps the 256-recipient capacity check below
// Convex's transaction read budget even when queued message bodies are at their maximum.
export async function queuedAttemptCount(ctx: Context, recipientOwner: string, toDevice: string, now: number) {
  const queued = await ctx.db.query("attempts").withIndex("by_recipient_state_expiry", (q) =>
    q.eq("recipientOwner", recipientOwner).eq("toDevice", toDevice).eq("state", "queued").gt("expiresAt", now)).take(256);
  return queued.length;
}
export const enqueue = mutation({
  args: { envelope: envelopeValidator },
  handler: async (ctx: WriteContext, { envelope: e }) => {
    const now = Date.now(), p = await authorize(ctx); ownAgent(p, e.fromDevice);
    if (p.owner !== e.owner) throw new Error("Wrong organization");
    identifier(e.messageId); identifier(e.idempotencyKey);
    const { sender, grant } = await route(ctx, e, now);
    hex(e.ephemeralKey, 32); hex(e.nonce, 12);
    const bodyCap = grant?.bodyCapBytes ?? 65536;
    if (e.ciphertext.length < 32 || e.ciphertext.length > (bodyCap + 16) * 2) throw new Error("Serialized body cap exceeded");
    hex(e.ciphertext);
    if (![e.createdAt, e.expiresAt, e.hop].every(Number.isSafeInteger)) throw new Error("Invalid integer metadata");
    if (e.hop < 1 || e.hop > 3) throw new Error("Hop limit exceeded");
    verify(sender.signingKey, e.signature, envelopeBytes(e));
    const requestDigest = digest(envelopeBytes(e));
    const previous = await ctx.db.query("attempts").withIndex("by_key", (q) => q.eq("owner", p.owner).eq("fromDevice", e.fromDevice).eq("idempotencyKey", e.idempotencyKey)).unique();
    if (previous) {
      if (previous.digest !== requestDigest) throw new Error("Idempotency key already used for a different request");
      return { messageId: previous.messageId, state: observedState(previous, now) };
    }
    if (e.expiresAt <= now || Math.abs(e.createdAt - now) > 300000 || e.expiresAt <= e.createdAt || e.expiresAt - e.createdAt > 86400000) throw new Error("Expired or invalid TTL");
    if (await attempt(ctx, p.owner, e.messageId)) throw new Error("Message ID already used");
    if (e.parent === null) {
      if (e.hop !== 1) throw new Error("Root hop must be 1");
    } else {
      const parent = await attempt(ctx, e.parent.owner, e.parent.messageId);
      if (!parent || parent.recipientOwner !== p.owner || parent.toDevice !== p.agentId || !["delivered", "claimed", "replied"].includes(parent.state)) throw new Error("Parent not observed by this agent");
      if (e.hop <= parent.hop) throw new Error("Hop lineage mismatch");
    }
    if (await queuedAttemptCount(ctx, e.recipientOwner, e.toDevice, now) >= 256) throw new Error("Recipient queue full");
    await rate(ctx, JSON.stringify(["agent", p.owner, p.agentId]), 120, now);
    await rate(ctx, JSON.stringify(["account", p.owner, p.member.issuer, p.member.subject]), 120, now);
    await rate(ctx, JSON.stringify(["org", p.owner]), 600, now);
    if (grant) await rate(ctx, JSON.stringify(["grant", grant.grantId]), grant.ratePerMinute, now);
    await ctx.db.insert("messages", e);
    const { ciphertext, signature, ephemeralKey, nonce, createdAt, ...metadata } = e;
    await ctx.db.insert("attempts", { ...metadata, digest: requestDigest, state: "queued" });
    return { messageId: e.messageId, state: "queued" as const };
  },
});
export const pending = query({
  args: { deviceId: v.string(), cursor: v.optional(v.union(v.null(), v.string())) },
  handler: async (ctx: Context, { deviceId, cursor }) => {
    const now = Date.now(), p = await authorize(ctx); ownAgent(p, deviceId);
    await activeDevice(ctx, p.owner, deviceId);
    // Return a cursor even if every inspected row is blocked by a revoked/changed grant.
    // A fixed take without a cursor would starve later valid messages until TTL.
    // 32 maximum-body envelopes leave ample metadata headroom under the 8 MiB
    // function-result limit as well as the 16 MiB transaction-read limit.
    // Keep the indexed range stable across pages and reactive reruns; filter TTL below.
    const page = await ctx.db.query("messages").withIndex("by_recipient", (q) => q.eq("recipientOwner", p.owner).eq("toDevice", deviceId))
      .paginate({ numItems: 32, cursor: cursor ?? null });
    const envelopes = [];
    for (const row of page.page) {
      if (row.expiresAt <= now) continue;
      try {
        await route(ctx, row, now);
        const { _id, _creationTime, ...envelope } = row;
        envelopes.push({ envelope, trust: row.owner === p.owner ? "same-org" as const : "external-untrusted" as const });
      } catch { /* revoked, unpinned and expired grants must not deliver previously queued traffic */ }
    }
    return { serverNow: now, envelopes, isDone: page.isDone, continueCursor: page.continueCursor };
  },
});
const observationArgs = {
  deviceId: v.string(), senderOwner: v.string(), messageId: v.string(), at: v.number(), signature: v.string(),
};
async function observeMessage(ctx: WriteContext, args: {
  deviceId: string; senderOwner: string; messageId: string; at: number; signature: string;
  state: "delivered" | "claimed" | "replied";
}) {
  const now = Date.now(), p = await authorize(ctx); ownAgent(p, args.deviceId);
  const recipient = await activeDevice(ctx, p.owner, p.agentId);
  if (!Number.isSafeInteger(args.at) || Math.abs(now - args.at) > 300000) throw new Error("Stale observation");
  verify(recipient.signingKey, args.signature, observationBytes({ ...args, owner: p.owner }));
  const a = await attempt(ctx, args.senderOwner, args.messageId);
  if (!a || a.recipientOwner !== p.owner || a.toDevice !== p.agentId) throw new Error("No message for this agent");
  await route(ctx, a, now);
  if (a.state === "expired" || (a.state === "queued" && a.expiresAt <= now)) throw new Error("Message TTL expired");
  if (args.state !== "delivered" && a.state === "queued") throw new Error("Delivery must be observed first");
  const rank = { queued: 0, expired: 0, delivered: 1, claimed: 2, replied: 3 };
  if (rank[args.state] > rank[a.state]) await ctx.db.patch(a._id, { state: args.state });
  const message = await ctx.db.query("messages").withIndex("by_message", (q) => q.eq("owner", args.senderOwner).eq("messageId", args.messageId)).unique();
  if (message) await ctx.db.delete(message._id);
  return null;
}
export const accept = mutation({
  args: observationArgs,
  handler: (ctx: WriteContext, args) => observeMessage(ctx, { ...args, state: "delivered" }),
});
export const observe = mutation({
  args: { ...observationArgs, state: observationValidator },
  handler: observeMessage,
});
export const status = query({
  args: { fromDevice: v.string(), messageId: v.string() },
  handler: async (ctx: Context, args) => {
    const now = Date.now(), p = await authorize(ctx); ownAgent(p, args.fromDevice);
    await activeDevice(ctx, p.owner, p.agentId);
    const a = await attempt(ctx, p.owner, args.messageId);
    if (!a || a.fromDevice !== p.agentId) throw new Error("No message for this agent");
    await route(ctx, a, now);
    return { state: observedState(a, now) };
  },
});
export const expire = internalMutation({
  args: {},
  handler: async (ctx: WriteContext) => {
    const expired = await ctx.db.query("messages").withIndex("by_expiry", (q) => q.lte("expiresAt", Date.now())).take(64);
    for (const message of expired) {
      const a = await attempt(ctx, message.owner, message.messageId);
      if (a?.state === "queued") await ctx.db.patch(a._id, { state: "expired" });
      await ctx.db.delete(message._id);
    }
    // Date.now() alone does not invalidate subscriptions. Persist grant expiry so active
    // grant views and pending query dependencies rerun even when no client writes occur.
    const grants = await ctx.db.query("grants").withIndex("by_expiry", (q) => q.gt("sweepAt", null).lte("sweepAt", Date.now())).take(64);
    for (const grant of grants) await ctx.db.patch(grant._id, { state: grant.state === "revoked" ? "revoked" : "expired", sweepAt: null });
    if (expired.length === 64 || grants.length === 64) await ctx.scheduler.runAfter(0, makeFunctionReference<"mutation">("relay:expire"), {});
    return null;
  },
});
