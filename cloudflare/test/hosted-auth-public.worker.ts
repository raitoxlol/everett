import { env as bindings, exports as workerExports } from 'cloudflare:workers';
import { evictDurableObject, reset, runInDurableObject } from 'cloudflare:test';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { sha256 } from '@noble/hashes/sha256';
import { namespace, deviceBytes, hexString } from '../src/protocol';
import { ed25519, x25519 } from '@noble/curves/ed25519';
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

const oauthClient = 'https://chatgpt.com/oauth/client.json';
const oauthRedirect = 'https://chatgpt.com/connector_platform_oauth_redirect';
const oauthResource = 'https://gateway.example/mcp';
const challenge = (verifier: string) => btoa(String.fromCharCode(...sha256(new TextEncoder().encode(verifier)))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
async function authorize(browser: Browser, verifier = 'v'.repeat(64)) {
  const params = new URLSearchParams({ client_id: oauthClient, redirect_uri: oauthRedirect, resource: oauthResource, response_type: 'code', scope: 'everett', code_challenge_method: 'S256', code_challenge: challenge(verifier), state: 'synthetic-client-state' });
  const response = await browser.request('/oauth/authorize?' + params);
  expect([302, 303]).includes(response.status);
  return new URL(response.headers.get('Location')!, issuer);
}

describe('Public enrollment: account isolation', () => {
  it('lists owner-scoped advisory device labels and renames only the caller', async () => {
    const alice = await loginAs(ALICE), bob = await loginAs(BOB);
    const a = await begin('alice-nuc'), a2 = await begin('alice-nuc'), b = await begin('private-bob');
    await approve(alice, a.user_code); await approve(alice, a2.user_code); await approve(bob, b.user_code);
    const pa = (await poll(a.device_code)).body, pa2 = (await poll(a2.device_code)).body, pb = (await poll(b.device_code)).body;
    const signing = new Uint8Array(32).fill(17), encryption = new Uint8Array(32).fill(18);
    const bundle = { owner: pa.owner!, deviceId: pa.agent_id!, signingKey: hexString(ed25519.getPublicKey(signing)), encryptionKey: hexString(x25519.getPublicKey(encryption)), signature: '' };
    bundle.signature = hexString(ed25519.sign(deviceBytes(bundle), signing));
    expect((await nativeCall(pa.token!, 'register', { device: bundle })).status).toBe(200);
    const signedBefore = await (await nativeCall(pa.token!, 'devices')).json();
    const response = await nativeCall(pa.token!, 'deviceLabels');
    expect(response.status).toBe(200);
    const labels = (await response.json() as { result: { labels: Record<string, string> } }).result.labels;
    expect(labels).toEqual({ [pa.agent_id!]: 'alice-nuc', [pa2.agent_id!]: 'alice-nuc' });
    expect(JSON.stringify(labels)).not.toContain(pb.agent_id!);
    expect((await nativeCall(pa.token!, 'renameDevice', { name: 'renamed-nuc', deviceId: pb.agent_id })).status).toBe(400);
    expect((await nativeCall(pa.token!, 'renameDevice', { name: '\u001b[31m' })).status).toBe(400);
    expect((await nativeCall(pa.token!, 'renameDevice', { name: 'renamed-nuc' })).status).toBe(200);
    const updated = await (await nativeCall(pa2.token!, 'deviceLabels')).json() as { result: { labels: Record<string, string> } };
    expect(updated.result.labels[pa.agent_id!]).toBe('renamed-nuc');
    expect(updated.result.labels[pa2.agent_id!]).toBe('alice-nuc');
    expect(await (await nativeCall(pa.token!, 'devices')).json()).toEqual(signedBefore);
    expect(signedBefore).toEqual({ result: [bundle] });
    const stub = env.AUTH_STATE!.get(env.AUTH_STATE!.idFromName(issuer));
    await evictDurableObject(stub);
    expect(JSON.stringify(await (await nativeCall(pb.token!, 'deviceLabels')).json())).not.toContain('renamed-nuc');
    expect(JSON.stringify(await (await nativeCall(pa.token!, 'deviceLabels')).json())).toContain('renamed-nuc');
    await runInDurableObject(stub, (_i, state) => { state.storage.sql.exec('DELETE FROM hosted_agents WHERE agent_id=?', pa2.agent_id!); });
    expect(JSON.stringify(await (await nativeCall(pa.token!, 'deviceLabels')).json())).not.toContain(pa2.agent_id!);
    expect((await nativeCall(pa2.token!, 'deviceLabels')).status).toBe(401);
    await alice.request('/enroll/revoke', form({ csrf: await alice.csrf(), agent_id: pa.agent_id! }));
    expect((await nativeCall(pa.token!, 'deviceLabels')).status).toBe(401);
    expect(JSON.stringify(await (await nativeCall(pa2.token!, 'deviceLabels')).json())).not.toContain('renamed-nuc');
  });

  it('batches 64 active labels, excludes relay revocations and preserves RPC error status', async () => {
    const alice = await loginAs(ALICE), pending = await begin('caller');
    await approve(alice, pending.user_code);
    const pa = (await poll(pending.device_code)).body;
    const identity = { issuer, subject: `github:${ALICE.id}`, org: null };
    const relay = env.OWNERS.get(env.OWNERS.idFromName(namespace(identity)));
    const auth = env.AUTH_STATE!.get(env.AUTH_STATE!.idFromName(issuer));
    const ids: string[] = [];
    for (let i = 0; i < 63; i++) {
      const enrolled = await relay.enroll(identity, 'admin', crypto.randomUUID());
      if (!enrolled.ok) throw new Error(enrolled.error);
      ids.push(enrolled.value.deviceId);
    }
    await runInDurableObject(auth, (_i, state) => {
      for (const id of ids) state.storage.sql.exec('INSERT INTO hosted_agents VALUES(?,?,?,?,?,?)', id, 'duplicate name', 'device', Date.now(), 0, identity.subject);
      state.storage.sql.exec('INSERT INTO hosted_agents VALUES(?,?,?,?,?,?)', 'legacy-unbound', 'must-not-leak', 'device', Date.now(), 0, null);
    });
    const listed = await (await nativeCall(pa.token!, 'deviceLabels')).json() as { result: { labels: Record<string, string> } };
    expect(Object.keys(listed.result.labels)).toHaveLength(64);
    expect(JSON.stringify(listed)).not.toContain('must-not-leak');
    expect(await relay.revokeCredential(identity, ids[0])).toBe(true);
    const after = await (await nativeCall(pa.token!, 'deviceLabels')).json() as { result: { labels: Record<string, string> } };
    expect(Object.keys(after.result.labels)).toHaveLength(63);
    expect(after.result.labels[ids[0]]).toBeUndefined();
    for (let i = 0; i < 30; i++) expect((await nativeCall(pa.token!, 'renameDevice', { name: 'caller' })).status).toBe(200);
    expect((await nativeCall(pa.token!, 'renameDevice', { name: 'caller' })).status).toBe(429);
  });

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

describe('Public enrollment: shared-cap isolation', () => {
  it('counts cancelled device requests against the per-requester live-row cap', async () => {
    for (let i = 0; i < 8; i++) {
      const device = await begin('flood-' + i);
      const cancelled = await fetchWorker('/v2/auth/cancel', json({ device_code: device.device_code }));
      expect(cancelled.status).toBe(200);
    }
    const ninth = await fetchWorker('/v2/auth/device', json({ name: 'one-too-many' }));
    expect(ninth.status).toBe(429);
  });

  it('caps anonymous GitHub sign-in starts per requester below the global queue', async () => {
    for (let i = 0; i < 5; i++) {
      const start = await new Browser().request('/auth/github?return_to=' + encodeURIComponent('/enroll'));
      expect([302, 303]).includes(start.status);
    }
    const sixth = await new Browser().request('/auth/github?return_to=' + encodeURIComponent('/enroll'));
    expect(sixth.status).toBe(429);
  });

  it('lets a stamped approving request finish enrollment without re-running quota', async () => {
    const alice = await loginAs(ALICE);
    const device = await begin('resume-device');
    const stub = env.AUTH_STATE!.get(env.AUTH_STATE!.idFromName(issuer));
    await runInDurableObject(stub, (_i, state) => {
      state.storage.sql.exec("UPDATE device_requests SET state='approving',account=? WHERE user_code=?", `github:${ALICE.id}`, device.user_code);
    });
    expect((await approve(alice, device.user_code)).status).toBe(303);
    const { body } = await poll(device.device_code);
    expect(body.state).toBe('approved');
    expect(body.owner).toBe(namespace({ issuer, subject: `github:${ALICE.id}`, org: null }));
  });

  it('counts stranded approving claims against the account credential cap', async () => {
    const alice = await loginAs(ALICE), bob = await loginAs(BOB);
    const stub = env.AUTH_STATE!.get(env.AUTH_STATE!.idFromName(issuer));
    await runInDurableObject(stub, (_i, state) => {
      for (let i = 0; i < 63; i++) state.storage.sql.exec("INSERT INTO hosted_agents(agent_id,name,kind,created_at,revoked,account) VALUES(?,?,?,?,0,?)", `agent-${i}`, 'n', 'device', Date.now(), `github:${ALICE.id}`);
      state.storage.sql.exec("INSERT INTO device_requests(poll_hash,user_code,name,state,agent_id,event_id,expires_at,last_poll,account,requester_hash) VALUES('stranded-poll','STRANDEDCODE','stranded','approving',NULL,'stranded-event',?,0,?,NULL)", Date.now() + 600000, `github:${ALICE.id}`);
    });
    const blocked = await begin('cap-check-alice');
    expect((await approve(alice, blocked.user_code)).status).toBe(429);
    const bobs = await begin('cap-check-bob');
    expect((await approve(bob, bobs.user_code)).status).toBe(303);
  });

  it('shows no request details when a foreign account opens a claimed OAuth consent page', async () => {
    const alice = await loginAs(ALICE), bob = await loginAs(BOB);
    const location = await authorize(alice);
    const csrf = await alice.csrf(location.pathname + location.search);
    const approved = await alice.request('/oauth/consent', form({ csrf, request_id: location.searchParams.get('request')!, decision: 'approve', name: 'alice-agent' }));
    expect(approved.status).toBe(303);

    const foreignPage = await bob.request(location.pathname + location.search);
    expect(foreignPage.status).toBe(403);
    const foreignHtml = await foreignPage.text();
    expect(foreignHtml).not.toContain('alice-agent');
    expect(foreignHtml).not.toContain('/oauth/consent');
    const foreignConsent = await bob.request('/oauth/consent', form({ csrf: await bob.csrf('/enroll'), request_id: location.searchParams.get('request')!, decision: 'approve', name: 'stolen' }));
    expect(foreignConsent.status).toBeGreaterThanOrEqual(400);
  });
});
