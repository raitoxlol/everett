import {
  queryGeneric as query, mutationGeneric as mutation,
  internalMutationGeneric as internalMutation,
  makeFunctionReference,
  type GenericQueryCtx, type GenericMutationCtx,
  type DataModelFromSchemaDefinition,
} from "convex/server";
import { v, type Infer } from "convex/values";
import nacl from "tweetnacl";
import { sha256 } from "@noble/hashes/sha256";
import schema, { deviceFields, envelopeFields } from "./schema";

type Model = DataModelFromSchemaDefinition<typeof schema>;
type Context = GenericQueryCtx<Model>;
type WriteContext = GenericMutationCtx<Model>;
const deviceValidator = v.object(deviceFields);
const envelopeValidator = v.object(envelopeFields);
type Device = Infer<typeof deviceValidator>;
type Envelope = Infer<typeof envelopeValidator>;
const encode = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));

export function deviceBytes(d: Device) {
  return encode(["everett-device-v1", d.owner, d.deviceId, d.signingKey, d.encryptionKey]);
}

export function envelopeBytes(e: Envelope) {
  const aad = JSON.stringify([
    "everett-relay-v1", e.owner, e.fromDevice, e.toDevice, e.messageId,
    e.idempotencyKey, e.createdAt, e.expiresAt, e.ephemeralKey, e.nonce,
  ]);
  return encode([aad, e.ciphertext]);
}

function hex(value: string, bytes?: number) {
  if (!/^(?:[0-9a-f]{2})+$/.test(value) || (bytes !== undefined && value.length !== bytes * 2)) {
    throw new Error("Invalid cryptographic field");
  }
  return Uint8Array.from(value.match(/../g)!.map((b) => parseInt(b, 16)));
}

function verify(key: string, signature: string, bytes: Uint8Array) {
  if (!nacl.sign.detached.verify(bytes, hex(signature, 64), hex(key, 32))) {
    throw new Error("Invalid device signature");
  }
}

function identifier(value: string) {
  if (!/^[A-Za-z0-9_.-]{1,160}$/.test(value)) throw new Error("Invalid identifier");
}

async function owner(ctx: Context) {
  const identity = await ctx.auth.getUserIdentity();
  if (!identity) throw new Error("Sign in required");
  return JSON.stringify([identity.issuer, identity.subject]);
}

async function device(ctx: Context, account: string, id: string) {
  identifier(id);
  const row = await ctx.db.query("devices")
    .withIndex("by_owner_device", (q) => q.eq("owner", account).eq("deviceId", id)).unique();
  if (!row) throw new Error("Device is not registered to this owner");
  return row;
}

export const register = mutation({
  args: { device: deviceValidator },
  handler: async (ctx: WriteContext, { device: d }) => {
    const account = await owner(ctx);
    if (d.owner !== account) throw new Error("Wrong account");
    identifier(d.deviceId);
    hex(d.encryptionKey, 32);
    verify(d.signingKey, d.signature, deviceBytes(d));
    const existing = await ctx.db.query("devices")
      .withIndex("by_owner_device", (q) => q.eq("owner", account).eq("deviceId", d.deviceId)).unique();
    if (existing) {
      if (existing.signingKey !== d.signingKey || existing.encryptionKey !== d.encryptionKey) {
        throw new Error("Device keys cannot be replaced; use a new device identity and confirm new pins");
      }
    } else {
      await ctx.db.insert("devices", d);
    }
    return null;
  },
});

export const devices = query({
  args: {},
  handler: async (ctx: Context) => {
    const account = await owner(ctx);
    const rows = await ctx.db.query("devices")
      .withIndex("by_owner_device", (q) => q.eq("owner", account)).take(256);
    return rows.map(({ owner, deviceId, signingKey, encryptionKey, signature }) =>
      ({ owner, deviceId, signingKey, encryptionKey, signature }));
  },
});

export const enqueue = mutation({
  args: { envelope: envelopeValidator },
  handler: async (ctx: WriteContext, { envelope: e }) => {
    const account = await owner(ctx);
    if (e.owner !== account) throw new Error("Wrong account");
    identifier(e.messageId); identifier(e.idempotencyKey);
    const sender = await device(ctx, account, e.fromDevice);
    await device(ctx, account, e.toDevice);
    hex(e.ephemeralKey, 32); hex(e.nonce, 12);
    if (e.ciphertext.length > (65536 + 16) * 2) throw new Error("Body cap exceeded");
    hex(e.ciphertext);
    for (const timestamp of [e.createdAt, e.expiresAt]) {
      if (!Number.isSafeInteger(timestamp)) throw new Error("Invalid timestamp");
    }
    verify(sender.signingKey, e.signature, envelopeBytes(e));
    const digest = Array.from(sha256(envelopeBytes(e)), (b) => b.toString(16).padStart(2, "0")).join("");
    const previous = await ctx.db.query("attempts")
      .withIndex("by_key", (q) => q.eq("owner", account).eq("fromDevice", e.fromDevice).eq("idempotencyKey", e.idempotencyKey)).unique();
    if (previous) {
      if (previous.digest !== digest) throw new Error("Idempotency key already used for a different request");
      return { messageId: previous.messageId, state: previous.state === "queued" && previous.expiresAt <= Date.now() ? "expired" : previous.state };
    }
    if (e.expiresAt <= Date.now() || e.createdAt > Date.now() + 300000 || e.expiresAt <= e.createdAt || e.expiresAt - e.createdAt > 86400000) {
      throw new Error("Expired or invalid TTL");
    }
    if (await ctx.db.query("attempts").withIndex("by_message", (q) => q.eq("owner", account).eq("messageId", e.messageId)).unique()) {
      throw new Error("Message ID already used");
    }
    await ctx.db.insert("messages", e);
    await ctx.db.insert("attempts", {
      owner: account, fromDevice: e.fromDevice, toDevice: e.toDevice,
      messageId: e.messageId, idempotencyKey: e.idempotencyKey, digest,
      expiresAt: e.expiresAt, retainUntil: e.expiresAt + 7 * 86400000, state: "queued",
    });
    return { messageId: e.messageId, state: "queued" };
  },
});

export const pending = query({
  args: { deviceId: v.string() },
  handler: async (ctx: Context, { deviceId }) => {
    const account = await owner(ctx);
    await device(ctx, account, deviceId);
    const rows = await ctx.db.query("messages")
      .withIndex("by_recipient", (q) => q.eq("owner", account).eq("toDevice", deviceId))
      .filter((q) => q.gt(q.field("expiresAt"), Date.now())).take(64);
    return rows.map(({ _id, _creationTime, ...envelope }) => envelope);
  },
});

export const accept = mutation({
  args: { deviceId: v.string(), messageId: v.string(), at: v.number(), signature: v.string() },
  handler: async (ctx: WriteContext, args) => {
    const account = await owner(ctx);
    const recipient = await device(ctx, account, args.deviceId);
    if (!Number.isSafeInteger(args.at) || Math.abs(Date.now() - args.at) > 300000) throw new Error("Stale acknowledgement");
    verify(recipient.signingKey, args.signature, encode(["everett-accept-v1", account, args.deviceId, args.messageId, args.at]));
    const attempt = await ctx.db.query("attempts")
      .withIndex("by_message", (q) => q.eq("owner", account).eq("messageId", args.messageId)).unique();
    if (!attempt || attempt.toDevice !== args.deviceId) throw new Error("No message for this device");
    if (attempt.state !== "accepted" && attempt.expiresAt <= Date.now()) throw new Error("Message TTL expired");
    if (attempt.state !== "accepted") {
      await ctx.db.patch(attempt._id, { state: "accepted" });
    }
    const message = await ctx.db.query("messages")
      .withIndex("by_message", (q) => q.eq("owner", account).eq("messageId", args.messageId)).unique();
    if (message) await ctx.db.delete(message._id);
    return null;
  },
});

export const status = query({
  args: { fromDevice: v.string(), messageId: v.string() },
  handler: async (ctx: Context, { fromDevice, messageId }) => {
    const account = await owner(ctx);
    await device(ctx, account, fromDevice);
    const attempt = await ctx.db.query("attempts")
      .withIndex("by_message", (q) => q.eq("owner", account).eq("messageId", messageId)).unique();
    if (!attempt || attempt.fromDevice !== fromDevice) throw new Error("No message for this device");
    return { state: attempt.state === "queued" && attempt.expiresAt <= Date.now() ? "expired" : attempt.state };
  },
});

export const expire = internalMutation({
  args: {},
  handler: async (ctx: WriteContext) => {
    const expired = await ctx.db.query("messages")
      .withIndex("by_expiry", (q) => q.lte("expiresAt", Date.now())).take(64);
    for (const message of expired) {
      const attempt = await ctx.db.query("attempts")
        .withIndex("by_message", (q) => q.eq("owner", message.owner).eq("messageId", message.messageId)).unique();
      if (attempt?.state === "queued") await ctx.db.patch(attempt._id, { state: "expired" });
      await ctx.db.delete(message._id);
    }
    const retained = await ctx.db.query("attempts")
      .withIndex("by_retention", (q) => q.lte("retainUntil", Date.now())).take(256);
    for (const attempt of retained) await ctx.db.delete(attempt._id);
    if (expired.length === 64 || retained.length === 256) {
      await ctx.scheduler.runAfter(0, makeFunctionReference<"mutation">("relay:expire"), {});
    }
    return null;
  },
});
