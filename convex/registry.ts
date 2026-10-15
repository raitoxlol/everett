import { queryGeneric as query, mutationGeneric as mutation } from "convex/server";
import { v } from "convex/values";
import { authorize, ownAgent, activeDevice, bundle, type Context, type WriteContext } from "./authorization";
import { encode, identifier, verify } from "./protocol";

export function metadataBytes(owner: string, deviceId: string, name: string, sshAddress: string | null) {
  return encode(["everett-registry-v2", owner, deviceId, name, sshAddress]);
}

export async function named(ctx: Context, owner: string, name: string) {
  const row = await ctx.db.query("devices").withIndex("by_owner_name", q => q.eq("owner", owner).eq("name", name)).unique();
  const legacy = await ctx.db.query("devices").withIndex("by_owner_device", q => q.eq("owner", owner).eq("deviceId", name)).unique();
  if (row && legacy && legacy.name === undefined && row._id !== legacy._id) throw new Error("Device name collision");
  return row ?? (legacy?.name === undefined ? legacy : null);
}

function view(row: Awaited<ReturnType<typeof activeDevice>>) {
  return {
    ...bundle(row), name: row.name ?? row.deviceId, sshAddress: row.sshAddress ?? null,
    lastSeenAt: row.lastSeenAt ?? null, metadataSignature: row.metadataSignature ?? null,
    ...("revokedAt" in row ? { revokedAt: row.revokedAt } : {}),
  };
}

export const publish = mutation({
  args: { deviceId: v.string(), name: v.string(), sshAddress: v.union(v.null(), v.string()), signature: v.string() },
  handler: async (ctx: WriteContext, args) => {
    const p = await authorize(ctx); ownAgent(p, args.deviceId);
    const row = await activeDevice(ctx, p.owner, p.agentId);
    identifier(args.name);
    if (args.sshAddress !== null && (args.sshAddress.length > 512 || args.sshAddress.startsWith("-") ||
        !/^[A-Za-z0-9_.-]+@(?:[A-Za-z0-9_.-]+|\[[0-9a-fA-F:]+\])(?::[0-9]{1,5})?$/.test(args.sshAddress) ||
        /:0+$/.test(args.sshAddress) || Number(args.sshAddress.match(/:(\d+)$/)?.[1] ?? 22) > 65535)) {
      throw new Error("Invalid SSH address; use user@host[:port]");
    }
    verify(row.signingKey, args.signature, metadataBytes(p.owner, p.agentId, args.name, args.sshAddress));
    const existing = await named(ctx, p.owner, args.name);
    if (existing && existing._id !== row._id) throw new Error(`Device name '${args.name}' is already in use in this account`);
    await ctx.db.patch(row._id, { name: args.name, sshAddress: args.sshAddress,
      metadataSignature: args.signature, lastSeenAt: Date.now() });
    return null;
  },
});

export const devices = query({
  args: { cursor: v.union(v.null(), v.string()) },
  handler: async (ctx: Context, args) => {
    const p = await authorize(ctx);
    const page = await ctx.db.query("devices").withIndex("by_owner_device", q => q.eq("owner", p.owner))
      .paginate({ cursor: args.cursor, numItems: 128 });
    return { devices: page.page.map(view), isDone: page.isDone, continueCursor: page.continueCursor };
  },
});

export const resolve = query({
  args: { name: v.string() },
  handler: async (ctx: Context, args) => {
    const p = await authorize(ctx); identifier(args.name);
    const row = await named(ctx, p.owner, args.name);
    if (!row) return null;
    await activeDevice(ctx, p.owner, row.deviceId);
    if ("revokedAt" in row && row.revokedAt !== null) throw new Error("Device is revoked");
    return view(row);
  },
});
