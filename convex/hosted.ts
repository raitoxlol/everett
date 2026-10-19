import {
  actionGeneric as action, queryGeneric as query, mutationGeneric as mutation,
  internalMutationGeneric as internalMutation, internalQueryGeneric as internalQuery,
  makeFunctionReference,
} from "convex/server";
import { v } from "convex/values";
import { sha256 } from "@noble/hashes/sha256";
import { bytesToHex } from "@noble/hashes/utils";
import { namespace, type Context, type WriteContext } from "./authorization";

export const hash = (value: string) => bytesToHex(sha256(new TextEncoder().encode(value)));
const ref = (name: string) => makeFunctionReference<"mutation">(`hosted:${name}`);
export const random = () => bytesToHex(crypto.getRandomValues(new Uint8Array(32)));
export function ownerGithubId() {
  const id = process.env.EVERETT_OWNER_GITHUB_ID;
  if (!id || !/^\d+$/.test(id)) throw new Error("Owner allowlist is not configured");
  return id;
}
export function ownerGithubLogin() {
  const login = process.env.EVERETT_OWNER_GITHUB_LOGIN;
  if (!login || !/^[A-Za-z0-9-]{1,39}$/.test(login)) throw new Error("Owner allowlist is not configured");
  return login;
}
export function checkGithub(profile: { id: number | string; login: string }) {
  if (String(profile.id) !== ownerGithubId() || profile.login.toLowerCase() !== ownerGithubLogin().toLowerCase()) {
    throw new Error("Owner only");
  }
}
export async function hostedSession(ctx: Context) {
  const identity = await ctx.auth.getUserIdentity();
  if (!identity || identity.issuer !== process.env.CONVEX_SITE_URL) throw new Error("Hosted sign in required");
  const [user, session, extra] = identity.subject.split("|");
  const userId = ctx.db.normalizeId("users", user);
  const sessionId = ctx.db.normalizeId("authSessions", session ?? "");
  if (!userId || !sessionId || extra !== undefined) throw new Error("Invalid hosted session");
  const row = await ctx.db.get(sessionId);
  if (!row || row.userId !== userId || row.expirationTime <= Date.now()) throw new Error("Hosted session revoked");
  return { userId, issuer: identity.issuer };
}
export async function ownerIdentity(ctx: Context) {
  const session = await hostedSession(ctx);
  const account = await ctx.db.query("authAccounts").withIndex("userIdAndProvider", q =>
    q.eq("userId", session.userId).eq("provider", "github")).unique();
  if (!account || account.providerAccountId !== ownerGithubId()) throw new Error("Owner only");
  return { ...session, owner: namespace(session.issuer, "", session.userId) };
}
export async function ensureOwnerMembership(ctx: WriteContext, owner: { owner: string; issuer: string; userId: string }) {
  const member = await ctx.db.query("memberships").withIndex("by_identity", q =>
    q.eq("owner", owner.owner).eq("issuer", owner.issuer).eq("subject", owner.userId)).unique();
  if (member?.status === "revoked") throw new Error("Membership tombstone cannot be reused");
  if (!member) await ctx.db.insert("memberships", { owner: owner.owner, issuer: owner.issuer, subject: owner.userId, role: "admin", status: "active" });
}
export async function agentCredential(ctx: Context, userId: string) {
  const id = ctx.db.normalizeId("users", userId);
  const credential = id && await ctx.db.query("credentials").withIndex("by_auth_user", q => q.eq("authUserId", id)).unique();
  if (!credential || credential.status !== "active") throw new Error("Agent credential denied or revoked");
  const member = await ctx.db.query("memberships").withIndex("by_identity", q =>
    q.eq("owner", credential.owner).eq("issuer", credential.issuer).eq("subject", credential.subject)).unique();
  if (!member || member.status !== "active") throw new Error("Organization membership denied");
  if (credential.issuer !== process.env.CONVEX_SITE_URL || credential.subject !== member.subject) throw new Error("Wrong hosted issuer");
  // Re-check the allowlist even for existing JWTs when the deployment owner changes.
  const account = await ctx.db.query("authAccounts").withIndex("userIdAndProvider", q =>
    q.eq("userId", ctx.db.normalizeId("users", credential.subject)!).eq("provider", "github")).unique();
  if (!account || account.providerAccountId !== ownerGithubId()) throw new Error("Owner only");
  return { credential, member };
}
export const authenticateAgent = internalQuery({
  args: { token: v.string() }, handler: async (ctx: Context, { token }) => {
    if (!/^evt_[a-f0-9]{64}$/.test(token)) throw new Error("Invalid agent token");
    const row = await ctx.db.query("credentials").withIndex("by_token", q => q.eq("tokenHash", hash(token))).unique();
    if (!row?.authUserId) throw new Error("Agent credential denied or revoked");
    await agentCredential(ctx, row.authUserId);
    return { userId: row.authUserId };
  },
});
export const claims = internalQuery({
  args: { userId: v.string() }, handler: async (ctx: Context, { userId }) => {
    const id = ctx.db.normalizeId("users", userId);
    const credential = id && await ctx.db.query("credentials").withIndex("by_auth_user", q => q.eq("authUserId", id)).unique();
    if (!credential) return {};
    const p = await agentCredential(ctx, userId);
    return { everett_agent: p.credential.agentId, everett_org: p.credential.subject };
  },
});
// Convex functions never see the caller's IP, so limits key on a client-chosen install key.
// A client rotating keys only reaches the global backstop, which counts unexpired rows only.
export const ENROLLMENT_LIMITS = { perClientPerMinute: 5, pendingPerClient: 3, pendingTotal: 5000 };
export const begin = action({
  args: { tokenHash: v.string(), name: v.string(), clientKey: v.string() }, handler: async (ctx, args): Promise<unknown> => {
    if (!/^[a-f0-9]{64}$/.test(args.tokenHash) || !/^[a-f0-9]{64}$/.test(args.clientKey) || !args.name.trim() || args.name.length > 100 || /[\x00-\x1f\x7f]/.test(args.name)) throw new Error("Invalid enrollment");
    ownerGithubId(); ownerGithubLogin();
    const deviceCode = random(), code = random().slice(0, 12).toUpperCase();
    const agentId = `agent-${random().slice(0, 32)}`;
    const expiresAt = Date.now() + 600_000;
    await ctx.runMutation(ref("createEnrollment"), { ...args, pollHash: hash(deviceCode), code, agentId, expiresAt });
    return { deviceCode, userCode: code, agentId, expiresAt, interval: 5,
      verificationUri: `${process.env.CONVEX_SITE_URL}/enroll` };
  },
});
export const createEnrollment = internalMutation({
  args: { tokenHash: v.string(), name: v.string(), clientKey: v.string(), pollHash: v.string(), code: v.string(), agentId: v.string(), expiresAt: v.number() },
  handler: async (ctx: WriteContext, args) => {
    const now = Date.now(), window = Math.floor(now / 60_000);
    const rate = await ctx.db.query("enrollmentRates").withIndex("by_client", q => q.eq("clientKey", args.clientKey)).unique();
    if (rate?.window === window && rate.count >= ENROLLMENT_LIMITS.perClientPerMinute) throw new Error("Enrollment rate limit");
    if (rate) await ctx.db.patch(rate._id, { window, count: rate.window === window ? rate.count + 1 : 1 });
    else await ctx.db.insert("enrollmentRates", { clientKey: args.clientKey, window, count: 1 });
    const mine = await ctx.db.query("enrollments").withIndex("by_client", q => q.eq("clientKey", args.clientKey).gt("expiresAt", now))
      .take(ENROLLMENT_LIMITS.pendingPerClient + 1);
    if (mine.filter(r => r.state === "pending").length >= ENROLLMENT_LIMITS.pendingPerClient) throw new Error("Too many pending enrollments for this client");
    const live = await ctx.db.query("enrollments").withIndex("by_expiry", q => q.gt("expiresAt", now)).take(ENROLLMENT_LIMITS.pendingTotal);
    if (live.length >= ENROLLMENT_LIMITS.pendingTotal) throw new Error("Too many pending enrollments");
    if (await ctx.db.query("enrollments").withIndex("by_code", q => q.eq("code", args.code)).unique()) throw new Error("Retry enrollment");
    await ctx.db.insert("enrollments", { ...args, state: "pending" });
  },
});
export const inspect = query({
  args: { code: v.string() }, handler: async (ctx: Context, { code }) => {
    await ownerIdentity(ctx);
    const row = await ctx.db.query("enrollments").withIndex("by_code", q => q.eq("code", code)).unique();
    if (!row || row.expiresAt <= Date.now()) throw new Error("Code expired or unknown");
    return { name: row.name, agentId: row.agentId, code: row.code, state: row.state };
  },
});
export const approve = mutation({
  args: { code: v.string(), approve: v.boolean() }, handler: async (ctx: WriteContext, args) => {
    const owner = await ownerIdentity(ctx);
    const row = await ctx.db.query("enrollments").withIndex("by_code", q => q.eq("code", args.code)).unique();
    if (!row || row.expiresAt <= Date.now() || row.state !== "pending") throw new Error("Code expired or already used");
    if (!args.approve) { await ctx.db.patch(row._id, { state: "denied" }); return null; }
    await ensureOwnerMembership(ctx, owner);
    if (await ctx.db.query("credentials").withIndex("by_agent", q => q.eq("issuer", owner.issuer).eq("agentId", row.agentId)).unique()) throw new Error("Agent ID already used");
    if (await ctx.db.query("credentials").withIndex("by_token", q => q.eq("tokenHash", row.tokenHash)).unique()) throw new Error("Agent token already bound");
    const authUserId = await ctx.db.insert("users", {});
    await ctx.db.insert("credentials", { owner: owner.owner, issuer: owner.issuer, subject: owner.userId,
      agentId: row.agentId, status: "active", authUserId, tokenHash: row.tokenHash, name: row.name });
    await ctx.db.patch(row._id, { state: "approved", owner: owner.owner });
    return null;
  },
});
export const poll = mutation({
  args: { deviceCode: v.string() }, handler: async (ctx: WriteContext, { deviceCode }) => {
    if (!/^[a-f0-9]{64}$/.test(deviceCode)) throw new Error("Invalid device code");
    const row = await ctx.db.query("enrollments").withIndex("by_poll", q => q.eq("pollHash", hash(deviceCode))).unique();
    if (!row || row.expiresAt <= Date.now()) return { state: "expired" };
    return { state: row.state, agentId: row.agentId, owner: row.owner ?? null };
  },
});
export const credentials = query({
  args: {}, handler: async (ctx: Context) => {
    const p = await ownerIdentity(ctx);
    const rows = await ctx.db.query("credentials").withIndex("by_owner", q => q.eq("owner", p.owner)).take(257);
    return { credentials: rows.slice(0, 256).map(r => ({ agentId: r.agentId, name: r.name ?? "", status: r.status, kind: r.kind ?? "device" })), truncated: rows.length > 256 };
  },
});
export const revoke = mutation({
  args: { agentId: v.string() }, handler: async (ctx: WriteContext, { agentId }) => {
    const p = await ownerIdentity(ctx);
    const row = await ctx.db.query("credentials").withIndex("by_agent", q => q.eq("issuer", p.issuer).eq("agentId", agentId)).unique();
    if (!row || row.owner !== p.owner) throw new Error("Unknown agent");
    await ctx.db.patch(row._id, { status: "revoked" });
    return null;
  },
});
export const revokeSelf = mutation({
  args: {}, handler: async (ctx: WriteContext) => {
    const { userId } = await hostedSession(ctx);
    const { credential } = await agentCredential(ctx, userId);
    await ctx.db.patch(credential._id, { status: "revoked" });
    return null;
  },
});
export const expire = internalMutation({
  args: {}, handler: async (ctx: WriteContext) => {
    const now = Date.now();
    const rows = await ctx.db.query("enrollments").withIndex("by_expiry", q => q.lte("expiresAt", now)).take(500);
    for (const row of rows) await ctx.db.delete(row._id);
    const rates = await ctx.db.query("enrollmentRates").withIndex("by_window", q => q.lt("window", Math.floor(now / 60_000))).take(500);
    for (const row of rates) await ctx.db.delete(row._id);
  },
});
