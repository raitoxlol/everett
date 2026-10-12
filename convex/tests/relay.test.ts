import { convexTest } from "convex-test";
import { makeFunctionReference } from "convex/server";
import { afterEach, describe, expect, test, vi } from "vitest";
import nacl from "tweetnacl";
import schema from "../schema";
import { deviceBytes, envelopeBytes } from "../relay";
import wire from "../../everett-rs/tests/fixtures/relay-wire.json";

const modules = {
  "../_generated/server.ts": async () => ({}),
  "../relay.ts": () => import("../relay"),
  "../agents.ts": () => import("../agents"),
};
const ref = (name: string) => makeFunctionReference<"mutation">(`relay:${name}`);
const query = (name: string) => makeFunctionReference<"query">(`relay:${name}`);
const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");
const issuer = "https://owner.example/";
process.env.OIDC_ISSUER = issuer;
const owner = JSON.stringify([issuer, "alice"]);
const aliceIdentity = { issuer, subject: "alice" };

function key(id: string, account = owner) {
  const pair = nacl.sign.keyPair();
  const device = {
    owner: account, deviceId: id, signingKey: hex(pair.publicKey),
    encryptionKey: hex(nacl.randomBytes(32)), signature: "",
  };
  device.signature = hex(nacl.sign.detached(deviceBytes(device), pair.secretKey));
  return { device, pair };
}

async function fixture(identity: typeof aliceIdentity & { org_id?: string } = aliceIdentity) {
  const backend = convexTest(schema, modules);
  const alice = backend.withIdentity(identity);
  const bob = backend.withIdentity({ issuer, subject: "bob" });
  const a = key("device-a"), b = key("device-b");
  await alice.mutation(ref("register"), { device: a.device });
  await alice.mutation(ref("register"), { device: b.device });
  const envelope = {
    owner, fromDevice: a.device.deviceId, toDevice: b.device.deviceId,
    messageId: "message-1", idempotencyKey: "request-1", createdAt: Date.now(),
    expiresAt: Date.now() + 3600000, ephemeralKey: hex(nacl.randomBytes(32)),
    nonce: hex(nacl.randomBytes(12)), ciphertext: hex(nacl.randomBytes(32)), signature: "",
  };
  const sign = () => {
    envelope.signature = hex(nacl.sign.detached(envelopeBytes(envelope), a.pair.secretKey));
  };
  sign();
  const acknowledgement = () => {
    const at = Date.now();
    return {
      deviceId: b.device.deviceId, messageId: envelope.messageId, at,
      signature: hex(nacl.sign.detached(new TextEncoder().encode(JSON.stringify([
        "everett-accept-v1", owner, b.device.deviceId, envelope.messageId, at,
      ])), b.pair.secretKey)),
    };
  };
  const accept = () => alice.mutation(ref("accept"), acknowledgement());
  return { backend, alice, bob, a, b, envelope, sign, accept, acknowledgement };
}

type Fixture = Awaited<ReturnType<typeof fixture>>;

async function expectOwnerCallsRejected(f: Fixture, client: Fixture["alice"], reason: string) {
  for (const [name, args] of [
    ["devices", {}],
    ["pending", { deviceId: f.b.device.deviceId }],
    ["status", { fromDevice: f.a.device.deviceId, messageId: f.envelope.messageId }],
  ] as const) {
    await expect(client.query(query(name), args)).rejects.toThrow(reason);
  }
  for (const [name, args] of [
    ["register", { device: f.a.device }],
    ["enqueue", { envelope: f.envelope }],
    ["accept", f.acknowledgement()],
  ] as const) {
    await expect(client.mutation(ref(name), args)).rejects.toThrow(reason);
  }
}

afterEach(() => vi.useRealTimers());

describe("real Convex functions against an in-memory backend (no network)", () => {
  test("Ed25519 device and envelope bytes match the shared Rust/JavaScript vector", () => {
    const pair = nacl.sign.keyPair.fromSeed(new Uint8Array(32).fill(1));
    expect(hex(nacl.sign.detached(deviceBytes(wire.device), pair.secretKey))).toBe(wire.device.signature);
    expect(hex(nacl.sign.detached(envelopeBytes(wire.envelope), pair.secretKey))).toBe(wire.envelope.signature);
  });
  test("requires auth and isolates all reads and writes by JWT issuer/subject", async () => {
    const f = await fixture();
    await expect(f.backend.query(query("devices"), {})).rejects.toThrow("Sign in");
    await expect(f.bob.mutation(ref("register"), { device: f.a.device })).rejects.toThrow("Wrong account");
    await expect(f.bob.mutation(ref("enqueue"), { envelope: f.envelope })).rejects.toThrow("Wrong account");
    await expect(f.bob.query(query("pending"), { deviceId: f.b.device.deviceId })).rejects.toThrow("not registered");
    expect(await f.bob.query(query("devices"), {})).toEqual([]);
    await f.alice.mutation(ref("enqueue"), { envelope: f.envelope });
    await expect(f.bob.query(query("status"), { fromDevice: f.a.device.deviceId, messageId: f.envelope.messageId })).rejects.toThrow();
    await expect(f.bob.mutation(ref("accept"), { deviceId: f.b.device.deviceId, messageId: f.envelope.messageId, at: Date.now(), signature: "00".repeat(64) })).rejects.toThrow();
    const sameSubjectOtherIssuer = f.backend.withIdentity({ issuer: "https://different.example/", subject: "alice" });
    await expect(sameSubjectOtherIssuer.query(query("devices"), {})).rejects.toThrow("issuer");
  });

  test("every owner relay entry point rejects missing auth, wrong issuer and agent tokens", async () => {
    const f = await fixture();
    await f.alice.mutation(ref("enqueue"), { envelope: f.envelope });
    await expectOwnerCallsRejected(f, f.backend, "Sign in");
    await expectOwnerCallsRejected(f, f.backend.withIdentity({ ...aliceIdentity, issuer: "https://evil.example/" }), "issuer");
    await expectOwnerCallsRejected(f, f.backend.withIdentity({ ...aliceIdentity, org_id: "team-a", agent_id: "agent-a", jti: "credential-a" }), "scoped agent relay");
    expect(await f.alice.query(query("pending"), { deviceId: f.b.device.deviceId })).toEqual([f.envelope]);
    expect(await f.alice.query(query("status"), { fromDevice: f.a.device.deviceId, messageId: f.envelope.messageId })).toEqual({ state: "queued" });
  });

  test("a signed org change cannot expose the same subject's personal devices or queued messages", async () => {
    const f = await fixture();
    await f.alice.mutation(ref("enqueue"), { envelope: f.envelope });
    const otherOrg = f.backend.withIdentity({ ...aliceIdentity, org_id: "team-a" });
    expect(await otherOrg.query(query("devices"), {})).toEqual([]);
    for (const [name, args] of [
      ["pending", { deviceId: f.b.device.deviceId }],
      ["status", { fromDevice: f.a.device.deviceId, messageId: f.envelope.messageId }],
    ] as const) await expect(otherOrg.query(query(name), args)).rejects.toThrow("org membership");
    for (const [name, args] of [
      ["register", { device: f.a.device }],
      ["enqueue", { envelope: f.envelope }],
      ["accept", f.acknowledgement()],
    ] as const) await expect(otherOrg.mutation(ref(name), args)).rejects.toThrow("org membership");
    expect(await f.alice.query(query("pending"), { deviceId: f.b.device.deviceId })).toEqual([f.envelope]);
    await f.accept();
    expect(await f.backend.run(ctx => ctx.db.query("messages").collect())).toEqual([]);
  });

  test("membership revocation gates every owner relay entry point even for a still-signed token", async () => {
    const f = await fixture({ ...aliceIdentity, org_id: "team-a" });
    await f.alice.mutation(ref("enqueue"), { envelope: f.envelope });
    const membership = (await f.backend.run(ctx => ctx.db.query("memberships").collect()))[0];
    const admin = f.backend.withIdentity({ issuer, subject: "admin-a", org_id: "team-a", org_role: "admin" });
    await admin.mutation(makeFunctionReference<"mutation">("agents:revokeMembership"), { membershipId: membership._id });
    await expectOwnerCallsRejected(f, f.alice, "membership revoked");
    expect((await f.backend.run(ctx => ctx.db.query("messages").collect()))[0]).toMatchObject(f.envelope);
    expect((await f.backend.run(ctx => ctx.db.query("attempts").collect()))[0].state).toBe("queued");
  });

  test("idempotency survives acceptance and rejects a changed envelope", async () => {
    const f = await fixture();
    const first = await f.alice.mutation(ref("enqueue"), { envelope: f.envelope });
    expect(await f.alice.mutation(ref("enqueue"), { envelope: f.envelope })).toEqual(first);
    expect(await f.backend.run((ctx) => ctx.db.query("messages").collect())).toHaveLength(1);
    await f.accept();
    expect(await f.alice.mutation(ref("enqueue"), { envelope: f.envelope })).toMatchObject({ state: "accepted" });
    f.envelope.nonce = "00".repeat(12); f.sign();
    await expect(f.alice.mutation(ref("enqueue"), { envelope: f.envelope })).rejects.toThrow("Idempotency");
  });

  test("signed acceptance deletes ciphertext but does not claim inbox pickup", async () => {
    const f = await fixture();
    await f.alice.mutation(ref("enqueue"), { envelope: f.envelope });
    expect(await f.alice.query(query("pending"), { deviceId: f.b.device.deviceId })).toEqual([f.envelope]);
    await f.accept(); await f.accept();
    expect(await f.alice.query(query("pending"), { deviceId: f.b.device.deviceId })).toEqual([]);
    expect(await f.backend.run((ctx) => ctx.db.query("messages").collect())).toEqual([]);
    const records = await f.backend.run((ctx) => ctx.db.query("attempts").collect());
    expect(records[0].state).toBe("accepted");
    expect(records[0]).not.toHaveProperty("ciphertext");
    expect(records[0]).not.toHaveProperty("text");
  });

  test("offline queue expires and cron deletes rows and eventually idempotency metadata", async () => {
    vi.useFakeTimers();
    const f = await fixture();
    await f.alice.mutation(ref("enqueue"), { envelope: f.envelope });
    vi.setSystemTime(f.envelope.expiresAt + 1);
    expect(await f.alice.query(query("pending"), { deviceId: f.b.device.deviceId })).toEqual([]);
    expect(await f.alice.query(query("status"), { fromDevice: f.a.device.deviceId, messageId: f.envelope.messageId })).toEqual({ state: "expired" });
    await f.backend.mutation(ref("expire"), {});
    expect(await f.backend.run((ctx) => ctx.db.query("messages").collect())).toEqual([]);
    expect(await f.alice.mutation(ref("enqueue"), { envelope: f.envelope })).toMatchObject({ state: "expired" });
    vi.setSystemTime(f.envelope.expiresAt + 8 * 86400000);
    await f.backend.mutation(ref("expire"), {});
    expect(await f.backend.run((ctx) => ctx.db.query("attempts").collect())).toEqual([]);
  });

  test("rejects tampered signatures, device replacement, unregistered routing, oversized bodies and TTL", async () => {
    const f = await fixture();
    f.envelope.ciphertext = "00".repeat(32);
    await expect(f.alice.mutation(ref("enqueue"), { envelope: f.envelope })).rejects.toThrow("signature");
    f.sign(); f.envelope.expiresAt = Date.now() - 1; f.sign();
    await expect(f.alice.mutation(ref("enqueue"), { envelope: f.envelope })).rejects.toThrow("TTL");
    f.envelope.expiresAt = Date.now() + 86400001; f.sign();
    await expect(f.alice.mutation(ref("enqueue"), { envelope: f.envelope })).rejects.toThrow("TTL");
    f.envelope.expiresAt = Date.now() + 3600000;
    f.envelope.ciphertext = "00".repeat(65553); f.sign();
    await expect(f.alice.mutation(ref("enqueue"), { envelope: f.envelope })).rejects.toThrow("cap");
    const replacement = key(f.a.device.deviceId);
    await expect(f.alice.mutation(ref("register"), { device: replacement.device })).rejects.toThrow("cannot be replaced");
    f.envelope.toDevice = "unknown"; f.sign();
    await expect(f.alice.mutation(ref("enqueue"), { envelope: f.envelope })).rejects.toThrow("not registered");
  });
});
