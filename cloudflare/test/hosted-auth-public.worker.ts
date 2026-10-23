import { env as bindings, exports as workerExports } from 'cloudflare:workers';
import { evictDurableObject, reset, runInDurableObject } from 'cloudflare:test';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { namespace } from '../src/protocol';
import type { Env } from '../src/index';

const issuer = 'https://everett.test';
const env = bindings as unknown as Env;
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
    const github = new URL(start.headers.get('Location')!);
    expect(github.origin).toBe('https://github.com');
    return this.request('/auth/callback/github?' + new URLSearchParams({ state: github.searchParams.get('state')!, code: 'synthetic-code', iss: 'https://github.com/login/oauth' }));
  }
  async csrf(path = '/enroll') {
    const response = await this.request(path);
    const token = (await response.text()).match(/name=["']csrf["'][^>]*value=["']([^"']+)["']/)?.[1];
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
    if (url.href === 'https://chatgpt.com/oauth/client.json') return Response.json({ client_id: 'https://chatgpt.com/oauth/client.json', redirect_uris: ['https://chatgpt.com/connector_platform_oauth_redirect'], token_endpoint_auth_method: 'none' });
    throw new Error('unexpected outbound provider request');
  });
});
afterEach(async () => { vi.restoreAllMocks(); await reset(); });

const ALICE = { id: 70001, login: 'alice-dev' }, BOB = { id: 70002, login: 'bob-dev' };
const loginAs = (user: { id: number; login: string }) => { const b = new Browser(); return b.login('/enroll', user.id, user.login).then(() => b); };

async function begin(name = 'public-device') {
  const response = await fetchWorker('/v2/auth/device', json({ name }));
  expect(response.status).toBe(200);
  return await response.json() as { device_code: string; user_code: string; verification_uri_complete: string };
}
async function approve(browser: Browser, userCode: string) {
  const csrf = await browser.csrf('/enroll?code=' + userCode);
  return browser.request('/enroll/decision', form({ csrf, user_code: userCode, decision: 'approve' }));
}
async function poll(deviceCode: string) {
  const response = await fetchWorker('/v2/auth/poll', json({ device_code: deviceCode }));
  return { status: response.status, body: await response.json() as { state: string; token?: string; agent_id?: string; owner?: string } };
}
async function nativeCall(token: string, operation: string, args: unknown = {}) {
  const init = json(args), headers = new Headers(init.headers);
  headers.set('Authorization', 'Bearer ' + token);
  return fetchWorker('/v2/relay/' + operation, { ...init, headers });
}
const whoami = async (token: string) => (await (await nativeCall(token, 'whoami')).json() as { result: { owner: string; deviceId: string; role: string } }).result;

describe('Public enrollment: account isolation', () => {
  it('lets two GitHub users approve their own devices into separate relay namespaces', async () => {
    const alice = await loginAs(ALICE), bob = await loginAs(BOB);
    const aDevice = await begin('alice-laptop'), bDevice = await begin('bob-desktop');
    expect((await approve(alice, aDevice.user_code)).status).toBe(303);
    expect((await approve(bob, bDevice.user_code)).status).toBe(303);
    const aPoll = await poll(aDevice.device_code), bPoll = await poll(bDevice.device_code);
    expect(aPoll.body.state).toBe('approved'); expect(bPoll.body.state).toBe('approved');
    expect(aPoll.body.owner).not.toBe(bPoll.body.owner);
    const aWho = await whoami(aPoll.body.token!), bWho = await whoami(bPoll.body.token!);
    expect(aWho.owner).toBe(namespace({ issuer, subject: `github:${ALICE.id}`, org: null }));
    expect(bWho.owner).toBe(namespace({ issuer, subject: `github:${BOB.id}`, org: null }));
  });

  it('hides a claimed request and its credentials from other accounts', async () => {
    const alice = await loginAs(ALICE), bob = await loginAs(BOB);
    const device = await begin('alice-laptop');
    expect((await approve(alice, device.user_code)).status).toBe(303);
    const { body } = await poll(device.device_code);

    const foreign = await (await bob.request('/enroll?code=' + device.user_code)).text();
    expect(foreign).not.toContain('alice-laptop');
    expect(foreign).not.toContain(body.agent_id!);
    expect((await approve(bob, device.user_code)).status).toBeGreaterThanOrEqual(400);

    const bobsList = await (await bob.request('/enroll')).text();
    expect(bobsList).not.toContain(body.agent_id!);
    const revoke = await bob.request('/enroll/revoke', form({ csrf: await bob.csrf(), agent_id: body.agent_id! }));
    expect(revoke.status).toBe(404);
    expect((await nativeCall(body.token!, 'whoami')).status).toBe(200);
  });

  it('binds an unclaimed code to whichever verified account approves it', async () => {
    const [alice, bob] = await Promise.all([loginAs(ALICE), loginAs(BOB)]);
    const device = await begin('shared-code');
    expect((await approve(bob, device.user_code)).status).toBe(303);
    expect((await approve(alice, device.user_code)).status).toBeGreaterThanOrEqual(400);
    const { body } = await poll(device.device_code);
    expect(body.owner).toBe(namespace({ issuer, subject: `github:${BOB.id}`, org: null }));
  });

  it('stays idempotent under repeated same-account approval and polling', async () => {
    const alice = await loginAs(ALICE);
    const device = await begin('retry-laptop');
    expect((await approve(alice, device.user_code)).status).toBe(303);
    const first = await poll(device.device_code), second = await poll(device.device_code);
    expect(first.body.state).toBe('approved');
    expect(second.body.state).toBe('consumed');
    expect(second.body.token).toBeUndefined();
  });

  it('keeps cross-owner relay delivery denied for public accounts', async () => {
    const alice = await loginAs(ALICE);
    const device = await begin('alice-laptop');
    expect((await approve(alice, device.user_code)).status).toBe(303);
    const { body } = await poll(device.device_code);
    const who = await whoami(body.token!);
    const envelope = {
      owner: who.owner, recipientOwner: 'other-namespace', grantId: null,
      fromDevice: who.deviceId, toDevice: 'agent-foreign', messageId: 'm1', idempotencyKey: 'k1',
      parent: null, hop: 1, createdAt: Date.now(), expiresAt: Date.now() + 60000,
      senderFingerprint: '0'.repeat(64), recipientFingerprint: '0'.repeat(64),
      ephemeralKey: '0'.repeat(64), nonce: '0'.repeat(24), ciphertext: 'ab'.repeat(48), signature: '0'.repeat(128),
    };
    const denied = await (await nativeCall(body.token!, 'enqueue', { envelope })).json() as { error?: string };
    expect(denied.error).toBe('cross_owner_not_supported');
  });

  it('backfills decided legacy rows to the owner and leaves live pending rows claimable', async () => {
    const stub = env.AUTH_STATE!.get(env.AUTH_STATE!.idFromName(issuer));
    await runInDurableObject(stub, (_i, state) => {
      state.storage.sql.exec("INSERT INTO device_requests VALUES('legacy-poll','LEGACYCODE123','legacy-device','approved','agent-legacy','legacy-event',? ,0,NULL,NULL)", Date.now() + 600000);
      state.storage.sql.exec("INSERT INTO device_requests VALUES('live-poll','LIVECODE1234','live-device','pending',NULL,'live-event',? ,0,NULL,NULL)", Date.now() + 600000);
      state.storage.sql.exec("DELETE FROM meta WHERE key='schema_version'");
    });
    await evictDurableObject(stub);
    await runInDurableObject(stub, (_i, state) => {
      const decided = state.storage.sql.exec<{ account: string | null }>("SELECT account FROM device_requests WHERE poll_hash='legacy-poll'").one();
      const live = state.storage.sql.exec<{ account: string | null }>("SELECT account FROM device_requests WHERE poll_hash='live-poll'").one();
      expect(decided.account).toBe('github:282002453');
      expect(live.account).toBeNull();
      expect(state.storage.sql.exec<{ value: string }>("SELECT value FROM meta WHERE key='schema_version'").one().value).toBe('1');
    });
  });
});
