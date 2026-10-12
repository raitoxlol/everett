import { queryGeneric as query, mutationGeneric as mutation, internalMutationGeneric as internalMutation, makeFunctionReference } from "convex/server";
import { v, type Infer } from "convex/values";
import { principal, remember, limit, hash, type Context, type WriteContext } from "./identity";
import { deviceFields, agentEnvelopeFields } from "./schema";
import { deviceBytes, hex, verify } from "./relay";

const envelopeValidator = v.object(agentEnvelopeFields);
type Envelope = Infer<typeof envelopeValidator>;
const bytes = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));

export function envelopeBytes(e: Envelope) {
  return bytes([JSON.stringify(["everett-agent-relay-v2", e.owner, e.fromDevice, e.toDevice,
    e.messageId, e.idempotencyKey, e.createdAt, e.expiresAt, e.ephemeralKey, e.nonce,
    [e.scope.fromOwner, e.scope.toOwner, e.scope.fromOrg, e.scope.toOrg, e.scope.fromAgent, e.scope.toAgent]]), e.ciphertext]);
}

async function agent(ctx: Context, id: string) {
  const row = await ctx.db.query("agents").withIndex("by_agent", q => q.eq("agentId", id)).unique();
  if (!row || row.revoked) throw new Error("Agent unavailable or revoked");
  const membership = await ctx.db.query("memberships").withIndex("by_identity", q => q.eq("org", row.org).eq("owner", row.owner)).unique();
  if (!membership || membership.revoked) throw new Error("Org membership revoked");
  return row;
}

async function actor(ctx: Context) {
  const p = await principal(ctx);
  if (!p.agent || !p.credential) throw new Error("Issuer agent_id and jti required");
  const a = await agent(ctx, p.agent);
  const c = await ctx.db.query("credentials").withIndex("by_credential", q => q.eq("credentialId", p.credential!)).unique();
  if (a.owner !== p.owner || a.org !== p.org || !c || c.revoked || c.owner !== p.owner || c.org !== p.org || c.agentId !== p.agent) throw new Error("Relay credential revoked or unregistered");
  return { ...p, device: a.device, agent: p.agent };
}

async function admin(ctx: Context) {
  const p = await principal(ctx);
  if (!p.admin) throw new Error("Org administrator required");
  return p;
}

async function pairGrant(ctx: Context, from: string, to: string) {
  const [left, right] = [from, to].sort();
  return ctx.db.query("grants").withIndex("by_pair", q => q.eq("leftAgent", left).eq("rightAgent", right))
    .filter(q => q.eq(q.field("revoked"), false)).unique();
}

async function allowed(ctx: Context, from: string, to: string) {
  const a = await agent(ctx, from), b = await agent(ctx, to);
  let grantId = null;
  if (a.org !== b.org) {
    const grant = await pairGrant(ctx, from, to);
    if (!grant || grant.revoked || !grant.leftApproved || !grant.rightApproved) throw new Error("Both org admins must approve this named-agent pair");
    grantId = grant._id;
  }
  return { a, b, grantId };
}

export const identity = query({
  args: {}, handler: async (ctx: Context) => {
    const p = await principal(ctx);
    return { owner: p.owner, org: p.org, agentId: p.agent, credentialId: p.credential, admin: p.admin };
  },
});

export const register = mutation({
  args: { device: v.object(deviceFields) }, handler: async (ctx: WriteContext, { device: d }) => {
    const p = await principal(ctx);
    if (!p.agent || !p.credential) throw new Error("Issuer agent_id and jti required");
    if (d.owner !== p.owner) throw new Error("Wrong account");
    if (!/^[A-Za-z0-9_.-]{1,160}$/.test(d.deviceId)) throw new Error("Invalid device ID");
    hex(d.encryptionKey, 32); verify(d.signingKey, d.signature, deviceBytes(d));
    await remember(ctx, p);
    const existing = await ctx.db.query("agents").withIndex("by_agent", q => q.eq("agentId", p.agent!)).unique();
    if (existing) {
      if (existing.revoked || existing.owner !== p.owner || existing.org !== p.org) throw new Error("Agent unavailable or revoked");
      if (existing.device.deviceId !== d.deviceId || existing.device.signingKey !== d.signingKey || existing.device.encryptionKey !== d.encryptionKey) throw new Error("Agent device keys cannot be replaced");
    } else {
      await limit(ctx, p.owner, p.org, "register-agent", 16);
      if ((await ctx.db.query("agents").withIndex("by_org", q => q.eq("org", p.org)).take(512)).length >= 512) throw new Error("Org agent cap exceeded");
      await ctx.db.insert("agents", { owner: p.owner, org: p.org, agentId: p.agent, device: d, revoked: false });
    }
    const c = await ctx.db.query("credentials").withIndex("by_credential", q => q.eq("credentialId", p.credential!)).unique();
    if (c && (c.revoked || c.owner !== p.owner || c.org !== p.org || c.agentId !== p.agent)) throw new Error("Relay credential revoked");
    if (!c) {
      await limit(ctx, p.owner, p.org, "credentials", 16);
      await ctx.db.insert("credentials", { owner: p.owner, org: p.org, agentId: p.agent, credentialId: p.credential, revoked: false });
    }
    return { owner: p.owner, org: p.org, agentId: p.agent, credentialId: p.credential, device: d };
  },
});

export const directory = query({
  args: {}, handler: async (ctx: Context) => {
    const p = await principal(ctx);
    if (p.agent) await actor(ctx);
    const rows = await ctx.db.query("agents").withIndex("by_org", q => q.eq("org", p.org)).take(512);
    const result = [];
    for (const row of rows) {
      try {
        const { owner, org, agentId, device } = await agent(ctx, row.agentId);
        result.push({ owner, org, agentId, device });
      } catch { continue; }
    }
    return result;
  },
});

export const revoke = mutation({
  args: { agentId: v.string() }, handler: async (ctx: WriteContext, { agentId }) => {
    const p = await admin(ctx), a = await agent(ctx, agentId);
    if (a.org !== p.org) throw new Error("Wrong org");
    await ctx.db.patch(a._id, { revoked: true });
    return null;
  },
});

export const revokeCredential = mutation({
  args: { credentialId: v.string() }, handler: async (ctx: WriteContext, { credentialId }) => {
    const p = await principal(ctx);
    if (p.agent) await actor(ctx);
    const c = await ctx.db.query("credentials").withIndex("by_credential", q => q.eq("credentialId", credentialId)).unique();
    if (!c || c.org !== p.org || (!p.admin && (c.owner !== p.owner || (p.agent && c.agentId !== p.agent)))) throw new Error("Wrong credential owner");
    await ctx.db.patch(c._id, { revoked: true });
    return null;
  },
});

export const memberships = query({
  args: {}, handler: async (ctx: Context) => {
    const p = await admin(ctx);
    return ctx.db.query("memberships").withIndex("by_identity", q => q.eq("org", p.org)).collect();
  },
});

export const credentials = query({
  args: {}, handler: async (ctx: Context) => {
    const p = await principal(ctx);
    if (p.agent) await actor(ctx);
    const agents = await ctx.db.query("agents").withIndex("by_org", q => q.eq("org", p.org)).take(512);
    const result = [];
    for (const a of agents) {
      if (!p.admin && (a.owner !== p.owner || (p.agent && a.agentId !== p.agent))) continue;
      const rows = await ctx.db.query("credentials").withIndex("by_agent", q => q.eq("agentId", a.agentId)).collect();
      result.push(...rows.map(({ credentialId, agentId, revoked }) => ({ credentialId, agentId, revoked })));
    }
    return result;
  },
});

export const revokeMembership = mutation({
  args: { membershipId: v.id("memberships") }, handler: async (ctx: WriteContext, { membershipId }) => {
    const p = await admin(ctx), m = await ctx.db.get(membershipId);
    if (!m || m.org !== p.org) throw new Error("Wrong org");
    await ctx.db.patch(m._id, { revoked: true });
    return null;
  },
});

export const propose = mutation({
  args: { localAgent: v.string(), remoteAgent: v.string() }, handler: async (ctx: WriteContext, { localAgent, remoteAgent }) => {
    const p = await admin(ctx), a = await agent(ctx, localAgent), b = await agent(ctx, remoteAgent);
    if (a.org !== p.org || a.org === b.org) throw new Error("Select your own named agent and an external named agent");
    const existing = await pairGrant(ctx, localAgent, remoteAgent);
    if (existing) return existing._id;
    await limit(ctx, p.owner, p.org, "grants", 16);
    const [left, right] = [a, b].sort((x, y) => x.agentId.localeCompare(y.agentId));
    return ctx.db.insert("grants", { owner: p.owner, org: left.org, rightOrg: right.org, leftAgent: left.agentId, rightAgent: right.agentId,
      leftApproved: false, rightApproved: false, revoked: false });
  },
});

export const decide = mutation({
  args: { grantId: v.id("grants"), approve: v.boolean() }, handler: async (ctx: WriteContext, { grantId, approve }) => {
    const p = await admin(ctx), g = await ctx.db.get(grantId);
    if (!g || (g.org !== p.org && g.rightOrg !== p.org)) throw new Error("Wrong org");
    if (!approve) {
      await ctx.db.patch(g._id, { revoked: true, leftApproved: false, rightApproved: false });
    } else {
      if (g.revoked) throw new Error("Grant revoked; requires a new bilateral grant");
      await agent(ctx, g.leftAgent); await agent(ctx, g.rightAgent);
      await ctx.db.patch(g._id, p.org === g.org ? { leftApproved: true } : { rightApproved: true });
    }
    return null;
  },
});

export const grants = query({
  args: {}, handler: async (ctx: Context) => {
    const p = await principal(ctx);
    if (p.agent) await actor(ctx);
    const left = await ctx.db.query("grants").withIndex("by_left", q => q.eq("org", p.org)).take(256);
    const right = await ctx.db.query("grants").withIndex("by_right", q => q.eq("rightOrg", p.org)).take(256);
    const results = [];
    for (const g of [...left, ...right]) {
      if (p.agent && g.leftAgent !== p.agent && g.rightAgent !== p.agent) continue;
      const localAgent = g.org === p.org ? g.leftAgent : g.rightAgent;
      const remoteAgent = g.org === p.org ? g.rightAgent : g.leftAgent;
      const active = !g.revoked && g.leftApproved && g.rightApproved;
      let remote = null;
      if (active) {
        try {
          await agent(ctx, localAgent);
          const a = await agent(ctx, remoteAgent);
          remote = { owner: a.owner, org: a.org, agentId: a.agentId, device: a.device };
        } catch { /* Revoked agents never publish usable peer keys. */ }
      }
      results.push({ grantId: g._id, localAgent, remoteAgent, localApproved: g.org === p.org ? g.leftApproved : g.rightApproved,
        remoteApproved: g.org === p.org ? g.rightApproved : g.leftApproved, revoked: g.revoked, active: active && remote !== null, remote });
    }
    return results;
  },
});

export const enqueue = mutation({
  args: { envelope: envelopeValidator }, handler: async (ctx: WriteContext, { envelope: e }) => {
    const p = await actor(ctx);
    if (e.owner !== p.owner || e.fromDevice !== p.device.deviceId || e.scope.fromOwner !== p.owner || e.scope.fromOrg !== p.org || e.scope.fromAgent !== p.agent) throw new Error("Forged agent sender");
    const { b, grantId } = await allowed(ctx, p.agent, e.scope.toAgent);
    if (e.scope.toOrg !== b.org || e.scope.toOwner !== b.owner || e.toDevice !== b.device.deviceId) throw new Error("Wrong recipient scope");
    for (const id of [e.messageId, e.idempotencyKey]) if (!/^[A-Za-z0-9_.-]{1,160}$/.test(id)) throw new Error("Invalid identifier");
    if (e.ciphertext.length < 32 || e.ciphertext.length > (65536 + 16) * 2) throw new Error("Body cap exceeded");
    hex(e.ephemeralKey, 32); hex(e.nonce, 12); hex(e.ciphertext);
    if (![e.createdAt, e.expiresAt].every(Number.isSafeInteger)) throw new Error("Invalid timestamp");
    verify(p.device.signingKey, e.signature, envelopeBytes(e));
    const digest = hash(new TextDecoder().decode(envelopeBytes(e)));
    const previous = await ctx.db.query("agentAttempts").withIndex("by_key", q => q.eq("fromAgent", p.agent).eq("idempotencyKey", e.idempotencyKey)).unique();
    if (previous) {
      if (previous.org !== p.org || previous.owner !== p.owner || previous.digest !== digest) throw new Error("Idempotency key already used");
      return { messageId: previous.messageId, state: previous.state === "queued" && previous.expiresAt <= Date.now() ? "expired" : previous.state };
    }
    if (e.expiresAt <= Date.now() || e.createdAt > Date.now() + 300000 || e.expiresAt <= e.createdAt || e.expiresAt - e.createdAt > 86400000) throw new Error("Expired or invalid TTL");
    if (await ctx.db.query("agentAttempts").withIndex("by_message", q => q.eq("org", p.org).eq("messageId", e.messageId)).unique()) throw new Error("Message ID already used");
    await limit(ctx, p.owner, p.org, "agent-enqueue", 60, p.agent);
    await limit(ctx, p.owner, p.org, "org-enqueue", 600, p.org);
    if ((await ctx.db.query("agentMessages").withIndex("by_recipient", q => q.eq("toOrg", b.org).eq("scope.toAgent", b.agentId)).take(256)).length >= 256) throw new Error("Recipient queue full");
    await ctx.db.insert("agentMessages", { ...e, org: p.org, toOrg: b.org, toOwner: b.owner, grantId, credentialId: p.credential! });
    await ctx.db.insert("agentAttempts", { owner: p.owner, org: p.org, toOrg: b.org, toOwner: b.owner, fromAgent: p.agent, toAgent: b.agentId,
      messageId: e.messageId, idempotencyKey: e.idempotencyKey, digest, expiresAt: e.expiresAt, retainUntil: e.expiresAt + 7 * 86400000, state: "queued" });
    return { messageId: e.messageId, state: "queued" };
  },
});

export const pending = query({
  args: {}, handler: async (ctx: Context) => {
    const p = await actor(ctx);
    const rows = await ctx.db.query("agentMessages").withIndex("by_recipient", q => q.eq("toOrg", p.org).eq("scope.toAgent", p.agent)).take(256);
    const result = [];
    for (const row of rows) {
      if (row.expiresAt <= Date.now() || row.toOwner !== p.owner) continue;
      try {
        const access = await allowed(ctx, row.scope.fromAgent, p.agent);
        if (access.grantId !== row.grantId) continue;
        const credential = await ctx.db.query("credentials").withIndex("by_credential", q => q.eq("credentialId", row.credentialId)).unique();
        if (!credential || credential.revoked) continue;
      } catch { continue; }
      const { _id, _creationTime, org, toOrg, toOwner, grantId, credentialId, ...envelope } = row;
      result.push({ envelope, external: row.org !== p.org, trust: row.org !== p.org ? "external-untrusted" : "same-org" });
    }
    return result;
  },
});

export const accept = mutation({
  args: { fromOrg: v.string(), messageId: v.string(), at: v.number(), signature: v.string() }, handler: async (ctx: WriteContext, args) => {
    const p = await actor(ctx);
    if (!Number.isSafeInteger(args.at) || Math.abs(Date.now() - args.at) > 300000) throw new Error("Stale acknowledgement");
    verify(p.device.signingKey, args.signature, bytes(["everett-agent-accept-v2", p.org, p.agent, args.fromOrg, args.messageId, args.at]));
    const a = await ctx.db.query("agentAttempts").withIndex("by_recipient_message", q => q.eq("toOrg", p.org).eq("toAgent", p.agent).eq("messageId", args.messageId).eq("org", args.fromOrg)).unique();
    if (!a || a.org !== args.fromOrg || a.toOwner !== p.owner) throw new Error("No message for this agent");
    await allowed(ctx, a.fromAgent, p.agent);
    if (a.state !== "accepted" && a.expiresAt <= Date.now()) throw new Error("Message TTL expired");
    const row = await ctx.db.query("agentMessages").withIndex("by_message", q => q.eq("org", a.org).eq("messageId", a.messageId)).unique();
    if (row) {
      const access = await allowed(ctx, a.fromAgent, p.agent);
      if (access.grantId !== row.grantId) throw new Error("Message grant revoked");
      const credential = await ctx.db.query("credentials").withIndex("by_credential", q => q.eq("credentialId", row.credentialId)).unique();
      if (!credential || credential.revoked) throw new Error("Sender credential revoked");
    }
    await ctx.db.patch(a._id, { state: "accepted" });
    if (row) await ctx.db.delete(row._id);
    return null;
  },
});

export const status = query({
  args: { messageId: v.string() }, handler: async (ctx: Context, { messageId }) => {
    const p = await actor(ctx);
    const a = await ctx.db.query("agentAttempts").withIndex("by_message", q => q.eq("org", p.org).eq("messageId", messageId)).unique();
    if (!a || a.owner !== p.owner || a.fromAgent !== p.agent) throw new Error("No message for this agent");
    return { state: a.state === "queued" && a.expiresAt <= Date.now() ? "expired" : a.state };
  },
});

export const expire = internalMutation({
  args: {}, handler: async (ctx: WriteContext) => {
    const rows = await ctx.db.query("agentMessages").withIndex("by_expiry", q => q.lte("expiresAt", Date.now())).take(64);
    for (const row of rows) {
      const a = await ctx.db.query("agentAttempts").withIndex("by_message", q => q.eq("org", row.org).eq("messageId", row.messageId)).unique();
      if (a?.state === "queued") await ctx.db.patch(a._id, { state: "expired" });
      await ctx.db.delete(row._id);
    }
    const attempts = await ctx.db.query("agentAttempts").withIndex("by_retention", q => q.lte("retainUntil", Date.now())).take(256);
    for (const row of attempts) await ctx.db.delete(row._id);
    const limits = await ctx.db.query("rateLimits").withIndex("by_expiry", q => q.lte("expiresAt", Date.now())).take(256);
    for (const row of limits) await ctx.db.delete(row._id);
    if (rows.length === 64 || attempts.length === 256 || limits.length === 256) await ctx.scheduler.runAfter(0, makeFunctionReference<"mutation">("agents:expire"), {});
    return null;
  },
});
