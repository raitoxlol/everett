import type { GenericQueryCtx, GenericMutationCtx, DataModelFromSchemaDefinition } from "convex/server";
import { sha256 } from "@noble/hashes/sha256";
import schema from "./schema";

export type Model = DataModelFromSchemaDefinition<typeof schema>;
export type Context = GenericQueryCtx<Model>;
export type WriteContext = GenericMutationCtx<Model>;
export const hash = (value: string) => Array.from(sha256(new TextEncoder().encode(value)), b => b.toString(16).padStart(2, "0")).join("");

function claim(value: unknown, name: string): string {
  if (typeof value !== "string" || !value.length || value.length > 160) throw new Error(`Missing or invalid issuer claim: ${name}`);
  return value;
}

export async function principal(ctx: Context) {
  const identity = await ctx.auth.getUserIdentity();
  if (!identity) throw new Error("Sign in required");
  if (!process.env.OIDC_ISSUER || identity.issuer !== process.env.OIDC_ISSUER) throw new Error("Untrusted OIDC issuer");
  const owner = JSON.stringify([identity.issuer, identity.subject]);
  const organization = identity.org_id === undefined ? ["personal", identity.subject] : ["org", claim(identity.org_id, "org_id")];
  const org = hash(JSON.stringify([identity.issuer, organization]));
  const agent = identity.agent_id === undefined ? null : hash(JSON.stringify([identity.issuer, identity.subject, claim(identity.agent_id, "agent_id")]));
  if (agent && identity.org_id === undefined) throw new Error("Issuer org_id required for agents");
  const credential = agent ? hash(JSON.stringify([identity.issuer, identity.subject, claim(identity.jti, "jti")])) : null;
  const admin = !agent && identity.org_id !== undefined && identity.org_role === "admin";
  const membership = await ctx.db.query("memberships").withIndex("by_identity", q => q.eq("org", org).eq("owner", owner)).unique();
  if (membership?.revoked) throw new Error("Org membership revoked");
  return { owner, org, agent, credential, admin };
}

export async function remember(ctx: WriteContext, p: Awaited<ReturnType<typeof principal>>) {
  const existing = await ctx.db.query("memberships").withIndex("by_identity", q => q.eq("org", p.org).eq("owner", p.owner)).unique();
  if (!existing) await ctx.db.insert("memberships", { owner: p.owner, org: p.org, revoked: false });
}

export async function limit(ctx: WriteContext, owner: string, org: string, key: string, maximum: number, bucket = owner) {
  const window = Math.floor(Date.now() / 60000);
  const row = await ctx.db.query("rateLimits").withIndex("by_key", q => q.eq("org", org).eq("bucket", bucket).eq("key", key)).unique();
  const count = row?.window === window ? row.count : 0;
  if (count >= maximum) throw new Error("Relay rate limit exceeded");
  const value = { window, count: count + 1, expiresAt: (window + 2) * 60000 };
  if (row) await ctx.db.patch(row._id, value);
  else await ctx.db.insert("rateLimits", { owner, org, bucket, key, ...value });
}
