import { queryGeneric as query, mutationGeneric as mutation } from "convex/server";
import { v } from "convex/values";
import { authorize, activeDevice, bundle, requireAdmin, type Context, type WriteContext, type Principal, type Model } from "./authorization";
import { fingerprint, identifier } from "./protocol";

export type Grant = Model["grants"]["document"];
export async function findGrant(ctx: Context, grantId: string) {
  identifier(grantId);
  const grant = await ctx.db.query("grants").withIndex("by_grant", (q) => q.eq("grantId", grantId)).unique();
  if (!grant) throw new Error("Missing cross-organization grant");
  return grant;
}
function side(p: Principal, g: Grant): "a" | "b" {
  if (p.owner === g.aOwner) return "a";
  if (p.owner === g.bOwner) return "b";
  throw new Error("Wrong organization for grant");
}
async function caller(ctx: Context) {
  const p = await authorize(ctx);
  await activeDevice(ctx, p.owner, p.agentId);
  return p;
}
async function adminGrant(ctx: Context, grantId: string) {
  const p = await caller(ctx);
  requireAdmin(p);
  const g = await findGrant(ctx, grantId);
  return { p, g, side: side(p, g) };
}
async function currentBundles(ctx: Context, g: Grant) {
  const a = await activeDevice(ctx, g.aOwner, g.aDevice);
  const b = await activeDevice(ctx, g.bOwner, g.bDevice);
  return { a, b, aFingerprint: fingerprint(a), bFingerprint: fingerprint(b), aGeneration: a.generation, bGeneration: b.generation };
}
function checkPins(s: "a" | "b", local: string, peer: string, a: string, b: string) {
  if (local !== (s === "a" ? a : b) || peer !== (s === "a" ? b : a)) {
    throw new Error("Explicit local and peer fingerprints do not match");
  }
}
export async function allowedGrant(ctx: Context, route: {
  owner: string; recipientOwner: string; fromDevice: string; toDevice: string; grantId: string | null;
}, now: number) {
  if (route.owner === route.recipientOwner) {
    if (route.grantId !== null) throw new Error("Same-organization messages must not use a grant");
    return null;
  }
  if (route.grantId === null) throw new Error("Missing cross-organization grant");
  const g = await findGrant(ctx, route.grantId);
  if (g.state !== "active" || (g.expiresAt !== null && g.expiresAt <= now)) throw new Error("Grant inactive, revoked or expired");
  const forward = route.owner === g.aOwner && route.fromDevice === g.aDevice && route.recipientOwner === g.bOwner && route.toDevice === g.bDevice;
  const reverse = route.owner === g.bOwner && route.fromDevice === g.bDevice && route.recipientOwner === g.aOwner && route.toDevice === g.aDevice;
  if (!forward && !reverse) throw new Error("Grant does not name these agents");
  const current = await currentBundles(ctx, g);
  if (current.aFingerprint !== g.aFingerprint || current.bFingerprint !== g.bFingerprint || current.aGeneration !== g.aGeneration || current.bGeneration !== g.bGeneration) {
    throw new Error("Grant key changed; bilateral reconfirmation required");
  }
  return g;
}

export const invite = mutation({
  args: {
    grantId: v.string(), deviceId: v.string(), peerOwner: v.string(), peerDeviceId: v.string(),
    localFingerprint: v.string(), peerFingerprint: v.string(), expiresAt: v.union(v.null(), v.number()),
    ratePerMinute: v.number(), bodyCapBytes: v.number(),
  },
  handler: async (ctx: WriteContext, args) => {
    const p = await caller(ctx); requireAdmin(p);
    identifier(args.grantId);
    if (p.owner === args.peerOwner) throw new Error("Grant requires different organizations");
    if (args.expiresAt !== null && (!Number.isSafeInteger(args.expiresAt) || args.expiresAt <= Date.now())) throw new Error("Invalid grant expiry");
    if (!Number.isInteger(args.ratePerMinute) || args.ratePerMinute < 1 || args.ratePerMinute > 120) throw new Error("Invalid grant rate");
    if (!Number.isInteger(args.bodyCapBytes) || args.bodyCapBytes < 256 || args.bodyCapBytes > 65536) throw new Error("Invalid grant body cap");
    const a = await activeDevice(ctx, p.owner, args.deviceId);
    const b = await activeDevice(ctx, args.peerOwner, args.peerDeviceId);
    checkPins("a", args.localFingerprint, args.peerFingerprint, fingerprint(a), fingerprint(b));
    if (await ctx.db.query("grants").withIndex("by_grant", (q) => q.eq("grantId", args.grantId)).unique()) throw new Error("Grant ID already used");
    await ctx.db.insert("grants", {
      grantId: args.grantId, aOwner: p.owner, aDevice: args.deviceId, bOwner: args.peerOwner, bDevice: args.peerDeviceId,
      aBundle: bundle(a), bBundle: bundle(b), aFingerprint: fingerprint(a), bFingerprint: fingerprint(b),
      aGeneration: a.generation, bGeneration: b.generation,
      state: "invited", confirmation: "a", expiresAt: args.expiresAt, sweepAt: args.expiresAt,
      ratePerMinute: args.ratePerMinute, bodyCapBytes: args.bodyCapBytes,
    });
    return { grantId: args.grantId, state: "invited" as const };
  },
});
export const accept = mutation({
  args: { grantId: v.string(), localFingerprint: v.string(), peerFingerprint: v.string() },
  handler: async (ctx: WriteContext, args) => {
    const { g, side: s } = await adminGrant(ctx, args.grantId);
    if (s !== "b" || g.state !== "invited") throw new Error("Only invited organization admin may accept");
    if (g.expiresAt !== null && g.expiresAt <= Date.now()) throw new Error("Grant expired");
    const current = await currentBundles(ctx, g);
    checkPins(s, args.localFingerprint, args.peerFingerprint, g.aFingerprint, g.bFingerprint);
    if (current.aFingerprint !== g.aFingerprint || current.bFingerprint !== g.bFingerprint || current.aGeneration !== g.aGeneration || current.bGeneration !== g.bGeneration) throw new Error("Invited key changed; reconfirm required");
    await ctx.db.patch(g._id, { state: "active", confirmation: "both" });
    return { grantId: g.grantId, state: "active" as const };
  },
});
export const reconfirm = mutation({
  args: { grantId: v.string(), localFingerprint: v.string(), peerFingerprint: v.string() },
  handler: async (ctx: WriteContext, args) => {
    const { g, side: s } = await adminGrant(ctx, args.grantId);
    if (g.state === "revoked" || g.state === "expired" || (g.expiresAt !== null && g.expiresAt <= Date.now())) throw new Error("Grant revoked or expired");
    const current = await currentBundles(ctx, g);
    checkPins(s, args.localFingerprint, args.peerFingerprint, current.aFingerprint, current.bFingerprint);
    const unchanged = current.aFingerprint === g.aFingerprint && current.bFingerprint === g.bFingerprint && current.aGeneration === g.aGeneration && current.bGeneration === g.bGeneration;
    if (g.state === "active" && unchanged) throw new Error("No grant key change to reconfirm");
    const both = g.state === "reconfirming" && unchanged && g.confirmation !== s;
    await ctx.db.patch(g._id, {
      aBundle: bundle(current.a), bBundle: bundle(current.b), aFingerprint: current.aFingerprint, bFingerprint: current.bFingerprint,
      aGeneration: current.aGeneration, bGeneration: current.bGeneration,
      state: both ? "active" : "reconfirming", confirmation: both ? "both" : s,
    });
    return { grantId: g.grantId, state: both ? "active" as const : "reconfirming" as const };
  },
});
export const revoke = mutation({
  args: { grantId: v.string() },
  handler: async (ctx: WriteContext, args) => {
    const { g } = await adminGrant(ctx, args.grantId);
    await ctx.db.patch(g._id, { state: "revoked" });
    return null;
  },
});
async function view(ctx: Context, g: Grant) {
  let access: string = g.state;
  if (g.state !== "revoked" && g.expiresAt !== null && g.expiresAt <= Date.now()) access = "expired";
  if (access === "active") {
    try {
      const c = await currentBundles(ctx, g);
      if (c.aFingerprint !== g.aFingerprint || c.bFingerprint !== g.bFingerprint || c.aGeneration !== g.aGeneration || c.bGeneration !== g.bGeneration) access = "key_changed";
    } catch {
      access = "agent_revoked";
    }
  }
  const { _id, _creationTime, sweepAt, ...fields } = g;
  return { ...fields, access };
}
export const get = query({
  args: { grantId: v.string() },
  handler: async (ctx: Context, args) => {
    const p = await caller(ctx), g = await findGrant(ctx, args.grantId), s = side(p, g);
    if (!p.admin && p.agentId !== (s === "a" ? g.aDevice : g.bDevice)) throw new Error("Grant does not name this agent");
    return view(ctx, g);
  },
});
export const list = query({
  args: {},
  handler: async (ctx: Context) => {
    const p = await caller(ctx);
    const a = await ctx.db.query("grants").withIndex("by_a", (q) => p.admin ? q.eq("aOwner", p.owner) : q.eq("aOwner", p.owner).eq("aDevice", p.agentId)).take(128);
    const b = await ctx.db.query("grants").withIndex("by_b", (q) => p.admin ? q.eq("bOwner", p.owner) : q.eq("bOwner", p.owner).eq("bDevice", p.agentId)).take(128);
    return { grants: await Promise.all([...a, ...b].map((g) => view(ctx, g))), truncated: a.length === 128 || b.length === 128 };
  },
});
