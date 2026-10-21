import { DurableObject } from 'cloudflare:workers';
import { z, ZodError } from 'zod';
import { sha256 } from '@noble/hashes/sha256';
import { digest, identifier, namespace, RelayError, type Identity, type Principal } from './protocol';
import { hostedConfig, requireOAuth, type HostedConfig, type HostedAuthEnv } from './hosted-config';

const MINUTE = 60000, DAY = 86400000;
const DEVICE_MS = 10 * MINUTE, CODE_MS = 5 * MINUTE, ACCESS_MS = 15 * MINUTE, REFRESH_MS = 30 * DAY, SESSION_MS = 12 * 60 * MINUTE;
const secretPattern = /^[a-f0-9]{64}$/;
const cleanName = z.string().trim().min(1).max(100).regex(/^[^\x00-\x1f\x7f]+$/);
const hash = (v: string) => digest(new TextEncoder().encode(v));
const random = () => Array.from(crypto.getRandomValues(new Uint8Array(32)), b => b.toString(16).padStart(2, '0')).join('');
const s256 = (v: string) => btoa(String.fromCharCode(...sha256(new TextEncoder().encode(v)))).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '');
const equal = (a: string, b: string) => { const x = hash(a), y = hash(b); let diff = 0; for (let i = 0; i < x.length; i++) diff |= x.charCodeAt(i) ^ y.charCodeAt(i); return diff === 0; };
const esc = (v: unknown) => String(v).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', Pragma: 'no-cache', 'X-Content-Type-Options': 'nosniff' } });
const redirect = (location: string, status = 303) => new Response(null, { status, headers: { Location: location, 'Cache-Control': 'no-store', 'Referrer-Policy': 'no-referrer' } });
const html = (body: string, status = 200, callbackOrigin?: string) => new Response(`<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Everett owner approval</title><body><main><h1>Everett owner approval</h1>${body}</main></body></html>`, { status, headers: {
  'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store', 'Referrer-Policy': 'no-referrer', 'X-Content-Type-Options': 'nosniff',
  'Content-Security-Policy': `default-src 'none'; form-action 'self'${callbackOrigin ? ` ${callbackOrigin}` : ''}; frame-ancestors 'none'; base-uri 'none'`, 'X-Frame-Options': 'DENY',
} });
async function boundedText(stream: ReadableStream<Uint8Array> | null, cap: number) {
  if (!stream) throw new RelayError('missing_body', 400);
  const reader = stream.getReader(), decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: false });
  let size = 0, text = '', timer: ReturnType<typeof setTimeout>;
  const deadline = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new RelayError('request_timeout', 408)), 5000); });
  try {
    for (;;) {
      const part = await Promise.race([reader.read(), deadline]);
      if (part.done) return text + decoder.decode();
      size += part.value.byteLength;
      if (size > cap) throw new RelayError('body_too_large', 413);
      text += decoder.decode(part.value, { stream: true });
    }
  } finally {
    clearTimeout(timer!);
    void reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
function fields(p: URLSearchParams, allowed?: string[]) {
  const result: Record<string, string> = {};
  for (const [key, value] of p) {
    if (Object.hasOwn(result, key) || (allowed && !allowed.includes(key))) throw new RelayError('invalid_request', 400);
    result[key] = value;
  }
  return result;
}
async function form(request: Request, allowed?: string[]) {
  if (!request.headers.get('Content-Type')?.toLowerCase().startsWith('application/x-www-form-urlencoded')) throw new RelayError('form_required', 415);
  return fields(new URLSearchParams(await boundedText(request.body, 16384)), allowed);
}
async function body(request: Request) {
  if (!request.headers.get('Content-Type')?.toLowerCase().startsWith('application/json')) throw new RelayError('json_required', 415);
  return JSON.parse(await boundedText(request.body, 16384)) as unknown;
}
function cookieName(c: HostedConfig, suffix: string) { return `${c.secureCookies ? '__Host-' : ''}everett-${suffix}`; }
function cookies(request: Request) {
  const result: Record<string, string> = {};
  for (const part of (request.headers.get('Cookie') ?? '').split(';')) {
    const i = part.indexOf('='); if (i < 1) continue;
    const key = part.slice(0, i).trim(), value = part.slice(i + 1).trim();
    if (Object.hasOwn(result, key)) throw new RelayError('invalid_cookie', 400);
    result[key] = value;
  }
  return result;
}
function setCookie(response: Response, c: HostedConfig, suffix: string, value: string, seconds: number) {
  response.headers.append('Set-Cookie', `${cookieName(c, suffix)}=${value}; Path=/; HttpOnly; SameSite=Lax; Max-Age=${seconds}${c.secureCookies ? '; Secure' : ''}`);
}
const csrfField = (csrf: string) => `<input type="hidden" name="csrf" value="${esc(csrf)}">`;

type Session = { session_hash: string; csrf_hash: string; github_id: string; login: string; expires_at: number };
type DeviceRequest = { poll_hash: string; user_code: string; name: string; state: string; agent_id: string | null; event_id: string; expires_at: number; last_poll: number };
type AuthRequest = { id: string; client_id: string; client_name: string; redirect_uri: string; resource: string; challenge: string; state: string | null; binding_hash: string; phase: string; event_id: string; agent_id: string | null; expires_at: number };
type Family = { id: string; agent_id: string; name: string; client_id: string; resource: string; expires_at: number; revoked: number };
type OAuthToken = { token_hash: string; kind: string; family_id: string; expires_at: number; used: number };
type AuthCode = { code_hash: string; request_id: string; agent_id: string; client_id: string; redirect_uri: string; resource: string; challenge: string; expires_at: number; used: number };
type Agent = { agent_id: string; name: string; kind: string; created_at: number; revoked: number };

export class HostedAuth extends DurableObject<HostedAuthEnv> {
  constructor(ctx: DurableObjectState, env: HostedAuthEnv) {
    super(ctx, env);
    ctx.storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS gh_states(state_hash TEXT PRIMARY KEY,binding_hash TEXT NOT NULL,return_to TEXT NOT NULL,expires_at INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS owner_sessions(session_hash TEXT PRIMARY KEY,csrf_hash TEXT NOT NULL,github_id TEXT NOT NULL,login TEXT NOT NULL,expires_at INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS device_requests(poll_hash TEXT PRIMARY KEY,user_code TEXT NOT NULL UNIQUE,name TEXT NOT NULL,state TEXT NOT NULL,agent_id TEXT,event_id TEXT NOT NULL UNIQUE,expires_at INTEGER NOT NULL,last_poll INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS native_tokens(token_hash TEXT PRIMARY KEY,agent_id TEXT NOT NULL UNIQUE,expires_at INTEGER NOT NULL,revoked INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS hosted_agents(agent_id TEXT PRIMARY KEY,name TEXT NOT NULL,kind TEXT NOT NULL,created_at INTEGER NOT NULL,revoked INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS auth_requests(id TEXT PRIMARY KEY,client_id TEXT NOT NULL,client_name TEXT NOT NULL,redirect_uri TEXT NOT NULL,resource TEXT NOT NULL,challenge TEXT NOT NULL,state TEXT,binding_hash TEXT NOT NULL,phase TEXT NOT NULL,event_id TEXT NOT NULL UNIQUE,agent_id TEXT,expires_at INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS auth_codes(code_hash TEXT PRIMARY KEY,request_id TEXT NOT NULL UNIQUE,agent_id TEXT NOT NULL,client_id TEXT NOT NULL,redirect_uri TEXT NOT NULL,resource TEXT NOT NULL,challenge TEXT NOT NULL,expires_at INTEGER NOT NULL,used INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS token_families(id TEXT PRIMARY KEY,agent_id TEXT NOT NULL,name TEXT NOT NULL,client_id TEXT NOT NULL,resource TEXT NOT NULL,expires_at INTEGER NOT NULL,revoked INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS oauth_tokens(token_hash TEXT PRIMARY KEY,kind TEXT NOT NULL,family_id TEXT NOT NULL,expires_at INTEGER NOT NULL,used INTEGER NOT NULL);
      CREATE INDEX IF NOT EXISTS oauth_family ON oauth_tokens(family_id);
      CREATE TABLE IF NOT EXISTS auth_rates(scope TEXT PRIMARY KEY,window INTEGER NOT NULL,count INTEGER NOT NULL);
    `);
  }
  #row<T extends Record<string, SqlStorageValue>>(query: string, ...args: SqlStorageValue[]) { return this.ctx.storage.sql.exec<T>(query, ...args).toArray()[0]; }
  #count(table: string) { return this.#row<{ n: number }>(`SELECT count(*) n FROM ${table}`)!.n; }
  #identity(c: HostedConfig): Identity { return { issuer: c.issuer, subject: `github:${c.ownerId}`, org: null }; }
  #owner(c: HostedConfig) { const identity = this.#identity(c); return this.env.OWNERS.get(this.env.OWNERS.idFromName(namespace(identity))); }
  #rate(scope: string, limit: number, windowMs = MINUTE) {
    const window = Math.floor(Date.now() / windowMs), row = this.#row<{ window: number; count: number }>('SELECT * FROM auth_rates WHERE scope=?', scope);
    const count = row?.window === window ? row.count : 0;
    if (count >= limit) throw new RelayError('rate_limit', 429);
    this.ctx.storage.sql.exec('INSERT INTO auth_rates VALUES(?,?,?) ON CONFLICT(scope) DO UPDATE SET window=excluded.window,count=excluded.count', scope, window, count + 1);
  }
  #cleanup() {
    const now = Date.now();
    for (const table of ['gh_states', 'owner_sessions', 'device_requests', 'auth_requests', 'auth_codes']) this.ctx.storage.sql.exec(`DELETE FROM ${table} WHERE rowid IN(SELECT rowid FROM ${table} WHERE expires_at<=? LIMIT 128)`, now);
    this.ctx.storage.sql.exec('DELETE FROM oauth_tokens WHERE rowid IN(SELECT rowid FROM oauth_tokens WHERE expires_at<=? LIMIT 256)', now);
    this.ctx.storage.sql.exec('DELETE FROM auth_rates WHERE window<?', Math.floor(now / MINUTE) - 2);
  }
  #session(request: Request, c: HostedConfig): { row: Session; csrf: string } | null {
    const raw = cookies(request)[cookieName(c, 'session')] ?? '', parts = raw.split('.');
    if (parts.length !== 2 || !parts.every(v => secretPattern.test(v))) return null;
    const row = this.#row<Session>('SELECT * FROM owner_sessions WHERE session_hash=?', hash(parts[0]));
    if (!row || row.expires_at <= Date.now() || row.github_id !== c.ownerId || row.login.toLowerCase() !== c.ownerLogin.toLowerCase() || !equal(row.csrf_hash, hash(parts[1]))) return null;
    return { row, csrf: parts[1] };
  }
  #mutation(request: Request, c: HostedConfig, p: Record<string, string>) {
    if (request.headers.get('Origin') !== c.issuer) throw new RelayError('origin_denied', 403);
    const session = this.#session(request, c);
    if (!session || !p.csrf || !equal(p.csrf, session.csrf)) throw new RelayError('owner_session_or_csrf_required', 403);
    return session;
  }
  async #externalJSON(url: string, init: RequestInit = {}): Promise<Record<string, unknown>> {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 5000);
    try {
      const response = await fetch(url, { ...init, redirect: 'manual', signal: controller.signal });
      if (!response.ok) throw new RelayError('provider_unavailable', 502);
      const result: unknown = JSON.parse(await boundedText(response.body, 16384));
      if (!result || typeof result !== 'object' || Array.isArray(result)) throw new RelayError('provider_response_invalid', 502);
      return result as Record<string, unknown>;
    } finally { clearTimeout(timeout); }
  }

  async fetch(request: Request): Promise<Response> {
    const failure = (error: unknown) => {
      if (error instanceof RelayError) return json({ error: error.code }, error.status);
      if (error instanceof ZodError || error instanceof SyntaxError || error instanceof TypeError) return json({ error: 'invalid_request' }, 400);
      return json({ error: 'temporarily_unavailable' }, 503);
    };
    try {
      const c = hostedConfig(this.env), url = new URL(request.url);
      if (url.origin !== c.issuer) throw new RelayError('origin_denied', 403);
      // Caller-controlled streams must never hold the issuer's state gate.
      if (request.method === 'POST') {
        const buffered = await boundedText(request.body, 16384);
        request = new Request(request, { body: buffered, redirect: 'manual' });
      }
      return await this.ctx.blockConcurrencyWhile(async () => {
        // Expected denials must not reject blockConcurrencyWhile and reset the DO.
        try {
          this.#cleanup();
          return await this.#route(request, url, c);
        } catch (error) { return failure(error); }
      });
    } catch (error) { return failure(error); }
  }
  async verifyAgent(token: string): Promise<Principal | null> {
    try {
      const c = hostedConfig(this.env);
      if (!/^evt_[a-f0-9]{64}$/.test(token)) return null;
      const row = this.#row<{ token_hash: string; agent_id: string; expires_at: number; revoked: number }>('SELECT * FROM native_tokens WHERE token_hash=?', hash(token));
      if (!row || row.revoked || row.expires_at <= Date.now() || !await this.#owner(c).credentialActive(this.#identity(c), row.agent_id)) return null;
      return { ...this.#identity(c), agentId: row.agent_id, role: 'member', expiresAt: row.expires_at };
    } catch { return null; }
  }

  async #route(request: Request, url: URL, c: HostedConfig): Promise<Response> {
    const path = url.pathname;
    if (request.method === 'GET') {
      if (path === '/auth/github') return this.#githubStart(request, url, c);
      if (path === '/auth/callback/github') return this.#githubCallback(request, url, c);
      if (path === '/enroll') return this.#enrollPage(request, url, c);
      if (path === '/.well-known/oauth-authorization-server') {
        requireOAuth(c);
        return json({ issuer: c.issuer, authorization_endpoint: `${c.issuer}/oauth/authorize`, token_endpoint: `${c.issuer}/oauth/token`,
          revocation_endpoint: `${c.issuer}/oauth/revoke`, introspection_endpoint: `${c.issuer}/oauth/introspect`,
          scopes_supported: ['everett'], response_types_supported: ['code'], response_modes_supported: ['query'],
          grant_types_supported: ['authorization_code', 'refresh_token'], code_challenge_methods_supported: ['S256'],
          token_endpoint_auth_methods_supported: ['none'], revocation_endpoint_auth_methods_supported: ['none'],
          authorization_response_iss_parameter_supported: true, client_id_metadata_document_supported: true });
      }
      if (path === '/oauth/authorize') { requireOAuth(c); return this.#authorize(request, url, c); }
    }
    if (request.method === 'POST') {
      if (path === '/v2/auth/device') return this.#deviceStart(request, c);
      if (path === '/v2/auth/poll') return this.#devicePoll(request, c);
      if (path === '/v2/auth/cancel') return this.#deviceCancel(request);
      if (path === '/enroll/decision') return this.#deviceDecision(request, c);
      if (path === '/enroll/revoke') return this.#browserRevoke(request, c);
      if (path === '/auth/logout') {
        const p = await form(request, ['csrf']), session = this.#mutation(request, c, p);
        this.ctx.storage.sql.exec('DELETE FROM owner_sessions WHERE session_hash=?', session.row.session_hash);
        const response = redirect(`${c.issuer}/enroll`); setCookie(response, c, 'session', '', 0); return response;
      }
      if (path === '/oauth/consent') { requireOAuth(c); return this.#consent(request, c); }
      if (path === '/oauth/token') { requireOAuth(c); return this.#token(request, c); }
      if (path === '/oauth/introspect') { requireOAuth(c); return this.#introspect(request, c); }
      if (path === '/oauth/revoke') { requireOAuth(c); return this.#revokeToken(request, c); }
    }
    return json({ error: 'not_found' }, 404);
  }

  #githubStart(request: Request, url: URL, c: HostedConfig) {
    this.#rate('github-start', 20);
    const p = fields(url.searchParams, ['return_to']);
    const returnTo = p.return_to ?? '/enroll';
    const target = new URL(returnTo, c.issuer);
    if (!returnTo.startsWith('/enroll') || target.origin !== c.issuer || target.pathname !== '/enroll' || target.hash || returnTo.length > 2048) throw new RelayError('invalid_return_to', 400);
    if (this.#count('gh_states') >= 100) throw new RelayError('pending_limit', 429);
    const state = random(), verifier = random(), binding = `${random()}.${verifier}`;
    this.ctx.storage.sql.exec('INSERT INTO gh_states VALUES(?,?,?,?)', hash(state), hash(binding), `${target.pathname}${target.search}`, Date.now() + DEVICE_MS);
    const github = new URL('https://github.com/login/oauth/authorize');
    github.searchParams.set('client_id', c.githubId); github.searchParams.set('redirect_uri', `${c.issuer}/auth/callback/github`);
    github.searchParams.set('state', state); github.searchParams.set('code_challenge', s256(verifier)); github.searchParams.set('code_challenge_method', 'S256');
    github.searchParams.set('scope', 'read:user'); github.searchParams.set('login', c.ownerLogin);
    const response = redirect(github.href, 302); setCookie(response, c, 'github', binding, 600); return response;
  }
  async #githubCallback(request: Request, url: URL, c: HostedConfig) {
    this.#rate('github-callback', 30);
    const p = fields(url.searchParams, ['state', 'code', 'error', 'error_description', 'error_uri']);
    if (!secretPattern.test(p.state ?? '')) throw new RelayError('invalid_github_state', 400);
    const row = this.#row<{ state_hash: string; binding_hash: string; return_to: string; expires_at: number }>('SELECT * FROM gh_states WHERE state_hash=?', hash(p.state));
    const binding = cookies(request)[cookieName(c, 'github')] ?? '', parts = binding.split('.');
    if (!row || row.expires_at <= Date.now() || parts.length !== 2 || !parts.every(v => secretPattern.test(v)) || !equal(hash(binding), row.binding_hash)) throw new RelayError('invalid_github_state', 400);
    // Consume before external I/O: an interrupted exchange requires a new login attempt.
    this.ctx.storage.sql.exec('DELETE FROM gh_states WHERE state_hash=?', row.state_hash);
    if (p.error || !p.code || p.code.length > 2048) throw new RelayError('github_login_denied', 403);
    const token = await this.#externalJSON('https://github.com/login/oauth/access_token', { method: 'POST', headers: { Accept: 'application/json', 'Content-Type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams({
      client_id: c.githubId, client_secret: c.githubSecret, code: p.code, redirect_uri: `${c.issuer}/auth/callback/github`, code_verifier: parts[1],
    }).toString() });
    if (typeof token.access_token !== 'string' || !token.access_token || token.access_token.length > 4096 || (token.token_type !== undefined && String(token.token_type).toLowerCase() !== 'bearer')) throw new RelayError('github_login_denied', 403);
    const user = await this.#externalJSON('https://api.github.com/user', { headers: { Authorization: `Bearer ${token.access_token}`, Accept: 'application/vnd.github+json', 'User-Agent': 'Everett', 'X-GitHub-Api-Version': '2022-11-28' } });
    const id = typeof user.id === 'number' && Number.isSafeInteger(user.id) ? String(user.id) : typeof user.id === 'string' ? user.id : '';
    if (id !== c.ownerId || typeof user.login !== 'string' || user.login.toLowerCase() !== c.ownerLogin.toLowerCase()) throw new RelayError('owner_only', 403);
    if (this.#count('owner_sessions') >= 32) this.ctx.storage.sql.exec('DELETE FROM owner_sessions WHERE session_hash IN(SELECT session_hash FROM owner_sessions ORDER BY expires_at LIMIT 1)');
    const sid = random(), csrf = random();
    this.ctx.storage.sql.exec('INSERT INTO owner_sessions VALUES(?,?,?,?,?)', hash(sid), hash(csrf), c.ownerId, user.login, Date.now() + SESSION_MS);
    const response = redirect(`${c.issuer}${row.return_to}`); setCookie(response, c, 'session', `${sid}.${csrf}`, SESSION_MS / 1000); setCookie(response, c, 'github', '', 0);
    return response;
  }

  async #deviceStart(request: Request, c: HostedConfig) {
    this.#rate('device-start', 30);
    const { name } = z.strictObject({ name: cleanName }).parse(await body(request));
    if (this.#count('device_requests') >= 100) throw new RelayError('pending_limit', 429);
    const poll = random(), code = random().slice(0, 12).toUpperCase();
    this.ctx.storage.sql.exec('INSERT INTO device_requests VALUES(?,?,?,?,?,?,?,?)', hash(poll), code, name, 'pending', null, crypto.randomUUID(), Date.now() + DEVICE_MS, 0);
    return json({ device_code: poll, user_code: code, verification_uri: `${c.issuer}/enroll`, verification_uri_complete: `${c.issuer}/enroll?code=${code}`, expires_in: 600, interval: 5 });
  }
  async #devicePoll(request: Request, c: HostedConfig) {
    this.#rate('device-poll', 600);
    const { device_code } = z.strictObject({ device_code: z.string().regex(secretPattern) }).parse(await body(request));
    const row = this.#row<DeviceRequest>('SELECT * FROM device_requests WHERE poll_hash=?', hash(device_code));
    if (!row || row.expires_at <= Date.now()) return json({ state: 'expired', interval: 5 });
    if (row.state !== 'approved') {
      if (row.state === 'pending' && row.last_poll && Date.now() - row.last_poll < 5000) throw new RelayError('slow_down', 429);
      this.ctx.storage.sql.exec('UPDATE device_requests SET last_poll=? WHERE poll_hash=?', Date.now(), row.poll_hash);
      return json({ state: row.state === 'approving' ? 'pending' : row.state, interval: 5 });
    }
    if (!row.agent_id || !await this.#owner(c).credentialActive(this.#identity(c), row.agent_id)) return json({ state: 'denied', interval: 5 });
    const token = `evt_${random()}`, expires = Date.now() + REFRESH_MS;
    this.ctx.storage.transactionSync(() => {
      this.ctx.storage.sql.exec('INSERT INTO native_tokens VALUES(?,?,?,?)', hash(token), row.agent_id, expires, 0);
      this.ctx.storage.sql.exec("UPDATE device_requests SET state='consumed' WHERE poll_hash=?", row.poll_hash);
    });
    return json({ state: 'approved', token, agent_id: row.agent_id, owner: namespace(this.#identity(c)), expires_at: expires });
  }
  async #deviceCancel(request: Request) {
    this.#rate('device-cancel', 100);
    const { device_code } = z.strictObject({ device_code: z.string().regex(secretPattern) }).parse(await body(request));
    const row = this.#row<DeviceRequest>('SELECT * FROM device_requests WHERE poll_hash=?', hash(device_code));
    if (!row || row.expires_at <= Date.now()) return json({ state: 'expired' });
    if (row.state === 'pending') this.ctx.storage.sql.exec("UPDATE device_requests SET state='cancelled' WHERE poll_hash=?", row.poll_hash);
    return json({ state: row.state === 'pending' ? 'cancelled' : row.state });
  }
  async #enroll(c: HostedConfig, eventId: string, name: string, kind: string) {
    if (this.#count('hosted_agents') >= 2048) throw new RelayError('credential_limit', 429);
    const result = await this.#owner(c).enroll(this.#identity(c), 'admin', eventId);
    if (!result.ok) throw new RelayError('enrollment_denied', 403);
    this.ctx.storage.sql.exec('INSERT OR IGNORE INTO hosted_agents VALUES(?,?,?,?,?)', result.value.deviceId, name, kind, Date.now(), 0);
    return result.value.deviceId;
  }
  async #deviceDecision(request: Request, c: HostedConfig) {
    this.#rate('browser-mutation', 60);
    const p = await form(request, ['csrf', 'user_code', 'decision']); this.#mutation(request, c, p);
    if (!/^[A-F0-9]{12}$/.test(p.user_code ?? '') || !['approve', 'deny'].includes(p.decision)) throw new RelayError('invalid_request', 400);
    const row = this.#row<DeviceRequest>('SELECT * FROM device_requests WHERE user_code=?', p.user_code);
    if (!row || row.expires_at <= Date.now() || !['pending', 'approving'].includes(row.state)) throw new RelayError('request_expired_or_decided', 409);
    if (p.decision === 'deny') {
      if (row.state !== 'pending') throw new RelayError('approval_in_progress', 409);
      this.ctx.storage.sql.exec("UPDATE device_requests SET state='denied' WHERE poll_hash=?", row.poll_hash);
    } else {
      this.ctx.storage.sql.exec("UPDATE device_requests SET state='approving' WHERE poll_hash=?", row.poll_hash);
      const agent = await this.#enroll(c, row.event_id, row.name, 'device');
      this.ctx.storage.sql.exec("UPDATE device_requests SET state='approved',agent_id=? WHERE poll_hash=?", agent, row.poll_hash);
    }
    return redirect(`${c.issuer}/enroll?code=${p.user_code}`);
  }

  async #live(c: HostedConfig, agentId: string) {
    const row = this.#row<Agent>('SELECT * FROM hosted_agents WHERE agent_id=?', agentId);
    return !!row && !row.revoked && await this.#owner(c).credentialActive(this.#identity(c), agentId);
  }
  async #enrollPage(request: Request, url: URL, c: HostedConfig) {
    const p = fields(url.searchParams, ['code', 'request']);
    const session = this.#session(request, c);
    const info = '<p>Only approve a request you started. Compare the full code and device name with your terminal.</p>';
    if (!session) {
      const returnTo = `${url.pathname}${url.search}`;
      return html(`${info}<p><a href="/auth/github?return_to=${esc(encodeURIComponent(returnTo))}">Sign in with GitHub</a></p>`);
    }
    let callbackOrigin: string | undefined;
    let content = `${info}<p>Signed in as ${esc(c.ownerLogin)}.</p><form method="post" action="/auth/logout">${csrfField(session.csrf)}<button>Sign out</button></form>`;
    if (p.request) {
      const row = this.#row<AuthRequest>('SELECT * FROM auth_requests WHERE id=?', identifier.parse(p.request));
      const binding = cookies(request)[cookieName(c, 'oauth')] ?? '';
      if (!row || row.expires_at <= Date.now() || !secretPattern.test(binding) || !equal(row.binding_hash, hash(binding))) return html(`${content}<p>Authorization request expired or belongs to another browser.</p>`, 403);
      if (c.redirects.includes(row.redirect_uri)) callbackOrigin = new URL(row.redirect_uri).origin;
      content += `<h2>Connect ${esc(row.client_name)}</h2><p>Client: ${esc(row.client_id)}</p><p>Resource: ${esc(row.resource)}</p><p>Redirect: ${esc(row.redirect_uri)}</p>`;
      if (row.phase === 'pending' || row.phase === 'approving') content += `<form method="post" action="/oauth/consent">${csrfField(session.csrf)}<input type="hidden" name="request_id" value="${esc(row.id)}"><label>Agent name <input name="name" required maxlength="100" value="argos"></label><button name="decision" value="approve">Approve this agent</button><button name="decision" value="deny">Deny</button></form>`;
      else content += '<p>This authorization was already decided. Start a new connection.</p>';
    } else {
      content += `<form method="get" action="/enroll"><label>Device code <input name="code" required maxlength="12" value="${esc(p.code ?? '')}"></label><button>Inspect</button></form>`;
      if (p.code) {
        if (!/^[A-F0-9]{12}$/.test(p.code)) throw new RelayError('invalid_user_code', 400);
        const row = this.#row<DeviceRequest>('SELECT * FROM device_requests WHERE user_code=?', p.code);
        if (!row || row.expires_at <= Date.now()) content += '<p>Request expired or unknown.</p>';
        else {
          content += `<h2>${esc(row.name)}</h2><p>Device code: <strong>${esc(row.user_code)}</strong></p><p>Status: ${esc(row.state)}</p>${row.agent_id ? `<p>Agent ID: ${esc(row.agent_id)}</p>` : ''}`;
          if (row.state === 'pending' || row.state === 'approving') content += `<form method="post" action="/enroll/decision">${csrfField(session.csrf)}<input type="hidden" name="user_code" value="${esc(row.user_code)}"><button name="decision" value="approve">Approve this agent</button><button name="decision" value="deny">Deny</button></form>`;
        }
      }
    }
    content += '<h2>Agent credentials</h2><ul>';
    const agents = this.ctx.storage.sql.exec<Agent>('SELECT * FROM hosted_agents ORDER BY created_at DESC LIMIT 257').toArray();
    for (const agent of agents.slice(0, 256)) {
      const live = await this.#live(c, agent.agent_id);
      content += `<li>${esc(agent.name)} · ${esc(agent.agent_id)} · ${esc(agent.kind)} · ${live ? 'active' : 'revoked'}${live ? `<form method="post" action="/enroll/revoke">${csrfField(session.csrf)}<input type="hidden" name="agent_id" value="${esc(agent.agent_id)}"><button>Revoke</button></form>` : ''}</li>`;
    }
    return html(`${content}</ul>${agents.length > 256 ? '<p>Only the newest 256 credentials are displayed.</p>' : ''}`, 200, callbackOrigin);
  }
  async #browserRevoke(request: Request, c: HostedConfig) {
    this.#rate('browser-mutation', 60);
    const p = await form(request, ['csrf', 'agent_id']); this.#mutation(request, c, p); identifier.parse(p.agent_id);
    if (!this.#row('SELECT agent_id FROM hosted_agents WHERE agent_id=?', p.agent_id)) throw new RelayError('unknown_agent', 404);
    if (!await this.#owner(c).revokeCredential(this.#identity(c), p.agent_id)) throw new RelayError('revocation_denied', 403);
    this.ctx.storage.transactionSync(() => {
      this.ctx.storage.sql.exec('UPDATE hosted_agents SET revoked=1 WHERE agent_id=?', p.agent_id);
      this.ctx.storage.sql.exec('UPDATE native_tokens SET revoked=1 WHERE agent_id=?', p.agent_id);
      this.ctx.storage.sql.exec('UPDATE token_families SET revoked=1 WHERE agent_id=?', p.agent_id);
    });
    return redirect(`${c.issuer}/enroll`);
  }

  async #authorize(request: Request, url: URL, c: HostedConfig) {
    this.#rate('oauth-authorize', 30);
    const p = fields(url.searchParams, ['client_id', 'redirect_uri', 'response_type', 'scope', 'state', 'resource', 'code_challenge', 'code_challenge_method']);
    if (p.response_type !== 'code' || p.scope !== 'everett' || p.code_challenge_method !== 'S256' || !/^[A-Za-z0-9_-]{43}$/.test(p.code_challenge ?? '')) throw new RelayError('invalid_request', 400);
    if (!c.clients.includes(p.client_id)) throw new RelayError('invalid_client', 400);
    if (!c.resources.includes(p.resource)) throw new RelayError('invalid_target', 400);
    if (!c.redirects.includes(p.redirect_uri)) throw new RelayError('invalid_redirect_uri', 400);
    if (p.state !== undefined && (p.state.length > 1024 || /[\x00-\x1f\x7f]/.test(p.state))) throw new RelayError('invalid_request', 400);
    if (this.#count('auth_requests') >= 100) throw new RelayError('pending_limit', 429);
    const doc = await this.#externalJSON(p.client_id, { headers: { Accept: 'application/json' } });
    if (doc.client_id !== p.client_id || !Array.isArray(doc.redirect_uris) || !doc.redirect_uris.every(v => typeof v === 'string') || !doc.redirect_uris.includes(p.redirect_uri) || doc.redirect_uris.length > 32) throw new RelayError('invalid_client', 400);
    const authMethods = doc.token_endpoint_auth_methods_supported;
    if ((doc.token_endpoint_auth_method !== undefined && doc.token_endpoint_auth_method !== 'none') || (authMethods !== undefined && (!Array.isArray(authMethods) || !authMethods.includes('none')))) throw new RelayError('invalid_client', 400);
    if (doc.grant_types !== undefined && (!Array.isArray(doc.grant_types) || !doc.grant_types.includes('authorization_code'))) throw new RelayError('invalid_client', 400);
    if (doc.response_types !== undefined && (!Array.isArray(doc.response_types) || !doc.response_types.includes('code'))) throw new RelayError('invalid_client', 400);
    const name = cleanName.safeParse(doc.client_name), clientName = name.success ? name.data : new URL(p.client_id).host;
    const binding = random(), id = crypto.randomUUID();
    this.ctx.storage.sql.exec('INSERT INTO auth_requests VALUES(?,?,?,?,?,?,?,?,?,?,?,?)', id, p.client_id, clientName, p.redirect_uri, p.resource, p.code_challenge, p.state ?? null, hash(binding), 'pending', crypto.randomUUID(), null, Date.now() + DEVICE_MS);
    const response = redirect(`${c.issuer}/enroll?request=${id}`, 302); setCookie(response, c, 'oauth', binding, 600); return response;
  }
  async #consent(request: Request, c: HostedConfig) {
    this.#rate('browser-mutation', 60);
    const p = await form(request, ['csrf', 'request_id', 'decision', 'name']); this.#mutation(request, c, p);
    const id = identifier.parse(p.request_id);
    if (!['approve', 'deny'].includes(p.decision)) throw new RelayError('invalid_request', 400);
    const row = this.#row<AuthRequest>('SELECT * FROM auth_requests WHERE id=?', id);
    const binding = cookies(request)[cookieName(c, 'oauth')] ?? '';
    if (!row || row.expires_at <= Date.now() || !['pending', 'approving'].includes(row.phase) || !secretPattern.test(binding) || !equal(hash(binding), row.binding_hash)) throw new RelayError('request_expired_or_browser_mismatch', 403);
    if (!c.clients.includes(row.client_id) || !c.resources.includes(row.resource) || !c.redirects.includes(row.redirect_uri)) throw new RelayError('authorization_no_longer_allowed', 403);
    const callback = new URL(row.redirect_uri);
    if (row.state !== null) callback.searchParams.set('state', row.state); callback.searchParams.set('iss', c.issuer);
    if (p.decision === 'deny') {
      if (row.phase !== 'pending') throw new RelayError('approval_in_progress', 409);
      this.ctx.storage.sql.exec("UPDATE auth_requests SET phase='denied' WHERE id=?", id);
      callback.searchParams.set('error', 'access_denied'); return redirect(callback.href);
    }
    const name = cleanName.parse(p.name);
    if (this.#count('auth_codes') >= 200) throw new RelayError('code_limit', 429);
    this.ctx.storage.sql.exec("UPDATE auth_requests SET phase='approving' WHERE id=?", id);
    const agentId = await this.#enroll(c, row.event_id, name, 'oauth'), code = random();
    this.ctx.storage.transactionSync(() => {
      this.ctx.storage.sql.exec('INSERT INTO auth_codes VALUES(?,?,?,?,?,?,?,?,?)', hash(code), id, agentId, row.client_id, row.redirect_uri, row.resource, row.challenge, Date.now() + CODE_MS, 0);
      this.ctx.storage.sql.exec("UPDATE auth_requests SET phase='approved',agent_id=? WHERE id=?", agentId, id);
    });
    callback.searchParams.set('code', code); return redirect(callback.href);
  }

  #family(id: string) { return this.#row<Family>('SELECT * FROM token_families WHERE id=?', id); }
  #revokeFamily(id: string) { this.ctx.storage.sql.exec('UPDATE token_families SET revoked=1 WHERE id=?', id); }
  async #familyLive(c: HostedConfig, family: Family | undefined) {
    return !!family && !family.revoked && family.expires_at > Date.now() && c.clients.includes(family.client_id) && c.resources.includes(family.resource) && await this.#live(c, family.agent_id);
  }
  #issueTokens(family: Family) {
    if (this.#count('oauth_tokens') >= 32766) throw new RelayError('token_limit', 429);
    const access = `eva_${random()}`, refresh = `evr_${random()}`, now = Date.now();
    this.ctx.storage.sql.exec('INSERT INTO oauth_tokens VALUES(?,?,?,?,?)', hash(access), 'access', family.id, Math.min(now + ACCESS_MS, family.expires_at), 0);
    this.ctx.storage.sql.exec('INSERT INTO oauth_tokens VALUES(?,?,?,?,?)', hash(refresh), 'refresh', family.id, family.expires_at, 0);
    return { access_token: access, token_type: 'Bearer', expires_in: Math.floor(Math.min(ACCESS_MS, family.expires_at - now) / 1000), refresh_token: refresh, scope: 'everett' };
  }
  async #token(request: Request, c: HostedConfig) {
    this.#rate('oauth-token', 120);
    const p = await form(request, ['grant_type', 'client_id', 'code', 'redirect_uri', 'code_verifier', 'resource', 'refresh_token', 'scope', 'client_secret', 'client_assertion', 'client_assertion_type']);
    if (request.headers.has('Authorization') || p.client_secret !== undefined || p.client_assertion !== undefined || p.client_assertion_type !== undefined || !c.clients.includes(p.client_id)) throw new RelayError('invalid_client', 401);
    if (p.grant_type === 'authorization_code') {
      if (!secretPattern.test(p.code ?? '')) throw new RelayError('invalid_grant', 400);
      const code = this.#row<AuthCode>('SELECT * FROM auth_codes WHERE code_hash=?', hash(p.code));
      if (!code) {
        const issuedFamily = this.#family(hash(p.code));
        if (issuedFamily?.client_id === p.client_id) this.#revokeFamily(issuedFamily.id);
        throw new RelayError('invalid_grant', 400);
      }
      if (code.client_id !== p.client_id || code.redirect_uri !== p.redirect_uri) throw new RelayError('invalid_grant', 400);
      if (code.used) { this.#revokeFamily(code.code_hash); throw new RelayError('invalid_grant', 400); }
      if (code.expires_at <= Date.now() || !/^[A-Za-z0-9._~-]{43,128}$/.test(p.code_verifier ?? '') || !equal(s256(p.code_verifier), code.challenge)) throw new RelayError('invalid_grant', 400);
      if (p.resource !== undefined && p.resource !== code.resource) throw new RelayError('invalid_target', 400);
      if (p.scope !== undefined && p.scope !== 'everett') throw new RelayError('invalid_scope', 400);
      if (!c.resources.includes(code.resource) || !c.redirects.includes(code.redirect_uri) || !await this.#live(c, code.agent_id)) throw new RelayError('invalid_grant', 400);
      if (this.#count('token_families') >= 2048) throw new RelayError('family_limit', 429);
      const agent = this.#row<Agent>('SELECT * FROM hosted_agents WHERE agent_id=?', code.agent_id)!;
      const family: Family = { id: code.code_hash, agent_id: code.agent_id, name: agent.name, client_id: code.client_id, resource: code.resource, expires_at: Date.now() + REFRESH_MS, revoked: 0 };
      const result = this.ctx.storage.transactionSync(() => {
        this.ctx.storage.sql.exec('UPDATE auth_codes SET used=1 WHERE code_hash=?', code.code_hash);
        this.ctx.storage.sql.exec('INSERT INTO token_families VALUES(?,?,?,?,?,?,?)', family.id, family.agent_id, family.name, family.client_id, family.resource, family.expires_at, 0);
        return this.#issueTokens(family);
      });
      return json(result);
    }
    if (p.grant_type === 'refresh_token') {
      if (!/^evr_[a-f0-9]{64}$/.test(p.refresh_token ?? '')) throw new RelayError('invalid_grant', 400);
      const token = this.#row<OAuthToken>('SELECT * FROM oauth_tokens WHERE token_hash=?', hash(p.refresh_token));
      const family = token && this.#family(token.family_id);
      if (!token || token.kind !== 'refresh' || !family || family.client_id !== p.client_id) throw new RelayError('invalid_grant', 400);
      if (token.used) { this.#revokeFamily(family.id); throw new RelayError('invalid_grant', 400); }
      if (token.expires_at <= Date.now() || !await this.#familyLive(c, family)) { this.#revokeFamily(family.id); throw new RelayError('invalid_grant', 400); }
      if (p.resource !== undefined && p.resource !== family.resource) throw new RelayError('invalid_target', 400);
      if (p.scope !== undefined && p.scope !== 'everett') throw new RelayError('invalid_scope', 400);
      const result = this.ctx.storage.transactionSync(() => {
        this.ctx.storage.sql.exec('UPDATE oauth_tokens SET used=1 WHERE token_hash=?', token.token_hash);
        return this.#issueTokens(family);
      });
      return json(result);
    }
    throw new RelayError('unsupported_grant_type', 400);
  }
  async #introspect(request: Request, c: HostedConfig) {
    const authorization = request.headers.get('Authorization') ?? '';
    if (!authorization.startsWith('Bearer ') || !equal(authorization.slice(7), c.introspectionSecret)) throw new RelayError('invalid_client', 401);
    this.#rate('oauth-introspect', 1200);
    const p = await form(request, ['token', 'token_type_hint']);
    if (!p.token || p.token.length > 1024) throw new RelayError('invalid_request', 400);
    if (!/^eva_[a-f0-9]{64}$/.test(p.token)) return json({ active: false });
    const token = this.#row<OAuthToken>('SELECT * FROM oauth_tokens WHERE token_hash=?', hash(p.token)), family = token && this.#family(token.family_id);
    if (!token || token.kind !== 'access' || token.expires_at <= Date.now() || !await this.#familyLive(c, family)) return json({ active: false });
    return json({ active: true, iss: c.issuer, aud: family!.resource, exp: Math.floor(token.expires_at / 1000), scope: 'everett', sub: family!.agent_id,
      everett_agent: family!.agent_id, everett_agent_name: family!.name, client_id: family!.client_id, token_type: 'Bearer' });
  }
  async #revokeToken(request: Request, c: HostedConfig) {
    this.#rate('oauth-revoke', 120);
    const p = await form(request, ['token', 'token_type_hint', 'client_id', 'client_secret', 'client_assertion']);
    if (request.headers.has('Authorization') || p.client_secret !== undefined || p.client_assertion !== undefined || !c.clients.includes(p.client_id)) throw new RelayError('invalid_client', 401);
    if (!p.token || p.token.length > 1024) throw new RelayError('invalid_request', 400);
    const token = this.#row<OAuthToken>('SELECT * FROM oauth_tokens WHERE token_hash=?', hash(p.token));
    const family = token && this.#family(token.family_id);
    if (family && family.client_id === p.client_id) this.#revokeFamily(family.id);
    return json({});
  }
}
