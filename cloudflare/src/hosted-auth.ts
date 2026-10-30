import { DurableObject } from 'cloudflare:workers';
import { z, ZodError } from 'zod';
import { sha256 } from '@noble/hashes/sha256';
import { DEVICE_LABEL_LIMIT, digest, identifier, namespace, RelayError, type Identity, type Principal } from './protocol';
import { hostedConfig, requireOAuth, requirePublicHost, type HostedConfig, type HostedAuthEnv } from './hosted-config';
import { authFontStyles } from './hosted-auth-fonts';

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
const authStyles = `<style>${authFontStyles}
:root{color-scheme:dark;--ground:#0e100f;--raised:#151816;--sunk:#1a1d1b;--ink:#e9ece9;--ink-soft:#adb3ae;--ink-faint:#8b918c;--line:#262a27;--line-strong:#444a46;--hot:#ec7d3f;--btn:#e9ece9;--on-btn:#0e100f;--focus:#f08a50;--sans:"Archivo",ui-sans-serif,system-ui,sans-serif;--mono:"JetBrains Mono",ui-monospace,SFMono-Regular,Menlo,monospace;--gutter:clamp(16px,4vw,48px);--max:1280px}
*,*::before,*::after{box-sizing:border-box}html{min-height:100%;background:var(--ground);-webkit-text-size-adjust:100%}body{min-height:100vh;margin:0;background:var(--ground);color:var(--ink);font:1.0625rem/1.6 var(--sans);-webkit-font-smoothing:antialiased}a{color:inherit}button,input{font:inherit;border-radius:0}button{cursor:pointer}button:focus-visible,a:focus-visible,input:focus-visible{outline:2px solid var(--focus);outline-offset:2px}[hidden]{display:none!important}::selection{background:var(--hot);color:var(--on-btn)}
.auth-shell{width:100%;max-width:calc(var(--max) + var(--gutter)*2);min-height:100vh;margin-inline:auto;padding-inline:var(--gutter);display:flex;flex-direction:column}.auth-header{height:56px;display:flex;align-items:center;border-bottom:1px solid var(--line);flex:none}.auth-brand{font-weight:800;font-size:1.45rem;font-stretch:72%;letter-spacing:-.01em;line-height:1}.auth-main{flex:1;padding:clamp(28px,5vh,64px) 0 clamp(64px,9vw,128px);border-bottom:1px solid var(--line)}.auth-content{display:grid;grid-template-columns:minmax(0,9fr) minmax(0,4fr);gap:24px clamp(24px,4vw,64px);align-items:end}.auth-account{grid-column:1/-1;display:flex;align-items:center;justify-content:space-between;gap:16px;padding:10px 0;border-bottom:1px solid var(--line);color:var(--ink-soft);font-size:.9rem}.auth-account strong{color:var(--ink)}.auth-account form{margin-left:auto}.auth-lead h1{margin:0;max-width:18ch;font-size:clamp(2.5rem,5.2vw,5.4rem);line-height:.9;font-weight:820;font-stretch:70%;letter-spacing:-.012em;text-wrap:balance}.auth-action{display:grid;gap:14px;justify-items:start;padding-bottom:.35em}.auth-copy{margin:0;max-width:38ch;color:var(--ink-soft);font-size:1.06rem;line-height:1.5}.auth-button,.auth-content button{display:inline-flex;align-items:center;justify-content:center;gap:6px;min-height:44px;padding:0 14px;border:0;background:var(--ink);color:var(--ground);font-size:.86rem;font-weight:600;text-align:center;text-decoration:none;transition:background .2s,color .2s}.auth-button:hover,.auth-content button:hover{background:color-mix(in oklab,var(--ink) 84%,var(--hot))}.auth-button:active,.auth-content button:active{transform:translateY(1px)}.auth-button svg{width:16px;height:16px;flex:none;fill:currentColor}.auth-content button[name="decision"][value="deny"]{padding:0 10px;background:transparent;color:var(--ink-soft);text-decoration:underline;text-underline-offset:3px}.auth-content button[name="decision"][value="deny"]:hover{color:var(--ink);background:transparent}.auth-signout{min-height:32px;padding:0 10px;border:1px solid var(--line-strong)!important;background:transparent!important;color:var(--ink-soft)!important;font-size:.78rem!important}.auth-signout:hover{color:var(--ink)!important;border-color:var(--ink)!important}.auth-content form{display:flex;flex-wrap:wrap;align-items:end;gap:8px}.auth-content label{display:grid;flex:1 1 220px;gap:7px;color:var(--ink-soft);font-size:.9rem;font-weight:600}.auth-content input:not([type="hidden"]){width:100%;height:46px;padding:0 12px;border:1px solid var(--line-strong);background:var(--raised);color:var(--ink);font:600 .95rem var(--mono)}.auth-content h2{margin:0;font-size:1.3rem;font-weight:760;font-stretch:76%;letter-spacing:-.01em;line-height:1.1}.auth-details{grid-column:1/-1;display:grid;gap:0;margin:0;border-top:1px solid var(--line-strong);border-bottom:1px solid var(--line)}.auth-details>div{display:grid;grid-template-columns:minmax(110px,1fr) minmax(0,2fr);gap:12px;padding:9px 0;border-top:1px solid var(--line)}.auth-details>div:first-child{border-top:0}.auth-details dt{color:var(--ink-faint);font-size:.82rem}.auth-details dd{margin:0;overflow-wrap:anywhere;font-size:.9rem}.auth-code{font:700 1.1rem/1.3 var(--mono);letter-spacing:.12em;color:var(--ink);font-variant-numeric:tabular-nums}.auth-state{grid-column:2;display:grid;gap:10px;border-top:1px solid var(--line-strong);padding-top:14px;color:var(--ink-soft)}.auth-state__title{margin:0;color:var(--ink);font-size:1.4rem;font-weight:780;font-stretch:76%;letter-spacing:-.01em;line-height:1.1}.auth-state[data-state="connected"] .auth-state__title{color:var(--hot)}.auth-credentials{grid-column:1/-1;display:grid;gap:12px;padding-top:22px;border-top:1px solid var(--line)}.auth-agent-list{display:grid;gap:0;margin:0;padding:0;border-top:1px solid var(--line-strong);list-style:none}.auth-agent-list>li{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:9px 0;border-bottom:1px solid var(--line);color:var(--ink-soft);font-size:.82rem;overflow-wrap:anywhere}.auth-agent-list form{flex:none}.auth-agent-list button{min-height:32px;font-size:.78rem}.auth-skip{position:absolute;left:12px;top:-60px;z-index:2;background:var(--ink);color:var(--ground);padding:8px 12px}.auth-skip:focus{top:12px}@media(max-width:900px){.auth-content{grid-template-columns:minmax(0,1fr)}.auth-lead h1{max-width:14ch}.auth-state{grid-column:1}.auth-details{grid-column:1/-1}.auth-account,.auth-credentials{grid-column:1/-1}}@media(max-width:560px){.auth-header{height:56px}.auth-main{padding-top:28px;padding-bottom:64px}.auth-content{gap:18px}.auth-action{padding-bottom:0}.auth-button{width:100%}.auth-content form:not(.auth-account form){align-items:stretch;flex-direction:column}.auth-agent-list>li{align-items:flex-start}.auth-details>div{grid-template-columns:92px minmax(0,1fr)}}@media(prefers-reduced-motion:reduce){*,*::before,*::after{transition:none!important}}
</style>`;
const authStateGridStyles = '<style>.auth-action>.auth-state{grid-column:1/-1}</style>';
const html = (body: string, status = 200, callbackOrigin?: string) => new Response(`<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="dark"><meta name="theme-color" content="#0e100f"><title>Everett · Account access</title>${authStyles}${authStateGridStyles}</head><body><a class="auth-skip" href="#main">Skip to content</a><div class="auth-shell"><header class="auth-header"><span class="auth-brand" aria-label="Everett">everett</span></header><main id="main" class="auth-main"><div class="auth-content">${body}</div></main></div></body></html>`, { status, headers: {
  'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store', 'Referrer-Policy': 'same-origin', 'X-Content-Type-Options': 'nosniff',
  'Content-Security-Policy': `default-src 'none'; style-src 'unsafe-inline'; font-src data:; form-action 'self'${callbackOrigin ? ` ${callbackOrigin}` : ''}; frame-ancestors 'none'; base-uri 'none'`, 'X-Frame-Options': 'DENY',
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
type DeviceRequest = { poll_hash: string; user_code: string; name: string; state: string; agent_id: string | null; event_id: string; expires_at: number; last_poll: number; account: string | null; requester_hash: string | null };
type AuthRequest = { id: string; client_id: string; client_name: string; redirect_uri: string; resource: string; challenge: string; state: string | null; binding_hash: string; phase: string; event_id: string; agent_id: string | null; expires_at: number; account: string | null };
type Family = { id: string; agent_id: string; name: string; client_id: string; resource: string; expires_at: number; revoked: number };
type OAuthToken = { token_hash: string; kind: string; family_id: string; expires_at: number; used: number };
type AuthCode = { code_hash: string; request_id: string; agent_id: string; client_id: string; redirect_uri: string; resource: string; challenge: string; expires_at: number; used: number };
type Agent = { agent_id: string; name: string; kind: string; created_at: number; revoked: number; account: string };
type Subject = `github:${string}`;
/// A GitHub-verified account. Built only from a validated session row or the
/// provider /user response at callback time, never from request parameters.
type Account = { subject: Subject; login: string; identity: Identity };

export class HostedAuth extends DurableObject<HostedAuthEnv> {
  constructor(ctx: DurableObjectState, env: HostedAuthEnv) {
    super(ctx, env);
    ctx.storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS gh_states(state_hash TEXT PRIMARY KEY,binding_hash TEXT NOT NULL,return_to TEXT NOT NULL,expires_at INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS owner_sessions(session_hash TEXT PRIMARY KEY,csrf_hash TEXT NOT NULL,github_id TEXT NOT NULL,login TEXT NOT NULL,expires_at INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS device_requests(poll_hash TEXT PRIMARY KEY,user_code TEXT NOT NULL UNIQUE,name TEXT NOT NULL,state TEXT NOT NULL,agent_id TEXT,event_id TEXT NOT NULL UNIQUE,expires_at INTEGER NOT NULL,last_poll INTEGER NOT NULL,account TEXT,requester_hash TEXT);
      CREATE TABLE IF NOT EXISTS native_tokens(token_hash TEXT PRIMARY KEY,agent_id TEXT NOT NULL UNIQUE,expires_at INTEGER NOT NULL,revoked INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS hosted_agents(agent_id TEXT PRIMARY KEY,name TEXT NOT NULL,kind TEXT NOT NULL,created_at INTEGER NOT NULL,revoked INTEGER NOT NULL,account TEXT);
      CREATE TABLE IF NOT EXISTS auth_requests(id TEXT PRIMARY KEY,client_id TEXT NOT NULL,client_name TEXT NOT NULL,redirect_uri TEXT NOT NULL,resource TEXT NOT NULL,challenge TEXT NOT NULL,state TEXT,binding_hash TEXT NOT NULL,phase TEXT NOT NULL,event_id TEXT NOT NULL UNIQUE,agent_id TEXT,expires_at INTEGER NOT NULL,account TEXT);
      CREATE TABLE IF NOT EXISTS auth_codes(code_hash TEXT PRIMARY KEY,request_id TEXT NOT NULL UNIQUE,agent_id TEXT NOT NULL,client_id TEXT NOT NULL,redirect_uri TEXT NOT NULL,resource TEXT NOT NULL,challenge TEXT NOT NULL,expires_at INTEGER NOT NULL,used INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS token_families(id TEXT PRIMARY KEY,agent_id TEXT NOT NULL,name TEXT NOT NULL,client_id TEXT NOT NULL,resource TEXT NOT NULL,expires_at INTEGER NOT NULL,revoked INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS oauth_tokens(token_hash TEXT PRIMARY KEY,kind TEXT NOT NULL,family_id TEXT NOT NULL,expires_at INTEGER NOT NULL,used INTEGER NOT NULL);
      CREATE INDEX IF NOT EXISTS oauth_family ON oauth_tokens(family_id);
      CREATE TABLE IF NOT EXISTS auth_rates(scope TEXT PRIMARY KEY,window INTEGER NOT NULL,count INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
    `);
    this.#migrate();
  }
  /// v0 → v1: account columns. Fresh databases reach v1 through the CREATE
  /// TABLE text; legacy databases gain the same columns here, then decided
  /// rows are backfilled to the configured owner (every pre-upgrade row was
  /// theirs) while live pending/cancelled rows stay unclaimed. Tombstones,
  /// used codes, and replay records are never rewritten or dropped. DO SQLite
  /// denies PRAGMA writes, so the schema version lives in `meta`. Backfill is
  /// re-attempted whenever NULL decided rows remain, so a first migration run
  /// with a missing owner name still converges once configuration is fixed.
  #migrate() {
    const has = (table: string, column: string) => {
      try { this.ctx.storage.sql.exec(`SELECT ${column} FROM ${table} LIMIT 0`); return true; } catch { return false; }
    };
    const version = this.#row<{ value: string }>("SELECT value FROM meta WHERE key='schema_version'")?.value;
    if (version !== undefined && version !== '1') return;
    if (version === undefined) {
      if (!has('device_requests', 'account')) this.ctx.storage.sql.exec('ALTER TABLE device_requests ADD COLUMN account TEXT');
      if (!has('device_requests', 'requester_hash')) this.ctx.storage.sql.exec('ALTER TABLE device_requests ADD COLUMN requester_hash TEXT');
      if (!has('hosted_agents', 'account')) this.ctx.storage.sql.exec('ALTER TABLE hosted_agents ADD COLUMN account TEXT');
      if (!has('auth_requests', 'account')) this.ctx.storage.sql.exec('ALTER TABLE auth_requests ADD COLUMN account TEXT');
      this.ctx.storage.sql.exec('CREATE INDEX IF NOT EXISTS agents_account ON hosted_agents(account)');
      this.ctx.storage.sql.exec('CREATE INDEX IF NOT EXISTS device_account ON device_requests(account)');
      this.ctx.storage.sql.exec("INSERT OR REPLACE INTO meta VALUES('schema_version','1')");
    }
    const owner = this.env.EVERETT_OWNER_GITHUB_ID ? `github:${this.env.EVERETT_OWNER_GITHUB_ID}` : null;
    if (owner && this.#row<{ n: number }>('SELECT count(*) n FROM hosted_agents WHERE account IS NULL')!.n +
      this.#row<{ n: number }>("SELECT count(*) n FROM device_requests WHERE account IS NULL AND state NOT IN ('pending','cancelled')")!.n +
      this.#row<{ n: number }>("SELECT count(*) n FROM auth_requests WHERE account IS NULL AND phase NOT IN ('pending')")!.n > 0) {
      this.ctx.storage.sql.exec('UPDATE hosted_agents SET account=? WHERE account IS NULL', owner);
      this.ctx.storage.sql.exec("UPDATE device_requests SET account=? WHERE account IS NULL AND state NOT IN ('pending','cancelled')", owner);
      this.ctx.storage.sql.exec("UPDATE auth_requests SET account=? WHERE account IS NULL AND phase NOT IN ('pending')", owner);
    }
  }
  #row<T extends Record<string, SqlStorageValue>>(query: string, ...args: SqlStorageValue[]) { return this.ctx.storage.sql.exec<T>(query, ...args).toArray()[0]; }
  #count(table: string) { return this.#row<{ n: number }>(`SELECT count(*) n FROM ${table}`)!.n; }
  #identityFor(subject: Subject, c: HostedConfig): Identity { return { issuer: c.issuer, subject, org: null }; }
  #relayFor(identity: Identity) { return this.env.OWNERS.get(this.env.OWNERS.idFromName(namespace(identity))); }
  /// The enrollment flag admits or denies an account at every entry/liveness
  /// point. Owner mode is a one-account allowlist; public mode accepts any
  /// well-formed GitHub subject. Flipping back to owner disables non-owner
  /// sessions, tokens, and OAuth liveness without touching stored state.
  #accountAllowed(c: HostedConfig, subject: string): subject is Subject {
    if (!/^github:[1-9][0-9]{0,19}$/.test(subject)) return false;
    return c.enrollment === 'public' || subject === `github:${c.ownerId}`;
  }
  #accountFor(githubId: string, login: string, c: HostedConfig): Account {
    const subject = `github:${githubId}` as Subject;
    return { subject, login, identity: this.#identityFor(subject, c) };
  }
  /// Abuse-bucket key for pre-auth callers: keyed, domain-separated hash of the
  /// edge-supplied client IP. Never an identity or authorization input.
  #requester(request: Request, c: HostedConfig): string {
    const ip = request.headers.get('CF-Connecting-IP') ?? 'unknown';
    return hash(`everett-requester-v1:${c.githubSecret}:${ip}`);
  }
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
  #session(request: Request, c: HostedConfig): { row: Session; csrf: string; account: Account } | null {
    const raw = cookies(request)[cookieName(c, 'session')] ?? '', parts = raw.split('.');
    if (parts.length !== 2 || !parts.every(v => secretPattern.test(v))) return null;
    const row = this.#row<Session>('SELECT * FROM owner_sessions WHERE session_hash=?', hash(parts[0]));
    if (!row || row.expires_at <= Date.now() || !equal(row.csrf_hash, hash(parts[1]))) return null;
    if (c.enrollment === 'owner' && (row.github_id !== c.ownerId || row.login.toLowerCase() !== c.ownerLogin.toLowerCase())) return null;
    if (!this.#accountAllowed(c, `github:${row.github_id}`)) return null;
    return { row, csrf: parts[1], account: this.#accountFor(row.github_id, row.login, c) };
  }
  #mutation(request: Request, c: HostedConfig, p: Record<string, string>) {
    if (request.headers.get('Origin') !== new URL(request.url).origin) throw new RelayError('origin_denied', 403);
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
      const c = hostedConfig(this.env), url = new URL(request.url), host = requirePublicHost(url, c);
      // Caller-controlled streams must never hold the issuer's state gate.
      if (request.method === 'POST') {
        const buffered = await boundedText(request.body, 16384);
        request = new Request(request, { body: buffered, redirect: 'manual' });
      }
      return await this.ctx.blockConcurrencyWhile(async () => {
        // Expected denials must not reject blockConcurrencyWhile and reset the DO.
        try {
          this.#cleanup();
          return await this.#route(request, url, c, host);
        } catch (error) { return failure(error); }
      });
    } catch (error) { return failure(error); }
  }
  async verifyAgent(token: string): Promise<Principal | null> {
    try {
      const c = hostedConfig(this.env);
      if (!/^evt_[a-f0-9]{64}$/.test(token)) return null;
      const row = this.#row<{ token_hash: string; agent_id: string; expires_at: number; revoked: number; account: string | null }>(
        'SELECT t.token_hash,t.agent_id,t.expires_at,t.revoked,a.account FROM native_tokens t LEFT JOIN hosted_agents a ON a.agent_id=t.agent_id WHERE t.token_hash=?', hash(token));
      if (!row || row.revoked || row.expires_at <= Date.now() || !row.account || !this.#accountAllowed(c, row.account)) return null;
      const identity = this.#identityFor(row.account, c);
      if (!await this.#relayFor(identity).credentialActive(identity, row.agent_id)) return null;
      return { ...identity, agentId: row.agent_id, role: 'member', expiresAt: row.expires_at };
    } catch { return null; }
  }

  async relayDeviceLabels(principal: Principal, rename?: string): Promise<{ ok: true; value: { labels: Record<string, string>; complete: boolean } } | { ok: false; error: string; status: number }> {
    try { return { ok: true, value: await this.#deviceLabels(principal, rename) }; }
    catch (error) {
      if (error instanceof RelayError) return { ok: false, error: error.code, status: error.status };
      if (error instanceof ZodError) return { ok: false, error: 'invalid_request', status: 400 };
      throw error;
    }
  }

  async #deviceLabels(principal: Principal, rename?: string): Promise<{ labels: Record<string, string>; complete: boolean }> {
    const c = hostedConfig(this.env);
    const caller = this.#row<Agent>('SELECT * FROM hosted_agents WHERE agent_id=?', principal.agentId);
    if (!caller || !caller.account || !this.#accountAllowed(c, caller.account) || caller.revoked || principal.expiresAt <= Date.now() ||
        namespace(this.#identityFor(caller.account, c)) !== namespace(principal) ||
        this.#identityFor(caller.account, c).subject !== principal.subject) throw new RelayError('invalid_token', 401);
    const active = await this.#relayFor(this.#identityFor(caller.account, c)).activeCredentialIds(principal);
    if (!active.ok) throw new RelayError(active.error, active.status);
    const activeIds = new Set(active.value);
    if (rename !== undefined) {
      const name = cleanName.parse(rename);
      this.#rate(`device-rename:${caller.account}`, 30);
      this.ctx.storage.sql.exec('UPDATE hosted_agents SET name=? WHERE agent_id=? AND account=?', name, principal.agentId, caller.account);
    }
    const labels: Record<string, string> = {};
    const rows = this.ctx.storage.sql.exec<Agent>('SELECT * FROM hosted_agents WHERE account=? AND revoked=0 ORDER BY agent_id LIMIT ?', caller.account, DEVICE_LABEL_LIMIT + 1).toArray();
    for (const row of rows.slice(0, DEVICE_LABEL_LIMIT)) {
      if (activeIds.has(row.agent_id)) labels[row.agent_id] = row.name;
    }
    return { labels, complete: rows.length <= DEVICE_LABEL_LIMIT && active.value.length <= DEVICE_LABEL_LIMIT };
  }

  async #route(request: Request, url: URL, c: HostedConfig, host: string): Promise<Response> {
    const path = url.pathname;
    if (request.method === 'GET') {
      if (path === '/auth/github') return this.#githubStart(request, url, c, host);
      if (path === '/auth/callback/github') return this.#githubCallback(request, url, c, host);
      if (path === '/enroll') return this.#enrollPage(request, url, c);
      if (path === '/.well-known/oauth-authorization-server') {
        requireOAuth(c);
        return json({ issuer: c.issuer, authorization_endpoint: `${host}/oauth/authorize`, token_endpoint: `${host}/oauth/token`,
          revocation_endpoint: `${host}/oauth/revoke`, introspection_endpoint: `${host}/oauth/introspect`,
          scopes_supported: ['everett'], response_types_supported: ['code'], response_modes_supported: ['query'],
          grant_types_supported: ['authorization_code', 'refresh_token'], code_challenge_methods_supported: ['S256'],
          token_endpoint_auth_methods_supported: ['none'], revocation_endpoint_auth_methods_supported: ['none'],
          authorization_response_iss_parameter_supported: true, client_id_metadata_document_supported: true });
      }
      if (path === '/oauth/authorize') { requireOAuth(c); return this.#authorize(request, url, c, host); }
    }
    if (request.method === 'POST') {
      if (path === '/v2/auth/device') return this.#deviceStart(request, c, host);
      if (path === '/v2/auth/poll') return this.#devicePoll(request, c);
      if (path === '/v2/auth/cancel') return this.#deviceCancel(request);
      if (path === '/enroll/decision') return this.#deviceDecision(request, c, host);
      if (path === '/enroll/revoke') return this.#browserRevoke(request, c, host);
      if (path === '/auth/logout') {
        const p = await form(request, ['csrf']), session = this.#mutation(request, c, p);
        this.ctx.storage.sql.exec('DELETE FROM owner_sessions WHERE session_hash=?', session.row.session_hash);
        const response = redirect(`${host}/enroll`); setCookie(response, c, 'session', '', 0); return response;
      }
      if (path === '/oauth/consent') { requireOAuth(c); return this.#consent(request, c); }
      if (path === '/oauth/token') { requireOAuth(c); return this.#token(request, c); }
      if (path === '/oauth/introspect') { requireOAuth(c); return this.#introspect(request, c); }
      if (path === '/oauth/revoke') { requireOAuth(c); return this.#revokeToken(request, c); }
    }
    return json({ error: 'not_found' }, 404);
  }

  #githubStart(request: Request, url: URL, c: HostedConfig, host: string) {
    this.#rate('github-start', 20);
    if (c.enrollment === 'public') this.#rate(`requester:${this.#requester(request, c)}:github-start`, 5);
    const p = fields(url.searchParams, ['return_to']);
    const returnTo = p.return_to ?? '/enroll';
    const target = new URL(returnTo, host);
    if (!returnTo.startsWith('/enroll') || target.origin !== host || target.pathname !== '/enroll' || target.hash || returnTo.length > 2048) throw new RelayError('invalid_return_to', 400);
    if (this.#count('gh_states') >= 100) throw new RelayError('pending_limit', 429);
    const state = random(), verifier = random(), binding = `${random()}.${verifier}`;
    this.ctx.storage.sql.exec('INSERT INTO gh_states VALUES(?,?,?,?)', hash(state), hash(binding), `${target.pathname}${target.search}`, Date.now() + DEVICE_MS);
    const github = new URL('https://github.com/login/oauth/authorize');
    github.searchParams.set('client_id', c.githubId); github.searchParams.set('redirect_uri', `${host}/auth/callback/github`);
    github.searchParams.set('state', state); github.searchParams.set('code_challenge', s256(verifier)); github.searchParams.set('code_challenge_method', 'S256');
    github.searchParams.set('scope', 'read:user');
    if (c.enrollment === 'owner') github.searchParams.set('login', c.ownerLogin);
    const response = redirect(github.href, 302); setCookie(response, c, 'github', binding, 600); return response;
  }
  async #githubCallback(request: Request, url: URL, c: HostedConfig, host: string) {
    this.#rate('github-callback', 30);
    const p = fields(url.searchParams, ['state', 'code', 'error', 'error_description', 'error_uri', 'iss']);
    if (p.iss !== undefined && p.iss !== 'https://github.com/login/oauth') throw new RelayError('invalid_github_issuer', 400);
    if (!secretPattern.test(p.state ?? '')) throw new RelayError('invalid_github_state', 400);
    const row = this.#row<{ state_hash: string; binding_hash: string; return_to: string; expires_at: number }>('SELECT * FROM gh_states WHERE state_hash=?', hash(p.state));
    const binding = cookies(request)[cookieName(c, 'github')] ?? '', parts = binding.split('.');
    if (!row || row.expires_at <= Date.now() || parts.length !== 2 || !parts.every(v => secretPattern.test(v)) || !equal(hash(binding), row.binding_hash)) throw new RelayError('invalid_github_state', 400);
    // Consume before external I/O: an interrupted exchange requires a new login attempt.
    this.ctx.storage.sql.exec('DELETE FROM gh_states WHERE state_hash=?', row.state_hash);
    if (p.error || !p.code || p.code.length > 2048) throw new RelayError('github_login_denied', 403);
    const token = await this.#externalJSON('https://github.com/login/oauth/access_token', { method: 'POST', headers: { Accept: 'application/json', 'Content-Type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams({
      client_id: c.githubId, client_secret: c.githubSecret, code: p.code, redirect_uri: `${host}/auth/callback/github`, code_verifier: parts[1],
    }).toString() });
    if (typeof token.access_token !== 'string' || !token.access_token || token.access_token.length > 4096 || (token.token_type !== undefined && String(token.token_type).toLowerCase() !== 'bearer')) throw new RelayError('github_login_denied', 403);
    const user = await this.#externalJSON('https://api.github.com/user', { headers: { Authorization: `Bearer ${token.access_token}`, Accept: 'application/vnd.github+json', 'User-Agent': 'Everett', 'X-GitHub-Api-Version': '2022-11-28' } });
    const id = typeof user.id === 'number' && Number.isSafeInteger(user.id) ? String(user.id) : typeof user.id === 'string' ? user.id : '';
    const login = typeof user.login === 'string' ? user.login : '';
    if (c.enrollment === 'owner' && (id !== c.ownerId || login.toLowerCase() !== c.ownerLogin.toLowerCase())) throw new RelayError('owner_only', 403);
    if (c.enrollment === 'public' && (!/^[1-9][0-9]{0,19}$/.test(id) || !/^[A-Za-z0-9-]{1,39}$/.test(login))) throw new RelayError('github_login_denied', 403);
    if (c.enrollment === 'public') {
      if (this.#row<{ n: number }>('SELECT count(*) n FROM owner_sessions WHERE github_id=?', id)!.n >= 8)
        this.ctx.storage.sql.exec('DELETE FROM owner_sessions WHERE session_hash IN(SELECT session_hash FROM owner_sessions WHERE github_id=? ORDER BY expires_at LIMIT 1)', id);
      if (this.#count('owner_sessions') >= 4096) throw new RelayError('session_limit', 429);
    } else if (this.#count('owner_sessions') >= 32) {
      this.ctx.storage.sql.exec('DELETE FROM owner_sessions WHERE session_hash IN(SELECT session_hash FROM owner_sessions ORDER BY expires_at LIMIT 1)');
    }
    const sid = random(), csrf = random();
    this.ctx.storage.sql.exec('INSERT INTO owner_sessions VALUES(?,?,?,?,?)', hash(sid), hash(csrf), id, login, Date.now() + SESSION_MS);
    const response = redirect(`${host}${row.return_to}`); setCookie(response, c, 'session', `${sid}.${csrf}`, SESSION_MS / 1000); setCookie(response, c, 'github', '', 0);
    return response;
  }

  async #deviceStart(request: Request, c: HostedConfig, host: string) {
    this.#rate('device-start', 30);
    const { name } = z.strictObject({ name: cleanName }).parse(await body(request));
    if (this.#count('device_requests') >= 100) throw new RelayError('pending_limit', 429);
    let requester: string | null = null;
    if (c.enrollment === 'public') {
      requester = this.#requester(request, c);
      this.#rate(`requester:${requester}:device-start`, 10);
      // Count every live row, not only pending: cancelled rows persist until
      // expiry and would otherwise let one requester reach the global cap.
      if (this.#row<{ n: number }>('SELECT count(*) n FROM device_requests WHERE requester_hash=? AND expires_at>?', requester, Date.now())!.n >= 8) throw new RelayError('pending_limit', 429);
    }
    const poll = random(), code = random().slice(0, 12).toUpperCase();
    this.ctx.storage.sql.exec('INSERT INTO device_requests(poll_hash,user_code,name,state,agent_id,event_id,expires_at,last_poll,account,requester_hash) VALUES(?,?,?,?,?,?,?,?,?,?)', hash(poll), code, name, 'pending', null, crypto.randomUUID(), Date.now() + DEVICE_MS, 0, null, requester);
    return json({ device_code: poll, user_code: code, verification_uri: `${host}/enroll`, verification_uri_complete: `${host}/enroll?code=${code}`, expires_in: 600, interval: 5 });
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
    if (!row.agent_id || !row.account || !this.#accountAllowed(c, row.account)) return json({ state: 'denied', interval: 5 });
    const identity = this.#identityFor(row.account, c);
    if (!await this.#relayFor(identity).credentialActive(identity, row.agent_id)) return json({ state: 'denied', interval: 5 });
    const token = `evt_${random()}`, expires = Date.now() + REFRESH_MS;
    this.ctx.storage.transactionSync(() => {
      this.ctx.storage.sql.exec('INSERT INTO native_tokens VALUES(?,?,?,?)', hash(token), row.agent_id, expires, 0);
      this.ctx.storage.sql.exec("UPDATE device_requests SET state='consumed' WHERE poll_hash=?", row.poll_hash);
    });
    return json({ state: 'approved', token, agent_id: row.agent_id, owner: namespace(identity), expires_at: expires });
  }
  async #deviceCancel(request: Request) {
    this.#rate('device-cancel', 100);
    const { device_code } = z.strictObject({ device_code: z.string().regex(secretPattern) }).parse(await body(request));
    const row = this.#row<DeviceRequest>('SELECT * FROM device_requests WHERE poll_hash=?', hash(device_code));
    if (!row || row.expires_at <= Date.now()) return json({ state: 'expired' });
    if (row.state === 'pending') this.ctx.storage.sql.exec("UPDATE device_requests SET state='cancelled' WHERE poll_hash=?", row.poll_hash);
    return json({ state: row.state === 'pending' ? 'cancelled' : row.state });
  }
  /// Unexpired approving requests reserve a credential slot even before their
  /// hosted_agents row exists, so a claim cannot overshoot the cap it passed.
  #inflightClaims(account?: string) {
    const now = Date.now();
    const d = account
      ? this.#row<{ n: number }>("SELECT count(*) n FROM device_requests WHERE state='approving' AND expires_at>? AND account=?", now, account)!.n
      : this.#row<{ n: number }>("SELECT count(*) n FROM device_requests WHERE state='approving' AND expires_at>?", now)!.n;
    const o = account
      ? this.#row<{ n: number }>("SELECT count(*) n FROM auth_requests WHERE phase='approving' AND expires_at>? AND account=?", now, account)!.n
      : this.#row<{ n: number }>("SELECT count(*) n FROM auth_requests WHERE phase='approving' AND expires_at>?", now)!.n;
    return d + o;
  }
  /// Enforced before a fresh claim only. The claim already bounded the
  /// enrollment, so a crashed resume replays the idempotent relay event and
  /// inserts without re-checking — it can never wedge at a cap it passed.
  #credentialQuota(c: HostedConfig, account: Account) {
    if (c.enrollment === 'public') {
      const inflight = this.#inflightClaims(account.subject);
      if (this.#row<{ n: number }>('SELECT count(*) n FROM hosted_agents WHERE account=?', account.subject)!.n + inflight >= 128) throw new RelayError('credential_limit', 429);
      if (this.#row<{ n: number }>('SELECT count(*) n FROM hosted_agents WHERE account=? AND revoked=0', account.subject)!.n + inflight >= 64) throw new RelayError('credential_limit', 429);
      if (this.#count('hosted_agents') + this.#inflightClaims() >= 8192) throw new RelayError('credential_limit', 429);
    } else if (this.#count('hosted_agents') + this.#inflightClaims() >= DEVICE_LABEL_LIMIT) {
      throw new RelayError('credential_limit', 429);
    }
  }
  async #enroll(c: HostedConfig, account: Account, eventId: string, name: string, kind: string) {
    const result = await this.#relayFor(account.identity).enroll(account.identity, 'admin', eventId);
    if (!result.ok) throw new RelayError('enrollment_denied', 403);
    this.ctx.storage.sql.exec('INSERT OR IGNORE INTO hosted_agents(agent_id,name,kind,created_at,revoked,account) VALUES(?,?,?,?,?,?)', result.value.deviceId, name, kind, Date.now(), 0, account.subject);
    return result.value.deviceId;
  }
  async #deviceDecision(request: Request, c: HostedConfig, host: string) {
    if (c.enrollment === 'owner') this.#rate('browser-mutation', 60);
    const p = await form(request, ['csrf', 'user_code', 'decision']); const session = this.#mutation(request, c, p);
    const account = session.account;
    if (c.enrollment === 'public') this.#rate(`browser-mutation:${account.subject}`, 60);
    if (!/^[A-F0-9]{12}$/.test(p.user_code ?? '') || !['approve', 'deny'].includes(p.decision)) throw new RelayError('invalid_request', 400);
    const row = this.#row<DeviceRequest>('SELECT * FROM device_requests WHERE user_code=?', p.user_code);
    if (!row || row.expires_at <= Date.now() || !['pending', 'approving'].includes(row.state)) throw new RelayError('request_expired_or_decided', 409);
    // The claim is stamped in the same statement as the pending transition so a
    // crash mid-approval cannot wedge the request or let a second account retry
    // the relay's immutable enrollment event under its own namespace.
    if (p.decision === 'deny') {
      if (row.state !== 'pending') throw new RelayError('approval_in_progress', 409);
      const wrote = this.ctx.storage.sql.exec("UPDATE device_requests SET state='denied',account=? WHERE poll_hash=? AND state='pending' AND (account IS NULL OR account=?)", account.subject, row.poll_hash, account.subject).rowsWritten;
      if (!wrote) throw new RelayError('request_expired_or_decided', 409);
    } else {
      if (row.state === 'pending') {
        this.#credentialQuota(c, account);
        const wrote = this.ctx.storage.sql.exec("UPDATE device_requests SET state='approving',account=? WHERE poll_hash=? AND state='pending' AND (account IS NULL OR account=?)", account.subject, row.poll_hash, account.subject).rowsWritten;
        if (!wrote) throw new RelayError('request_expired_or_decided', 409);
      } else if (row.account !== account.subject) {
        throw new RelayError('request_expired_or_decided', 409);
      }
      const agent = await this.#enroll(c, account, row.event_id, row.name, 'device');
      this.ctx.storage.sql.exec("UPDATE device_requests SET state='approved',agent_id=? WHERE poll_hash=? AND account=?", agent, row.poll_hash, account.subject);
    }
    return redirect(`${host}/enroll?code=${p.user_code}`);
  }

  async #live(c: HostedConfig, agentId: string) {
    const row = this.#row<Agent>('SELECT * FROM hosted_agents WHERE agent_id=?', agentId);
    if (!row || row.revoked || !row.account || !this.#accountAllowed(c, row.account)) return false;
    const identity = this.#identityFor(row.account, c);
    return await this.#relayFor(identity).credentialActive(identity, agentId);
  }
  async #enrollPage(request: Request, url: URL, c: HostedConfig) {
    const p = fields(url.searchParams, ['code', 'request']);
    const session = this.#session(request, c);
    if (!session) {
      const returnTo = `${url.pathname}${url.search}`;
      const signIn = `<div class="auth-lead"><h1>Sign in to Everett</h1></div><div class="auth-action"><p class="auth-copy">Sign in with GitHub to review this device request. Check its name and full code before approving.</p><a class="auth-button" href="/auth/github?return_to=${esc(encodeURIComponent(returnTo))}"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 .8a11.2 11.2 0 0 0-3.54 21.83c.56.1.77-.24.77-.54v-2.1c-3.14.68-3.8-1.34-3.8-1.34-.51-1.3-1.25-1.65-1.25-1.65-1.02-.7.08-.69.08-.69 1.12.08 1.7 1.14 1.7 1.14 1 .1.44 2.78 3.26 1.97.1-.73.4-1.24.72-1.53-2.5-.28-5.12-1.25-5.12-5.57 0-1.23.44-2.23 1.15-3.02-.12-.28-.5-1.43.1-2.98 0 0 .94-.3 3.08 1.15a10.7 10.7 0 0 1 5.6 0c2.13-1.45 3.07-1.15 3.07-1.15.61 1.55.23 2.7.11 2.98.72.79 1.15 1.79 1.15 3.02 0 4.33-2.63 5.28-5.14 5.56.4.35.77 1.03.77 2.08v3.13c0 .3.2.65.78.54A11.2 11.2 0 0 0 12 .8Z"/></svg><span>Continue with GitHub</span></a></div>`;
      return html(signIn);
    }
    let callbackOrigin: string | undefined;
    let content = `<div class="auth-account"><span>Signed in as <strong>${esc(c.enrollment === 'owner' ? c.ownerLogin : session.account.login)}</strong></span><form method="post" action="/auth/logout">${csrfField(session.csrf)}<button class="auth-signout">Sign out</button></form></div>`;
    if (p.request) {
      const row = this.#row<AuthRequest>('SELECT * FROM auth_requests WHERE id=?', identifier.parse(p.request));
      const binding = cookies(request)[cookieName(c, 'oauth')] ?? '';
      const foreign = !!row && row.account !== null && row.account !== session.account.subject;
      if (!row || row.expires_at <= Date.now() || foreign || !secretPattern.test(binding) || !equal(row.binding_hash, hash(binding))) return html(`${content}<div class="auth-lead"><h1>Request expired</h1></div><section class="auth-state" data-state="expired"><h2 class="auth-state__title">This request is no longer available</h2><p>Authorization request expired or belongs to another browser.</p></section>`, 403);
      if (c.redirects.includes(row.redirect_uri)) callbackOrigin = new URL(row.redirect_uri).origin;
      content += `<div class="auth-lead"><h1>Connect ${esc(row.client_name)}</h1></div><div class="auth-action"><p class="auth-copy">Review the connection details before approving access.</p><dl class="auth-details"><div><dt>Client</dt><dd>${esc(row.client_id)}</dd></div><div><dt>Resource</dt><dd>${esc(row.resource)}</dd></div><div><dt>Redirect</dt><dd>${esc(row.redirect_uri)}</dd></div></dl>`;
      if (row.phase === 'pending' || row.phase === 'approving') content += `<form method="post" action="/oauth/consent">${csrfField(session.csrf)}<input type="hidden" name="request_id" value="${esc(row.id)}"><label>Agent name <input name="name" required maxlength="100" value="argos"></label><button name="decision" value="approve">Approve this agent</button><button name="decision" value="deny">Deny</button></form>`;
      else content += '<section class="auth-state" data-state="expired"><h2 class="auth-state__title">Already decided</h2><p>This authorization was already decided. Start a new connection.</p></section>';
      content += '</div>';
    } else {
      content += `<div class="auth-lead"><h1>Approve a device</h1><p class="auth-copy">Only approve a request you started. Compare the full code and device name with your terminal.</p></div><div class="auth-action"><form method="get" action="/enroll"><label for="device-code">Device code <input id="device-code" name="code" required maxlength="12" autocomplete="one-time-code" spellcheck="false" value="${esc(p.code ?? '')}"></label><button>Inspect</button></form>`;
      if (p.code) {
        if (!/^[A-F0-9]{12}$/.test(p.code)) throw new RelayError('invalid_user_code', 400);
        const row = this.#row<DeviceRequest>('SELECT * FROM device_requests WHERE user_code=?', p.code);
        const foreign = !!row && row.account !== null && row.account !== session.account.subject;
        if (!row || row.expires_at <= Date.now() || foreign) content += '<section class="auth-state" data-state="expired"><h2 class="auth-state__title">Request expired</h2><p>Start sign-in again in your CLI to get a new code.</p></section>';
        else {
          content += `<dl class="auth-details auth-details--device"><div><dt>Device</dt><dd><strong>${esc(row.name)}</strong></dd></div><div><dt>Full code</dt><dd><span class="auth-code">${esc(row.user_code)}</span></dd></div><div><dt>Request state</dt><dd>${esc(row.state)}</dd></div>${row.agent_id ? `<div><dt>Agent ID</dt><dd>${esc(row.agent_id)}</dd></div>` : ''}</dl>`;
          if ((row.state === 'approved' || row.state === 'consumed') && row.agent_id) {
            if (await this.#live(c, row.agent_id)) content += '<section class="auth-state" data-state="connected"><h2 class="auth-state__title">Connected</h2><p>Approval complete. You can close this tab and return to your terminal. Your CLI will finish signing in automatically.</p></section>';
            else content += '<section class="auth-state" data-state="revoked"><h2 class="auth-state__title">Credential revoked</h2><p>Start a new sign-in in your CLI if you want to connect this agent again.</p></section>';
          } else if (row.state === 'denied') content += '<section class="auth-state" data-state="denied"><h2 class="auth-state__title">Request denied</h2><p>This agent was not approved. Start a new sign-in in your CLI to try again.</p></section>';
          else if (row.state === 'cancelled') content += '<section class="auth-state" data-state="cancelled"><h2 class="auth-state__title">Request cancelled</h2><p>Start a new sign-in in your CLI to continue.</p></section>';
          else content += `<section class="auth-state" data-state="pending"><h2 class="auth-state__title">Waiting for approval</h2><p>Check the full code and device name. Keep your CLI running while you review this request.</p></section>`;
          if (row.state === 'pending' || row.state === 'approving') content += `<form class="auth-decision" method="post" action="/enroll/decision">${csrfField(session.csrf)}<input type="hidden" name="user_code" value="${esc(row.user_code)}"><button name="decision" value="approve">Approve this agent</button><button name="decision" value="deny">Deny</button></form>`;
        }
      }
      content += '</div>';
    }
    content += '<section class="auth-credentials"><h2>Agent credentials</h2><ul class="auth-agent-list">';
    const agents = this.ctx.storage.sql.exec<Agent>('SELECT * FROM hosted_agents WHERE account=? ORDER BY created_at DESC LIMIT 257', session.account.subject).toArray();
    for (const agent of agents.slice(0, 256)) {
      const live = await this.#live(c, agent.agent_id);
      content += `<li>${esc(agent.name)} · ${esc(agent.agent_id)} · ${esc(agent.kind)} · ${live ? 'active' : 'revoked'}${live ? `<form method="post" action="/enroll/revoke">${csrfField(session.csrf)}<input type="hidden" name="agent_id" value="${esc(agent.agent_id)}"><button>Revoke</button></form>` : ''}</li>`;
    }
    return html(`${content}</ul>${agents.length > 256 ? '<p>Only the newest 256 credentials are displayed.</p>' : ''}</section>`, 200, callbackOrigin);
  }
  async #browserRevoke(request: Request, c: HostedConfig, host: string) {
    if (c.enrollment === 'owner') this.#rate('browser-mutation', 60);
    const p = await form(request, ['csrf', 'agent_id']); const session = this.#mutation(request, c, p); identifier.parse(p.agent_id);
    if (c.enrollment === 'public') this.#rate(`browser-mutation:${session.account.subject}`, 60);
    if (!this.#row('SELECT agent_id FROM hosted_agents WHERE agent_id=? AND account=?', p.agent_id, session.account.subject)) throw new RelayError('unknown_agent', 404);
    if (!await this.#relayFor(session.account.identity).revokeCredential(session.account.identity, p.agent_id)) throw new RelayError('revocation_denied', 403);
    this.ctx.storage.transactionSync(() => {
      this.ctx.storage.sql.exec('UPDATE hosted_agents SET revoked=1 WHERE agent_id=?', p.agent_id);
      this.ctx.storage.sql.exec('UPDATE native_tokens SET revoked=1 WHERE agent_id=?', p.agent_id);
      this.ctx.storage.sql.exec('UPDATE token_families SET revoked=1 WHERE agent_id=?', p.agent_id);
    });
    return redirect(`${host}/enroll`);
  }

  async #authorize(request: Request, url: URL, c: HostedConfig, host: string) {
    this.#rate('oauth-authorize', 30);
    if (c.enrollment === 'public') this.#rate(`requester:${this.#requester(request, c)}:oauth-authorize`, 5);
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
    this.ctx.storage.sql.exec('INSERT INTO auth_requests(id,client_id,client_name,redirect_uri,resource,challenge,state,binding_hash,phase,event_id,agent_id,expires_at,account) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)', id, p.client_id, clientName, p.redirect_uri, p.resource, p.code_challenge, p.state ?? null, hash(binding), 'pending', crypto.randomUUID(), null, Date.now() + DEVICE_MS, null);
    const response = redirect(`${host}/enroll?request=${id}`, 302); setCookie(response, c, 'oauth', binding, 600); return response;
  }
  async #consent(request: Request, c: HostedConfig) {
    if (c.enrollment === 'owner') this.#rate('browser-mutation', 60);
    const p = await form(request, ['csrf', 'request_id', 'decision', 'name']); const session = this.#mutation(request, c, p);
    const account = session.account;
    if (c.enrollment === 'public') this.#rate(`browser-mutation:${account.subject}`, 60);
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
      const wrote = this.ctx.storage.sql.exec("UPDATE auth_requests SET phase='denied',account=? WHERE id=? AND phase='pending' AND (account IS NULL OR account=?)", account.subject, id, account.subject).rowsWritten;
      if (!wrote) throw new RelayError('request_expired_or_browser_mismatch', 403);
      callback.searchParams.set('error', 'access_denied'); return redirect(callback.href);
    }
    const name = cleanName.parse(p.name);
    if (this.#count('auth_codes') >= 200) throw new RelayError('code_limit', 429);
    if (row.phase === 'pending') {
      this.#credentialQuota(c, account);
      const wrote = this.ctx.storage.sql.exec("UPDATE auth_requests SET phase='approving',account=? WHERE id=? AND phase='pending' AND (account IS NULL OR account=?)", account.subject, id, account.subject).rowsWritten;
      if (!wrote) throw new RelayError('request_expired_or_browser_mismatch', 403);
    } else if (row.account !== account.subject) {
      throw new RelayError('request_expired_or_browser_mismatch', 403);
    }
    const agentId = await this.#enroll(c, account, row.event_id, name, 'oauth'), code = random();
    this.ctx.storage.transactionSync(() => {
      this.ctx.storage.sql.exec('INSERT INTO auth_codes VALUES(?,?,?,?,?,?,?,?,?)', hash(code), id, agentId, row.client_id, row.redirect_uri, row.resource, row.challenge, Date.now() + CODE_MS, 0);
      this.ctx.storage.sql.exec("UPDATE auth_requests SET phase='approved',agent_id=? WHERE id=? AND account=?", agentId, id, account.subject);
    });
    callback.searchParams.set('code', code); return redirect(callback.href);
  }

  #family(id: string) { return this.#row<Family>('SELECT * FROM token_families WHERE id=?', id); }
  #revokeFamily(id: string) { this.ctx.storage.sql.exec('UPDATE token_families SET revoked=1 WHERE id=?', id); }
  async #familyLive(c: HostedConfig, family: Family | undefined) {
    return !!family && !family.revoked && family.expires_at > Date.now() && c.clients.includes(family.client_id) && c.resources.includes(family.resource) && await this.#live(c, family.agent_id);
  }
  /// Public-mode per-account token bounds: the family→agent→account join is
  /// the same canonical ownership column the rest of the broker uses. Bounds
  /// both the issuance rate and retained rows so one account cannot starve or
  /// fill the shared oauth_tokens table.
  #tokenQuota(c: HostedConfig, agentId: string) {
    if (c.enrollment !== 'public') return;
    const account = this.#row<{ account: string }>('SELECT account FROM hosted_agents WHERE agent_id=?', agentId)?.account;
    if (!account) throw new RelayError('invalid_grant', 400);
    this.#rate(`oauth-token:${account}`, 40);
    const retained = this.#row<{ n: number }>('SELECT count(*) n FROM oauth_tokens t JOIN token_families f ON t.family_id=f.id JOIN hosted_agents a ON a.agent_id=f.agent_id WHERE a.account=?', account)!.n;
    if (retained > 2046) throw new RelayError('token_limit', 429);
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
      this.#tokenQuota(c, code.agent_id);
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
      this.#tokenQuota(c, family.agent_id);
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
