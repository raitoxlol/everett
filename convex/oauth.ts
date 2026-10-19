import {
  actionGeneric as action, httpActionGeneric as httpAction, queryGeneric as query,
  internalMutationGeneric as internalMutation, internalQueryGeneric as internalQuery, makeFunctionReference,
} from "convex/server";
import { v, type GenericId } from "convex/values";
import { sha256 } from "@noble/hashes/sha256";
import { ensureOwnerMembership, hash, ownerGithubId, ownerIdentity, random } from "./hosted";
import type { Context, WriteContext } from "./authorization";

export const SCOPE = "everett";
const ACCESS_MS = 15 * 60_000, REFRESH_MS = 30 * 86400_000, CODE_MS = 5 * 60_000, REQUEST_MS = 10 * 60_000;
const internalRef = <T extends "query" | "mutation">(kind: T, name: string) => makeFunctionReference<T>(`oauth:${name}`);
const list = (name: string) => (process.env[name] ?? "").split(",").map(s => s.trim()).filter(Boolean);
const b64url = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
export const s256 = (verifier: string) => b64url(sha256(new TextEncoder().encode(verifier)));
const json = (body: unknown, status = 200, extra: Record<string, string> = {}) => new Response(JSON.stringify(body), { status,
  headers: { "Content-Type": "application/json", "Cache-Control": "no-store", Pragma: "no-cache", ...extra } });
const text = (body: string, status: number) => new Response(body, { status,
  headers: { "Content-Type": "text/plain; charset=utf-8", "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff" } });
const found = (location: string) => new Response(null, { status: 302, headers: { Location: location, "Cache-Control": "no-store" } });
const oauthError = (error: string, description: string, status = 400) => json({ error, error_description: description }, status);
class OAuthError extends Error { constructor(readonly code: string, message: string) { super(message); } }

function exactUrl(raw: string, kind: string, plaintextLoopback: boolean) {
  const url = new URL(raw);
  const loopback = url.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname);
  if ((url.protocol !== "https:" && !(plaintextLoopback && loopback)) || url.hash || url.username || url.password || url.href !== raw) {
    throw new Error(`${kind} must be an exact absolute https URL: ${raw}`);
  }
  return raw;
}
export function oauthConfig() {
  ownerGithubId();
  const issuer = process.env.CONVEX_SITE_URL;
  if (!issuer) throw new Error("CONVEX_SITE_URL is not configured");
  const resources = list("EVERETT_OAUTH_RESOURCES").map(r => exactUrl(r, "EVERETT_OAUTH_RESOURCES", true));
  const redirects = list("EVERETT_OAUTH_REDIRECT_URIS").map(r => exactUrl(r, "EVERETT_OAUTH_REDIRECT_URIS", false));
  const clients = list("EVERETT_OAUTH_CLIENT_IDS").map(r => exactUrl(r, "EVERETT_OAUTH_CLIENT_IDS", false));
  if (!resources.length || !redirects.length) throw new Error("OAuth is not configured: set EVERETT_OAUTH_RESOURCES and EVERETT_OAUTH_REDIRECT_URIS");
  return { issuer, resources, redirects, clients };
}
export function metadata(issuer: string) {
  return {
    issuer,
    authorization_endpoint: `${issuer}/oauth/authorize`,
    token_endpoint: `${issuer}/oauth/token`,
    registration_endpoint: `${issuer}/oauth/register`,
    revocation_endpoint: `${issuer}/oauth/revoke`,
    introspection_endpoint: `${issuer}/oauth/introspect`,
    scopes_supported: [SCOPE],
    response_types_supported: ["code"],
    response_modes_supported: ["query"],
    grant_types_supported: ["authorization_code", "refresh_token"],
    code_challenge_methods_supported: ["S256"],
    token_endpoint_auth_methods_supported: ["none"],
    revocation_endpoint_auth_methods_supported: ["none"],
    authorization_response_iss_parameter_supported: true,
    client_id_metadata_document_supported: true,
  };
}

type Client = { clientId: string; name: string; redirectUris: string[] };
const strings = (value: unknown) => Array.isArray(value) && value.every(x => typeof x === "string") ? value as string[] : null;
const cleanName = (value: unknown, fallback: string) =>
  typeof value === "string" && value.trim() && value.length <= 100 && !/[\x00-\x1f\x7f]/.test(value) ? value.trim() : fallback;
// A client must be able to use public PKCE ("none"); confidential-only clients are refused.
function acceptsPublicAuth(doc: Record<string, unknown>) {
  const many = strings(doc.token_endpoint_auth_methods_supported);
  if (many) return many.includes("none");
  return doc.token_endpoint_auth_method === undefined || doc.token_endpoint_auth_method === "none";
}
async function fetchClientMetadata(clientId: string, allowed: string[]): Promise<Client> {
  const url = new URL(clientId);
  if (url.protocol !== "https:" || url.pathname === "/" || url.hash || url.username || url.password) throw new OAuthError("invalid_client", "client_id is not a valid metadata document URL");
  if (allowed.length && !allowed.includes(clientId)) throw new OAuthError("invalid_client", "client_id is not on EVERETT_OAUTH_CLIENT_IDS");
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new OAuthError("invalid_client", "client metadata fetch timed out")), 5000); });
  try {
    const response = await Promise.race([fetch(clientId, { redirect: "error", headers: { Accept: "application/json" } }), timeout]);
    if (!response.ok) throw new OAuthError("invalid_client", `client metadata returned HTTP ${response.status}`);
    const body = await Promise.race([response.text(), timeout]);
    if (body.length > 16_384) throw new OAuthError("invalid_client", "client metadata is too large");
    const doc = JSON.parse(body) as Record<string, unknown>;
    const redirectUris = strings(doc.redirect_uris);
    if (doc.client_id !== clientId || !redirectUris?.length) throw new OAuthError("invalid_client", "client metadata must repeat client_id and list redirect_uris");
    if (!acceptsPublicAuth(doc)) throw new OAuthError("invalid_client", "client must support token_endpoint_auth_method none");
    return { clientId, name: cleanName(doc.client_name, url.host), redirectUris };
  } catch (error) {
    throw error instanceof OAuthError ? error : new OAuthError("invalid_client", "client metadata document is unreadable");
  } finally { clearTimeout(timer); }
}

export const client = internalQuery({
  args: { clientId: v.string() }, handler: async (ctx: Context, { clientId }) =>
    await ctx.db.query("oauthClients").withIndex("by_client", q => q.eq("clientId", clientId)).unique(),
});
async function limit(ctx: WriteContext, scope: string, max: number) {
  const window = Math.floor(Date.now() / 60_000);
  const rate = await ctx.db.query("rates").withIndex("by_scope", q => q.eq("scope", scope)).unique();
  if (rate?.window === window && rate.count >= max) throw new OAuthError("temporarily_unavailable", "rate limited");
  if (rate) await ctx.db.patch(rate._id, { window, count: rate.window === window ? rate.count + 1 : 1 });
  else await ctx.db.insert("rates", { scope, window, count: 1 });
}
export const register = internalMutation({
  args: { clientId: v.string(), name: v.string(), redirectUris: v.array(v.string()) }, handler: async (ctx: WriteContext, args) => {
    await limit(ctx, "oauth-register", 10);
    if ((await ctx.db.query("oauthClients").take(100)).length >= 100) throw new OAuthError("invalid_client_metadata", "client registration limit reached");
    await ctx.db.insert("oauthClients", { ...args, createdAt: Date.now() });
  },
});
export const createRequest = internalMutation({
  args: { requestId: v.string(), clientId: v.string(), clientName: v.string(), redirectUri: v.string(), codeChallenge: v.string(),
    state: v.union(v.null(), v.string()), scope: v.string(), resource: v.string(), expiresAt: v.number() },
  handler: async (ctx: WriteContext, args) => {
    await limit(ctx, "oauth-authorize", 30);
    if ((await ctx.db.query("oauthRequests").take(200)).length >= 200) throw new OAuthError("temporarily_unavailable", "too many pending authorizations");
    await ctx.db.insert("oauthRequests", args);
  },
});
function redirectWith(redirectUri: string, params: Record<string, string | null>) {
  const url = new URL(redirectUri);
  for (const [k, value] of Object.entries(params)) if (value !== null) url.searchParams.set(k, value);
  return url.href;
}

export const inspectRequest = query({
  args: { requestId: v.string() }, handler: async (ctx: Context, { requestId }) => {
    await ownerIdentity(ctx);
    const row = await ctx.db.query("oauthRequests").withIndex("by_request", q => q.eq("requestId", requestId)).unique();
    if (!row || row.expiresAt <= Date.now()) throw new Error("Authorization request expired or unknown");
    return { clientName: row.clientName, clientId: row.clientId, redirectUri: row.redirectUri, resource: row.resource, scope: row.scope };
  },
});
export const decide = action({
  args: { requestId: v.string(), approve: v.boolean(), name: v.string() }, handler: async (ctx, args): Promise<{ redirect: string }> => {
    const code = random(), agentId = `agent-${random().slice(0, 32)}`;
    return await ctx.runMutation(internalRef("mutation", "recordDecision"), { ...args, code, agentId });
  },
});
export const recordDecision = internalMutation({
  args: { requestId: v.string(), approve: v.boolean(), name: v.string(), code: v.string(), agentId: v.string() },
  handler: async (ctx: WriteContext, args) => {
    const owner = await ownerIdentity(ctx);
    const { issuer } = oauthConfig();
    const row = await ctx.db.query("oauthRequests").withIndex("by_request", q => q.eq("requestId", args.requestId)).unique();
    if (!row || row.expiresAt <= Date.now()) throw new Error("Authorization request expired or unknown");
    await ctx.db.delete(row._id);
    if (!args.approve) return { redirect: redirectWith(row.redirectUri, { error: "access_denied", error_description: "owner denied the request", state: row.state, iss: issuer }) };
    const name = args.name.trim();
    if (!name || name.length > 100 || /[\x00-\x1f\x7f]/.test(name)) throw new Error("Invalid agent name");
    await ensureOwnerMembership(ctx, owner);
    if (await ctx.db.query("credentials").withIndex("by_agent", q => q.eq("issuer", owner.issuer).eq("agentId", args.agentId)).unique()) throw new Error("Agent ID already used");
    const credential = await ctx.db.insert("credentials", { owner: owner.owner, issuer: owner.issuer, subject: owner.userId,
      agentId: args.agentId, status: "active", name, kind: "oauth", clientId: row.clientId });
    await ctx.db.insert("oauthCodes", { codeHash: hash(args.code), credential, clientId: row.clientId, redirectUri: row.redirectUri,
      codeChallenge: row.codeChallenge, resource: row.resource, scope: row.scope, expiresAt: Date.now() + CODE_MS, used: false });
    return { redirect: redirectWith(row.redirectUri, { code: args.code, state: row.state, iss: issuer }) };
  },
});

// Every OAuth use re-checks the credential, the owner's membership and the GitHub owner allowlist.
async function liveCredential(ctx: Context, id: GenericId<"credentials">) {
  const credential = await ctx.db.get(id);
  if (!credential || credential.status !== "active" || credential.kind !== "oauth") return null;
  const member = await ctx.db.query("memberships").withIndex("by_identity", q =>
    q.eq("owner", credential.owner).eq("issuer", credential.issuer).eq("subject", credential.subject)).unique();
  if (!member || member.status !== "active" || credential.issuer !== process.env.CONVEX_SITE_URL) return null;
  const userId = ctx.db.normalizeId("users", credential.subject);
  const account = userId && await ctx.db.query("authAccounts").withIndex("userIdAndProvider", q =>
    q.eq("userId", userId).eq("provider", "github")).unique();
  if (!account || account.providerAccountId !== ownerGithubId()) return null;
  return credential;
}
async function revokeFamily(ctx: WriteContext, family: string) {
  for (const row of await ctx.db.query("oauthTokens").withIndex("by_family", q => q.eq("family", family)).collect()) {
    if (!row.revoked) await ctx.db.patch(row._id, { revoked: true });
  }
}
type Grant = { error: string; description: string } | { access: string; refresh: string; scope: string; expiresIn: number };
async function issue(ctx: WriteContext, base: { family: string; credential: GenericId<"credentials">; clientId: string; resource: string; scope: string },
  access: string, refresh: string): Promise<Grant> {
  const now = Date.now();
  await ctx.db.insert("oauthTokens", { ...base, tokenHash: hash(access), kind: "access", issuedAt: now, expiresAt: now + ACCESS_MS, used: false, revoked: false });
  await ctx.db.insert("oauthTokens", { ...base, tokenHash: hash(refresh), kind: "refresh", issuedAt: now, expiresAt: now + REFRESH_MS, used: false, revoked: false });
  return { access, refresh, scope: base.scope, expiresIn: ACCESS_MS / 1000 };
}
const denied = (description: string, error = "invalid_grant"): Grant => ({ error, description });
export const exchange = internalMutation({
  args: { grantType: v.string(), clientId: v.string(), code: v.optional(v.string()), codeVerifier: v.optional(v.string()),
    redirectUri: v.optional(v.string()), refreshToken: v.optional(v.string()), resource: v.optional(v.string()),
    scope: v.optional(v.string()), access: v.string(), refresh: v.string() },
  handler: async (ctx: WriteContext, a): Promise<Grant> => {
    oauthConfig();
    if (a.grantType === "authorization_code") {
      const row = a.code && await ctx.db.query("oauthCodes").withIndex("by_hash", q => q.eq("codeHash", hash(a.code!))).unique();
      if (!row) return denied("unknown authorization code");
      if (row.used) { await revokeFamily(ctx, row.codeHash); return denied("authorization code already used; its tokens are revoked"); }
      await ctx.db.patch(row._id, { used: true });
      if (row.expiresAt <= Date.now()) return denied("authorization code expired");
      if (row.clientId !== a.clientId || row.redirectUri !== a.redirectUri) return denied("client_id or redirect_uri does not match the authorization");
      if (!a.codeVerifier || !/^[A-Za-z0-9._~-]{43,128}$/.test(a.codeVerifier) || s256(a.codeVerifier) !== row.codeChallenge) return denied("PKCE verification failed");
      if (a.resource !== undefined && a.resource !== row.resource) return denied("resource does not match the authorization", "invalid_target");
      if (!await liveCredential(ctx, row.credential)) return denied("agent credential revoked or owner no longer allowed");
      return await issue(ctx, { family: row.codeHash, credential: row.credential, clientId: row.clientId, resource: row.resource, scope: row.scope }, a.access, a.refresh);
    }
    if (a.grantType === "refresh_token") {
      const row = a.refreshToken && await ctx.db.query("oauthTokens").withIndex("by_hash", q => q.eq("tokenHash", hash(a.refreshToken!))).unique();
      if (!row || row.kind !== "refresh" || row.revoked) return denied("refresh token revoked or unknown");
      if (row.used) { await revokeFamily(ctx, row.family); return denied("refresh token already rotated; its family is revoked"); }
      if (row.expiresAt <= Date.now()) return denied("refresh token expired");
      if (row.clientId !== a.clientId) return denied("client_id does not match the refresh token");
      if (a.resource !== undefined && a.resource !== row.resource) return denied("resource does not match the refresh token", "invalid_target");
      if (a.scope !== undefined && a.scope.split(" ").some(s => !row.scope.split(" ").includes(s))) return denied("scope exceeds the original grant", "invalid_scope");
      if (!await liveCredential(ctx, row.credential)) { await revokeFamily(ctx, row.family); return denied("agent credential revoked or owner no longer allowed"); }
      await ctx.db.patch(row._id, { used: true });
      return await issue(ctx, { family: row.family, credential: row.credential, clientId: row.clientId, resource: row.resource, scope: row.scope }, a.access, a.refresh);
    }
    return denied("grant_type must be authorization_code or refresh_token", "unsupported_grant_type");
  },
});
export const introspect = internalQuery({
  args: { token: v.string() }, handler: async (ctx: Context, { token }) => {
    const { issuer } = oauthConfig();
    const row = await ctx.db.query("oauthTokens").withIndex("by_hash", q => q.eq("tokenHash", hash(token))).unique();
    if (!row || row.kind !== "access" || row.revoked || row.expiresAt <= Date.now()) return { active: false };
    const credential = await liveCredential(ctx, row.credential);
    if (!credential) return { active: false };
    return { active: true, iss: issuer, aud: row.resource, sub: credential.agentId, client_id: row.clientId, scope: row.scope,
      token_type: "Bearer", iat: Math.floor(row.issuedAt / 1000), exp: Math.floor(row.expiresAt / 1000),
      everett_agent: credential.agentId, everett_agent_name: credential.name ?? "" };
  },
});
export const revokeToken = internalMutation({
  args: { token: v.string(), clientId: v.optional(v.string()) }, handler: async (ctx: WriteContext, { token, clientId }) => {
    const row = await ctx.db.query("oauthTokens").withIndex("by_hash", q => q.eq("tokenHash", hash(token))).unique();
    if (row && (clientId === undefined || clientId === row.clientId)) await revokeFamily(ctx, row.family);
  },
});
export const expire = internalMutation({
  args: {}, handler: async (ctx: WriteContext) => {
    const now = Date.now();
    for (const table of ["oauthRequests", "oauthCodes", "oauthTokens"] as const) {
      for (const row of await ctx.db.query(table).withIndex("by_expiry", q => q.lte("expiresAt", now)).take(500)) await ctx.db.delete(row._id);
    }
  },
});

async function form(request: Request) {
  if (!(request.headers.get("content-type") ?? "").toLowerCase().startsWith("application/x-www-form-urlencoded")) return null;
  const body = await request.text();
  if (body.length > 8192) return null;
  const params = new URLSearchParams(body);
  const out: Record<string, string> = {};
  for (const [k, value] of params) { if (k in out) return null; out[k] = value; }
  return out;
}
function configured() {
  try { return oauthConfig(); } catch { return null; }
}
export const authorizationServerMetadata = httpAction(async () => {
  const cfg = configured();
  if (!cfg) return text("OAuth is not configured on this deployment", 503);
  return json(metadata(cfg.issuer), 200, { "Access-Control-Allow-Origin": "*", "Cache-Control": "max-age=300" });
});
export const authorize = httpAction(async (ctx, request) => {
  const cfg = configured();
  if (!cfg) return text("OAuth is not configured on this deployment", 503);
  const params = new URL(request.url).searchParams;
  const one = (k: string) => { const all = params.getAll(k); return all.length === 1 ? all[0] : all.length ? undefined : null; };
  const clientId = one("client_id"), redirectUri = one("redirect_uri");
  if (!clientId || !redirectUri) return text("client_id and redirect_uri are required exactly once", 400);
  let client: Client;
  try {
    const row = clientId.startsWith("https://") ? null : await ctx.runQuery(internalRef("query", "client"), { clientId }) as Client | null;
    client = row ?? await fetchClientMetadata(clientId, cfg.clients);
  } catch (error) { return text(`Unknown OAuth client: ${(error as Error).message}`, 400); }
  if (!client.redirectUris.includes(redirectUri) || !cfg.redirects.includes(redirectUri)) {
    return text(`redirect_uri is not allowlisted. Add it to EVERETT_OAUTH_REDIRECT_URIS exactly: ${redirectUri}`, 400);
  }
  const state = one("state");
  const fail = (error: string, description: string) => found(redirectWith(redirectUri, { error, error_description: description, state: state ?? null, iss: cfg.issuer }));
  if (state === undefined || (state && state.length > 512)) return fail("invalid_request", "state must appear once and be at most 512 characters");
  if (one("response_type") !== "code") return fail("unsupported_response_type", "response_type must be code");
  const challenge = one("code_challenge");
  if (one("code_challenge_method") !== "S256" || !challenge || !/^[A-Za-z0-9_-]{43}$/.test(challenge)) return fail("invalid_request", "PKCE S256 code_challenge is required");
  const resource = one("resource");
  if (!resource || !cfg.resources.includes(resource)) return fail("invalid_target", "resource must be one of the configured Everett gateway URLs");
  const scope = one("scope");
  if (scope === undefined || (scope && scope.split(" ").some(s => s !== SCOPE))) return fail("invalid_scope", `only scope ${SCOPE} is supported`);
  const requestId = random();
  try {
    await ctx.runMutation(internalRef("mutation", "createRequest"), { requestId, clientId, clientName: client.name, redirectUri,
      codeChallenge: challenge, state, scope: SCOPE, resource, expiresAt: Date.now() + REQUEST_MS });
  } catch (error) { return fail("temporarily_unavailable", (error as Error).message); }
  return found(`${cfg.issuer}/enroll?oauth=${requestId}`);
});
export const token = httpAction(async (ctx, request) => {
  if (!configured()) return oauthError("temporarily_unavailable", "OAuth is not configured", 503);
  const p = await form(request);
  if (!p) return oauthError("invalid_request", "send one application/x-www-form-urlencoded value per parameter");
  if (request.headers.get("authorization") || p.client_secret || p.client_assertion) return oauthError("invalid_client", "only public PKCE clients (token_endpoint_auth_method none) are supported", 401);
  if (!p.client_id) return oauthError("invalid_request", "client_id is required");
  const result = await ctx.runMutation(internalRef("mutation", "exchange"), {
    grantType: p.grant_type ?? "", clientId: p.client_id, code: p.code, codeVerifier: p.code_verifier, redirectUri: p.redirect_uri,
    refreshToken: p.refresh_token, resource: p.resource, scope: p.scope, access: `eva_${random()}`, refresh: `evr_${random()}`,
  }) as Grant;
  if ("error" in result) return oauthError(result.error, result.description);
  return json({ access_token: result.access, token_type: "Bearer", expires_in: result.expiresIn, refresh_token: result.refresh, scope: result.scope });
});
export const registerClient = httpAction(async (ctx, request) => {
  const cfg = configured();
  if (!cfg) return oauthError("temporarily_unavailable", "OAuth is not configured", 503);
  const body = await request.text();
  let doc: Record<string, unknown> | null = null;
  try { if (body.length <= 16_384) doc = JSON.parse(body); } catch { /* reported below */ }
  if (!doc || typeof doc !== "object" || Array.isArray(doc)) return oauthError("invalid_client_metadata", "send a JSON client metadata document");
  const redirectUris = strings(doc.redirect_uris);
  if (!redirectUris?.length || redirectUris.some(u => !cfg.redirects.includes(u))) {
    return oauthError("invalid_redirect_uri", "every redirect_uri must be on EVERETT_OAUTH_REDIRECT_URIS");
  }
  if (doc.token_endpoint_auth_method !== undefined && doc.token_endpoint_auth_method !== "none") return oauthError("invalid_client_metadata", "token_endpoint_auth_method must be none");
  const grants = strings(doc.grant_types ?? ["authorization_code", "refresh_token"]);
  if (!grants || grants.some(g => g !== "authorization_code" && g !== "refresh_token")) return oauthError("invalid_client_metadata", "unsupported grant_types");
  const clientId = `evc_${random().slice(0, 32)}`, name = cleanName(doc.client_name, "OAuth client");
  try { await ctx.runMutation(internalRef("mutation", "register"), { clientId, name, redirectUris }); }
  catch (error) { return oauthError("invalid_client_metadata", (error as Error).message); }
  return json({ client_id: clientId, client_id_issued_at: Math.floor(Date.now() / 1000), client_name: name, redirect_uris: redirectUris,
    token_endpoint_auth_method: "none", grant_types: grants, response_types: ["code"] }, 201);
});
function bearerMatches(header: string | null, secret: string | undefined) {
  if (!secret || secret.length < 32 || !header?.startsWith("Bearer ")) return false;
  const a = hash(header.slice(7)), b = hash(secret);
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}
export const introspectToken = httpAction(async (ctx, request) => {
  if (!configured()) return oauthError("temporarily_unavailable", "OAuth is not configured", 503);
  if (!bearerMatches(request.headers.get("authorization"), process.env.EVERETT_OAUTH_INTROSPECTION_SECRET)) {
    return oauthError("invalid_client", "introspection requires the gateway's EVERETT_OAUTH_INTROSPECTION_SECRET", 401);
  }
  const p = await form(request);
  if (!p?.token) return oauthError("invalid_request", "token is required");
  return json(await ctx.runQuery(internalRef("query", "introspect"), { token: p.token }));
});
export const revoke = httpAction(async (ctx, request) => {
  if (!configured()) return oauthError("temporarily_unavailable", "OAuth is not configured", 503);
  const p = await form(request);
  if (!p?.token) return oauthError("invalid_request", "token is required");
  await ctx.runMutation(internalRef("mutation", "revokeToken"), { token: p.token, clientId: p.client_id });
  return new Response(null, { status: 200, headers: { "Cache-Control": "no-store" } });
});
