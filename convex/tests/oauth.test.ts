import { convexTest } from "convex-test";
import { makeFunctionReference } from "convex/server";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import schema from "../schema";
import { s256 } from "../oauth";

const modules = {
  "../_generated/server.ts": async () => ({}),
  "../hosted.ts": () => import("../hosted"),
  "../oauth.ts": () => import("../oauth"),
  "../relay.ts": () => import("../relay"),
  "../grants.ts": () => import("../grants"),
  "../auth.ts": () => import("../auth"),
  "../http.ts": () => import("../http"),
};
const issuer = "https://hosted.convex.site";
const resource = "https://dot.example.com/mcp";
const redirectUri = "https://chatgpt.com/connector_platform_oauth_redirect";
const secret = "s".repeat(40);
const verifier = "v".repeat(43);
const decide = makeFunctionReference<"action">("oauth:decide");
const inspectRequest = makeFunctionReference<"query">("oauth:inspectRequest");
beforeEach(() => {
  vi.stubEnv("EVERETT_OWNER_GITHUB_ID", "12345");
  vi.stubEnv("EVERETT_OWNER_GITHUB_LOGIN", "raitoxlol");
  vi.stubEnv("CONVEX_SITE_URL", issuer);
  vi.stubEnv("EVERETT_OAUTH_RESOURCES", resource);
  vi.stubEnv("EVERETT_OAUTH_REDIRECT_URIS", redirectUri);
  vi.stubEnv("EVERETT_OAUTH_INTROSPECTION_SECRET", secret);
});
afterEach(() => { vi.unstubAllEnvs(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

async function fixture() {
  const backend = convexTest(schema, modules);
  const identity = async (githubId: string) => {
    const ids = await backend.run(async ctx => {
      const userId = await ctx.db.insert("users", {});
      const sessionId = await ctx.db.insert("authSessions", { userId, expirationTime: Date.now() + 86400_000 });
      await ctx.db.insert("authAccounts", { userId, provider: "github", providerAccountId: githubId });
      return { userId, sessionId };
    });
    return backend.withIdentity({ issuer, subject: `${ids.userId}|${ids.sessionId}` });
  };
  const owner = await identity("12345"), other = await identity("99999");
  const post = async (path: string, params: Record<string, string>, headers: Record<string, string> = {}) =>
    await backend.fetch(path, { method: "POST", headers: { "Content-Type": "application/x-www-form-urlencoded", ...headers }, body: new URLSearchParams(params).toString() });
  const register = async () => {
    const response = await backend.fetch("/oauth/register", { method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ client_name: "ChatGPT", redirect_uris: [redirectUri], token_endpoint_auth_method: "none" }) });
    expect(response.status).toBe(201);
    return (await response.json()).client_id as string;
  };
  const authorize = async (clientId: string, overrides: Record<string, string> = {}) => {
    const params = new URLSearchParams({ response_type: "code", client_id: clientId, redirect_uri: redirectUri, state: "st-1",
      code_challenge: s256(verifier), code_challenge_method: "S256", resource, scope: "everett", ...overrides });
    return await backend.fetch(`/oauth/authorize?${params}`);
  };
  const bindings = new Map<string, string>();
  const pending = async (clientId: string, overrides: Record<string, string> = {}) => {
    const response = await authorize(clientId, overrides);
    expect(response.status).toBe(302);
    const location = new URL(response.headers.get("Location")!);
    expect(location.origin + location.pathname).toBe(`${issuer}/enroll`);
    const requestId = location.searchParams.get("oauth")!;
    const cookie = response.headers.get("Set-Cookie")!;
    expect(cookie).toMatch(/Path=\/enroll; Max-Age=600; Secure; SameSite=Lax$/);
    const [id, binding] = cookie.split(";")[0].split("=")[1].split(".");
    expect(id).toBe(requestId);
    bindings.set(requestId, binding);
    return requestId;
  };
  const binding = (requestId: string) => bindings.get(requestId)!;
  const approve = async (clientId: string, name = "ChatGPT dot") => {
    const requestId = await pending(clientId);
    const { redirect } = await owner.action(decide, { requestId, approve: true, name, binding: binding(requestId) }) as { redirect: string };
    const url = new URL(redirect);
    expect(url.origin + url.pathname).toBe(redirectUri);
    expect(url.searchParams.get("iss")).toBe(issuer);
    expect(url.searchParams.get("state")).toBe("st-1");
    return url.searchParams.get("code")!;
  };
  const exchange = async (clientId: string, code: string, overrides: Record<string, string> = {}) =>
    await post("/oauth/token", { grant_type: "authorization_code", client_id: clientId, code, code_verifier: verifier, redirect_uri: redirectUri, resource, ...overrides });
  const refresh = async (clientId: string, refreshToken: string, overrides: Record<string, string> = {}) =>
    await post("/oauth/token", { grant_type: "refresh_token", client_id: clientId, refresh_token: refreshToken, resource, ...overrides });
  const introspect = async (token: string) =>
    await (await post("/oauth/introspect", { token }, { Authorization: `Bearer ${secret}` })).json();
  const tokens = async (clientId: string) => {
    const response = await exchange(clientId, await approve(clientId));
    expect(response.status).toBe(200);
    expect(response.headers.get("Cache-Control")).toBe("no-store");
    return await response.json() as { access_token: string; refresh_token: string; expires_in: number; scope: string; token_type: string };
  };
  return { backend, owner, other, post, register, authorize, pending, binding, approve, exchange, refresh, introspect, tokens };
}

describe("owner-only OAuth authorization server", () => {
  test("publishes RFC 8414 metadata with S256, iss, CIMD and public-client auth; unconfigured fails closed", async () => {
    const f = await fixture();
    const response = await f.backend.fetch("/.well-known/oauth-authorization-server");
    expect(response.status).toBe(200);
    expect(await response.json()).toMatchObject({
      issuer, authorization_endpoint: `${issuer}/oauth/authorize`, token_endpoint: `${issuer}/oauth/token`,
      registration_endpoint: `${issuer}/oauth/register`, code_challenge_methods_supported: ["S256"],
      authorization_response_iss_parameter_supported: true, client_id_metadata_document_supported: true,
      token_endpoint_auth_methods_supported: ["none"], scopes_supported: ["everett"],
      grant_types_supported: ["authorization_code", "refresh_token"],
    });
    vi.stubEnv("EVERETT_OAUTH_REDIRECT_URIS", "");
    expect((await f.backend.fetch("/.well-known/oauth-authorization-server")).status).toBe(503);
    vi.stubEnv("EVERETT_OAUTH_REDIRECT_URIS", redirectUri);
    vi.stubEnv("EVERETT_OWNER_GITHUB_ID", "");
    expect((await f.backend.fetch("/.well-known/oauth-authorization-server")).status).toBe(503);
  });
  test("each owner approval mints a distinct revocable agent identity bound to the resource", async () => {
    const f = await fixture();
    const clientId = await f.register();
    const one = await f.tokens(clientId), two = await f.tokens(clientId);
    expect(one).toMatchObject({ token_type: "Bearer", expires_in: 900, scope: "everett" });
    expect(one.access_token).toMatch(/^eva_[a-f0-9]{64}$/);
    expect(one.refresh_token).toMatch(/^evr_[a-f0-9]{64}$/);
    const a = await f.introspect(one.access_token), b = await f.introspect(two.access_token);
    expect(a).toMatchObject({ active: true, iss: issuer, aud: resource, client_id: clientId, scope: "everett", everett_agent_name: "ChatGPT dot" });
    expect(a.sub).toMatch(/^agent-[a-f0-9]{32}$/);
    expect(a.everett_agent).toBe(a.sub);
    expect(a.exp - a.iat).toBe(900);
    expect(b.sub).not.toBe(a.sub);
    const listed = await f.owner.query(makeFunctionReference<"query">("hosted:credentials"), {});
    expect(listed.credentials).toEqual(expect.arrayContaining([
      { agentId: a.sub, name: "ChatGPT dot", status: "active", kind: "oauth" },
      { agentId: b.sub, name: "ChatGPT dot", status: "active", kind: "oauth" },
    ]));
  });
  test("PKCE S256 is required at authorize and verified at token; codes are single use", async () => {
    const f = await fixture();
    const clientId = await f.register();
    for (const overrides of [{ code_challenge_method: "plain" }, { code_challenge: "short" }] as Record<string, string>[]) {
      const location = new URL((await f.authorize(clientId, overrides)).headers.get("Location")!);
      expect(location.origin + location.pathname).toBe(redirectUri);
      expect(Object.fromEntries(location.searchParams)).toMatchObject({ error: "invalid_request", state: "st-1", iss: issuer });
    }
    const wrong = await f.exchange(clientId, await f.approve(clientId), { code_verifier: "w".repeat(43) });
    expect(wrong.status).toBe(400);
    expect(await wrong.json()).toMatchObject({ error: "invalid_grant", error_description: "PKCE verification failed" });
    const code = await f.approve(clientId);
    const ok = await (await f.exchange(clientId, code)).json();
    expect((await f.introspect(ok.access_token)).active).toBe(true);
    const replay = await f.exchange(clientId, code);
    expect(await replay.json()).toMatchObject({ error: "invalid_grant" });
    expect(await f.introspect(ok.access_token)).toEqual({ active: false });
    expect(await (await f.refresh(clientId, ok.refresh_token)).json()).toMatchObject({ error: "invalid_grant" });
  });
  test("wrong GitHub owner cannot inspect or approve; deny returns access_denied with iss", async () => {
    const f = await fixture();
    const clientId = await f.register();
    const requestId = await f.pending(clientId);
    await expect(f.other.query(inspectRequest, { requestId })).rejects.toThrow("Owner only");
    await expect(f.other.action(decide, { requestId, approve: true, name: "x", binding: f.binding(requestId) })).rejects.toThrow("Owner only");
    await expect(f.backend.action(decide, { requestId, approve: true, name: "x", binding: f.binding(requestId) })).rejects.toThrow("Hosted sign in required");
    expect(await f.owner.query(inspectRequest, { requestId })).toMatchObject({ clientName: "ChatGPT", resource, redirectUri, scope: "everett" });
    const { redirect } = await f.owner.action(decide, { requestId, approve: false, name: "x", binding: f.binding(requestId) }) as { redirect: string };
    expect(Object.fromEntries(new URL(redirect).searchParams)).toMatchObject({ error: "access_denied", state: "st-1", iss: issuer });
    await expect(f.owner.action(decide, { requestId, approve: true, name: "x", binding: f.binding(requestId) })).rejects.toThrow("expired or unknown");
    expect(await f.backend.run(ctx => ctx.db.query("credentials").collect())).toEqual([]);
  });
  test("approval from a browser that did not start the request fails, even for the owner", async () => {
    const f = await fixture();
    const clientId = await f.register();
    // The attacker starts the flow in their own browser and forwards the /enroll link to the owner.
    const requestId = await f.pending(clientId);
    const victim = await f.pending(clientId);
    for (const wrong of ["", "0".repeat(64), f.binding(victim)]) {
      await expect(f.owner.action(decide, { requestId, approve: true, name: "x", binding: wrong })).rejects.toThrow("different browser");
    }
    expect(await f.backend.run(ctx => ctx.db.query("credentials").collect())).toEqual([]);
    expect(await f.owner.query(inspectRequest, { requestId })).toMatchObject({ clientName: "ChatGPT", redirectHost: "chatgpt.com" });
    await f.owner.action(decide, { requestId, approve: true, name: "x", binding: f.binding(requestId) });
  });
  test("unused DCR clients expire, the oldest unused is evicted when full, and limits are per source", async () => {
    const f = await fixture();
    const dcr = async (source: string) => await f.backend.fetch("/oauth/register", { method: "POST",
      headers: { "Content-Type": "application/json", "X-Forwarded-For": `${source}, 10.0.0.1` },
      body: JSON.stringify({ client_name: "ChatGPT", redirect_uris: [redirectUri], token_endpoint_auth_method: "none" }) });
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(new Date("2026-10-20T00:00:00Z"));
      const used = await f.register();
      await f.approve(used);
      for (let i = 0; i < 10; i++) expect((await dcr("198.51.100.7")).status).toBe(201);
      expect((await dcr("198.51.100.7")).status).toBe(400);
      expect((await dcr("203.0.113.9")).status).toBe(201);
      for (let minute = 1; minute <= 10; minute++) {
        vi.setSystemTime(new Date(Date.parse("2026-10-20T00:00:00Z") + minute * 61_000));
        for (let i = 0; i < 10; i++) expect((await dcr("198.51.100.7")).status).toBe(201);
      }
      const clients = await f.backend.run(ctx => ctx.db.query("oauthClients").collect());
      expect(clients.filter(c => c.expiresAt !== undefined)).toHaveLength(100);
      expect(clients.find(c => c.clientId === used)).not.toHaveProperty("expiresAt");
      vi.setSystemTime(new Date(Date.parse("2026-10-20T00:00:00Z") + 3 * 3600_000));
      await f.backend.mutation(makeFunctionReference<"mutation">("oauth:expire"), {});
      const left = await f.backend.run(ctx => ctx.db.query("oauthClients").collect());
      expect(left.map(c => c.clientId)).toEqual([used]);
    } finally { vi.useRealTimers(); }
  });
  test("resource indicator mismatches are rejected at authorize, token and refresh", async () => {
    const f = await fixture();
    const clientId = await f.register();
    const location = new URL((await f.authorize(clientId, { resource: "https://evil.example.com/mcp" })).headers.get("Location")!);
    expect(Object.fromEntries(location.searchParams)).toMatchObject({ error: "invalid_target", iss: issuer });
    const mismatch = await f.exchange(clientId, await f.approve(clientId), { resource: "https://other.example.com/mcp" });
    expect(await mismatch.json()).toMatchObject({ error: "invalid_target" });
    const t = await f.tokens(clientId);
    expect(await (await f.refresh(clientId, t.refresh_token, { resource: "https://other.example.com/mcp" })).json()).toMatchObject({ error: "invalid_target" });
    expect((await f.introspect(t.access_token)).aud).toBe(resource);
  });
  test("redirect URIs and clients outside the allowlist never receive a redirect", async () => {
    const f = await fixture();
    const clientId = await f.register();
    const bad = await f.authorize(clientId, { redirect_uri: "https://chatgpt.com/other" });
    expect(bad.status).toBe(400);
    expect(bad.headers.get("Location")).toBeNull();
    expect(await bad.text()).toContain("EVERETT_OAUTH_REDIRECT_URIS");
    expect((await f.authorize("evc_unknown")).status).toBe(400);
    const dcr = await f.backend.fetch("/oauth/register", { method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ redirect_uris: ["https://attacker.example/cb"] }) });
    expect(await dcr.json()).toMatchObject({ error: "invalid_redirect_uri" });
    const confidential = await f.backend.fetch("/oauth/register", { method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ redirect_uris: [redirectUri], token_endpoint_auth_method: "client_secret_basic" }) });
    expect(await confidential.json()).toMatchObject({ error: "invalid_client_metadata" });
  });
  test("refresh tokens rotate; replaying a rotated token revokes the whole family", async () => {
    const f = await fixture();
    const clientId = await f.register();
    const first = await f.tokens(clientId);
    const second = await (await f.refresh(clientId, first.refresh_token)).json();
    expect(second.refresh_token).not.toBe(first.refresh_token);
    expect(second.access_token).not.toBe(first.access_token);
    const a = await f.introspect(first.access_token), b = await f.introspect(second.access_token);
    expect(b.active).toBe(true);
    expect(b.sub).toBe(a.sub);
    expect(await (await f.refresh("evc_other", second.refresh_token)).json()).toMatchObject({ error: "invalid_grant" });
    expect(await (await f.refresh(clientId, first.refresh_token)).json()).toMatchObject({ error: "invalid_grant" });
    expect(await f.introspect(second.access_token)).toEqual({ active: false });
    expect(await (await f.refresh(clientId, second.refresh_token)).json()).toMatchObject({ error: "invalid_grant" });
  });
  test("owner revocation on /enroll invalidates issued access and refresh tokens", async () => {
    const f = await fixture();
    const clientId = await f.register();
    const t = await f.tokens(clientId);
    const { sub } = await f.introspect(t.access_token);
    await f.owner.mutation(makeFunctionReference<"mutation">("hosted:revoke"), { agentId: sub });
    expect(await f.introspect(t.access_token)).toEqual({ active: false });
    expect(await (await f.refresh(clientId, t.refresh_token)).json()).toMatchObject({ error: "invalid_grant" });
    const t2 = await f.tokens(clientId);
    await f.post("/oauth/revoke", { token: t2.refresh_token, client_id: clientId });
    expect(await f.introspect(t2.access_token)).toEqual({ active: false });
  });
  test("introspection requires the gateway secret and confidential token auth is refused", async () => {
    const f = await fixture();
    const clientId = await f.register();
    const t = await f.tokens(clientId);
    expect((await f.post("/oauth/introspect", { token: t.access_token })).status).toBe(401);
    expect((await f.post("/oauth/introspect", { token: t.access_token }, { Authorization: "Bearer wrong" })).status).toBe(401);
    const response = await f.post("/oauth/token", { grant_type: "refresh_token", client_id: clientId, refresh_token: t.refresh_token, client_secret: "x" });
    expect(response.status).toBe(401);
    expect(await f.introspect(t.refresh_token)).toEqual({ active: false });
  });
  test("client ID metadata documents are fetched, must self-identify and must allow public auth", async () => {
    const f = await fixture();
    const cimd = "https://chatgpt.com/oauth/client.json";
    const doc = { client_id: cimd, client_name: "ChatGPT", redirect_uris: [redirectUri], token_endpoint_auth_methods_supported: ["none", "private_key_jwt"], token_endpoint_auth_method: "private_key_jwt" };
    const fetchMock = vi.fn(async () => new Response(JSON.stringify(doc)));
    vi.stubGlobal("fetch", fetchMock);
    const t = await f.tokens(cimd);
    expect(fetchMock).toHaveBeenCalledWith(cimd, expect.objectContaining({ redirect: "error" }));
    expect((await f.introspect(t.access_token)).client_id).toBe(cimd);
    fetchMock.mockImplementation(async () => new Response(JSON.stringify({ ...doc, token_endpoint_auth_methods_supported: ["private_key_jwt"] })));
    expect((await f.authorize(cimd)).status).toBe(400);
    fetchMock.mockImplementation(async () => new Response(JSON.stringify({ ...doc, client_id: "https://evil.example/client.json" })));
    expect((await f.authorize(cimd)).status).toBe(400);
    vi.stubEnv("EVERETT_OAUTH_CLIENT_IDS", "https://example.com/other.json");
    fetchMock.mockImplementation(async () => new Response(JSON.stringify(doc)));
    expect((await f.authorize(cimd)).status).toBe(400);
  });
});
