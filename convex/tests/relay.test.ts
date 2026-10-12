import { convexTest } from "convex-test";
import { makeFunctionReference } from "convex/server";
import { afterEach, describe, expect, test, vi } from "vitest";
import nacl from "tweetnacl";
import schema from "../schema";
import { namespace } from "../authorization";
import { aadBytes, deviceBytes, envelopeBytes, fingerprint, observationBytes, rotationBytes, hexString, type Envelope } from "../protocol";
import { key, seal, open, resign, type Keys } from "./crypto.test-helper";
import wire from "./relay-wire-v2.json";

const modules = {
  "../_generated/server.ts": async () => ({}),
  "../relay.ts": () => import("../relay"),
  "../grants.ts": () => import("../grants"),
};
const ref = (name: string) => makeFunctionReference<"mutation">(name.includes(":") ? name : `relay:${name}`);
const query = (name: string) => makeFunctionReference<"query">(name.includes(":") ? name : `relay:${name}`);
const issuer = "https://issuer.example/";
const ownerA = namespace(issuer, "alice", "org-a"), ownerB = namespace(issuer, "bob", "org-b");

async function fixture() {
  const backend = convexTest(schema, modules);
  const identities = [
    { issuer, subject: "alice", everett_org: "org-a", everett_agent: "agent-a", everett_role: "admin" },
    { issuer, subject: "bob", everett_org: "org-b", everett_agent: "agent-b", everett_role: "admin" },
    { issuer, subject: "alice-member", everett_org: "org-a", everett_agent: "agent-c", everett_role: "member" },
    { issuer, subject: "bob-member", everett_org: "org-b", everett_agent: "agent-d", everett_role: "member" },
  ];
  const clients = identities.map((i) => backend.withIdentity(i));
  const keys = [key("agent-a", ownerA, 1), key("agent-b", ownerB, 3), key("agent-c", ownerA, 5), key("agent-d", ownerB, 9)];
  for (let i = 0; i < keys.length; i++) {
    const identity = identities[i];
    await backend.mutation(ref("provisionIdentity"), { issuer, subject: identity.subject, org: identity.everett_org, agentId: identity.everett_agent, role: i < 2 ? "admin" : "member" });
    await clients[i].mutation(ref("register"), { device: keys[i].device });
  }
  const [a, b, c, d] = clients, [ka, kb, kc, kd] = keys;
  const invite = (ratePerMinute = 4, bodyCapBytes = 1024, expiresAt: number | null = null) => a.mutation(ref("grants:invite"), {
    grantId: "grant-ab", deviceId: ka.device.deviceId, peerOwner: ownerB, peerDeviceId: kb.device.deviceId,
    localFingerprint: fingerprint(ka.device), peerFingerprint: fingerprint(kb.device), expiresAt, ratePerMinute, bodyCapBytes,
  });
  const activate = async (ratePerMinute = 4, bodyCapBytes = 1024, expiresAt: number | null = null) => {
    await invite(ratePerMinute, bodyCapBytes, expiresAt);
    await b.mutation(ref("grants:accept"), { grantId: "grant-ab", localFingerprint: fingerprint(kb.device), peerFingerprint: fingerprint(ka.device) });
  };
  const observe = (recipient: typeof a, recipientKeys: Keys, e: Envelope, state: "delivered" | "claimed" | "replied" = "delivered") => {
    const args = { deviceId: recipientKeys.device.deviceId, senderOwner: e.owner, messageId: e.messageId, state, at: Date.now() };
    const signature = hexString(nacl.sign.detached(observationBytes({ ...args, owner: recipientKeys.device.owner }), recipientKeys.pair.secretKey));
    return recipient.mutation(ref("observe"), { ...args, signature });
  };
  return { backend, identities, a, b, c, d, ka, kb, kc, kd, invite, activate, observe };
}
afterEach(() => vi.useRealTimers());

type Fixture = Awaited<ReturnType<typeof fixture>>;
async function expectRoutingDenied(f: Fixture, client: Fixture["a"] | Fixture["backend"], reason: RegExp) {
  const e = seal(f.ka, f.kc);
  const observation = { deviceId: f.ka.device.deviceId, senderOwner: ownerA, messageId: e.messageId,
    at: Date.now(), signature: "00".repeat(64) };
  for (const [name, args] of [
    ["whoami", {}], ["devices", {}], ["credentials", {}], ["memberships", {}],
    ["pending", { deviceId: f.ka.device.deviceId }],
    ["status", { fromDevice: f.ka.device.deviceId, messageId: e.messageId }],
    ["grants:list", {}], ["grants:get", { grantId: "grant-ab" }],
  ] as const) await expect(client.query(query(name), args)).rejects.toThrow(reason);
  for (const [name, args] of [
    ["register", { device: f.ka.device }],
    ["rotate", { device: f.ka.device, oldFingerprint: fingerprint(f.ka.device), signature: "00".repeat(64) }],
    ["revokeAgent", { deviceId: "agent-c" }], ["revokeMember", { subject: "alice-member" }],
    ["enqueue", { envelope: e }], ["accept", observation],
    ["observe", { ...observation, state: "delivered" }],
    ["grants:invite", { grantId: "grant-ab", deviceId: "agent-a", peerOwner: ownerB, peerDeviceId: "agent-b",
      localFingerprint: fingerprint(f.ka.device), peerFingerprint: fingerprint(f.kb.device),
      expiresAt: null, ratePerMinute: 30, bodyCapBytes: 65536 }],
    ["grants:accept", { grantId: "grant-ab", localFingerprint: fingerprint(f.ka.device), peerFingerprint: fingerprint(f.kb.device) }],
    ["grants:reconfirm", { grantId: "grant-ab", localFingerprint: fingerprint(f.ka.device), peerFingerprint: fingerprint(f.kb.device) }],
    ["grants:revoke", { grantId: "grant-ab" }],
  ] as const) await expect(client.mutation(ref(name), args)).rejects.toThrow(reason);
}

describe("actual Convex functions, offline", () => {
  test("every relay and grant entry point rejects missing auth, wrong issuer and legacy claim-only enrollment", async () => {
    const f = await fixture();
    await expectRoutingDenied(f, f.backend, /Sign in/);
    await expectRoutingDenied(f, f.backend.withIdentity({ ...f.identities[0], issuer: "https://evil.example/" }), /membership/);
    await expectRoutingDenied(f, f.backend.withIdentity({ issuer, subject: "alice", org_id: "org-a", agent_id: "agent-a", jti: "token-1" }), /everett_agent/);
  });
  test("signed org changes cannot expose or mutate another namespace's queued messages", async () => {
    const f = await fixture(), e = seal(f.ka, f.kc);
    await f.a.mutation(ref("enqueue"), { envelope: e });
    await expectRoutingDenied(f, f.backend.withIdentity({ ...f.identities[0], everett_org: "org-b" }), /membership/);
    expect((await f.c.query(query("pending"), { deviceId: "agent-c" })).envelopes[0].envelope).toEqual(e);
    await f.observe(f.c, f.kc, e);
    expect(await f.backend.run(ctx => ctx.db.query("messages").collect())).toEqual([]);
  });
  test("membership revocation gates every relay and grant entry point with a still-valid identity", async () => {
    const f = await fixture(), e = seal(f.kc, f.ka);
    await f.c.mutation(ref("enqueue"), { envelope: e });
    await f.a.mutation(ref("revokeMember"), { subject: "alice-member" });
    await expectRoutingDenied(f, f.c, /membership/);
    expect((await f.backend.run(ctx => ctx.db.query("messages").collect()))[0]).toMatchObject(e);
  });
  test("agent MCP credential and membership views are bounded by org and role", async () => {
    const f = await fixture();
    const own = await f.c.query(query("credentials"), {});
    expect(own.credentials.map((row: { agentId: string }) => row.agentId)).toEqual(["agent-c"]);
    const admin = await f.a.query(query("credentials"), {});
    expect(admin.credentials.map((row: { agentId: string }) => row.agentId).sort()).toEqual(["agent-a", "agent-c"]);
    expect(admin.truncated).toBe(false);
    expect(JSON.stringify(admin)).not.toMatch(/token|secret|signingKey|encryptionKey/);
    const members = await f.a.query(query("memberships"), {});
    expect(members.memberships.map((row: { subject: string }) => row.subject).sort()).toEqual(["alice", "alice-member"]);
    await expect(f.c.query(query("memberships"), {})).rejects.toThrow();
    await f.a.mutation(ref("revokeAgent"), { deviceId: "agent-c" });
    await expect(f.c.query(query("credentials"), {})).rejects.toThrow();
    expect((await f.a.query(query("credentials"), {})).credentials.find((row: { agentId: string }) => row.agentId === "agent-c")?.status).toBe("revoked");
    await expect(f.backend.query(query("credentials"), {})).rejects.toThrow("Sign in");
  });
  test("v2 Rust integration vector fixes device, AAD, envelope, rotation, observation and encryption bytes", () => {
    const a = key("agent-a", ownerA, 1), b = key("agent-b", ownerB, 3);
    const e = seal(a, b, { grantId: "grant-ab", createdAt: 1000, expiresAt: 3601000 });
    const utf8 = (bytes: Uint8Array) => new TextDecoder().decode(bytes);
    expect(a.device).toEqual(wire.device); expect(b.device).toEqual(wire.recipient);
    expect(utf8(deviceBytes(a.device))).toBe(wire.deviceSignedUtf8);
    expect(fingerprint(a.device)).toBe(wire.fingerprint);
    expect(e).toEqual(wire.envelope);
    expect(utf8(aadBytes(e))).toBe(wire.aadUtf8);
    expect(utf8(envelopeBytes(e))).toBe(wire.envelopeSignedUtf8);
    expect(open(wire.envelope, b)).toBe(wire.plaintextUtf8);
    const next = key("agent-a", ownerA, 13);
    expect(next.device).toEqual(wire.newDevice);
    expect(utf8(rotationBytes(next.device, fingerprint(a.device)))).toBe(wire.rotationSignedUtf8);
    expect(hexString(nacl.sign.detached(rotationBytes(next.device, fingerprint(a.device)), a.pair.secretKey))).toBe(wire.rotation.signature);
    const observation = { ...wire.observation, state: "delivered" as const };
    expect(utf8(observationBytes(observation))).toBe(wire.observationSignedUtf8);
    expect(hexString(nacl.sign.detached(observationBytes(observation), b.pair.secretKey))).toBe(wire.observation.signature);
  });
  test("requires OIDC identity, authoritative membership and issuer-controlled per-agent claim", async () => {
    const f = await fixture();
    await expect(f.backend.query(query("whoami"), {})).rejects.toThrow("Sign in");
    const ownerBearer = f.backend.withIdentity({ issuer, subject: "alice", everett_org: "org-a" });
    await expect(ownerBearer.query(query("devices"), {})).rejects.toThrow("everett_agent");
    const stranger = f.backend.withIdentity({ issuer, subject: "mallory", everett_org: "org-a", everett_agent: "agent-a" });
    await expect(stranger.mutation(ref("register"), { device: f.ka.device })).rejects.toThrow("membership");
    const wrongOrg = f.backend.withIdentity({ ...f.identities[0], everett_org: "org-b" });
    await expect(wrongOrg.query(query("whoami"), {})).rejects.toThrow("membership");
    const wrongIssuer = f.backend.withIdentity({ ...f.identities[0], issuer: "https://other.example/" });
    await expect(wrongIssuer.query(query("whoami"), {})).rejects.toThrow("membership");
    const wrongAgent = f.backend.withIdentity({ ...f.identities[0], everett_agent: "agent-c" });
    await expect(wrongAgent.query(query("whoami"), {})).rejects.toThrow("credential");
    expect(await f.a.query(query("devices"), {})).toEqual([f.ka.device, f.kc.device]);
  });
  test("fallback account tuple is separated from org namespace and distinct agents share one account", async () => {
    const f = await fixture(), owner = namespace(issuer, "personal", null);
    expect(owner).not.toBe(namespace(issuer, "unused", "personal"));
    for (const id of ["personal-1", "personal-2"]) {
      await f.backend.mutation(ref("provisionIdentity"), { issuer, subject: "personal", org: null, agentId: id, role: "member" });
      const client = f.backend.withIdentity({ issuer, subject: "personal", everett_agent: id });
      await client.mutation(ref("register"), { device: key(id, owner, 11).device });
      expect(await client.query(query("whoami"), {})).toMatchObject({ owner, deviceId: id });
    }
  });
  test("JWT agent cannot register, send, read, observe or inspect status as a different device", async () => {
    const f = await fixture(), e = seal(f.ka, f.kc);
    await expect(f.a.mutation(ref("register"), { device: f.kc.device })).rejects.toThrow("match device");
    await expect(f.c.mutation(ref("enqueue"), { envelope: e })).rejects.toThrow("match device");
    await expect(f.a.query(query("pending"), { deviceId: f.kc.device.deviceId })).rejects.toThrow("match device");
    await expect(f.c.query(query("status"), { fromDevice: f.ka.device.deviceId, messageId: e.messageId })).rejects.toThrow("match device");
    await expect(f.observe(f.a, f.kc, e)).rejects.toThrow("match device");
    e.owner = ownerB; resign(e, f.ka);
    await expect(f.a.mutation(ref("enqueue"), { envelope: e })).rejects.toThrow("Wrong organization");
  });
  test("same-org E2EE roundtrip, ciphertext-only storage and tamper detection", async () => {
    const f = await fixture(), e = seal(f.ka, f.kc);
    await f.a.mutation(ref("enqueue"), { envelope: e });
    const pending = await f.c.query(query("pending"), { deviceId: f.kc.device.deviceId });
    expect(pending.envelopes[0].trust).toBe("same-org");
    expect(JSON.parse(open(pending.envelopes[0].envelope, f.kc)).text).toBe("hello");
    expect(JSON.stringify(await f.backend.run((ctx) => ctx.db.query("messages").collect()))).not.toContain("hello");
    const tampered = { ...e, ciphertext: "00" + e.ciphertext.slice(2) };
    expect(() => open(tampered, f.kc)).toThrow();
    await expect(f.a.mutation(ref("enqueue"), { envelope: tampered })).rejects.toThrow("signature");
    expect(() => open({ ...e, recipientOwner: ownerB }, f.kc)).toThrow();
  });
  test("cross-org denied until both admins explicitly pin both key bundles", async () => {
    const f = await fixture(), e = seal(f.ka, f.kb, { grantId: "grant-ab" });
    await expect(f.a.mutation(ref("enqueue"), { envelope: e })).rejects.toThrow("Missing");
    await f.invite();
    await expect(f.a.mutation(ref("enqueue"), { envelope: e })).rejects.toThrow("inactive");
    await expect(f.a.mutation(ref("grants:accept"), { grantId: "grant-ab", localFingerprint: fingerprint(f.ka.device), peerFingerprint: fingerprint(f.kb.device) })).rejects.toThrow("invited organization");
    await expect(f.b.mutation(ref("grants:accept"), { grantId: "grant-ab", localFingerprint: fingerprint(f.kb.device), peerFingerprint: "00".repeat(32) })).rejects.toThrow("fingerprints");
    await f.b.mutation(ref("grants:accept"), { grantId: "grant-ab", localFingerprint: fingerprint(f.kb.device), peerFingerprint: fingerprint(f.ka.device) });
    await f.a.mutation(ref("enqueue"), { envelope: e });
    const pending = await f.b.query(query("pending"), { deviceId: f.kb.device.deviceId });
    expect(pending.envelopes[0]).toMatchObject({ envelope: e, trust: "external-untrusted" });
    expect(JSON.parse(open(pending.envelopes[0].envelope, f.kb)).text).toBe("hello");
    const g = await f.b.query(query("grants:get"), { grantId: "grant-ab" });
    expect(g).toMatchObject({ aBundle: f.ka.device, bBundle: f.kb.device, access: "active" });
    await expect(f.c.query(query("grants:get"), { grantId: "grant-ab" })).rejects.toThrow("does not name");
  });
  test("invite must pin current fingerprints; role claim alone cannot confer admin", async () => {
    const f = await fixture();
    await expect(f.a.mutation(ref("grants:invite"), {
      grantId: "bad-pin", deviceId: "agent-a", peerOwner: ownerB, peerDeviceId: "agent-b",
      localFingerprint: fingerprint(f.ka.device), peerFingerprint: "00".repeat(32), expiresAt: null, ratePerMinute: 2, bodyCapBytes: 1024,
    })).rejects.toThrow("fingerprints");
    const spoofRole = f.backend.withIdentity({ ...f.identities[2], everett_role: "admin" });
    await expect(spoofRole.mutation(ref("revokeAgent"), { deviceId: "agent-a" })).rejects.toThrow("admin");
    const noRoleClaim = f.backend.withIdentity({ ...f.identities[0], everett_role: "member" });
    await expect(noRoleClaim.mutation(ref("revokeAgent"), { deviceId: "agent-c" })).rejects.toThrow("admin");
  });
  test("grants name exactly two agents and permit both directions, never a whole org", async () => {
    const f = await fixture(); await f.activate();
    const e = seal(f.ka, f.kd, { grantId: "grant-ab" });
    await expect(f.a.mutation(ref("enqueue"), { envelope: e })).rejects.toThrow("does not name");
    await expect(f.c.mutation(ref("enqueue"), { envelope: seal(f.kc, f.kb, { grantId: "grant-ab" }) })).rejects.toThrow("does not name");
    await expect(f.a.mutation(ref("enqueue"), { envelope: seal(f.ka, f.kb) })).rejects.toThrow("Missing");
    await f.b.mutation(ref("enqueue"), { envelope: seal(f.kb, f.ka, { grantId: "grant-ab" }) });
    expect((await f.a.query(query("pending"), { deviceId: "agent-a" })).envelopes[0].trust).toBe("external-untrusted");
  });
  test.each(["a", "b"])("either-side grant revocation blocks queued delivery and all new sends (%s)", async (side) => {
    const f = await fixture(); await f.activate();
    const e = seal(f.ka, f.kb, { grantId: "grant-ab" });
    await f.a.mutation(ref("enqueue"), { envelope: e });
    await f[side as "a" | "b"].mutation(ref("grants:revoke"), { grantId: "grant-ab" });
    expect((await f.b.query(query("pending"), { deviceId: "agent-b" })).envelopes).toEqual([]);
    await expect(f.a.mutation(ref("enqueue"), { envelope: e })).rejects.toThrow("revoked");
    await expect(f.observe(f.b, f.kb, e)).rejects.toThrow("revoked");
    await expect(f.b.mutation(ref("grants:reconfirm"), { grantId: "grant-ab", localFingerprint: fingerprint(f.kb.device), peerFingerprint: fingerprint(f.ka.device) })).rejects.toThrow("revoked");
  });
  test("revoked agent denied using still-valid identity, including every re-queried subscription and enrollment", async () => {
    const f = await fixture(); await f.activate();
    const e = seal(f.ka, f.kb, { grantId: "grant-ab" });
    await f.a.mutation(ref("enqueue"), { envelope: e });
    expect((await f.b.query(query("pending"), { deviceId: "agent-b" })).envelopes).toHaveLength(1);
    await f.b.mutation(ref("revokeAgent"), { deviceId: "agent-b" });
    for (const [name, args] of [["pending", { deviceId: "agent-b" }], ["devices", {}], ["whoami", {}], ["grants:list", {}], ["grants:get", { grantId: "grant-ab" }]] as const) {
      await expect(f.b.query(query(name), args)).rejects.toThrow("revoked");
    }
    await expect(f.b.mutation(ref("register"), { device: f.kb.device })).rejects.toThrow("revoked");
    await expect(f.observe(f.b, f.kb, e)).rejects.toThrow("revoked");
    await expect(f.a.mutation(ref("enqueue"), { envelope: e })).rejects.toThrow("revoked");
    await expect(f.backend.mutation(ref("provisionIdentity"), { issuer, subject: "bob", org: "org-b", agentId: "agent-b", role: "admin" })).rejects.toThrow("tombstone");
  });
  test("membership revocation overrides all valid JWT claims and prevents re-provisioning", async () => {
    const f = await fixture();
    await f.a.mutation(ref("revokeMember"), { subject: "alice-member" });
    await expect(f.c.query(query("whoami"), {})).rejects.toThrow("membership");
    await expect(f.c.mutation(ref("register"), { device: f.kc.device })).rejects.toThrow("membership");
    await expect(f.backend.mutation(ref("provisionIdentity"), { issuer, subject: "alice-member", org: "org-a", agentId: "brand-new", role: "member" })).rejects.toThrow("tombstone");
  });
  test("key rotation needs old signing key and new self-signature; grants block until both reconfirm", async () => {
    const f = await fixture(); await f.activate();
    const oldEnvelope = seal(f.ka, f.kb, { grantId: "grant-ab" });
    await f.a.mutation(ref("enqueue"), { envelope: oldEnvelope });
    const next = key("agent-a", ownerA, 13), oldFingerprint = fingerprint(f.ka.device);
    const signature = hexString(nacl.sign.detached(rotationBytes(next.device, oldFingerprint), f.ka.pair.secretKey));
    await expect(f.a.mutation(ref("register"), { device: next.device })).rejects.toThrow("cannot be replaced");
    await expect(f.a.mutation(ref("rotate"), { device: next.device, oldFingerprint, signature: next.device.signature })).rejects.toThrow("signature");
    await f.a.mutation(ref("rotate"), { device: next.device, oldFingerprint, signature });
    const g = await f.b.query(query("grants:get"), { grantId: "grant-ab" });
    expect(g.access).toBe("key_changed");
    expect((await f.b.query(query("pending"), { deviceId: "agent-b" })).envelopes).toEqual([]);
    const e = seal(next, f.kb, { grantId: "grant-ab", messageId: "new-keys", idempotencyKey: "new-keys" });
    await expect(f.a.mutation(ref("enqueue"), { envelope: e })).rejects.toThrow("key changed");
    const argsA = { grantId: "grant-ab", localFingerprint: fingerprint(next.device), peerFingerprint: fingerprint(f.kb.device) };
    expect(await f.a.mutation(ref("grants:reconfirm"), argsA)).toMatchObject({ state: "reconfirming" });
    await expect(f.a.mutation(ref("enqueue"), { envelope: e })).rejects.toThrow("inactive");
    expect(await f.a.mutation(ref("grants:reconfirm"), argsA)).toMatchObject({ state: "reconfirming" });
    await f.b.mutation(ref("grants:reconfirm"), { grantId: "grant-ab", localFingerprint: fingerprint(f.kb.device), peerFingerprint: fingerprint(next.device) });
    await f.a.mutation(ref("enqueue"), { envelope: e });
    expect((await f.b.query(query("pending"), { deviceId: "agent-b" })).envelopes).toHaveLength(1);
    await expect(f.a.mutation(ref("enqueue"), { envelope: oldEnvelope })).rejects.toThrow("key changed");
  });
  test("rotating back to a pinned key never restores a grant without bilateral confirmation", async () => {
    const f = await fixture(); await f.activate();
    const oldEnvelope = seal(f.ka, f.kb, { grantId: "grant-ab" });
    await f.a.mutation(ref("enqueue"), { envelope: oldEnvelope });
    const next = key("agent-a", ownerA, 13);
    for (const [old, replacement] of [[f.ka, next], [next, f.ka]]) {
      const oldFingerprint = fingerprint(old.device);
      const signature = hexString(nacl.sign.detached(rotationBytes(replacement.device, oldFingerprint), old.pair.secretKey));
      await f.a.mutation(ref("rotate"), { device: replacement.device, oldFingerprint, signature });
    }
    expect((await f.b.query(query("grants:get"), { grantId: "grant-ab" })).access).toBe("key_changed");
    expect((await f.b.query(query("pending"), { deviceId: "agent-b" })).envelopes).toEqual([]);
    await expect(f.observe(f.b, f.kb, oldEnvelope)).rejects.toThrow("key changed");
    await expect(f.a.mutation(ref("enqueue"), { envelope: oldEnvelope })).rejects.toThrow("key changed");
    const argsA = { grantId: "grant-ab", localFingerprint: fingerprint(f.ka.device), peerFingerprint: fingerprint(f.kb.device) };
    await f.a.mutation(ref("grants:reconfirm"), argsA);
    await expect(f.a.mutation(ref("enqueue"), { envelope: oldEnvelope })).rejects.toThrow("inactive");
    await f.b.mutation(ref("grants:reconfirm"), { grantId: "grant-ab", localFingerprint: fingerprint(f.kb.device), peerFingerprint: fingerprint(f.ka.device) });
    await f.a.mutation(ref("enqueue"), { envelope: oldEnvelope });
    expect((await f.b.query(query("pending"), { deviceId: "agent-b" })).envelopes).toHaveLength(1);
  });
  test("local forwarding consumes hops without requiring a server row for each local inbox", async () => {
    const f = await fixture(); await f.activate();
    const root = seal(f.ka, f.kb, { grantId: "grant-ab" });
    await f.a.mutation(ref("enqueue"), { envelope: root }); await f.observe(f.b, f.kb, root);
    const afterLocal = seal(f.kb, f.ka, { grantId: "grant-ab", parent: { owner: root.owner, messageId: root.messageId }, hop: 3, messageId: "after-local", idempotencyKey: "after-local" });
    await f.b.mutation(ref("enqueue"), { envelope: afterLocal });
    const reset = seal(f.kb, f.ka, { ...afterLocal, hop: 1, messageId: "reset-local", idempotencyKey: "reset-local" });
    await expect(f.b.mutation(ref("enqueue"), { envelope: reset })).rejects.toThrow("lineage");
  });
  test("cursor pages past >64 newly unpinned ciphertext rows without starving newer valid mail", async () => {
    const f = await fixture();
    for (let i = 0; i < 66; i++) {
      await f.a.mutation(ref("enqueue"), { envelope: seal(f.ka, f.kc, { messageId: `stale-${i}`, idempotencyKey: `stale-key-${i}` }) });
    }
    const next = key("agent-a", ownerA, 13), oldFingerprint = fingerprint(f.ka.device);
    const signature = hexString(nacl.sign.detached(rotationBytes(next.device, oldFingerprint), f.ka.pair.secretKey));
    await f.a.mutation(ref("rotate"), { device: next.device, oldFingerprint, signature });
    const valid = seal(next, f.kc, { messageId: "after-rotate", idempotencyKey: "after-rotate" });
    await f.a.mutation(ref("enqueue"), { envelope: valid });
    const first = await f.c.query(query("pending"), { deviceId: "agent-c" });
    expect(first.envelopes).toEqual([]); expect(first.isDone).toBe(false);
    const second = await f.c.query(query("pending"), { deviceId: "agent-c", cursor: first.continueCursor });
    expect(second.envelopes).toEqual([{ envelope: valid, trust: "same-org" }]);
  });
  test("grant fixed-minute rate is shared across directions and retries do not consume quota", async () => {
    const f = await fixture(); await f.activate(1);
    const e = seal(f.ka, f.kb, { grantId: "grant-ab" });
    await f.a.mutation(ref("enqueue"), { envelope: e });
    await f.a.mutation(ref("enqueue"), { envelope: e });
    await expect(f.b.mutation(ref("enqueue"), { envelope: seal(f.kb, f.ka, { grantId: "grant-ab", messageId: "m2", idempotencyKey: "k2" }) })).rejects.toThrow("rate");
  });
  test("serialized encrypted body cap is independently enforced per grant", async () => {
    const f = await fixture(); await f.activate(4, 256);
    await expect(f.a.mutation(ref("enqueue"), { envelope: seal(f.ka, f.kb, { grantId: "grant-ab" }, JSON.stringify({ text: "x".repeat(256) })) })).rejects.toThrow("body cap");
  });
  test("durable idempotency survives delivery, TTL cleanup and time; conflicting keys and IDs denied", async () => {
    vi.useFakeTimers(); const f = await fixture(), e = seal(f.ka, f.kc);
    expect(await f.a.mutation(ref("enqueue"), { envelope: e })).toMatchObject({ state: "queued" });
    await f.observe(f.c, f.kc, e);
    expect(await f.a.mutation(ref("enqueue"), { envelope: e })).toMatchObject({ state: "delivered" });
    const changed = seal(f.ka, f.kc, { messageId: "changed" });
    await expect(f.a.mutation(ref("enqueue"), { envelope: changed })).rejects.toThrow("Idempotency");
    const sameId = seal(f.ka, f.kc, { idempotencyKey: "different" });
    await expect(f.a.mutation(ref("enqueue"), { envelope: sameId })).rejects.toThrow("Message ID");
    vi.setSystemTime(e.expiresAt + 365 * 86400000);
    await f.backend.mutation(ref("expire"), {});
    expect(await f.a.mutation(ref("enqueue"), { envelope: e })).toMatchObject({ state: "delivered" });
    expect(await f.backend.run((ctx) => ctx.db.query("attempts").collect())).toHaveLength(1);
  });
  test("delivery deletes ciphertext, states advance only on signed observations and cannot regress", async () => {
    const f = await fixture(), e = seal(f.ka, f.kc);
    await f.a.mutation(ref("enqueue"), { envelope: e });
    await expect(f.observe(f.c, f.kc, e, "claimed")).rejects.toThrow("first");
    await f.c.query(query("pending"), { deviceId: "agent-c" });
    expect(await f.a.query(query("status"), { fromDevice: "agent-a", messageId: e.messageId })).toEqual({ state: "queued" });
    await f.observe(f.c, f.kc, e); await f.observe(f.c, f.kc, e);
    expect(await f.backend.run((ctx) => ctx.db.query("messages").collect())).toEqual([]);
    await f.observe(f.c, f.kc, e, "claimed"); await f.observe(f.c, f.kc, e, "replied"); await f.observe(f.c, f.kc, e);
    expect(await f.a.query(query("status"), { fromDevice: "agent-a", messageId: e.messageId })).toEqual({ state: "replied" });
    const [record] = await f.backend.run((ctx) => ctx.db.query("attempts").collect());
    for (const field of ["ciphertext", "text", "signature", "ephemeralKey", "nonce"]) expect(record).not.toHaveProperty(field);
  });
  test("hop lineage is persisted and capped at 3 across devices AND organizations", async () => {
    const f = await fixture(); await f.activate();
    const e1 = seal(f.kc, f.ka);
    await f.c.mutation(ref("enqueue"), { envelope: e1 }); await f.observe(f.a, f.ka, e1);
    const e2 = seal(f.ka, f.kb, { grantId: "grant-ab", parent: { owner: e1.owner, messageId: e1.messageId }, hop: 2, messageId: "m2", idempotencyKey: "k2" });
    await f.a.mutation(ref("enqueue"), { envelope: e2 }); await f.observe(f.b, f.kb, e2);
    const e3 = seal(f.kb, f.kd, { parent: { owner: e2.owner, messageId: e2.messageId }, hop: 3, messageId: "m3", idempotencyKey: "k3" });
    await f.b.mutation(ref("enqueue"), { envelope: e3 }); await f.observe(f.d, f.kd, e3);
    const e4 = seal(f.kd, f.kb, { parent: { owner: e3.owner, messageId: e3.messageId }, hop: 4, messageId: "m4", idempotencyKey: "k4" });
    await expect(f.d.mutation(ref("enqueue"), { envelope: e4 })).rejects.toThrow("Hop limit");
    await expect(f.d.mutation(ref("enqueue"), { envelope: seal(f.kd, f.kb, { ...e4, hop: 1 }) })).rejects.toThrow("lineage");
    await expect(f.b.mutation(ref("enqueue"), { envelope: seal(f.kb, f.kd, { parent: { owner: e1.owner, messageId: e1.messageId }, hop: 2, messageId: "wrong", idempotencyKey: "wrong" }) })).rejects.toThrow("Parent");
    await expect(f.b.mutation(ref("enqueue"), { envelope: seal(f.kb, f.kd, { hop: 2, messageId: "root", idempotencyKey: "root" }) })).rejects.toThrow("Root");
  });
  test("TTL is server-clock checked; cleanup deletes ciphertext, not durable replay records", async () => {
    vi.useFakeTimers(); const f = await fixture(), e = seal(f.ka, f.kc);
    await f.a.mutation(ref("enqueue"), { envelope: e });
    vi.setSystemTime(e.expiresAt + 1);
    expect((await f.c.query(query("pending"), { deviceId: "agent-c" })).envelopes).toEqual([]);
    expect(await f.a.query(query("status"), { fromDevice: "agent-a", messageId: e.messageId })).toEqual({ state: "expired" });
    await expect(f.observe(f.c, f.kc, e)).rejects.toThrow("TTL");
    await f.backend.mutation(ref("expire"), {});
    expect(await f.backend.run((ctx) => ctx.db.query("messages").collect())).toEqual([]);
    expect(await f.a.mutation(ref("enqueue"), { envelope: e })).toMatchObject({ state: "expired" });
  });
  test("grant expiration hides queued traffic and rejects new sends", async () => {
    vi.useFakeTimers(); const f = await fixture(); await f.activate(4, 1024, Date.now() + 1000);
    const e = seal(f.ka, f.kb, { grantId: "grant-ab" }); await f.a.mutation(ref("enqueue"), { envelope: e });
    vi.advanceTimersByTime(1001);
    expect((await f.b.query(query("pending"), { deviceId: "agent-b" })).envelopes).toEqual([]);
    await expect(f.a.mutation(ref("enqueue"), { envelope: e })).rejects.toThrow("expired");
    await f.backend.mutation(ref("expire"), {});
    expect(await f.a.query(query("grants:get"), { grantId: "grant-ab" })).toMatchObject({ state: "expired", access: "expired" });
    await expect(f.a.mutation(ref("grants:reconfirm"), { grantId: "grant-ab", localFingerprint: fingerprint(f.ka.device), peerFingerprint: fingerprint(f.kb.device) })).rejects.toThrow("expired");
  });
  test("invalid cryptographic fields, timestamps, signatures and body limits denied", async () => {
    vi.useFakeTimers(); const f = await fixture();
    for (const change of [{ expiresAt: Date.now() - 1 }, { expiresAt: Date.now() + 86400001 }, { createdAt: Date.now() - 300001 }, { nonce: "not-hex" }, { hop: 1.5 }]) {
      const e = seal(f.ka, f.kc); Object.assign(e, change); resign(e, f.ka);
      await expect(f.a.mutation(ref("enqueue"), { envelope: e })).rejects.toThrow();
    }
    await expect(f.a.mutation(ref("enqueue"), { envelope: seal(f.ka, f.kc, {}, "x".repeat(65537)) })).rejects.toThrow("body cap");
    const e = seal(f.ka, f.kc, { toDevice: "unknown" });
    await expect(f.a.mutation(ref("enqueue"), { envelope: e })).rejects.toThrow("not registered");
    const missingNewSignature = { ...key("agent-a", ownerA, 15).device, signature: "00".repeat(64) };
    await expect(f.a.mutation(ref("rotate"), { device: missingNewSignature, oldFingerprint: fingerprint(f.ka.device), signature: "00".repeat(64) })).rejects.toThrow("signature");
  });
});
