import type { DataModelFromSchemaDefinition, GenericQueryCtx, GenericMutationCtx } from "convex/server";
import schema from "./schema";
import { identifier, type Device } from "./protocol";

export type Model = DataModelFromSchemaDefinition<typeof schema>;
export type Context = GenericQueryCtx<Model>;
export type WriteContext = GenericMutationCtx<Model>;

export function namespace(issuer: string, subject: string, org: string | null) {
  return JSON.stringify([issuer, org === null ? "account" : "org", org ?? subject]);
}

export async function authorize(ctx: Context) {
  // Convex validates OIDC issuer, signature, audience and expiry before exposing this identity.
  const identity = await ctx.auth.getUserIdentity();
  if (!identity) throw new Error("Sign in required");
  const agentId = identity.everett_agent;
  const org = identity.everett_org;
  if (typeof agentId !== "string") throw new Error("Issuer-controlled everett_agent required");
  identifier(agentId);
  if (org !== undefined && (typeof org !== "string" || org.length === 0 || org.length > 160)) {
    throw new Error("Invalid organization claim");
  }
  const owner = namespace(identity.issuer, identity.subject, org === undefined ? null : org as string);
  const member = await ctx.db.query("memberships").withIndex("by_identity", (q) =>
    q.eq("owner", owner).eq("issuer", identity.issuer).eq("subject", identity.subject)).unique();
  if (!member || member.status !== "active") throw new Error("Organization membership denied");
  const credential = await ctx.db.query("credentials").withIndex("by_agent", (q) =>
    q.eq("issuer", identity.issuer).eq("agentId", agentId)).unique();
  if (!credential || credential.status !== "active" || credential.owner !== owner || credential.subject !== identity.subject) {
    throw new Error("Agent credential denied or revoked");
  }
  return { owner, agentId, member, credential, admin: member.role === "admin" && identity.everett_role === "admin" };
}
export type Principal = Awaited<ReturnType<typeof authorize>>;
export function requireAdmin(p: Principal) {
  if (!p.admin) throw new Error("Organization admin required");
}
export function ownAgent(p: Principal, deviceId: string) {
  if (deviceId !== p.agentId) throw new Error("JWT agent does not match device");
}
export async function activeDevice(ctx: Context, owner: string, deviceId: string) {
  identifier(deviceId);
  const row = await ctx.db.query("devices").withIndex("by_owner_device", (q) =>
    q.eq("owner", owner).eq("deviceId", deviceId)).unique();
  if (!row) throw new Error("Device is not registered");
  // Read both authoritative rows on EVERY access, including subscriptions to other agents.
  const parts: unknown = JSON.parse(owner);
  if (!Array.isArray(parts) || typeof parts[0] !== "string") throw new Error("Invalid owner namespace");
  const credential = await ctx.db.query("credentials").withIndex("by_agent", (q) =>
    q.eq("issuer", parts[0] as string).eq("agentId", deviceId)).unique();
  if (!credential || credential.owner !== owner || credential.status !== "active") throw new Error("Agent credential revoked");
  const member = await ctx.db.query("memberships").withIndex("by_identity", (q) =>
    q.eq("owner", owner).eq("issuer", credential.issuer).eq("subject", credential.subject)).unique();
  if (!member || member.status !== "active") throw new Error("Organization membership denied");
  return row;
}
export function bundle(d: Device): Device {
  return { owner: d.owner, deviceId: d.deviceId, signingKey: d.signingKey, encryptionKey: d.encryptionKey, signature: d.signature };
}
