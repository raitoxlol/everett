import { env as bindings, exports as workerExports } from 'cloudflare:workers';
import { evictDurableObject, listDurableObjectIds, reset, runInDurableObject } from 'cloudflare:test';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { sha256 } from '@noble/hashes/sha256';
import { hkdf } from '@noble/hashes/hkdf';
import { ed25519, x25519 } from '@noble/curves/ed25519';
import { chacha20poly1305 } from '@noble/ciphers/chacha';
import { aadBytes, bytes, deviceBytes, encode, envelopeBytes, fingerprint, hexString, observationBytes, type Device, type Envelope, type Observation } from '../src/protocol';
import type { Env } from '../src/index';

const issuer = 'https://everett.test';
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
const fetchWorker = (path: string, init?: RequestInit) => workerExports.default.fetch(new Request(issuer + path, { ...init, redirect: 'manual' }));
const json = (body: unknown): RequestInit => ({ method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
const form = (body: Record<string, string>): RequestInit => ({ method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams(body).toString() });

class Browser {
  cookies = new Map<string, string>();
  async request(path: string, init: RequestInit = {}) {
    const headers = new Headers(init.headers);
    headers.set('Cookie', [...this.cookies].map(([k, v]) => `${k}=${v}`).join('; '));
    if (init.method === 'POST') headers.set('Origin', issuer);
    const response = await fetchWorker(path, { ...init, headers });
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
    if (url.href === 'https://github.com/login/oauth/access_token' && request.method === 'POST') return Response.json({ access_token: 'synthetic-provider-response-only', token_type: 'bearer' });
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
const nativeWhoami = (token: string) => nativeCall(token, 'whoami');
const challenge = (verifier: string) => btoa(String.fromCharCode(...sha256(new TextEncoder().encode(verifier)))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
async function authorize(browser: Browser, verifier = 'v'.repeat(64), overrides: Record<string, string> = {}) {
  const params = new URLSearchParams({ client_id: client, redirect_uri: redirect, resource, response_type: 'code', scope: 'everett', code_challenge_method: 'S256', code_challenge: challenge(verifier), state: 'synthetic-client-state', ...overrides });
  return browser.request('/oauth/authorize?' + params);
}
async function consent(browser: Browser, verifier = 'v'.repeat(64)) {
  const response = await authorize(browser, verifier);
  expect([302, 303]).includes(response.status);
  const location = new URL(response.headers.get('Location')!, issuer);
  const requestId = location.searchParams.get('request')!;
  expect(Boolean(requestId)).toBe(true);
  expectAuthShell(await (await browser.request(location.pathname + location.search)).text());
  const csrf = await browser.csrf(location.pathname + location.search);
  const approved = await browser.request('/oauth/consent', form({ csrf, request_id: requestId, decision: 'approve', name: 'argos-test' }));
  expect(approved.status).toBe(303);
  const callback = new URL(approved.headers.get('Location')!);
  expect(callback.origin + callback.pathname).toBe(redirect);
  expect(callback.searchParams.get('state')).toBe('synthetic-client-state');
  expect(callback.searchParams.get('iss')).toBe(issuer);
  return callback.searchParams.get('code')!;
}
async function exchange(code: string, verifier = 'v'.repeat(64), overrides: Record<string, string> = {}) {
  return fetchWorker('/oauth/token', form({ grant_type: 'authorization_code', code, client_id: client, redirect_uri: redirect, resource, code_verifier: verifier, ...overrides }));
}
async function introspect(token: string) {
  const init = form({ token }), headers = new Headers(init.headers);
  headers.set('Authorization', 'Bearer synthetic-introspection-test-value-only');
  const response = await fetchWorker('/oauth/introspect', { ...init, headers });
  expect(response.status).toBe(200);
  return await response.json() as { active: boolean; sub?: string; everett_agent?: string; aud?: string; iss?: string; scope?: string; exp?: number };
}

describe('Hosted device login through real Worker routes', () => {
  it('keeps validation and revocation responsive while an anonymous body stalls', async () => {
    const browser = new Browser(); await browser.login();
    const agent = await enroll(browser);
    const csrf = await browser.csrf();
    let close!: () => void;
    let pulled!: () => void;
    const started = new Promise<void>(resolve => { pulled = resolve; });
    const stream = new ReadableStream<Uint8Array>({
      start(controller) { close = () => controller.close(); },
      pull() { pulled(); },
    });
    const stalled = fetchWorker('/v2/auth/poll', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: stream });
    await started;
    // Let the request reach the Durable Object before issuing legitimate work.
    await new Promise(resolve => setTimeout(resolve, 25));
    const deadline = new Promise<never>((_, reject) => setTimeout(() => reject(new Error('auth blocked by anonymous body')), 1000));
    try {
      expect((await Promise.race([nativeWhoami(agent.token), deadline])).status).toBe(200);
      expect((await Promise.race([browser.request('/enroll/revoke', form({ csrf, agent_id: agent.agent_id })), deadline])).status).toBe(303);
      expect((await nativeWhoami(agent.token)).status).toBe(401);
    } finally { close(); await stalled; }
    expect((await fetchWorker('/v2/auth/poll', { ...json({}), body: 'x'.repeat(16385) })).status).toBe(413);
  });
  it('keeps pending enrollment anonymous, enforces poll interval and supports cancellation without membership', async () => {
    const pending = await begin();
    expect(pending.expires_in).toBe(600); expect(pending.interval).toBe(5);
    expect(new URL(pending.verification_uri_complete).origin).toBe(issuer);
    expect((await fetchWorker('/v2/auth/poll', json({ device_code: pending.device_code }))).status).toBe(200);
    expect((await fetchWorker('/v2/auth/poll', json({ device_code: pending.device_code }))).status).toBe(429);
    expect((await fetchWorker('/v2/auth/cancel', json({ device_code: pending.device_code }))).status).toBe(200);
    const response = await fetchWorker('/v2/auth/poll', json({ device_code: pending.device_code }));
    expect((await response.json() as { state: string }).state).toBe('cancelled');
    expect(await memberCount()).toBe(0);
    expect((await nativeWhoami('bogus')).status).toBe(401);
  });
  it('times out an unfinished body without creating membership', async () => {
    const stream = new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(new TextEncoder().encode('{')); } });
    const result = await fetchWorker('/v2/auth/poll', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: stream });
    expect(result.status).toBe(408);
    expect(await memberCount()).toBe(0);
  }, 10000);
  it('binds GitHub state to the browser and refuses non-owner ID or login', async () => {
    const browser = new Browser();
    const start = await browser.request('/auth/github');
    const state = new URL(start.headers.get('Location')!).searchParams.get('state')!;
    expect((await fetchWorker('/auth/callback/github?' + new URLSearchParams({ state, code: 'synthetic-code' }))).status).toBe(400);
    expect((await new Browser().login('/enroll', 1)).status).toBe(403);
    expect((await new Browser().login('/enroll', 282002453, 'imposter')).status).toBe(403);
    expect(await memberCount()).toBe(0);
  });
  it('sets a referrer policy that preserves same-origin POST Origin while redirects stay private', async () => {
    const page = await fetchWorker('/enroll');
    expect(page.headers.get('Referrer-Policy')).toBe('same-origin');

    const start = await fetchWorker('/auth/github');
    expect(start.headers.get('Referrer-Policy')).toBe('no-referrer');

    const callback = await new Browser().login();
    expect(callback.headers.get('Referrer-Policy')).toBe('no-referrer');
  });
  it('validates a supplied GitHub issuer, rejects duplicate issuer parameters, and accepts legacy callbacks without one', async () => {
    const start = async () => {
      const browser = new Browser();
      const response = await browser.request('/auth/github');
      const state = new URL(response.headers.get('Location')!).searchParams.get('state')!;
      return { browser, state };
    };

    const valid = await new Browser().login();
    expect(valid.status).toBe(303);

    const legacy = await start();
    expect((await legacy.browser.request('/auth/callback/github?' + new URLSearchParams({ state: legacy.state, code: 'synthetic-code' }))).status).toBe(303);

    const mismatch = await start();
    expect((await mismatch.browser.request('/auth/callback/github?' + new URLSearchParams({ state: mismatch.state, code: 'synthetic-code', iss: 'https://github.com' }))).status).toBe(400);

    const duplicate = await start();
    const query = new URLSearchParams({ state: duplicate.state, code: 'synthetic-code', iss: 'https://github.com/login/oauth' });
    expect((await duplicate.browser.request('/auth/callback/github?' + query + '&iss=https%3A%2F%2Fattacker.example')).status).toBe(400);
    expect(await memberCount()).toBe(0);
  });
  it('delivers one credential once, rejects browser-session bearer tokens, persists and revokes independently', async () => {
    const browser = new Browser(); expect([302, 303]).includes((await browser.login()).status);
    const first = await enroll(browser), second = await enroll(browser, 'second-test');
    expect(first.agent_id !== second.agent_id).toBe(true);
    expect((await nativeWhoami(first.token)).status).toBe(200);
    const again = await fetchWorker('/v2/auth/poll', json({ device_code: first.pending.device_code }));
    expect((await again.json() as { state: string }).state).toBe('consumed');
    for (const cookie of browser.cookies.values()) expect((await nativeWhoami(cookie)).status).toBe(401);
    await evictDurableObject(env.AUTH_STATE!.get(env.AUTH_STATE!.idFromName(issuer)));
    expect((await nativeWhoami(first.token)).status).toBe(200);
    const revoked = await browser.request('/enroll/revoke', form({ csrf: await browser.csrf(), agent_id: first.agent_id }));
    expect(revoked.status).toBe(303);
    expect((await nativeWhoami(first.token)).status).toBe(401);
    expect((await nativeWhoami(second.token)).status).toBe(200);
  });
  it('shows close guidance only after authoritative approval and never renders the native token', async () => {
    const pending = await begin('browser-flow');
    const pagePath = '/enroll?code=' + pending.user_code;
    const anonymous = await (await fetchWorker(pagePath)).text();
    expectAuthShell(anonymous);
    expect(anonymous).toContain('<h1>Sign in to Everett</h1>');
    expect(anonymous).toContain('Continue with GitHub');
    expect(anonymous).toContain('class="auth-button"');
    expect(anonymous).toContain('href="/auth/github?return_to=');
    expect(anonymous).toContain(`%2Fenroll%3Fcode%3D${pending.user_code}`);
    expect(anonymous).not.toContain(pending.device_code);

    const browser = new Browser(); await browser.login();
    const before = await (await browser.request(pagePath)).text();
    expectAuthShell(before);
    expectAuthState(before, 'pending');
    expectNestedAuthStateLayout(before, 'pending');
    expect(before.includes('Waiting for approval')).toBe(true);
    expect(before.includes('You can close this tab')).toBe(false);
    expect(before.includes(pending.device_code)).toBe(false);
    const decision = await browser.request('/enroll/decision', form({ csrf: await browser.csrf(pagePath), user_code: pending.user_code, decision: 'approve' }));
    expect(decision.status).toBe(303);
    const approved = await (await browser.request(pagePath)).text();
    expectAuthShell(approved);
    expectAuthState(approved, 'connected');
    expectNestedAuthStateLayout(approved, 'connected');
    expect(approved).toContain('<h2 class="auth-state__title">Connected</h2>');
    expect(approved.includes('Approval complete')).toBe(true);
    expect(approved.includes('You can close this tab')).toBe(true);
    const result = await fetchWorker('/v2/auth/poll', json({ device_code: pending.device_code }));
    const native = await result.json() as { token: string; agent_id: string };
    const consumed = await (await browser.request(pagePath)).text();
    expectAuthShell(consumed);
    expectAuthState(consumed, 'connected');
    expect(consumed.includes('Approval complete')).toBe(true);
    expect(consumed.includes(native.token)).toBe(false);
    expect((await browser.request('/enroll/revoke', form({ csrf: await browser.csrf(pagePath), agent_id: native.agent_id }))).status).toBe(303);
    const revoked = await (await browser.request(pagePath)).text();
    expectAuthShell(revoked);
    expectAuthState(revoked, 'revoked');
    expect(revoked.includes('Credential revoked')).toBe(true);
    expect(revoked.includes('You can close this tab')).toBe(false);
  });
  it('requires owner session, CSRF and exact Origin for approval', async () => {
    const pending = await begin(), browser = new Browser();
    expect((await fetchWorker('/enroll/decision', form({ csrf: 'forged', user_code: pending.user_code, decision: 'approve' }))).status).toBeGreaterThanOrEqual(400);
    await browser.login();
    const csrf = await browser.csrf('/enroll?code=' + pending.user_code);
    const init = form({ csrf, user_code: pending.user_code, decision: 'approve' }), headers = new Headers(init.headers);
    headers.set('Cookie', [...browser.cookies].map(([k, v]) => `${k}=${v}`).join('; ')); headers.set('Origin', 'https://attacker.example');
    expect((await fetchWorker('/enroll/decision', { ...init, headers })).status).toBe(403);
    expect((await browser.request('/enroll/decision', form({ csrf: 'forged', user_code: pending.user_code, decision: 'approve' }))).status).toBe(403);
    expect(await memberCount()).toBe(0);
  });
  it('keeps browser administrator authority out of issued native credentials', async () => {
    const browser = new Browser(); await browser.login();
    const first = await enroll(browser), second = await enroll(browser, 'second-device');
    const whoami = await (await nativeWhoami(first.token)).json() as { result: { owner: string; role: string } };
    expect(whoami.result.role).toBe('member');
    const seed = new Uint8Array(32).fill(7);
    const device = { owner: whoami.result.owner, deviceId: first.agent_id, signingKey: hexString(ed25519.getPublicKey(seed)), encryptionKey: hexString(x25519.getPublicKey(new Uint8Array(32).fill(9))), signature: '' };
    device.signature = hexString(ed25519.sign(deviceBytes(device), seed));
    expect((await nativeCall(first.token, 'register', { device })).status).toBe(200);
    expect((await nativeCall(first.token, 'revokeAgent', { deviceId: second.agent_id })).status).toBe(403);
    expect((await nativeCall(first.token, 'revokeMember', { subject: 'github:282002453' })).status).toBe(403);
    expect((await nativeWhoami(second.token)).status).toBe(200);
  });
  it('carries approved credentials through signed HTTP registration, encrypted send/read/reply and independent revocation', async () => {
    // Exercise the production Worker entrypoint throughout; only GitHub is synthetic.
    // Expiring provider-token fields are accepted without retaining provider tokens.
    vi.mocked(globalThis.fetch).mockImplementationOnce(async () => Response.json({
      access_token: 'synthetic-short-lived-provider-only', token_type: 'bearer', expires_in: 28800,
      refresh_token: 'synthetic-provider-refresh-only', refresh_token_expires_in: 15811200,
    }));
    const browser = new Browser(); expect((await browser.login()).status).toBe(303);
    const first = await enroll(browser, 'mac-synthetic'), second = await enroll(browser, 'nuc-synthetic');
    async function relay<T>(token: string, operation: string, args: unknown = {}): Promise<T> {
      const response = await nativeCall(token, operation, args);
      expect(response.status).toBe(200);
      return (await response.json() as { result: T }).result;
    }
    async function register(credential: typeof first, seed: number) {
      const identity = await relay<{ owner: string; deviceId: string; role: string }>(credential.token, 'whoami');
      expect(identity.deviceId).toBe(credential.agent_id); expect(identity.role).toBe('member');
      const signing = new Uint8Array(32).fill(seed), encryption = new Uint8Array(32).fill(seed + 1);
      const device: Device = { owner: identity.owner, deviceId: identity.deviceId, signingKey: hexString(ed25519.getPublicKey(signing)), encryptionKey: hexString(x25519.getPublicKey(encryption)), signature: '' };
      device.signature = hexString(ed25519.sign(deviceBytes(device), signing));
      await relay(credential.token, 'register', { device });
      return { ...credential, device, signing, encryption };
    }
    const mac = await register(first, 31), nuc = await register(second, 33);
    expect(await relay(mac.token, 'device', { deviceId: nuc.agent_id })).toEqual(nuc.device);
    const encoder = new TextEncoder();
    function encrypt(from: typeof mac, to: typeof mac, text: string, parent: Envelope | null = null): Envelope {
      const ephemeral = new Uint8Array(32).fill(parent ? 42 : 41), now = Date.now();
      const envelope: Envelope = {
        owner: from.device.owner, recipientOwner: to.device.owner, grantId: null,
        fromDevice: from.agent_id, toDevice: to.agent_id, messageId: crypto.randomUUID(), idempotencyKey: crypto.randomUUID(),
        parent: parent ? { owner: parent.owner, messageId: parent.messageId } : null, hop: parent ? parent.hop + 1 : 1,
        createdAt: now, expiresAt: now + 60000, senderFingerprint: fingerprint(from.device), recipientFingerprint: fingerprint(to.device),
        ephemeralKey: hexString(x25519.getPublicKey(ephemeral)), nonce: '12'.repeat(12), ciphertext: '', signature: '',
      };
      const shared = x25519.getSharedSecret(ephemeral, bytes(to.device.encryptionKey));
      const key = hkdf(sha256, shared, encoder.encode('everett-relay-v2'), aadBytes(envelope), 32);
      envelope.ciphertext = hexString(chacha20poly1305(key, bytes(envelope.nonce), aadBytes(envelope)).encrypt(encode({ text })));
      envelope.signature = hexString(ed25519.sign(envelopeBytes(envelope), from.signing));
      return envelope;
    }
    function decrypt(recipient: typeof mac, envelope: Envelope) {
      const shared = x25519.getSharedSecret(recipient.encryption, bytes(envelope.ephemeralKey));
      const key = hkdf(sha256, shared, encoder.encode('everett-relay-v2'), aadBytes(envelope), 32);
      return JSON.parse(new TextDecoder().decode(chacha20poly1305(key, bytes(envelope.nonce), aadBytes(envelope)).decrypt(bytes(envelope.ciphertext))));
    }
    async function observe(recipient: typeof mac, envelope: Envelope, state: Observation['state']) {
      const observation: Observation = { deviceId: recipient.agent_id, senderOwner: envelope.owner, messageId: envelope.messageId, state, at: Date.now(), signature: '' };
      observation.signature = hexString(ed25519.sign(observationBytes(recipient.device.owner, observation), recipient.signing));
      await relay(recipient.token, 'observe', observation);
    }
    const sent = encrypt(mac, nuc, 'synthetic Mac to NUC');
    expect(await relay(mac.token, 'enqueue', { envelope: sent })).toEqual({ messageId: sent.messageId, state: 'queued' });
    await evictDurableObject(env.AUTH_STATE!.get(env.AUTH_STATE!.idFromName(issuer)));
    for (const id of await listDurableObjectIds(env.OWNERS)) await evictDurableObject(env.OWNERS.get(id));
    const pending = await relay<{ envelopes: { envelope: Envelope }[] }>(nuc.token, 'pending', { deviceId: nuc.agent_id });
    expect(pending.envelopes).toHaveLength(1);
    expect(decrypt(nuc, pending.envelopes[0].envelope)).toEqual({ text: 'synthetic Mac to NUC' });
    await observe(nuc, sent, 'delivered');
    const reply = encrypt(nuc, mac, 'synthetic NUC reply', sent);
    await relay(nuc.token, 'enqueue', { envelope: reply });
    await observe(nuc, sent, 'replied');
    const replies = await relay<{ envelopes: { envelope: Envelope }[] }>(mac.token, 'pending', { deviceId: mac.agent_id });
    expect(replies.envelopes).toHaveLength(1);
    expect(decrypt(mac, replies.envelopes[0].envelope)).toEqual({ text: 'synthetic NUC reply' });
    await observe(mac, reply, 'delivered');
    expect(await relay(mac.token, 'status', { fromDevice: mac.agent_id, messageId: sent.messageId })).toEqual({ state: 'replied' });
    expect((await browser.request('/enroll/revoke', form({ csrf: await browser.csrf(), agent_id: nuc.agent_id }))).status).toBe(303);
    expect((await nativeWhoami(nuc.token)).status).toBe(401);
    expect((await nativeCall(nuc.token, 'pending', { deviceId: nuc.agent_id })).status).toBe(401);
    expect((await nativeWhoami(mac.token)).status).toBe(200);
    expect((await nativeCall(mac.token, 'enqueue', { envelope: encrypt(mac, nuc, 'must be refused') })).status).toBe(403);
    expect(vi.mocked(globalThis.fetch)).toHaveBeenCalledTimes(2);
  });
});

describe('OAuth resource-bound authorization', () => {
  it('advertises only implemented grants and refuses unknown clients, resources and redirects', async () => {
    const metadata = await (await fetchWorker('/.well-known/oauth-authorization-server')).json() as Record<string, unknown>;
    expect(metadata.issuer).toBe(issuer); expect(metadata.registration_endpoint).toBeUndefined();
    expect(metadata.code_challenge_methods_supported).toEqual(['S256']);
    const browser = new Browser();
    const cases: Record<string, string>[] = [{ resource: 'https://attacker.example/mcp' }, { redirect_uri: 'https://attacker.example/callback' }, { client_id: 'https://attacker.example/client.json' }, { code_challenge_method: 'plain' }, { scope: 'admin' }];
    for (const bad of cases) {
      const result = await authorize(browser, 'v'.repeat(64), bad);
      expect(result.status).toBeGreaterThanOrEqual(400);
      expect(result.headers.get('Location')).toBeNull();
    }
  });
  it('exchanges consent once, rejects OAuth tokens as native credentials and detects refresh replay', async () => {
    const browser = new Browser(); await browser.login();
    const code = await consent(browser), result = await exchange(code);
    expect(result.status).toBe(200);
    const tokens = await result.json() as { access_token: string; refresh_token: string; expires_in: number };
    expect(tokens.expires_in).toBe(900);
    expect(/^eva_[0-9a-f]{64}$/.test(tokens.access_token)).toBe(true);
    const checked = await introspect(tokens.access_token);
    expect(checked.active).toBe(true); expect(checked.iss).toBe(issuer); expect(checked.aud).toBe(resource);
    expect(checked.sub).toBe(checked.everett_agent); expect(checked.scope).toBe('everett');
    expect((await nativeWhoami(tokens.access_token)).status).toBe(401);
    const refreshBody = { grant_type: 'refresh_token', refresh_token: tokens.refresh_token, client_id: client, resource };
    const rotated = await fetchWorker('/oauth/token', form(refreshBody)); expect(rotated.status).toBe(200);
    const next = await rotated.json() as { access_token: string; refresh_token: string };
    expect((await introspect(next.access_token)).active).toBe(true);
    expect((await fetchWorker('/oauth/token', form(refreshBody))).status).toBe(400);
    expect((await introspect(tokens.access_token)).active).toBe(false);
    expect((await introspect(next.access_token)).active).toBe(false);
    expect((await fetchWorker('/oauth/token', form({ ...refreshBody, refresh_token: next.refresh_token }))).status).toBe(400);
  });
  it('rejects wrong PKCE and exact token binding without minting credentials', async () => {
    const browser = new Browser(); await browser.login();
    const code = await consent(browser);
    expect((await exchange(code, 'x'.repeat(64))).status).toBe(400);
    expect((await exchange(code, 'v'.repeat(64), { resource: 'https://attacker.example/mcp' })).status).toBe(400);
    expect((await exchange(code, 'v'.repeat(64), { redirect_uri: 'https://attacker.example/callback' })).status).toBe(400);
  });
  it('revokes tokens after authorization-code replay', async () => {
    const browser = new Browser(); await browser.login();
    const code = await consent(browser), result = await exchange(code);
    expect(result.status).toBe(200);
    const tokens = await result.json() as { access_token: string };
    expect((await introspect(tokens.access_token)).active).toBe(true);
    expect((await exchange(code)).status).toBe(400);
    expect((await introspect(tokens.access_token)).active).toBe(false);
  });
  it('retains authorization-code replay detection after expiring code rows', async () => {
    const browser = new Browser(); await browser.login();
    const code = await consent(browser), result = await exchange(code);
    expect(result.status).toBe(200);
    const tokens = await result.json() as { access_token: string };
    const auth = env.AUTH_STATE!.get(env.AUTH_STATE!.idFromName(issuer));
    await runInDurableObject(auth, (_instance, state) => { state.storage.sql.exec('UPDATE auth_codes SET expires_at=0'); });
    expect((await exchange(code)).status).toBe(400);
    expect((await introspect(tokens.access_token)).active).toBe(false);
  });
  it('serializes simultaneous code exchanges and revokes the issued family on replay', async () => {
    const browser = new Browser(); await browser.login();
    const code = await consent(browser);
    const responses = await Promise.all([exchange(code), exchange(code)]);
    expect(responses.map(r => r.status).sort()).toEqual([200, 400]);
    const tokens = await responses.find(r => r.status === 200)!.json() as { access_token: string };
    expect((await introspect(tokens.access_token)).active).toBe(false);
  });
  it('serializes simultaneous refreshes and revokes the issued family on replay', async () => {
    const browser = new Browser(); await browser.login();
    const response = await exchange(await consent(browser));
    const tokens = await response.json() as { access_token: string; refresh_token: string };
    const refresh = () => fetchWorker('/oauth/token', form({ grant_type: 'refresh_token', refresh_token: tokens.refresh_token, client_id: client, resource }));
    const responses = await Promise.all([refresh(), refresh()]);
    expect(responses.map(r => r.status).sort()).toEqual([200, 400]);
    const next = await responses.find(r => r.status === 200)!.json() as { access_token: string };
    expect((await introspect(tokens.access_token)).active).toBe(false);
    expect((await introspect(next.access_token)).active).toBe(false);
  });
  it('binds OAuth consent to the initiating browser even when another browser is also the owner', async () => {
    const first = new Browser(), second = new Browser(); await first.login(); await second.login();
    const response = await authorize(first), target = new URL(response.headers.get('Location')!, issuer);
    const requestId = target.searchParams.get('request')!;
    const csrf = await second.csrf();
    const approved = await second.request('/oauth/consent', form({ csrf, request_id: requestId, decision: 'approve', name: 'stolen-request' }));
    expect(approved.status).toBe(403);
    expect(await memberCount()).toBe(0);
  });
  it('allows explicit denial and request expiry without creating an agent', async () => {
    const browser = new Browser(); await browser.login();
    const denied = await begin();
    expect((await browser.request('/enroll/decision', form({ csrf: await browser.csrf('/enroll?code=' + denied.user_code), user_code: denied.user_code, decision: 'deny' }))).status).toBe(303);
    const response = await fetchWorker('/v2/auth/poll', json({ device_code: denied.device_code }));
    expect((await response.json() as { state: string }).state).toBe('denied');
    const deniedPage = await (await browser.request('/enroll?code=' + denied.user_code)).text();
    expectAuthShell(deniedPage);
    expectAuthState(deniedPage, 'denied');
    expect(deniedPage.includes('Request denied')).toBe(true);
    expect(deniedPage.includes('You can close this tab')).toBe(false);
    const expired = await begin('expiry-test');
    const auth = env.AUTH_STATE!.get(env.AUTH_STATE!.idFromName(issuer));
    await runInDurableObject(auth, (_instance, state) => { state.storage.sql.exec('UPDATE device_requests SET expires_at=0'); });
    const result = await fetchWorker('/v2/auth/poll', json({ device_code: expired.device_code }));
    expect((await result.json() as { state: string }).state).toBe('expired');
    const expiredPage = await (await browser.request('/enroll?code=' + expired.user_code)).text();
    expectAuthShell(expiredPage);
    expectAuthState(expiredPage, 'expired');
    expect(expiredPage.includes('Request expired')).toBe(true);
    expect(expiredPage.includes('You can close this tab')).toBe(false);
    expect(await memberCount()).toBe(0);
  });
  it('stores only token hashes and rejects duplicate token parameters', async () => {
    const browser = new Browser(); await browser.login();
    const native = await enroll(browser);
    const code = await consent(browser), response = await exchange(code);
    expect(response.status).toBe(200);
    const oauth = await response.json() as { access_token: string; refresh_token: string };
    const auth = env.AUTH_STATE!.get(env.AUTH_STATE!.idFromName(issuer));
    const safe = await runInDurableObject(auth, (_instance, state) => {
      const rows = state.storage.sql.exec<{ token_hash: string }>('SELECT token_hash FROM native_tokens UNION ALL SELECT token_hash FROM oauth_tokens').toArray();
      return rows.length === 3 && rows.every(r => /^[a-f0-9]{64}$/.test(r.token_hash));
    });
    expect(safe).toBe(true);
    expect((await nativeWhoami(native.token)).status).toBe(200);
    expect((await introspect(oauth.access_token)).active).toBe(true);
    const malformed = form({ grant_type: 'refresh_token', refresh_token: oauth.refresh_token, client_id: client, resource });
    malformed.body = String(malformed.body) + '&client_id=duplicate';
    expect((await fetchWorker('/oauth/token', malformed)).status).toBe(400);
    expect((await introspect(oauth.access_token)).active).toBe(true);
  });
  it('applies owner revocation to introspection and refresh immediately and protects introspection secret', async () => {
    const browser = new Browser(); await browser.login();
    const result = await exchange(await consent(browser)); expect(result.status).toBe(200);
    const tokens = await result.json() as { access_token: string; refresh_token: string };
    const identity = await introspect(tokens.access_token); expect(identity.active).toBe(true);
    expect((await fetchWorker('/oauth/introspect', form({ token: tokens.access_token }))).status).toBe(401);
    expect((await browser.request('/enroll/revoke', form({ csrf: await browser.csrf(), agent_id: identity.everett_agent! }))).status).toBe(303);
    expect((await introspect(tokens.access_token)).active).toBe(false);
    expect((await fetchWorker('/oauth/token', form({ grant_type: 'refresh_token', refresh_token: tokens.refresh_token, client_id: client, resource }))).status).toBe(400);
  });
});
