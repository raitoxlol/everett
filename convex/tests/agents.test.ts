import { convexTest } from "convex-test";
import { makeFunctionReference } from "convex/server";
import { afterEach, expect, test, vi } from "vitest";
import nacl from "tweetnacl";
import schema from "../schema";
import { deviceBytes } from "../relay";
import { envelopeBytes } from "../agents";
import wire from "../../everett-rs/tests/fixtures/relay-wire.json";

const issuer = "https://owner.example/";
process.env.OIDC_ISSUER = issuer;
const modules = { "../_generated/server.ts": async () => ({}), "../relay.ts": () => import("../relay"), "../agents.ts": () => import("../agents") };
const m = (name: string) => makeFunctionReference<"mutation">(`agents:${name}`);
const q = (name: string) => makeFunctionReference<"query">(`agents:${name}`);
const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");
const token = (subject: string, org_id: string, extra = {}) => ({ issuer, subject, org_id, ...extra });

async function fixture() {
  const db = convexTest(schema, modules);
  const adminA = db.withIdentity(token("admin-a", "a", { org_role: "admin" }));
  const adminB = db.withIdentity(token("admin-b", "b", { org_role: "admin" }));
  const member = db.withIdentity(token("member", "a"));
  async function register(subject: string, org: string, agent_id = subject) {
    const claims = token(subject, org, { agent_id, jti: `${subject}-credential` });
    const client = db.withIdentity(claims);
    const pair = nacl.sign.keyPair();
    const device = { owner: JSON.stringify([issuer, subject]), deviceId: `${subject}-device`, signingKey: hex(pair.publicKey), encryptionKey: hex(nacl.randomBytes(32)), signature: "" };
    device.signature = hex(nacl.sign.detached(deviceBytes(device), pair.secretKey));
    const a = await client.mutation(m("register"), { device }) as { owner: string; org: string; agentId: string; credentialId: string };
    return { client, pair, device, ...a, claims };
  }
  const a = await register("agent-a", "a"), b = await register("agent-b", "b"), c = await register("agent-c", "a");
  const e = { owner: a.owner, fromDevice: a.device.deviceId, toDevice: b.device.deviceId,
    messageId: "message", idempotencyKey: "key", createdAt: Date.now(), expiresAt: Date.now() + 3600000,
    ephemeralKey: hex(nacl.randomBytes(32)), nonce: hex(nacl.randomBytes(12)), ciphertext: hex(nacl.randomBytes(32)), signature: "",
    scope: { fromOwner: a.owner, toOwner: b.owner, fromOrg: a.org, toOrg: b.org, fromAgent: a.agentId, toAgent: b.agentId } };
  const sign = () => e.signature = hex(nacl.sign.detached(envelopeBytes(e), a.pair.secretKey));
  sign();
  async function grant() {
    const grantId = await adminA.mutation(m("propose"), { localAgent: a.agentId, remoteAgent: b.agentId });
    await adminA.mutation(m("decide"), { grantId, approve: true });
    await adminB.mutation(m("decide"), { grantId, approve: true });
    return grantId;
  }
  const accept = (client = b.client, recipient = b) => {
    const at = Date.now();
    const signature = hex(nacl.sign.detached(new TextEncoder().encode(JSON.stringify(["everett-agent-accept-v2", recipient.org, recipient.agentId, a.org, e.messageId, at])), recipient.pair.secretKey));
    return client.mutation(m("accept"), { fromOrg: a.org, messageId: e.messageId, at, signature });
  };
  return { db, adminA, adminB, member, register, a, b, c, e, sign, grant, accept };
}

afterEach(() => vi.useRealTimers());

test("agent v2 signed bytes match the Rust wire vector", () => {
  const e = {...wire.envelope,scope:{fromOwner:wire.envelope.owner,toOwner:JSON.stringify([issuer,"bob"]),fromOrg:"a".repeat(64),toOrg:"b".repeat(64),fromAgent:"1".repeat(64),toAgent:"2".repeat(64)}};
  expect(hex(nacl.sign.detached(envelopeBytes(e),nacl.sign.keyPair.fromSeed(new Uint8Array(32).fill(1)).secretKey))).toBe("e847093718c3d7590679ca9186e3b2cd7bba2be00a0969ba7703aa43ddb115d8f7ae326c174b289b6b2e838598af81bca5bc524de259fa664ab16e9e1373bb05");
});

test("all public boundaries fail closed without OIDC, wrong issuer and invented client identity", async () => {
  const f = await fixture();
  for (const [name, args] of [["identity", {}], ["directory", {}], ["credentials", {}], ["memberships", {}], ["grants", {}], ["pending", {}], ["status", { messageId: "message" }]] as const) {
    await expect(f.db.query(q(name), args)).rejects.toThrow("Sign in");
    await expect(f.db.withIdentity({ ...f.a.claims, issuer: "https://evil.example/" }).query(q(name), args)).rejects.toThrow("issuer");
  }
  const membershipId = (await f.db.run(ctx => ctx.db.query("memberships").collect()))[0]._id;
  for (const [name, args] of [
    ["register", { device: f.a.device }], ["enqueue", { envelope: f.e }], ["accept", { fromOrg: f.a.org, messageId: "message", at: Date.now(), signature: "00".repeat(64) }],
    ["propose", { localAgent: f.a.agentId, remoteAgent: f.b.agentId }], ["decide", { grantId: "invalid", approve: true }],
    ["revoke", { agentId: f.a.agentId }], ["revokeCredential", { credentialId: f.a.credentialId }], ["revokeMembership", { membershipId }],
  ]) {
    await expect(f.db.mutation(m(name as string), args as Record<string, unknown>)).rejects.toThrow();
    await expect(f.db.withIdentity({...f.a.claims,issuer:"https://evil.example/"}).mutation(m(name as string), args as Record<string, unknown>)).rejects.toThrow();
  }
  await expect(f.a.client.query(q("pending"), { org: f.b.org, agentId: f.b.agentId })).rejects.toThrow();
  await expect(f.a.client.mutation(m("register"), { device: f.b.device })).rejects.toThrow("Wrong account");
  await expect(f.db.withIdentity(token("agent-no-jti", "a", { agent_id: "x" })).query(q("identity"), {})).rejects.toThrow("jti");
  await expect(f.db.withIdentity({ issuer, subject: "no-org", agent_id: "x", jti: "x" }).query(q("identity"), {})).rejects.toThrow("org_id");
  await expect(f.a.client.query(makeFunctionReference<"query">("relay:devices"), {})).rejects.toThrow("scoped agent relay");
});

test("signed org membership, issuer agent IDs and server-stamped rows isolate same-subject/claim spoofing", async () => {
  const f = await fixture();
  expect(await f.a.client.query(q("directory"), {})).toHaveLength(2);
  expect(await f.b.client.query(q("directory"), {})).toHaveLength(1);
  const other = await f.register("different-subject", "b", "agent-a");
  expect(other.agentId).not.toBe(f.a.agentId);
  await expect(f.db.withIdentity({ ...f.a.claims, org_id: "b" }).query(q("pending"), {})).rejects.toThrow("credential");
  f.e.scope.fromAgent = f.c.agentId; f.sign();
  await expect(f.a.client.mutation(m("enqueue"), { envelope: f.e })).rejects.toThrow("Forged");
  for (const table of ["agents", "credentials", "memberships", "rateLimits"] as const) {
    for (const row of await f.db.run(ctx => ctx.db.query(table).collect())) {
      expect(row.owner).toBeTruthy(); expect(row.org).toBeTruthy();
    }
  }
});

test("cross-org denies by default; requires both real admins and only the exact named pair", async () => {
  const f = await fixture();
  await expect(f.a.client.mutation(m("enqueue"), { envelope: f.e })).rejects.toThrow("Both org admins");
  const grantId = await f.adminA.mutation(m("propose"), { localAgent: f.a.agentId, remoteAgent: f.b.agentId });
  for (const client of [f.a.client, f.member]) await expect(client.mutation(m("decide"), { grantId, approve: true })).rejects.toThrow("administrator");
  await expect(f.adminB.mutation(m("propose"), { localAgent: f.a.agentId, remoteAgent: f.b.agentId })).rejects.toThrow("your own");
  await f.adminA.mutation(m("decide"), { grantId, approve: true });
  await expect(f.a.client.mutation(m("enqueue"), { envelope: f.e })).rejects.toThrow("Both org admins");
  expect((await f.a.client.query(q("grants"), {}))[0].remote).toBeNull();
  await f.adminB.mutation(m("decide"), { grantId, approve: true });
  await f.a.client.mutation(m("enqueue"), { envelope: f.e });
  expect((await f.b.client.query(q("pending"), {}))[0]).toMatchObject({ external: true, trust: "external-untrusted" });
  const identity = await f.db.withIdentity(token("foreign-admin", "c", { org_role: "admin" }));
  await expect(identity.mutation(m("decide"), { grantId, approve: false })).rejects.toThrow("Wrong org");
  expect(await f.c.client.query(q("grants"), {})).toEqual([]);
  expect(await f.c.client.query(q("pending"), {})).toEqual([]);
  await expect(f.c.client.query(q("status"), { messageId: "message" })).rejects.toThrow("No message");
  await expect(f.accept(f.c.client, f.c)).rejects.toThrow("No message");
});

test.each(["a", "b"])("either org (%s) can revoke; regrant cannot revive old queued ciphertext", async side => {
  const f = await fixture(), grantId = await f.grant();
  await f.a.client.mutation(m("enqueue"), { envelope: f.e });
  await (side === "a" ? f.adminA : f.adminB).mutation(m("decide"), { grantId, approve: false });
  expect(await f.b.client.query(q("pending"), {})).toEqual([]);
  await expect(f.accept()).rejects.toThrow("Both org admins");
  await expect(f.a.client.mutation(m("enqueue"), { envelope: f.e })).rejects.toThrow("Both org admins");
  await expect(f.adminA.mutation(m("decide"), { grantId, approve: true })).rejects.toThrow("revoked");
  const newGrant = await f.grant();
  expect(newGrant).not.toBe(grantId);
  expect(await f.b.client.query(q("pending"), {})).toEqual([]);
});

test("admin membership/credential controls and recipient queue caps cannot be bypassed", async () => {
  const f = await fixture(); const grantId = await f.grant();
  await expect(f.a.client.query(q("memberships"), {})).rejects.toThrow("administrator");
  await expect(f.member.query(q("memberships"), {})).rejects.toThrow("administrator");
  expect(await f.adminA.query(q("memberships"), {})).toHaveLength(2);
  expect(await f.a.client.query(q("credentials"), {})).toEqual([{credentialId:f.a.credentialId,agentId:f.a.agentId,revoked:false}]);
  expect(await f.adminA.query(q("credentials"), {})).toHaveLength(2);
  await f.a.client.mutation(m("enqueue"), {envelope:f.e});
  await f.db.run(async ctx => {
    for (let i=1;i<256;i++) await ctx.db.insert("agentMessages",{...f.e,messageId:`seed-${i}`,org:f.a.org,toOrg:f.b.org,toOwner:f.b.owner,grantId,credentialId:f.a.credentialId});
  });
  f.e.messageId="full-queue";f.e.idempotencyKey="full-queue";f.sign();
  await expect(f.a.client.mutation(m("enqueue"), {envelope:f.e})).rejects.toThrow("queue full");
});

test("per-agent revoke blocks current/future tokens and legacy bypass; token revoke is immediate and irreversible", async () => {
  const f = await fixture(); await f.grant();
  await f.a.client.mutation(m("enqueue"), { envelope: f.e });
  await expect(f.b.client.mutation(m("revokeCredential"), { credentialId: f.a.credentialId })).rejects.toThrow("owner");
  await f.adminA.mutation(m("revokeCredential"), { credentialId: f.a.credentialId });
  await expect(f.a.client.query(q("pending"), {})).rejects.toThrow("credential");
  await expect(f.a.client.mutation(m("register"), { device: f.a.device })).rejects.toThrow("credential");
  expect(await f.b.client.query(q("pending"), {})).toEqual([]);
  await expect(f.accept()).rejects.toThrow("credential");
  const next = f.db.withIdentity({ ...f.a.claims, jti: "issuer-issued-next-token" });
  await next.mutation(m("register"), { device: f.a.device });
  await expect(f.adminB.mutation(m("revoke"), { agentId: f.a.agentId })).rejects.toThrow("Wrong org");
  await f.adminA.mutation(m("revoke"), { agentId: f.a.agentId });
  await expect(next.query(q("pending"), {})).rejects.toThrow("revoked");
  await expect(f.db.withIdentity({ ...f.a.claims, jti: "new-token" }).mutation(m("register"), { device: f.a.device })).rejects.toThrow("revoked");
  expect((await f.b.client.query(q("grants"), {}))[0].active).toBe(false);
});

test("org membership revocation gates every agent operation and removes sender from recipient delivery", async () => {
  const f = await fixture(); await f.grant();
  await f.a.client.mutation(m("enqueue"), { envelope: f.e });
  const membership = (await f.db.run(ctx => ctx.db.query("memberships").collect())).find(row => row.owner === f.a.owner)!;
  await expect(f.adminB.mutation(m("revokeMembership"), { membershipId: membership._id })).rejects.toThrow("Wrong org");
  await f.adminA.mutation(m("revokeMembership"), { membershipId: membership._id });
  expect(await f.c.client.query(q("directory"), {})).toHaveLength(1);
  for (const name of ["identity", "pending", "directory", "grants"]) await expect(f.a.client.query(q(name), {})).rejects.toThrow("membership");
  await expect(f.a.client.query(q("status"), { messageId: "message" })).rejects.toThrow("membership");
  expect(await f.b.client.query(q("pending"), {})).toEqual([]);
});

test("same-org named agents can communicate, but signed sender/recipient scope, signatures, timestamps and caps cannot be forged", async () => {
  const f = await fixture();
  f.e.toDevice = f.c.device.deviceId;
  f.e.scope.toOwner = f.c.owner; f.e.scope.toOrg = f.c.org; f.e.scope.toAgent = f.c.agentId; f.sign();
  await f.a.client.mutation(m("enqueue"), { envelope: f.e });
  expect((await f.c.client.query(q("pending"), {}))[0]).toMatchObject({ external: false, trust: "same-org" });
  f.e.messageId = "new-message"; f.e.idempotencyKey = "new-key";
  f.e.scope.toOrg = f.b.org; f.sign();
  await expect(f.a.client.mutation(m("enqueue"), { envelope: f.e })).rejects.toThrow("recipient scope");
  f.e.scope.toOrg = f.c.org; f.sign();
  f.e.ciphertext = "ff".repeat(32);
  await expect(f.a.client.mutation(m("enqueue"), { envelope: f.e })).rejects.toThrow("signature");
  f.e.ciphertext = "00".repeat(65553); f.sign();
  await expect(f.a.client.mutation(m("enqueue"), { envelope: f.e })).rejects.toThrow("cap");
  f.e.ciphertext = "00".repeat(32); f.e.expiresAt = f.e.createdAt + 86400001; f.sign();
  await expect(f.a.client.mutation(m("enqueue"), { envelope: f.e })).rejects.toThrow("TTL");
  f.e.expiresAt = f.e.createdAt + 1.5; f.sign();
  await expect(f.a.client.mutation(m("enqueue"), { envelope: f.e })).rejects.toThrow("timestamp");
  const changed = { ...f.a.device, encryptionKey: "00".repeat(32) };
  changed.signature = hex(nacl.sign.detached(deviceBytes(changed), f.a.pair.secretKey));
  await expect(f.a.client.mutation(m("register"), { device: changed })).rejects.toThrow("cannot be replaced");
});

test("idempotent signed acceptance deletes only ciphertext; sender-only status never claims pickup", async () => {
  const f = await fixture(); await f.grant();
  await f.a.client.mutation(m("enqueue"), { envelope: f.e });
  await f.a.client.mutation(m("enqueue"), { envelope: f.e });
  await expect(f.b.client.mutation(m("accept"), { fromOrg: f.a.org, messageId: "message", at: Date.now(), signature: "00".repeat(64) })).rejects.toThrow("signature");
  await f.accept(); await f.accept();
  expect(await f.db.run(ctx => ctx.db.query("agentMessages").collect())).toEqual([]);
  expect(await f.a.client.query(q("status"), { messageId: "message" })).toEqual({ state: "accepted" });
  await expect(f.b.client.query(q("status"), { messageId: "message" })).rejects.toThrow("No message");
  expect(await f.a.client.mutation(m("enqueue"), { envelope: f.e })).toMatchObject({ state: "accepted" });
  const tombstones = await f.db.run(ctx => ctx.db.query("agentAttempts").collect());
  expect(tombstones[0]).not.toHaveProperty("ciphertext"); expect(tombstones[0]).not.toHaveProperty("text");
  f.e.nonce = "00".repeat(12); f.sign();
  await expect(f.a.client.mutation(m("enqueue"), { envelope: f.e })).rejects.toThrow("Idempotency");
});

test("TTL and rate limits are enforced independently of client claims; cron purges ciphertext, metadata and counters", async () => {
  vi.useFakeTimers(); const f = await fixture(); await f.grant();
  for (let i = 0; i < 60; i++) { f.e.messageId = `message-${i}`; f.e.idempotencyKey = `key-${i}`; f.sign(); await f.a.client.mutation(m("enqueue"), { envelope: f.e }); }
  f.e.messageId = "rate-blocked"; f.e.idempotencyKey = "rate-blocked"; f.sign();
  await expect(f.a.client.mutation(m("enqueue"), { envelope: f.e })).rejects.toThrow("rate limit");
  vi.setSystemTime(f.e.expiresAt + 1);
  expect(await f.b.client.query(q("pending"), {})).toEqual([]);
  expect(await f.a.client.query(q("status"), { messageId: "message-0" })).toEqual({ state: "expired" });
  await f.db.mutation(m("expire"), {});
  expect(await f.db.run(ctx => ctx.db.query("agentMessages").collect())).toEqual([]);
  vi.setSystemTime(f.e.expiresAt + 8 * 86400000); await f.db.mutation(m("expire"), {});
  expect(await f.db.run(ctx => ctx.db.query("agentAttempts").collect())).toEqual([]);
  expect(await f.db.run(ctx => ctx.db.query("rateLimits").collect())).toEqual([]);
});
