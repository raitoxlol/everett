import { convexTest } from "convex-test";
import { makeFunctionReference } from "convex/server";
import { afterEach, describe, expect, test, vi } from "vitest";
import nacl from "tweetnacl";
import schema from "../schema";
import { namespace } from "../authorization";
import { queuedAttemptCount } from "../relay";
import { aadBytes, deviceBytes, sshBindingBytes, envelopeBytes, fingerprint, observationBytes, rotationBytes, hexString, type Envelope } from "../protocol";
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
afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); });

type Fixture = Awaited<ReturnType<typeof fixture>>;
async function expectRoutingDenied(f: Fixture, client: Fixture["a"] | Fixture["backend"], reason: RegExp) {
  const e = seal(f.ka, f.kc);
  const observation = { deviceId: f.ka.device.deviceId, senderOwner: ownerA, messageId: e.messageId,
    at: Date.now(), signature: "00".repeat(64) };
  for (const [name, args] of [
    ["whoami", {}], ["devices", {}], ["revokedDevices", {}], ["credentials", {}], ["memberships", {}],
    ["pending", { deviceId: f.ka.device.deviceId }],
    ["status", { fromDevice: f.ka.device.deviceId, messageId: e.messageId }],
    ["grants:list", {}], ["grants:get", { grantId: "grant-ab" }],
  ] as const) await expect(client.query(query(name), args)).rejects.toThrow(reason);
  for (const [name, args] of [
    ["register", { device: f.ka.device }],
    ["rotate", { device: f.ka.device, oldFingerprint: fingerprint(f.ka.device), signature: "00".repeat(64) }],
    ["revokeAgent", { deviceId: "agent-c" }], ["revokeDevice", { deviceId: "agent-c" }], ["revokeMember", { subject: "alice-member" }],
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
  test("self-revocation records a permanent device and credential tombstone", async () => {
    const f = await fixture();
    vi.spyOn(Date, "now").mockReturnValue(1700000000000);
    await f.c.mutation(ref("revokeDevice"), { deviceId: "agent-c" });
    expect(await f.a.query(query("revokedDevices"), {})).toEqual({ devices: [{ deviceId: "agent-c", revokedAt: 1700000000000 }], cursor: null });
    await expectRoutingDenied(f, f.c, /revoked/);
    await expect(f.c.mutation(ref("register"), { device: f.kc.device })).rejects.toThrow("revoked");
    await expect(f.backend.mutation(ref("provisionIdentity"), {
      issuer, subject: "alice-member", org: "org-a", agentId: "agent-c", role: "member",
    })).rejects.toThrow("tombstone");
    await f.a.mutation(ref("revokeDevice"), { deviceId: "agent-c" });
    expect(await f.a.query(query("revokedDevices"), {})).toEqual({ devices: [{ deviceId: "agent-c", revokedAt: 1700000000000 }], cursor: null });
  });
  test("admin revokes only its namespace and members cannot revoke other devices", async () => {
    const f = await fixture();
    await expect(f.c.mutation(ref("revokeDevice"), { deviceId: "agent-a" })).rejects.toThrow("admin");
    const forgedAdmin = f.backend.withIdentity({ ...f.identities[2], everett_role: "admin" });
    await expect(forgedAdmin.mutation(ref("revokeDevice"), { deviceId: "agent-a" })).rejects.toThrow("admin");
    await expect(f.a.mutation(ref("revokeDevice"), { deviceId: "agent-d" })).rejects.toThrow("organization");
    await f.a.mutation(ref("revokeDevice"), { deviceId: "agent-c" });
    await f.b.mutation(ref("revokeDevice"), { deviceId: "agent-d" });
    expect(await f.a.query(query("revokedDevices"), {})).toEqual({ devices: [{ deviceId: "agent-c", revokedAt: expect.any(Number) }], cursor: null });
    expect(await f.b.query(query("revokedDevices"), {})).toEqual({ devices: [{ deviceId: "agent-d", revokedAt: expect.any(Number) }], cursor: null });
    await expect(f.c.query(query("device"), { deviceId: "agent-a" })).rejects.toThrow("revoked");
    await expect(f.a.query(query("device"), { deviceId: "agent-c" })).rejects.toThrow("revoked");
  });
  test("device tombstones deny registration even if a credential was incorrectly restored", async () => {
    const f = await fixture();
    await f.a.mutation(ref("revokeDevice"), { deviceId: "agent-c" });
    await f.backend.run(async ctx => {
      const row = await ctx.db.query("credentials").withIndex("by_agent", q => q.eq("issuer", issuer).eq("agentId", "agent-c")).unique();
      await ctx.db.patch(row!._id, { status: "active" });
    });
    await expect(f.c.mutation(ref("register"), { device: f.kc.device })).rejects.toThrow("tombstone");
    await expect(f.c.query(query("devices"), {})).rejects.toThrow("revoked");
  });
  test("revoked devices page past 256 tombstones and skip active rows", async () => {
    const f = await fixture();
    await f.backend.run(async ctx => {
      for (let i = 0; i < 300; i++) {
        await ctx.db.insert("devices", { ...f.kc.device, deviceId: `z-revoked-${String(i).padStart(3, "0")}`, generation: 0, revokedAt: 1000 + i });
      }
    });
    const first = await f.a.query(query("revokedDevices"), {});
    expect(first.devices).toHaveLength(256);
    expect(first.cursor).toEqual(expect.any(String));
    const second = await f.a.query(query("revokedDevices"), { cursor: first.cursor });
    expect(second.cursor).toBeNull();
    const ids = [...first.devices, ...second.devices].map(d => d.deviceId);
    expect(ids).toHaveLength(300);
    expect(new Set(ids).size).toBe(300);
    expect(ids.every(id => id.startsWith("z-revoked-"))).toBe(true);
    expect(await f.b.query(query("revokedDevices"), {})).toEqual({ devices: [], cursor: null });
  });
  const sshKey = (seed: number) => {
    const pair = nacl.sign.keyPair.fromSeed(new Uint8Array(32).fill(seed));
    const prefix = [0, 0, 0, 11, ...new TextEncoder().encode("ssh-ed25519"), 0, 0, 0, 32];
    return { pair, publicKey: Buffer.from([...prefix, ...pair.publicKey]).toString("base64") };
  };
  const binding = (owner: string, deviceId: string, sshDeviceId: string, key: ReturnType<typeof sshKey>, signer = key) => ({
    sshDeviceId, sshPublicKey: key.publicKey,
    sshSignature: hexString(nacl.sign.detached(sshBindingBytes(owner, deviceId, sshDeviceId, key.publicKey), signer.pair.secretKey)),
  });
  test("SSH binding bytes match the Rust client", () => {
    expect(new TextDecoder().decode(sshBindingBytes("[\"i\",\"org\",\"o\"]", "agent", "ssh-id", "AAAA")))
      .toBe('["everett-ssh-bind-v1","[\\"i\\",\\"org\\",\\"o\\"]","agent","ssh-id","AAAA"]');
  });
  test("registration binds a proven SSH peer id once and revocation reports it", async () => {
    const f = await fixture();
    const keyC = sshKey(21);
    await expect(f.c.mutation(ref("register"), { device: f.kc.device, ssh: binding(ownerA, "agent-c", "bad id!", keyC) })).rejects.toThrow();
    const proof = binding(ownerA, "agent-c", "ssh-c", keyC);
    await f.c.mutation(ref("register"), { device: f.kc.device, ssh: proof });
    await f.c.mutation(ref("register"), { device: f.kc.device, ssh: binding(ownerA, "agent-c", "ssh-other", keyC) });
    await f.a.mutation(ref("revokeDevice"), { deviceId: "agent-c" });
    expect(await f.a.query(query("revokedDevices"), {})).toEqual({
      devices: [{ deviceId: "agent-c", ...proof, revokedAt: expect.any(Number) }], cursor: null,
    });
  });
  test("a member cannot claim another device's SSH id to prune it by self-revoking", async () => {
    const f = await fixture();
    const victim = sshKey(31), attacker = sshKey(32);
    const victimProof = binding(ownerA, "agent-a", "ssh-victim", victim);
    await f.a.mutation(ref("register"), { device: f.ka.device, ssh: victimProof });
    const claims = [
      { sshDeviceId: "ssh-victim" },
      { ...victimProof },
      binding(ownerA, "agent-c", "ssh-victim", victim, attacker),
      { ...victimProof, sshSignature: "00".repeat(64) },
      binding(ownerA, "agent-c", "ssh-victim", attacker),
    ];
    for (const ssh of claims) {
      await expect(f.c.mutation(ref("register"), { device: f.kc.device, ssh } as never)).rejects.toThrow();
    }
    await f.c.mutation(ref("revokeDevice"), { deviceId: "agent-c" });
    expect(await f.a.query(query("revokedDevices"), {})).toEqual({
      devices: [{ deviceId: "agent-c", revokedAt: expect.any(Number) }], cursor: null,
    });
  });
  test("device rows written before revokedAt existed stay active and can be revoked", async () => {
    const f = await fixture();
    await f.backend.run(async ctx => {
      const row = await ctx.db.query("devices").withIndex("by_owner_device", q => q.eq("owner", f.kc.device.owner).eq("deviceId", "agent-c")).unique();
      const { _id, _creationTime, revokedAt, ...legacy } = row!;
      void revokedAt;
      await ctx.db.replace(_id, legacy);
    });
    await f.c.mutation(ref("register"), { device: f.kc.device });
    await expect(f.c.query(query("devices"), {})).resolves.toEqual(expect.any(Array));
    expect(await f.a.query(query("revokedDevices"), {})).toEqual({ devices: [], cursor: null });
    await f.a.mutation(ref("revokeDevice"), { deviceId: "agent-c" });
    await expect(f.c.mutation(ref("register"), { device: f.kc.device })).rejects.toThrow("revoked");
    expect((await f.a.query(query("revokedDevices"), {})).devices.map((d: { deviceId: string }) => d.deviceId)).toEqual(["agent-c"]);
  });
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
  test("exact device lookup is active, owner-scoped and survives a revoked historical prefix", async () => {
    const f = await fixture(), late = key("zz-active", ownerA, 201), memberDevice = key("member-late", ownerA, 202);
    await f.backend.run(async (ctx) => {
      for (let i = 0; i < 256; i++) {
        const historical = key(`000-revoked-${String(i).padStart(3, "0")}`, ownerA, (i % 180) + 20);
        await ctx.db.insert("devices", { ...historical.device, generation: 0, revokedAt: 1000 });
        await ctx.db.insert("credentials", { issuer, agentId: historical.device.deviceId, owner: ownerA, subject: "alice", status: "revoked" });
      }
      await ctx.db.insert("devices", { ...late.device, generation: 0, revokedAt: null });
      await ctx.db.insert("credentials", { issuer, agentId: late.device.deviceId, owner: ownerA, subject: "alice", status: "active" });
      await ctx.db.insert("devices", { ...memberDevice.device, generation: 0, revokedAt: null });
      await ctx.db.insert("credentials", { issuer, agentId: memberDevice.device.deviceId, owner: ownerA, subject: "alice-member", status: "active" });
    });
    expect(await f.a.query(query("devices"), {})).toEqual([]);
    expect(await f.a.query(query("device"), { deviceId: late.device.deviceId })).toEqual(late.device);
    await expect(f.a.query(query("device"), { deviceId: "000-revoked-000" })).rejects.toThrow("revoked");
    await expect(f.a.query(query("device"), { deviceId: f.kb.device.deviceId })).rejects.toThrow("registered");
    await expect(f.a.query(query("device"), { deviceId: "unknown-device" })).rejects.toThrow("registered");

    const next = key("agent-a", ownerA, 203), oldFingerprint = fingerprint(f.ka.device);
    const signature = hexString(nacl.sign.detached(rotationBytes(next.device, oldFingerprint), f.ka.pair.secretKey));
    await f.a.mutation(ref("rotate"), { device: next.device, oldFingerprint, signature });
    expect(await f.a.query(query("device"), { deviceId: "agent-a" })).toEqual(next.device);

    await f.a.mutation(ref("revokeAgent"), { deviceId: "agent-c" });
    await expect(f.c.query(query("device"), { deviceId: "agent-a" })).rejects.toThrow("revoked");
    await f.a.mutation(ref("revokeMember"), { subject: "alice-member" });
    await expect(f.a.query(query("device"), { deviceId: memberDevice.device.deviceId })).rejects.toThrow("membership");
  }, 30000);
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
    expect(second.envelopes).toEqual([]); expect(second.isDone).toBe(false);
    const third = await f.c.query(query("pending"), { deviceId: "agent-c", cursor: second.continueCursor });
    expect(third.envelopes).toEqual([{ envelope: valid, trust: "same-org" }]);
  });
  test("pending pages past expired rows with stable cursors as time advances", async () => {
    vi.useFakeTimers(); const now = Date.now();
    const f = await fixture();
    for (let i = 0; i < 33; i++) {
      await f.a.mutation(ref("enqueue"), { envelope: seal(f.ka, f.kc, { messageId: `expired-${i}`, idempotencyKey: `expired-${i}`, expiresAt: now + 1000 }) });
    }
    const fresh = seal(f.ka, f.kc, { messageId: "fresh-page", idempotencyKey: "fresh-page" });
    await f.a.mutation(ref("enqueue"), { envelope: fresh });
    vi.setSystemTime(now + 1001);
    const first = await f.c.query(query("pending"), { deviceId: "agent-c" });
    expect(first.envelopes).toEqual([]); expect(first.isDone).toBe(false);
    vi.setSystemTime(now + 2001);
    const second = await f.c.query(query("pending"), { deviceId: "agent-c", cursor: first.continueCursor });
    expect(second.envelopes).toEqual([{ envelope: fresh, trust: "same-org" }]);
    expect(second.isDone).toBe(true);
  });
  test("grant fixed-minute rate is shared across directions and retries do not consume quota", async () => {
    const f = await fixture(); await f.activate(1);
    const e = seal(f.ka, f.kb, { grantId: "grant-ab" });
    await f.a.mutation(ref("enqueue"), { envelope: e });
    await f.a.mutation(ref("enqueue"), { envelope: e });
    await expect(f.b.mutation(ref("enqueue"), { envelope: seal(f.kb, f.ka, { grantId: "grant-ab", messageId: "m2", idempotencyKey: "k2" }) })).rejects.toThrow("rate");
  });
  test("account quota spans issuer-provisioned credentials and retries do not consume it", async () => {
    vi.useFakeTimers(); const f = await fixture();
    const extra = key("agent-extra", ownerA, 11);
    await f.backend.mutation(ref("provisionIdentity"), { issuer, subject: "alice", org: "org-a", agentId: "agent-extra", role: "admin" });
    const client = f.backend.withIdentity({ ...f.identities[0], everett_agent: "agent-extra" });
    await client.mutation(ref("register"), { device: extra.device });
    const scope = JSON.stringify(["account", ownerA, issuer, "alice"]);
    await f.backend.run(ctx => ctx.db.insert("rates", { scope, window: Math.floor(Date.now() / 60000), count: 119 }));
    const e = seal(f.ka, f.kc);
    await f.a.mutation(ref("enqueue"), { envelope: e });
    await f.a.mutation(ref("enqueue"), { envelope: e });
    await expect(client.mutation(ref("enqueue"), { envelope: seal(extra, f.kc, { messageId: "over-account", idempotencyKey: "over-account" }) })).rejects.toThrow("rate");
    await f.c.mutation(ref("enqueue"), { envelope: seal(f.kc, f.ka, { messageId: "other-account", idempotencyKey: "other-account" }) });
    const rows = await f.backend.run(ctx => ctx.db.query("rates").collect());
    expect(rows.find(row => row.scope === scope)?.count).toBe(120);
    expect(rows.find(row => row.scope === JSON.stringify(["agent", ownerA, "agent-extra"]))).toBeUndefined();
    expect(await f.backend.run(ctx => ctx.db.query("attempts").collect())).toHaveLength(2);
  });
  test("organization quota spans subjects but never another organization", async () => {
    vi.useFakeTimers(); const f = await fixture();
    const scope = JSON.stringify(["org", ownerA]);
    await f.backend.run(ctx => ctx.db.insert("rates", { scope, window: Math.floor(Date.now() / 60000), count: 599 }));
    const e = seal(f.ka, f.kc);
    await f.a.mutation(ref("enqueue"), { envelope: e });
    await f.a.mutation(ref("enqueue"), { envelope: e });
    await expect(f.c.mutation(ref("enqueue"), { envelope: seal(f.kc, f.ka, { messageId: "over-org", idempotencyKey: "over-org" }) })).rejects.toThrow("rate");
    await f.b.mutation(ref("enqueue"), { envelope: seal(f.kb, f.kd, { messageId: "other-org", idempotencyKey: "other-org" }) });
    expect((await f.backend.run(ctx => ctx.db.query("rates").collect())).find(row => row.scope === scope)?.count).toBe(600);
    expect(await f.backend.run(ctx => ctx.db.query("attempts").collect())).toHaveLength(2);
  });
  test("recipient queue capacity uses ciphertext-free attempt metadata at 256 maximum-size envelopes", async () => {
    vi.useFakeTimers(); const f = await fixture(), now = Date.now();
    const first = seal(f.ka, f.kc, { messageId: "queued-0", idempotencyKey: "queued-0" });
    for (let i = 0; i < 256; i++) {
      vi.setSystemTime(now + Math.floor(i / 120) * 60000);
      const e = i === 0 ? first : seal(f.ka, f.kc, { messageId: `queued-${i}`, idempotencyKey: `queued-${i}` }, "x".repeat(65536));
      await f.a.mutation(ref("enqueue"), { envelope: e });
    }
    const overflow = seal(f.ka, f.kc, { messageId: "overflow", idempotencyKey: "overflow" });
    await expect(f.a.mutation(ref("enqueue"), { envelope: overflow })).rejects.toThrow("queue full");
    expect(await f.backend.run(ctx => ctx.db.query("attempts").collect())).toHaveLength(256);
    const agentScope = JSON.stringify(["agent", ownerA, "agent-a"]);
    expect((await f.backend.run(ctx => ctx.db.query("rates").collect())).find(row => row.scope === agentScope)?.count).toBe(16);
    await f.a.mutation(ref("enqueue"), { envelope: first });
    expect(await f.backend.run(ctx => ctx.db.query("messages").collect())).toHaveLength(256);
    expect(await f.backend.run(ctx => ctx.db.query("attempts").collect())).toHaveLength(256);
    await f.b.mutation(ref("enqueue"), { envelope: seal(f.kb, f.kd, { messageId: "other-recipient", idempotencyKey: "other-recipient" }) });
    await f.observe(f.c, f.kc, first);
    await f.a.mutation(ref("enqueue"), { envelope: overflow });
    vi.setSystemTime(overflow.expiresAt + 1);
    const fresh = seal(f.ka, f.kc, { messageId: "after-expiry", idempotencyKey: "after-expiry" });
    await f.a.mutation(ref("enqueue"), { envelope: fresh });
    // Four bounded 64-row batches clear the 256 expired ciphertext rows; the fifth is idempotent.
    for (let i = 0; i < 5; i++) await f.backend.mutation(ref("expire"), {});
    const remaining = await f.backend.run(ctx => ctx.db.query("messages").collect());
    expect(remaining).toHaveLength(1);
    expect(remaining[0]).toMatchObject(fresh);
    expect(await f.backend.run(ctx => ctx.db.query("attempts").collect())).toHaveLength(259);
    expect((await f.c.query(query("pending"), { deviceId: "agent-c" })).envelopes).toEqual([{ envelope: fresh, trust: "same-org" }]);
  }, 90000);
  test("queue capacity instrumentation rejects a ciphertext read and filters compact queued attempts", async () => {
    const calls: Array<[string, ...unknown[]]> = [];
    const predicates = {
      eq(field: string, value: unknown) { calls.push(["eq", field, value]); return predicates; },
      gt(field: string, value: unknown) { calls.push(["gt", field, value]); return predicates; },
    };
    const ctx = { db: {
      query(table: string) {
        calls.push(["query", table]);
        if (table === "messages") throw new Error("capacity must not materialize ciphertext messages");
        return { withIndex(index: string, apply: (q: typeof predicates) => unknown) {
          calls.push(["withIndex", index]); apply(predicates);
          return { take: async (limit: number) => Array.from({ length: limit }, () => ({ state: "queued" })) };
        } };
      },
    } };
    expect(await queuedAttemptCount(ctx as never, ownerA, "agent-c", 1234)).toBe(256);
    expect(calls).toEqual([
      ["query", "attempts"], ["withIndex", "by_recipient_state_expiry"],
      ["eq", "recipientOwner", ownerA], ["eq", "toDevice", "agent-c"], ["eq", "state", "queued"], ["gt", "expiresAt", 1234],
    ]);
  });
  test("maximum-size pending pages stay below the 8 MiB result limit and drain without loss", async () => {
    const f = await fixture();
    const expected = new Set<string>();
    for (let i = 0; i < 70; i++) {
      const e = seal(f.ka, f.kc, { messageId: `page-${i}`, idempotencyKey: `page-${i}` }, "x".repeat(65536));
      expected.add(e.messageId);
      await f.a.mutation(ref("enqueue"), { envelope: e });
    }
    const seen = new Set<string>();
    let cursor: string | null = null;
    for (let pageNumber = 0; pageNumber < 10; pageNumber++) {
      const page: { envelopes: Array<{ envelope: Envelope }>; isDone: boolean; continueCursor: string } = await f.c.query(query("pending"), { deviceId: "agent-c", cursor });
      expect(Buffer.byteLength(JSON.stringify(page), "utf8")).toBeLessThan(8 * 1024 * 1024);
      for (const row of page.envelopes) {
        expect(seen.has(row.envelope.messageId)).toBe(false);
        seen.add(row.envelope.messageId);
      }
      if (page.isDone) break;
      cursor = page.continueCursor;
    }
    expect(seen).toEqual(expected);
  }, 90000);
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
