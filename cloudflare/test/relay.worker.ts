import { env as bindings, exports as workerExports } from 'cloudflare:workers';
import { runDurableObjectAlarm, runInDurableObject, evictDurableObject, reset } from 'cloudflare:test';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ed25519, x25519 } from '@noble/curves/ed25519';
import { hkdf } from '@noble/hashes/hkdf';
import { sha256 } from '@noble/hashes/sha256';
import { chacha20poly1305 } from '@noble/ciphers/chacha';
import { handleRequest, type Env, type Operation } from '../src/index';
import type { RelayResult } from '../src/owner-relay';
import { bindingVerifier } from '../src/auth';
import {
  namespace, deviceBytes, fingerprint, envelopeBytes, aadBytes, observationBytes,
  rotationBytes, encode, hexString, bytes, verify, validateDevice,
  type Device, type Envelope, type Principal, type Identity, type Observation,
} from '../src/protocol';
import fixture from './fixtures/relay-wire-v2.json';

const env = bindings as unknown as Env;
const encoder = new TextEncoder();
const secret = (n: number) => new Uint8Array(32).fill(n);
const sign = (data: Uint8Array, n: number) => hexString(ed25519.sign(data, secret(n)));
const org = () => `org-${crypto.randomUUID()}`;
function device(p: Principal, n: number): Device {
  const d = {
    owner: namespace(p), deviceId: p.agentId,
    signingKey: hexString(ed25519.getPublicKey(secret(n))),
    encryptionKey: hexString(x25519.getPublicKey(secret(n + 1))), signature: '',
  };
  d.signature = sign(deviceBytes(d), n); return d;
}
async function agent(space: string, subject = 'alice', role: 'admin' | 'member' = 'admin', n = 1) {
  const identity: Identity = { issuer: 'https://issuer.example/', subject, org: space };
  const stub = env.OWNERS.get(env.OWNERS.idFromName(namespace(identity)));
  const result = await stub.enroll(identity, role, crypto.randomUUID());
  if (!result.ok) throw new Error(result.error);
  const enrolled = result.value;
  const p: Principal = { ...identity, agentId: enrolled.deviceId, role, expiresAt: Date.now() + 3600000 };
  const d = device(p, n);
  await call(p, 'register', { device: d });
  return { p, d, stub, n };
}
async function call(p: Principal, op: Operation, args: unknown = {}): Promise<any> {
  const stub = env.OWNERS.get(env.OWNERS.idFromName(namespace(p)));
  const response: RelayResult = JSON.parse(await stub.relay(p, op, args));
  if (!response.ok) throw new Error(response.error);
  return response.value;
}
function envelope(a: Awaited<ReturnType<typeof agent>>, b: Awaited<ReturnType<typeof agent>>, overrides: Partial<Envelope> = {}): Envelope {
  const now = Date.now();
  const e: Envelope = {
    owner: namespace(a.p), recipientOwner: namespace(b.p), grantId: null,
    fromDevice: a.p.agentId, toDevice: b.p.agentId,
    messageId: crypto.randomUUID(), idempotencyKey: crypto.randomUUID(), parent: null,
    hop: 1, createdAt: now, expiresAt: now + 60000,
    senderFingerprint: fingerprint(a.d), recipientFingerprint: fingerprint(b.d),
    ephemeralKey: hexString(x25519.getPublicKey(secret(101))), nonce: '11'.repeat(12), ciphertext: '', signature: '',
    ...overrides,
  };
  const shared = x25519.getSharedSecret(secret(101), bytes(b.d.encryptionKey));
  const key = hkdf(sha256, shared, encoder.encode('everett-relay-v2'), aadBytes(e), 32);
  e.ciphertext = hexString(chacha20poly1305(key, bytes(e.nonce), aadBytes(e)).encrypt(encode({ kind: 'test', text: 'synthetic worker relay' })));
  e.signature = sign(envelopeBytes(e), a.n); return e;
}
function observe(b: Awaited<ReturnType<typeof agent>>, e: Envelope, state: Observation['state'] = 'delivered') {
  const o: Observation = { deviceId: b.p.agentId, senderOwner: e.owner, messageId: e.messageId, state, at: Date.now(), signature: '' };
  o.signature = sign(observationBytes(namespace(b.p), o), b.n); return o;
}
const request = (op: string, body: unknown = {}, headers: Record<string, string> = {}) => new Request(`https://everett.test/v2/relay/${op}`, { method: 'POST', headers: { 'Content-Type': 'application/json', ...headers }, body: JSON.stringify(body) });
afterEach(async () => { vi.restoreAllMocks(); await reset(); });

describe('Workers boundary', () => {
  it('fails closed without authentication and has no public enrollment or principal-header bypass', async () => {
    const result = await workerExports.default.fetch(request('whoami', {}, { 'X-Everett-User': 'alice', Authorization: 'Bearer synthetic' }));
    expect(result.status).toBe(503);
    expect(await result.json()).toEqual({ error: 'authentication_unconfigured' });
    expect((await workerExports.default.fetch(request('enroll'))).status).toBe(404);
    expect((await workerExports.default.fetch(new Request('https://everett.test/v2/relay/whoami'))).status).toBe(405);
  });
  it('accepts principals only from the private verifier binding and rejects expired tokens', async () => {
    const a = await agent(org());
    let forwarded: Request | undefined;
    const verifier = bindingVerifier({ fetch: async (input: RequestInfo | URL, init?: RequestInit) => {
      forwarded = new Request(input, init); return Response.json(a.p);
    } } as Fetcher);
    expect(await verifier.verify(request('whoami', {}, { Authorization: 'Bearer synthetic', 'X-User': 'forged' }))).toEqual(a.p);
    expect(forwarded?.url).toBe('https://everett-auth.internal/verify-agent');
    expect(forwarded?.headers.get('X-User')).toBeNull();
    await expect(bindingVerifier({ fetch: async () => Response.json({ ...a.p, expiresAt: 0 }) } as unknown as Fetcher).verify(request('whoami', {}, { Authorization: 'Bearer synthetic' }))).rejects.toThrow('invalid_token');
  });
  it('routes verified HTTP calls to actual SQLite DO state and returns denial statuses', async () => {
    const a = await agent(org());
    const verifier = { verify: async () => a.p };
    const ok = await handleRequest(request('whoami'), env, verifier);
    expect(ok.status).toBe(200); expect(await ok.json()).toEqual({ result: { owner: namespace(a.p), deviceId: a.p.agentId, role: 'admin' } });
    const bad = await handleRequest(request('device', { deviceId: 'absent' }), env, verifier);
    expect(bad.status).toBe(403); expect(await bad.json()).toEqual({ error: 'device_unavailable' });
    const oversized = await handleRequest(request('whoami', { text: 'x'.repeat(150001) }), env, verifier);
    expect(oversized.status).toBe(413);
    const expired = await handleRequest(request('whoami'), env, { verify: async () => ({ ...a.p, expiresAt: 0 }) });
    expect(expired.status).toBe(401);
  });
});

describe('Immutable identity and keys', () => {
  it('checks and revokes credentials through the private owner control plane without requiring device keys', async () => {
    const identity: Identity = { issuer: 'https://issuer.example/', subject: 'alice', org: null };
    const stub = env.OWNERS.get(env.OWNERS.idFromName(namespace(identity)));
    const event = crypto.randomUUID(), enrollment = await stub.enroll(identity, 'admin', event);
    expect(enrollment.ok).toBe(true);
    if (!enrollment.ok) throw new Error('enrollment failed');
    const id = enrollment.value.deviceId;
    expect(await stub.credentialActive(identity, id)).toBe(true);
    expect(await stub.credentialActive({ ...identity, subject: 'mallory' }, id)).toBe(false);
    expect(await stub.revokeCredential({ ...identity, subject: 'mallory' }, id)).toBe(false);
    expect(await stub.revokeCredential(identity, 'unknown')).toBe(false);
    expect(await stub.revokeCredential(identity, id)).toBe(true);
    expect(await stub.revokeCredential(identity, id)).toBe(true);
    expect(await stub.credentialActive(identity, id)).toBe(false);
    expect((await stub.enroll(identity, 'admin', event)).ok).toBe(false);
    const next = await stub.enroll(identity, 'admin', crypto.randomUUID());
    expect(next.ok && next.value.deviceId !== id).toBe(true);
  });

  it('generates agent IDs and binds enrollment idempotency, owner and revocation permanently', async () => {
    const space = org(), identity = { issuer: 'https://issuer.example/', subject: 'alice', org: space };
    const stub = env.OWNERS.get(env.OWNERS.idFromName(namespace(identity)));
    const event = crypto.randomUUID(), one = await stub.enroll(identity, 'admin', event);
    if (!one.ok) throw new Error(one.error);
    expect(one.value.deviceId).toMatch(/^agent-[0-9a-f]{32}$/);
    expect(await stub.enroll(identity, 'admin', event)).toEqual(one);
    expect(await stub.enroll({ ...identity, subject: 'bob' }, 'admin', event)).toMatchObject({ ok: false, error: 'Enrollment event binding is immutable' });
    expect(await stub.enroll({ ...identity, org: org() }, 'admin', crypto.randomUUID())).toMatchObject({ ok: false, error: 'Owner namespace is immutable' });
    const p = { ...identity, role: 'admin' as const, agentId: one.value.deviceId, expiresAt: Date.now() + 10000 };
    await call(p, 'register', { device: device(p, 1) });
    await call(p, 'revokeAgent', { deviceId: p.agentId });
    expect(await stub.enroll(identity, 'admin', event)).toMatchObject({ ok: false, error: 'Credential tombstone cannot be reused' });
    const fresh = await stub.enroll(identity, 'admin', crypto.randomUUID());
    expect(fresh.ok && fresh.value.deviceId).not.toBe(one.value.deviceId);
  });
  it('denies forged identity, signature, owner, agent and unsigned key replacement', async () => {
    const a = await agent(org()), b = await agent(a.p.org!, 'bob', 'member', 3);
    await expect(call({ ...b.p, agentId: a.p.agentId }, 'whoami')).rejects.toThrow('credential_or_membership_revoked');
    await expect(call(b.p, 'register', { device: a.d })).rejects.toThrow('agent_mismatch');
    await expect(call(a.p, 'register', { device: { ...a.d, owner: namespace(b.p) + 'x' } })).rejects.toThrow('wrong_owner');
    await expect(call(a.p, 'register', { device: { ...a.d, signature: '00'.repeat(64) } })).rejects.toThrow('invalid_signature');
    await expect(call(a.p, 'register', { device: device(a.p, 11) })).rejects.toThrow('signed_rotation_required');
    await expect(call({ ...b.p, role: 'admin' }, 'revokeAgent', { deviceId: a.p.agentId })).rejects.toThrow('admin_required');
  });
  it('isolates owners and makes membership revocation terminal', async () => {
    const a = await agent(org()), other = await agent(org(), 'other', 'admin', 5), b = await agent(a.p.org!, 'bob', 'member', 3);
    await expect(call(other.p, 'device', { deviceId: a.p.agentId })).rejects.toThrow('device_unavailable');
    await call(a.p, 'revokeMember', { subject: b.p.subject });
    await expect(call(b.p, 'whoami')).rejects.toThrow('credential_or_membership_revoked');
    expect(await b.stub.enroll({ issuer: b.p.issuer, subject: b.p.subject, org: b.p.org }, 'member', crypto.randomUUID())).toMatchObject({ ok: false, error: 'Membership tombstone cannot be reused' });
    expect(await call(a.p, 'whoami')).toMatchObject({ deviceId: a.p.agentId });
  });
});

it('matches the existing public v2 canonical bytes, fingerprints, signatures and encrypted plaintext', () => {
  const e = fixture.envelope as Envelope;
  expect(new TextDecoder().decode(deviceBytes(fixture.device))).toBe(fixture.deviceSignedUtf8);
  expect(fingerprint(fixture.device)).toBe(fixture.fingerprint);
  expect(new TextDecoder().decode(aadBytes(e))).toBe(fixture.aadUtf8);
  expect(new TextDecoder().decode(envelopeBytes(e))).toBe(fixture.envelopeSignedUtf8);
  validateDevice(fixture.device); validateDevice(fixture.recipient);
  verify(fixture.device.signingKey, e.signature, envelopeBytes(e));
  const observation = { ...fixture.observation, state: 'delivered' as const };
  expect(new TextDecoder().decode(observationBytes(observation.owner, observation))).toBe(fixture.observationSignedUtf8);
  verify(fixture.recipient.signingKey, observation.signature, observationBytes(observation.owner, observation));
  expect(new TextDecoder().decode(rotationBytes(fixture.newDevice, fixture.rotation.oldFingerprint))).toBe(fixture.rotationSignedUtf8);
  verify(fixture.device.signingKey, fixture.rotation.signature, rotationBytes(fixture.newDevice, fixture.rotation.oldFingerprint));
  const shared = x25519.getSharedSecret(secret(fixture.testOnlySeeds.recipientEncryption), bytes(e.ephemeralKey));
  const key = hkdf(sha256, shared, encoder.encode('everett-relay-v2'), aadBytes(e), 32);
  expect(new TextDecoder().decode(chacha20poly1305(key, bytes(e.nonce), aadBytes(e)).decrypt(bytes(e.ciphertext)))).toBe(fixture.plaintextUtf8);
});

describe('Encrypted queue and observations', () => {
  it('stores encrypted bytes, reads without consuming, requires signed delivery, and keeps monotonic status', async () => {
    const a = await agent(org()), b = await agent(a.p.org!, 'bob', 'member', 3), e = envelope(a, b);
    expect(await call(a.p, 'enqueue', { envelope: e })).toEqual({ messageId: e.messageId, state: 'queued' });
    for (let i = 0; i < 2; i++) expect((await call(b.p, 'pending', { deviceId: b.p.agentId })).envelopes).toEqual([{ envelope: e, trust: 'same-org' }]);
    const shared = x25519.getSharedSecret(secret(b.n + 1), bytes(e.ephemeralKey));
    const key = hkdf(sha256, shared, encoder.encode('everett-relay-v2'), aadBytes(e), 32);
    expect(JSON.parse(new TextDecoder().decode(chacha20poly1305(key, bytes(e.nonce), aadBytes(e)).decrypt(bytes(e.ciphertext))))).toMatchObject({ text: 'synthetic worker relay' });
    await expect(call(b.p, 'observe', observe(b, e, 'claimed'))).rejects.toThrow('delivery_required');
    await expect(call(b.p, 'observe', { ...observe(b, e), signature: '00'.repeat(64) })).rejects.toThrow('invalid_signature');
    await call(b.p, 'observe', observe(b, e));
    expect((await call(b.p, 'pending', { deviceId: b.p.agentId })).envelopes).toEqual([]);
    await call(b.p, 'observe', observe(b, e, 'replied'));
    await call(b.p, 'observe', observe(b, e, 'claimed'));
    expect(await call(a.p, 'status', { fromDevice: a.p.agentId, messageId: e.messageId })).toEqual({ state: 'replied' });
    expect(await call(a.p, 'enqueue', { envelope: e })).toEqual({ messageId: e.messageId, state: 'replied' });
  });
  it('serializes duplicate requests and rejects changed idempotency payloads and reused message IDs', async () => {
    const a = await agent(org()), b = await agent(a.p.org!, 'bob', 'member', 3), e = envelope(a, b);
    await Promise.all(Array.from({ length: 5 }, () => call(a.p, 'enqueue', { envelope: e })));
    expect((await call(b.p, 'pending', { deviceId: b.p.agentId })).envelopes).toHaveLength(1);
    const changed = envelope(a, b, { messageId: e.messageId, idempotencyKey: e.idempotencyKey, nonce: '22'.repeat(12) });
    await expect(call(a.p, 'enqueue', { envelope: changed })).rejects.toThrow('idempotency_conflict');
    const reused = envelope(a, b, { messageId: e.messageId });
    await expect(call(a.p, 'enqueue', { envelope: reused })).rejects.toThrow('message_id_reused');
  });
  it('denies cross-owner routing, metadata tampering, impersonation and invalid TTL/hop lineage', async () => {
    const a = await agent(org()), b = await agent(a.p.org!, 'bob', 'member', 3), other = await agent(org(), 'other', 'admin', 5);
    await expect(call(a.p, 'enqueue', { envelope: envelope(a, other) })).rejects.toThrow('cross_owner_not_supported');
    const e = envelope(a, b);
    await expect(call(a.p, 'enqueue', { envelope: { ...e, nonce: 'ff'.repeat(12) } })).rejects.toThrow('invalid_signature');
    await expect(call(b.p, 'enqueue', { envelope: e })).rejects.toThrow('agent_mismatch');
    await expect(call(a.p, 'enqueue', { envelope: envelope(a, b, { createdAt: Date.now() - 300001, expiresAt: Date.now() + 1000 }) })).rejects.toThrow('invalid_ttl');
    await expect(call(a.p, 'enqueue', { envelope: envelope(a, b, { hop: 2 }) })).rejects.toThrow('invalid_root_hop');
    const forward = envelope(b, a, { parent: { owner: e.owner, messageId: e.messageId }, hop: 2 });
    await call(a.p, 'enqueue', { envelope: e });
    await expect(call(b.p, 'enqueue', { envelope: forward })).rejects.toThrow('invalid_parent_lineage');
    await call(b.p, 'observe', observe(b, e));
    expect(await call(b.p, 'enqueue', { envelope: forward })).toMatchObject({ state: 'queued' });
  });
  it('checks revocation on pending and retries, without affecting another owner agent', async () => {
    const a = await agent(org()), b = await agent(a.p.org!, 'bob', 'member', 3), e = envelope(a, b);
    await call(a.p, 'enqueue', { envelope: e });
    await call(a.p, 'revokeAgent', { deviceId: b.p.agentId });
    await expect(call(b.p, 'pending', { deviceId: b.p.agentId })).rejects.toThrow('credential_or_membership_revoked');
    await expect(call(a.p, 'enqueue', { envelope: e })).rejects.toThrow('device_unavailable');
    expect(await call(a.p, 'whoami')).toMatchObject({ deviceId: a.p.agentId });
  });
  it('requires old-key rotation signatures and blocks old queued envelopes immediately', async () => {
    const a = await agent(org()), b = await agent(a.p.org!, 'bob', 'member', 3), e = envelope(a, b), d = device(b.p, 11);
    await call(a.p, 'enqueue', { envelope: e });
    const args = { device: d, oldFingerprint: fingerprint(b.d), signature: sign(rotationBytes(d, fingerprint(b.d)), 11) };
    await expect(call(b.p, 'rotate', args)).rejects.toThrow('invalid_signature');
    args.signature = sign(rotationBytes(d, fingerprint(b.d)), b.n);
    expect(await call(b.p, 'rotate', args)).toEqual({ fingerprint: fingerprint(d) });
    expect((await call(b.p, 'pending', { deviceId: b.p.agentId })).envelopes).toEqual([]);
    await expect(call(a.p, 'enqueue', { envelope: e })).rejects.toThrow('device_key_changed');
  });
  it('expires ciphertext while retaining permanent replay metadata and allows exact expired retry', async () => {
    const a = await agent(org()), b = await agent(a.p.org!, 'bob', 'member', 3), now = Date.now();
    const e = envelope(a, b, { createdAt: now, expiresAt: now + 100 });
    await call(a.p, 'enqueue', { envelope: e });
    await new Promise(resolve => setTimeout(resolve, 150));
    expect((await call(b.p, 'pending', { deviceId: b.p.agentId })).envelopes).toEqual([]);
    await runDurableObjectAlarm(a.stub);
    expect(await call(a.p, 'enqueue', { envelope: e })).toEqual({ messageId: e.messageId, state: 'expired' });
    const stored = await runInDurableObject(a.stub, (_instance, state) => ({
      attempts: state.storage.sql.exec<{ n: number }>('SELECT count(*) n FROM attempts').one().n,
      messages: state.storage.sql.exec<{ n: number }>('SELECT count(*) n FROM messages').one().n,
    }));
    expect(stored).toEqual({ attempts: 1, messages: 0 });
  });
  it('pages over blocked rows so later routable messages cannot starve', async () => {
    const a = await agent(org()), b = await agent(a.p.org!, 'bob', 'member', 3), c = await agent(a.p.org!, 'carol', 'member', 5);
    for (let i = 0; i < 32; i++) await call(a.p, 'enqueue', { envelope: envelope(a, b) });
    const last = envelope(c, b); await call(c.p, 'enqueue', { envelope: last });
    await call(a.p, 'revokeAgent', { deviceId: a.p.agentId });
    const first = await call(b.p, 'pending', { deviceId: b.p.agentId });
    expect(first.envelopes).toEqual([]); expect(first.isDone).toBe(false);
    const second = await call(b.p, 'pending', { deviceId: b.p.agentId, cursor: first.continueCursor });
    expect(second.isDone).toBe(true); expect(second.envelopes[0].envelope.messageId).toBe(last.messageId);
  });
});

it('shares account rate budgets across agents and rolls back partially updated counters', async () => {
  vi.spyOn(Date, 'now').mockReturnValue(Date.now());
  const a = await agent(org()), a2 = await agent(a.p.org!, 'alice', 'admin', 7), b = await agent(a.p.org!, 'bob', 'member', 3);
  for (let i = 0; i < 120; i++) await call(a.p, 'enqueue', { envelope: envelope(a, b) });
  await expect(call(a2.p, 'enqueue', { envelope: envelope(a2, b) })).rejects.toThrow('rate_limit');
  const counter = await runInDurableObject(a.stub, (_instance, state) => state.storage.sql.exec<{ scope: string }>('SELECT scope FROM rates WHERE scope=?', JSON.stringify(['agent', a2.p.agentId])).toArray());
  expect(counter).toEqual([]);
});

it('caps recipient ciphertext at 256 and frees capacity only after signed delivery', async () => {
  vi.spyOn(Date, 'now').mockReturnValue(Date.now());
  const a = await agent(org()), b = await agent(a.p.org!, 'bob', 'member', 3), c = await agent(a.p.org!, 'carol', 'member', 5), r = await agent(a.p.org!, 'recipient', 'member', 7);
  const first = envelope(a, r); await call(a.p, 'enqueue', { envelope: first });
  for (let i = 1; i < 120; i++) await call(a.p, 'enqueue', { envelope: envelope(a, r) });
  for (let i = 0; i < 120; i++) await call(b.p, 'enqueue', { envelope: envelope(b, r) });
  for (let i = 0; i < 16; i++) await call(c.p, 'enqueue', { envelope: envelope(c, r) });
  await expect(call(c.p, 'enqueue', { envelope: envelope(c, r) })).rejects.toThrow('recipient_queue_full');
  expect(await call(a.p, 'enqueue', { envelope: first })).toMatchObject({ state: 'queued' });
  const page = await call(r.p, 'pending', { deviceId: r.p.agentId });
  expect(page.envelopes).toHaveLength(32); expect(page.isDone).toBe(false);
  await call(r.p, 'observe', observe(r, first));
  expect(await call(c.p, 'enqueue', { envelope: envelope(c, r) })).toMatchObject({ state: 'queued' });
});

it('recovers queue, identity, replay and revocation from SQLite after object eviction', async () => {
  const a = await agent(org()), b = await agent(a.p.org!, 'bob', 'member', 3), e = envelope(a, b);
  await call(a.p, 'enqueue', { envelope: e });
  await evictDurableObject(a.stub);
  expect((await call(b.p, 'pending', { deviceId: b.p.agentId })).envelopes[0].envelope).toEqual(e);
  expect(await call(a.p, 'enqueue', { envelope: e })).toMatchObject({ state: 'queued' });
  await call(a.p, 'revokeAgent', { deviceId: b.p.agentId });
  await evictDurableObject(a.stub);
  await expect(call(b.p, 'whoami')).rejects.toThrow('credential_or_membership_revoked');
  expect(await call(a.p, 'whoami')).toMatchObject({ deviceId: a.p.agentId });
});
