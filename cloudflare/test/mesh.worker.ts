import { env as bindings, exports as workerExports } from 'cloudflare:workers';
import { evictDurableObject, listDurableObjectIds, reset, runInDurableObject } from 'cloudflare:test';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { sha256 } from '@noble/hashes/sha256';
import { hkdf } from '@noble/hashes/hkdf';
import { ed25519, x25519 } from '@noble/curves/ed25519';
import { chacha20poly1305 } from '@noble/ciphers/chacha';
import { aadBytes, bytes, deviceBytes, encode, envelopeBytes, fingerprint, hexString, namespace, observationBytes, RelayError, type Device, type Envelope, type Observation } from '../src/protocol';
import { hostedConfig, type HostedAuthEnv } from '../src/hosted-config';
import type { Env } from '../src/index';

const issuer = 'https://everett.test';
const relayHost = 'https://relay.everett.test';
const resource = 'https://gateway.example/mcp';
const redirect = 'https://chatgpt.com/connector_platform_oauth_redirect';
const client = 'https://chatgpt.com/oauth/client.json';
const env = bindings as unknown as Env;
async function memberCount() {
  let count = 0;
  for (const id of await listDurableObjectIds(env.OWNERS)) {
    count += await runInDurableObject(env.OWNERS.get(id), (_instance, state) => state.storage.sql.exec<{ n: number }>('SELECT count(*) n FROM members').one().n);
  }
  return count;
}
const fetchOn = (origin: string, path: string, init?: RequestInit) => workerExports.default.fetch(new Request(origin + path, { ...init, redirect: 'manual' }));
const fetchWorker = (path: string, init?: RequestInit) => fetchOn(issuer, path, init);
const json = (body: unknown): RequestInit => ({ method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
const form = (body: Record<string, string>): RequestInit => ({ method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams(body).toString() });

class Browser {
  cookies = new Map<string, string>();
  constructor(readonly origin = issuer) {}
  async request(path: string, init: RequestInit = {}) {
    const headers = new Headers(init.headers);
    headers.set('Cookie', [...this.cookies].map(([k, v]) => `${k}=${v}`).join('; '));
    if (init.method === 'POST') headers.set('Origin', this.origin);
    const response = await fetchOn(this.origin, path, { ...init, headers });
    for (const cookie of response.headers.getAll('Set-Cookie')) {
      const pair = cookie.split(';', 1)[0], at = pair.indexOf('=');
      this.cookies.set(pair.slice(0, at), pair.slice(at + 1));
    }
    return response;
  }
  async login(returnTo = '/enroll', userId = 282002453, login = 'raitoxlol') {
    providerUser = { id: userId, login };
    const start = await this.request('/auth/github?return_to=' + encodeURIComponent(returnTo));
    expect(start.status).toBe(302);
    const github = new URL(start.headers.get('Location')!);
    expect(github.origin).toBe('https://github.com');
    expect(github.searchParams.get('code_challenge_method')).toBe('S256');
    return this.request('/auth/callback/github?' + new URLSearchParams({ state: github.searchParams.get('state')!, code: 'synthetic-code', iss: 'https://github.com/login/oauth' }));
  }
  async csrf(path = '/enroll') {
    const response = await this.request(path);
    expect(response.status).toBe(200);
    const html = await response.text();
    const token = html.match(/name=["']csrf["'][^>]*value=["']([^"']+)["']/)?.[1];
    expect(Boolean(token)).toBe(true);
    return token!;
  }
}

let providerUser = { id: 282002453, login: 'raitoxlol' };
beforeEach(() => {
  providerUser = { id: 282002453, login: 'raitoxlol' };
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
    const request = new Request(input, init), url = new URL(request.url);
    if (url.href === 'https://github.com/login/oauth/access_token' && request.method === 'POST') {
      const body = new TextDecoder().decode(await request.arrayBuffer());
      expect(body).toContain(encodeURIComponent(`${issuer}/auth/callback/github`));
      expect(body).not.toContain(relayHost);
      return Response.json({ access_token: 'synthetic-provider-response-only', token_type: 'bearer' });
    }
    if (url.href === 'https://api.github.com/user' && request.method === 'GET') return Response.json(providerUser);
    if (url.href === client) return Response.json({ client_id: client, client_name: 'Synthetic ChatGPT', redirect_uris: [redirect], token_endpoint_auth_method: 'none' });
    throw new Error('unexpected outbound provider request');
  });
});
afterEach(async () => { vi.restoreAllMocks(); await reset(); });

async function begin(name = 'mac-mini-test') {
  const response = await fetchWorker('/v2/auth/device', json({ name }));
  expect(response.status).toBe(200);
  return await response.json() as { device_code: string; user_code: string; expires_in: number; interval: number; verification_uri_complete: string };
}
function expectAuthShell(page: string) {
  expect(page).toContain('<html lang="en">');
  expect(page).toContain('<header class="auth-header">');
  expect(page).toContain('<main id="main" class="auth-main">');
  expect(page).toContain('<span class="auth-brand" aria-label="Everett">everett</span>');
  expect(page).toContain('<div class="auth-content">');
  expect(page).toMatch(/<div class="auth-lead"><h1>[^<]+<\/h1>/);
  expect(page).not.toContain('auth-card');
  expect(page).not.toContain('PRIVATE BY DESIGN');
  expect(page).not.toContain('SECURE DEVICE ACCESS');
}
function expectAuthState(page: string, state: 'pending' | 'connected' | 'denied' | 'cancelled' | 'expired' | 'revoked') {
  expect(page).toContain(`<section class="auth-state" data-state="${state}"`);
}
function expectNestedAuthStateLayout(page: string, state: 'pending' | 'connected') {
  expect(page).toMatch(new RegExp(`<div class="auth-action">[\\s\\S]*?<section class="auth-state" data-state="${state}"`));
  expect(page).toContain('<style>.auth-action>.auth-state{grid-column:1/-1}</style>');
}
async function enroll(browser: Browser, name = 'mac-mini-test') {
  const pending = await begin(name);
  const csrf = await browser.csrf('/enroll?code=' + pending.user_code);
  const approved = await browser.request('/enroll/decision', form({ csrf, user_code: pending.user_code, decision: 'approve' }));
  expect(approved.status).toBe(303);
  const response = await fetchWorker('/v2/auth/poll', json({ device_code: pending.device_code }));
  expect(response.status).toBe(200);
  const credential = await response.json() as { state: string; token: string; agent_id: string; expires_at: number };
  expect(credential.state).toBe('approved');
  expect(/^evt_[0-9a-f]{64}$/.test(credential.token)).toBe(true);
  return { ...credential, pending };
}
async function nativeCall(token: string, operation: string, args: unknown = {}) {
  const init = json(args);
  const headers = new Headers(init.headers); headers.set('Authorization', 'Bearer ' + token);
  return fetchWorker('/v2/relay/' + operation, { ...init, headers });
}

const meshCall = (token: string, path = '/v1/mesh/snapshot', init: RequestInit = {}) => fetchWorker(path, { ...init, headers: { ...Object.fromEntries(new Headers(init.headers)), Authorization: 'Bearer ' + token } });
async function setup() {
 const browser = new Browser(); await browser.login(); const credential = await enroll(browser);
 const response = await meshCall(credential.token, '/v1/mesh/setup', json({ origin: 'https://console.example' }));
 expect(response.status).toBe(201);
 const grant = await response.json() as { id: string; token: string; approvalUrl: string; expiresAt: number };
 return { browser, credential, grant };
}
async function approve(s: Awaited<ReturnType<typeof setup>>) {
 const csrf = await s.browser.csrf('/enroll?mesh=' + s.grant.id);
 expect((await s.browser.request('/enroll/mesh', form({ csrf, id: s.grant.id, decision: 'approve' }))).status).toBe(303);
}
describe('mesh v1 opt-in boundary', () => {
 it('starts denied, requires explicit owner consent and never upgrades a read grant', async () => {
  const s = await setup();
  expect((await meshCall(s.grant.token)).status).toBe(403);
  const page = await (await s.browser.request('/enroll?mesh=' + s.grant.id)).text();
  expect(page).toContain('mesh:read:v1'); expect(page).toContain('https://console.example');
  await approve(s);
  expect((await meshCall(s.grant.token)).status).toBe(200);
  expect((await nativeCall(s.grant.token, 'devices')).status).toBe(401);
  expect((await meshCall(s.grant.token, '/v1/mesh/setup', json({ origin: 'https://evil.example' }))).status).toBe(401);
 });
 it('enforces CSRF, browser origin and account ownership on decisions', async () => {
  const s = await setup();
  expect((await s.browser.request('/enroll/mesh', form({ csrf: 'forged', id: s.grant.id, decision: 'approve' }))).status).toBe(403);
  expect((await fetchWorker('/enroll/mesh', form({ csrf: await s.browser.csrf(), id: s.grant.id, decision: 'approve' }))).status).toBe(403);
  expect((await meshCall('emr_' + '0'.repeat(64))).status).toBe(401);
 });
 it('rechecks revocation for an existing open-reader token and clears further reads', async () => {
  const s = await setup(); await approve(s);
  const read = () => meshCall(s.grant.token);
  expect((await read()).status).toBe(200);
  expect((await s.browser.request('/enroll/mesh', form({ csrf: await s.browser.csrf(), id: s.grant.id, decision: 'revoke' }))).status).toBe(303);
  for (let i = 0; i < 3; i++) { const r = await read(); expect(r.status).toBe(403); expect(r.headers.get('Cache-Control')).toBe('no-store'); }
 });
 it('stops access when the initiating native credential is revoked', async () => {
  const s = await setup(); await approve(s);
  expect((await s.browser.request('/enroll/revoke', form({ csrf: await s.browser.csrf(), agent_id: s.credential.agent_id }))).status).toBe(303);
  expect((await meshCall(s.grant.token)).status).toBe(403);
 });
 it('expires grants, refuses unexpected scope and refuses non-HTTPS console origins', async () => {
  const s = await setup();
  expect((await meshCall(s.credential.token, '/v1/mesh/setup', json({ origin: 'http://console.example' }))).status).toBe(400);
  expect((await meshCall(s.credential.token, '/v1/mesh/setup', json({ origin: 'https://console.example', scope: 'write' }))).status).toBe(400);
  await approve(s);
  vi.spyOn(Date, 'now').mockReturnValue(s.grant.expiresAt + 1);
  expect((await meshCall(s.grant.token)).status).toBe(403);
 });
 it('projects genuine signed relay receipts, session IDs and aggregate queue without content or keys', async () => {
  const s = await setup(); const second = await enroll(s.browser, 'nuc-test');
  async function register(c: typeof s.credential, seed: number) {
   const who = await (await nativeCall(c.token, 'whoami')).json() as any;
   const signing = new Uint8Array(32).fill(seed);
   const device: Device = { owner: who.result.owner, deviceId: c.agent_id, signingKey: hexString(ed25519.getPublicKey(signing)), encryptionKey: hexString(x25519.getPublicKey(new Uint8Array(32).fill(seed+1))), signature: '' };
   device.signature = hexString(ed25519.sign(deviceBytes(device), signing));
   expect((await nativeCall(c.token, 'register', { device })).status).toBe(200);
   return { ...c, device, signing };
  }
  const a = await register(s.credential, 31), b = await register(second, 33);
  await nativeCall(a.token, 'exposeSession', { session: 'session-real', harness: 'codex', card: 'PRIVATE-CARD-BODY' });
  const now = Date.now();
  const e: Envelope = { owner:a.device.owner, recipientOwner:a.device.owner, grantId:null, fromDevice:a.agent_id, toDevice:b.agent_id, messageId:crypto.randomUUID(), idempotencyKey:crypto.randomUUID(), parent:null, hop:1, createdAt:now, expiresAt:now+60000, senderFingerprint:fingerprint(a.device), recipientFingerprint:fingerprint(b.device), ephemeralKey:hexString(x25519.getPublicKey(new Uint8Array(32).fill(41))), nonce:'12'.repeat(12), ciphertext:'ab'.repeat(32), signature:'' };
  e.signature=hexString(ed25519.sign(envelopeBytes(e),a.signing));
  expect((await nativeCall(a.token,'enqueue',{ envelope:e })).status).toBe(200);
  await approve(s);
  let snapshot = await (await meshCall(s.grant.token)).json() as any;
  expect(snapshot.version).toBe('everett.mesh.v1'); expect(snapshot.source).toBe('cloudflare'); expect(snapshot.queue.queued).toBe(1);
  expect(snapshot.sessions).toContainEqual(expect.objectContaining({ sessionId:'session-real', deviceId:a.agent_id, harness:'codex', selfReported:{ status:null, confidence:'unavailable', observedAt:null } }));
  const o: Observation = { deviceId:b.agent_id,senderOwner:e.owner,messageId:e.messageId,state:'delivered',at:Date.now(),signature:'' };
  o.signature=hexString(ed25519.sign(observationBytes(e.owner,o),b.signing));
  expect((await nativeCall(b.token,'observe',o)).status).toBe(200);
  await evictDurableObject(env.AUTH_STATE!.get(env.AUTH_STATE!.idFromName(issuer)));
  for (const id of await listDurableObjectIds(env.OWNERS)) await evictDurableObject(env.OWNERS.get(id));
  const response = await meshCall(s.grant.token); snapshot = await response.json() as any;
  expect(snapshot.queue.queued).toBe(0);
  expect(snapshot.receipts).toContainEqual(expect.objectContaining({ messageId:e.messageId,state:'delivered',deviceId:b.agent_id,at:o.at,confidence:'signed-device-observation' }));
  const text = JSON.stringify(snapshot);
  for (const privateValue of ['PRIVATE-CARD-BODY', e.ciphertext, a.device.signingKey, a.token, s.grant.token, 'idempotencyKey', 'signature', 'envelope']) expect(text).not.toContain(privateValue);
  expect(snapshot.devices).toContainEqual(expect.objectContaining({ deviceId:a.agent_id,lastSeen:expect.any(Number) }));
  expect(response.headers.get('Cache-Control')).toBe('no-store');
 });
});
