import { convexTest } from "convex-test";
import { makeFunctionReference } from "convex/server";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import schema from "../schema";
import { checkGithub, hash } from "../hosted";
import { generateKeyPairSync } from "node:crypto";
import { enrollmentPage } from "../enrollmentPage";
import { key } from "./crypto.test-helper";

const modules = {
  "../_generated/server.ts": async () => ({}),
  "../hosted.ts": () => import("../hosted"),
  "../relay.ts": () => import("../relay"),
  "../grants.ts": () => import("../grants"),
  "../auth.ts": () => import("../auth"),
  "../http.ts": () => import("../http"),
};
const mutation = (name: string) => makeFunctionReference<"mutation">(`hosted:${name}`);
const query = (name: string) => makeFunctionReference<"query">(`hosted:${name}`);
const action = makeFunctionReference<"action">("hosted:begin");
const issuer = "https://hosted.convex.site";
const token = `evt_${"a".repeat(64)}`;
beforeEach(() => {
  vi.stubEnv("EVERETT_OWNER_GITHUB_ID", "12345");
  vi.stubEnv("EVERETT_OWNER_GITHUB_LOGIN", "raitoxlol");
  vi.stubEnv("CONVEX_SITE_URL", issuer);
});
afterEach(() => { vi.unstubAllEnvs(); vi.useRealTimers(); });
async function fixture() {
  const backend = convexTest(schema, modules);
  const identity = async (githubId: string) => {
    const ids = await backend.run(async ctx => {
      const userId = await ctx.db.insert("users", {});
      const sessionId = await ctx.db.insert("authSessions", { userId, expirationTime: Date.now() + 86400_000 });
      await ctx.db.insert("authAccounts", { userId, provider: "github", providerAccountId: githubId });
      return { userId, sessionId };
    });
    return { ...ids, client: backend.withIdentity({ issuer, subject: `${ids.userId}|${ids.sessionId}` }) };
  };
  const owner = await identity("12345"), other = await identity("99999");
  const begin = async (secret = token, name = "Devin") => await backend.action(action, { tokenHash: hash(secret), name }) as {
    agentId: string; deviceCode: string; userCode: string; expiresAt: number; verificationUri: string;
  };
  const enroll = async (secret = token) => {
    const enrollment = await begin(secret);
    await owner.client.mutation(mutation("approve"), { code: enrollment.userCode, approve: true });
    const authenticated = await backend.query(query("authenticateAgent"), { token: secret });
    const sessionId = await backend.run(ctx => ctx.db.insert("authSessions", { userId: authenticated.userId, expirationTime: Date.now() + 86400_000 }));
    const claims = await backend.query(query("claims"), { userId: authenticated.userId });
    const identity = { issuer, subject: `${authenticated.userId}|${sessionId}`, ...claims };
    return { enrollment, identity, sessionId, userId: authenticated.userId, client: backend.withIdentity(identity) };
  };
  return { backend, owner, other, begin, enroll };
}
describe("owner-only hosted enrollment", () => {
  test("GitHub profile must match numeric owner ID and login; missing allowlist fails closed", () => {
    expect(() => checkGithub({ id: 12345, login: "RAITOXLOL" })).not.toThrow();
    expect(() => checkGithub({ id: 99999, login: "raitoxlol" })).toThrow("Owner only");
    expect(() => checkGithub({ id: 12345, login: "somebody" })).toThrow("Owner only");
    vi.stubEnv("EVERETT_OWNER_GITHUB_ID", "");
    expect(() => checkGithub({ id: 12345, login: "raitoxlol" })).toThrow("allowlist");
  });
  test("only owner approval creates membership and a fresh agent; pending records contain only secret hashes", async () => {
    const f = await fixture(), e = await f.begin();
    expect(e.agentId).toMatch(/^agent-[a-f0-9]{32}$/);
    expect(e.verificationUri).toBe(`${issuer}/enroll`);
    const row = await f.backend.run(ctx => ctx.db.query("enrollments").unique());
    expect(JSON.stringify(row)).not.toContain(token);
    expect(JSON.stringify(row)).not.toContain(e.deviceCode);
    expect(await f.backend.run(ctx => ctx.db.query("credentials").collect())).toEqual([]);
    expect(await f.owner.client.query(query("inspect"), { code: e.userCode })).toMatchObject({ agentId: e.agentId, state: "pending" });
    await f.owner.client.mutation(mutation("approve"), { code: e.userCode, approve: true });
    expect(await f.backend.mutation(mutation("poll"), { deviceCode: e.deviceCode })).toMatchObject({ state: "approved", agentId: e.agentId });
    const rows = await f.backend.run(ctx => ctx.db.query("credentials").collect());
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ tokenHash: hash(token), agentId: e.agentId, status: "active" });
    expect(await f.backend.run(ctx => ctx.db.query("memberships").collect())).toHaveLength(1);
    expect(JSON.stringify(await f.owner.client.query(query("credentials"), {}))).not.toContain(hash(token));
  });
  test("anonymous, non-owner and generic OIDC identities cannot inspect, approve, list or revoke", async () => {
    const f = await fixture(), e = await f.begin();
    const generic = f.backend.withIdentity({ issuer: "https://external.example", subject: f.owner.userId });
    for (const client of [f.backend, f.other.client, generic]) {
      await expect(client.query(query("inspect"), { code: e.userCode })).rejects.toThrow();
      await expect(client.query(query("credentials"), {})).rejects.toThrow();
      await expect(client.mutation(mutation("approve"), { code: e.userCode, approve: true })).rejects.toThrow();
      await expect(client.mutation(mutation("revoke"), { agentId: e.agentId })).rejects.toThrow();
    }
    expect(await f.backend.run(ctx => ctx.db.query("credentials").collect())).toEqual([]);
  });
  test("denial, expired code, duplicate approval and wrong polling secret cannot create an agent", async () => {
    const f = await fixture(), e = await f.begin();
    expect(await f.backend.mutation(mutation("poll"), { deviceCode: "f".repeat(64) })).toEqual({ state: "expired" });
    await f.owner.client.mutation(mutation("approve"), { code: e.userCode, approve: false });
    await expect(f.owner.client.mutation(mutation("approve"), { code: e.userCode, approve: true })).rejects.toThrow();
    expect(await f.backend.mutation(mutation("poll"), { deviceCode: e.deviceCode })).toMatchObject({ state: "denied" });
    const expired = await f.begin();
    vi.spyOn(Date, "now").mockReturnValue(expired.expiresAt + 1);
    await expect(f.owner.client.mutation(mutation("approve"), { code: expired.userCode, approve: true })).rejects.toThrow();
    expect(await f.backend.mutation(mutation("poll"), { deviceCode: expired.deviceCode })).toEqual({ state: "expired" });
    expect(await f.backend.run(ctx => ctx.db.query("credentials").collect())).toEqual([]);
    vi.restoreAllMocks();
  });
  test("token authenticates exactly its agent, never the owner or another agent", async () => {
    const f = await fixture(), a = await f.enroll(), b = await f.enroll(`evt_${"b".repeat(64)}`);
    expect(a.userId).not.toBe(f.owner.userId);
    expect(a.userId).not.toBe(b.userId);
    expect(await a.client.query(makeFunctionReference<"query">("relay:whoami"), {})).toMatchObject({ deviceId: a.enrollment.agentId, role: "member" });
    await expect(f.owner.client.query(makeFunctionReference<"query">("relay:whoami"), {})).rejects.toThrow("Agent credential");
    await expect(a.client.mutation(mutation("approve"), { code: b.enrollment.userCode, approve: true })).rejects.toThrow("Owner only");
    await expect(a.client.query(query("credentials"), {})).rejects.toThrow("Owner only");
    const forged = f.backend.withIdentity({ ...a.identity, everett_agent: b.enrollment.agentId });
    await expect(forged.query(makeFunctionReference<"query">("relay:whoami"), {})).rejects.toThrow("mismatch");
    const device = key(b.enrollment.agentId, JSON.stringify([issuer, "org", f.owner.userId]), 1).device;
    await expect(a.client.mutation(makeFunctionReference<"mutation">("relay:register"), { device })).rejects.toThrow("agent");
    await expect(f.backend.query(query("authenticateAgent"), { token: `evt_${"c".repeat(64)}` })).rejects.toThrow("denied");
    await expect(f.backend.query(query("authenticateAgent"), { token: "owner-token" })).rejects.toThrow("Invalid agent");
  });
  test("revocation blocks fresh auth, custom claims and existing JWT relay operations; never recycles IDs", async () => {
    const f = await fixture(), a = await f.enroll(), b = await f.enroll(`evt_${"b".repeat(64)}`);
    await f.owner.client.mutation(mutation("revoke"), { agentId: a.enrollment.agentId });
    await expect(f.backend.query(query("authenticateAgent"), { token })).rejects.toThrow("revoked");
    await expect(f.backend.query(query("claims"), { userId: a.userId })).rejects.toThrow("revoked");
    for (const [kind, name, args] of [
      ["query", "relay:whoami", {}], ["query", "relay:devices", {}], ["query", "relay:pending", { deviceId: a.enrollment.agentId }],
      ["mutation", "hosted:revokeSelf", {}],
    ] as const) {
      if (kind === "query") await expect(a.client.query(makeFunctionReference<"query">(name), args)).rejects.toThrow("revoked");
      else await expect(a.client.mutation(makeFunctionReference<"mutation">(name), args)).rejects.toThrow("revoked");
    }
    expect(await b.client.query(makeFunctionReference<"query">("relay:whoami"), {})).toMatchObject({ deviceId: b.enrollment.agentId });
    const e = await f.begin(`evt_${"d".repeat(64)}`);
    await f.backend.run(async ctx => { const row = await ctx.db.query("enrollments").withIndex("by_code", q => q.eq("code", e.userCode)).unique(); await ctx.db.patch(row!._id, { agentId: a.enrollment.agentId }); });
    await expect(f.owner.client.mutation(mutation("approve"), { code: e.userCode, approve: true })).rejects.toThrow("already used");
    expect(await f.backend.run(ctx => ctx.db.query("credentials").collect())).toHaveLength(2);
  });
  test("membership, session, owner allowlist and self revocation gate still-valid sessions", async () => {
    const f = await fixture(), a = await f.enroll();
    await a.client.mutation(mutation("revokeSelf"), {});
    await expect(f.backend.query(query("authenticateAgent"), { token })).rejects.toThrow("revoked");
    const b = await f.enroll(`evt_${"b".repeat(64)}`);
    vi.stubEnv("EVERETT_OWNER_GITHUB_ID", "77777");
    await expect(b.client.query(makeFunctionReference<"query">("relay:whoami"), {})).rejects.toThrow("Owner only");
    vi.stubEnv("EVERETT_OWNER_GITHUB_ID", "12345");
    await f.backend.run(ctx => ctx.db.delete(b.sessionId));
    await expect(b.client.query(makeFunctionReference<"query">("relay:whoami"), {})).rejects.toThrow("session revoked");
    const c = await f.enroll(`evt_${"c".repeat(64)}`);
    await f.backend.run(async ctx => { const row = await ctx.db.query("memberships").unique(); await ctx.db.patch(row!._id, { status: "revoked" }); });
    await expect(f.backend.query(query("authenticateAgent"), { token: `evt_${"c".repeat(64)}` })).rejects.toThrow("membership");
    await expect(c.client.query(makeFunctionReference<"query">("relay:whoami"), {})).rejects.toThrow("membership");
    const e = await f.begin();
    await expect(f.owner.client.mutation(mutation("approve"), { code: e.userCode, approve: true })).rejects.toThrow("tombstone");
  });
  test("limits public enrollment and validates names and token hashes", async () => {
    const f = await fixture();
    await expect(f.backend.action(action, { tokenHash: token, name: "test" })).rejects.toThrow("Invalid enrollment");
    await expect(f.begin(token, "bad\nname")).rejects.toThrow("Invalid enrollment");
    for (let i = 0; i < 30; i++) await f.begin();
    await expect(f.begin()).rejects.toThrow("rate limit");
  });
  test("real Convex Auth agent provider mints scoped JWTs; revocation denies refresh and fresh sign-in", async () => {
    const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
    vi.stubEnv("JWT_PRIVATE_KEY", privateKey.export({ type: "pkcs8", format: "pem" }).toString());
    const f = await fixture(), a = await f.enroll();
    const signIn = makeFunctionReference<"action">("auth:signIn");
    const response = await f.backend.action(signIn, { provider: "agent", params: { token } });
    const claims = JSON.parse(Buffer.from(response.tokens.token.split(".")[1], "base64url").toString());
    expect(claims).toMatchObject({ iss: issuer, aud: "convex", everett_agent: a.enrollment.agentId, everett_org: f.owner.userId });
    expect(claims.exp - claims.iat).toBeLessThanOrEqual(900);
    const agent = f.backend.withIdentity({ issuer: claims.iss, subject: claims.sub, everett_agent: claims.everett_agent, everett_org: claims.everett_org });
    expect(await agent.query(makeFunctionReference<"query">("relay:whoami"), {})).toMatchObject({ deviceId: a.enrollment.agentId, role: "member" });
    await f.owner.client.mutation(mutation("revoke"), { agentId: a.enrollment.agentId });
    await expect(agent.query(makeFunctionReference<"query">("relay:whoami"), {})).rejects.toThrow("revoked");
    await expect(f.backend.action(signIn, { provider: "agent", params: { token } })).rejects.toThrow("revoked");
    await expect(f.backend.action(signIn, { refreshToken: response.tokens.refreshToken })).rejects.toThrow("revoked");
  });
  test("approval page script parses; HTTP serves no-store, frame-blocking CSP without third-party scripts", async () => {
    const cloud = "https://hosted.convex.cloud";
    vi.stubEnv("CONVEX_CLOUD_URL", cloud);
    const page = enrollmentPage(cloud);
    const script = page.split("<script>")[1].split("</script>")[0];
    expect(() => new Function(script)).not.toThrow();
    const f = await fixture();
    const response = await f.backend.fetch("/enroll");
    expect(response.status).toBe(200);
    expect(response.headers.get("Cache-Control")).toBe("no-store");
    expect(response.headers.get("Content-Security-Policy")).toContain("frame-ancestors 'none'");
    expect(response.headers.get("Content-Security-Policy")).toContain("script-src 'sha256-");
    expect(await response.text()).toBe(page);
  });
});
