import { env as bindings, exports as workerExports } from 'cloudflare:workers';
import { runDurableObjectAlarm, runInDurableObject, evictDurableObject, reset } from 'cloudflare:test';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ed25519, x25519 } from '@noble/curves/ed25519';
import { hkdf } from '@noble/hashes/hkdf';
import { sha256 } from '@noble/hashes/sha256';
import { chacha20poly1305 } from '@noble/ciphers/chacha';
import { handleRequest, type Env } from '../src/index';
import { bindingVerifier } from '../src/auth';
import { boundedText } from '../src/http-body';
import {
  namespace, deviceBytes, fingerprint, envelopeBytes, aadBytes, observationBytes,
  rotationBytes, encode, hexString, bytes, verify, validateDevice,
  threadAadBytes, threadCopyBytes, threadPairBytes, threadContentCommitment,
  type ThreadMember, type ThreadPost,
  type Device, type Envelope, type Principal, type Identity, type Observation, type Operation,
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
  const response = await stub.relay(p, op, args);
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
function threadPost(
  author: Awaited<ReturnType<typeof agent>>,
  members: [ThreadMember, ThreadMember],
  threadRef: { creatorOwner: string; threadId: string },
  idempotencyKey: string,
  text: string,
): ThreadPost {
  const blindingSecret = crypto.getRandomValues(new Uint8Array(32));
  const context = {
    creatorOwner: threadRef.creatorOwner,
    threadId: threadRef.threadId,
    messageId: crypto.randomUUID(),
    idempotencyKey,
    authorOwner: namespace(author.p),
    authorDeviceId: author.p.agentId,
    authorFingerprint: fingerprint(author.d),
    commitment: threadContentCommitment(blindingSecret, text),
  };
  const textBytes = encoder.encode(text);
  const length = new Uint8Array(4);
  new DataView(length.buffer).setUint32(0, textBytes.length, false);
  const recordPrefix = encoder.encode('everett-thread-record-v1\0');
  const payloadPrefix = encoder.encode('everett-thread-payload-v1\0');
  const plaintext = new Uint8Array(recordPrefix.length + blindingSecret.length + payloadPrefix.length + length.length + textBytes.length);
  let offset = 0;
  for (const part of [recordPrefix, blindingSecret, payloadPrefix, length, textBytes]) {
    plaintext.set(part, offset);
    offset += part.length;
  }
  const copies = members.map(recipient => {
    const ephemeralSecret = crypto.getRandomValues(new Uint8Array(32));
    const ephemeralKey = hexString(x25519.getPublicKey(ephemeralSecret));
    const aad = threadAadBytes(context, recipient);
    const shared = x25519.getSharedSecret(ephemeralSecret, bytes(recipient.device.encryptionKey));
    const key = hkdf(sha256, shared, encoder.encode('everett-thread-copy-v1'), aad, 32);
    const nonce = crypto.getRandomValues(new Uint8Array(12));
    const copy = {
      recipient, ephemeralKey, nonce: hexString(nonce),
      ciphertext: hexString(chacha20poly1305(key, nonce, aad).encrypt(plaintext)),
      signature: '',
    };
    copy.signature = sign(threadCopyBytes(context, copy), author.n);
    return copy;
  }) as unknown as ThreadPost['copies'];
  return { context, copies, pairSignature: sign(threadPairBytes(context, copies), author.n) };
}
function openThreadCopy(
  recipient: Awaited<ReturnType<typeof agent>>,
  context: ThreadPost['context'],
  copy: ThreadPost['copies'][number],
) {
  const aad = threadAadBytes(context, copy.recipient);
  const shared = x25519.getSharedSecret(secret(recipient.n + 1), bytes(copy.ephemeralKey));
  const key = hkdf(sha256, shared, encoder.encode('everett-thread-copy-v1'), aad, 32);
  const plaintext = chacha20poly1305(key, bytes(copy.nonce), aad).decrypt(bytes(copy.ciphertext));
  const recordPrefix = encoder.encode('everett-thread-record-v1\0');
  const payloadPrefix = encoder.encode('everett-thread-payload-v1\0');
  expect(Array.from(plaintext.slice(0, recordPrefix.length))).toEqual(Array.from(recordPrefix));
  const secretStart = recordPrefix.length;
  const blindingSecret = plaintext.slice(secretStart, secretStart + 32);
  const payloadStart = secretStart + 32;
  expect(Array.from(plaintext.slice(payloadStart, payloadStart + payloadPrefix.length))).toEqual(Array.from(payloadPrefix));
  const lengthOffset = payloadStart + payloadPrefix.length;
  const textLength = new DataView(plaintext.buffer, plaintext.byteOffset + lengthOffset, 4).getUint32(0, false);
  const text = new TextDecoder().decode(plaintext.slice(lengthOffset + 4));
  expect(textLength).toBe(encoder.encode(text).length);
  expect(threadContentCommitment(blindingSecret, text)).toBe(context.commitment);
  return text;
}
function observe(b: Awaited<ReturnType<typeof agent>>, e: Envelope, state: Observation['state'] = 'delivered') {
  const o: Observation = { deviceId: b.p.agentId, senderOwner: e.owner, messageId: e.messageId, state, at: Date.now(), signature: '' };
  o.signature = sign(observationBytes(namespace(b.p), o), b.n); return o;
}
const request = (op: string, body: unknown = {}, headers: Record<string, string> = {}) => new Request(`https://everett.test/v2/relay/${op}`, { method: 'POST', headers: { 'Content-Type': 'application/json', ...headers }, body: JSON.stringify(body) });
const notificationClients: WebSocket[] = [];
afterEach(async () => {
  await Promise.all(notificationClients.splice(0).map(socket => new Promise<void>(resolve => {
    if (socket.readyState === WebSocket.CLOSED) return resolve();
    socket.addEventListener('close', () => resolve(), { once: true });
    socket.close();
  })));
  vi.restoreAllMocks(); await reset();
});

async function watch(p: Principal, extra: Record<string, string> = {}) {
  const response = await handleRequest(new Request('https://everett.test/v2/relay/watch', {
    headers: { Upgrade: 'websocket', ...extra },
  }), env, { verify: async () => p });
  expect(response.status).toBe(101);
  const socket = response.webSocket!;
  const frames: string[] = [];
  const closed: number[] = [];
  socket.addEventListener('message', event => { frames.push(String(event.data)); });
  socket.addEventListener('close', event => { closed.push(event.code); });
  socket.accept(); notificationClients.push(socket);
  await vi.waitFor(() => expect(frames).toEqual(['{"type":"inbox_changed"}']));
  return { socket, frames, closed };
}

describe('hibernating receiver notifications', () => {
  it('keeps the attachment bounded for long verified identities', async () => {
    const identity: Identity = { issuer: `https://issuer.example/${'x'.repeat(1800)}`, subject: '界'.repeat(256), org: org() };
    const stub = env.OWNERS.get(env.OWNERS.idFromName(namespace(identity)));
    const result = await stub.enroll(identity, 'admin', crypto.randomUUID());
    if (!result.ok) throw new Error(result.error);
    const p: Principal = { ...identity, agentId: result.value.deviceId, role: 'admin', expiresAt: Date.now() + 60000 };
    const d = device(p, 41);
    await call(p, 'register', { device: d });
    await watch(p);
    const attachment = await runInDurableObject(stub, async (_, state) => state.getWebSockets(p.agentId)[0].deserializeAttachment());
    expect(attachment).toEqual({ agentId: p.agentId, expiresAt: p.expiresAt, fingerprint: fingerprint(d) });
    expect(encoder.encode(JSON.stringify(attachment)).length).toBeLessThan(512);
  });

  it('closes sockets on membership revocation and signed device rotation', async () => {
    const space = org(), a = await agent(space), b = await agent(space, 'bob', 'member', 3);
    const receiver = await watch(b.p);
    const replacement = device(b.p, 11);
    await call(b.p, 'rotate', { device: replacement, oldFingerprint: fingerprint(b.d), signature: sign(rotationBytes(replacement, fingerprint(b.d)), b.n) });
    await vi.waitFor(() => expect(receiver.closed).toEqual([1008]));
    const reconnected = await watch(b.p);
    await call(a.p, 'revokeMember', { subject: b.p.subject });
    await vi.waitFor(() => expect(reconnected.closed).toEqual([1008]));
  });

  it('sends only a fixed recipient hint after durable enqueue, with no idle alarm', async () => {
    const space = org(), a = await agent(space), b = await agent(space, 'bob', 'member', 3), outsider = await agent(org());
    const sender = await watch(a.p), receiver = await watch(b.p), other = await watch(outsider.p);
    const message = envelope(a, b);
    await expect(call(a.p, 'enqueue', { envelope: { ...message, signature: '00'.repeat(64) } })).rejects.toThrow('invalid_signature');
    expect(receiver.frames).toHaveLength(1);
    expect(await call(a.p, 'enqueue', { envelope: message })).toMatchObject({ state: 'queued' });
    await vi.waitFor(() => expect(receiver.frames).toHaveLength(2));
    expect(receiver.frames).toEqual(['{"type":"inbox_changed"}', '{"type":"inbox_changed"}']);
    expect(sender.frames).toHaveLength(1); expect(other.frames).toHaveLength(1);
    expect((await call(b.p, 'pending', { deviceId: b.p.agentId })).envelopes[0].envelope).toEqual(message);
    expect(receiver.frames.join('')).not.toContain(message.ciphertext);
    expect(receiver.frames.join('')).not.toContain(message.messageId);
    const idle = await agent(org()); await watch(idle.p);
    expect(await runInDurableObject(idle.stub, async (_, state) => state.storage.getAlarm())).toBeNull();
  });

  it('restores hibernating socket attachments and recovers queued work on reconnect', async () => {
    const space = org(), a = await agent(space), b = await agent(space, 'bob', 'member', 3);
    const receiver = await watch(b.p);
    await evictDurableObject(b.stub);
    const message = envelope(a, b);
    await call(a.p, 'enqueue', { envelope: message });
    await vi.waitFor(() => expect(receiver.frames).toHaveLength(2));
    receiver.socket.close();
    await vi.waitFor(() => expect(receiver.closed).toHaveLength(1));
    const reconnected = await watch(b.p);
    expect(reconnected.frames).toHaveLength(1);
    expect((await call(b.p, 'pending', { deviceId: b.p.agentId })).envelopes).toHaveLength(1);
  });

  it('closes revoked receivers immediately and refuses their next subscription', async () => {
    const space = org(), a = await agent(space), b = await agent(space, 'bob', 'member', 3);
    const receiver = await watch(b.p);
    await call(a.p, 'revokeAgent', { deviceId: b.p.agentId });
    await vi.waitFor(() => expect(receiver.closed).toEqual([1008]));
    const denied = await handleRequest(new Request('https://everett.test/v2/relay/watch', { headers: { Upgrade: 'websocket' } }), env, { verify: async () => b.p });
    expect(denied.status).toBe(403);
  });

  it('reauthorizes attachments before hints and never forwards expired socket notifications', async () => {
    const space = org(), a = await agent(space), b = await agent(space, 'bob', 'member', 3);
    const receiver = await watch({ ...b.p, expiresAt: Date.now() + 1200 });
    await new Promise(resolve => setTimeout(resolve, 1250));
    await call(a.p, 'enqueue', { envelope: envelope(a, b) });
    await vi.waitFor(() => expect(receiver.closed).toEqual([1008]));
    expect(receiver.frames).toHaveLength(1);
    expect((await call(b.p, 'pending', { deviceId: b.p.agentId })).envelopes).toHaveLength(1);
  });

  it('ignores client identity headers and refuses application messages on notification sockets', async () => {
    const a = await agent(org()), other = await agent(org());
    const receiver = await watch(a.p, { 'X-Everett-Principal': JSON.stringify(other.p) });
    expect(await runInDurableObject(a.stub, async (_, state) => state.getWebSockets(a.p.agentId).length)).toBe(1);
    receiver.socket.send('{"type":"send","text":"untrusted"}');
    await vi.waitFor(() => expect(receiver.closed).toEqual([1008]));
    expect((await call(a.p, 'pending', { deviceId: a.p.agentId })).envelopes).toEqual([]);
  });

  it('bounds connections per credential and rejects invalid upgrade and authentication', async () => {
    const a = await agent(org()); await watch(a.p); await watch(a.p);
    const limited = await handleRequest(new Request('https://everett.test/v2/relay/watch', { headers: { Upgrade: 'websocket' } }), env, { verify: async () => a.p });
    expect(limited.status).toBe(429);
    const plain = await handleRequest(new Request('https://everett.test/v2/relay/watch'), env, { verify: async () => a.p });
    expect(plain.status).toBe(426);
    const expired = await handleRequest(new Request('https://everett.test/v2/relay/watch', { headers: { Upgrade: 'websocket' } }), env, { verify: async () => ({ ...a.p, expiresAt: 1 }) });
    expect(expired.status).toBe(401);
    const denied = await handleRequest(new Request('https://everett.test/v2/relay/watch', { headers: { Upgrade: 'websocket' } }), env);
    expect(denied.status).toBe(503);
  });
});

describe('authenticated author/session directory', () => {
  it('exposes only opt-in cards with server-accounted authors, without pairing', async () => {
    const space = org(), a = await agent(space), b = await agent(space, 'bob', 'member', 3), other = await agent(org());
    const card = { session: 'session-b', harness: 'codex', card: 'Reviewing messaging' };
    expect((await call(a.p, 'sessionDirectory')).sessions).toEqual([]);
    await call(b.p, 'exposeSession', card);
    const list = await call(a.p, 'sessionDirectory');
    expect(list.sessions).toEqual([expect.objectContaining({ author: 'bob', agent: b.p.agentId, ...card,
      owner: namespace(a.p), target: `id:${b.p.agentId}/codex/session-b` })]);
    expect((await call(other.p, 'sessionDirectory')).sessions).toEqual([]);
    await expect(call(b.p, 'exposeSession', { ...card, author: 'alice' })).rejects.toThrow('invalid_request');
    await call(a.p, 'exposeSession', { ...card, remove: true });
    expect((await call(a.p, 'sessionDirectory')).sessions).toHaveLength(1);
    await call(b.p, 'exposeSession', { ...card, card: 'updated' });
    expect((await call(a.p, 'sessionDirectory')).sessions[0].card).toBe('updated');
    await call(b.p, 'exposeSession', { ...card, remove: true });
    expect((await call(a.p, 'sessionDirectory')).sessions).toEqual([]);
  });
  it('keeps a published session bound to its harness until explicitly withdrawn', async () => {
    const a = await agent(org());
    const card = { session: 'shared-id', harness: 'hermes', card: 'original' };
    await call(a.p, 'exposeSession', card);
    const collision = { ...card, harness: 'codex' };
    await expect(call(a.p, 'exposeSession', collision)).rejects.toThrow('session_harness_conflict');
    await expect(call(a.p, 'exposeSession', { ...collision, remove: true })).rejects.toThrow('session_harness_conflict');
    expect((await call(a.p, 'sessionDirectory')).sessions).toEqual([expect.objectContaining(card)]);
    // Existing CLI clients use "unknown" when --remove has no --harness.
    await call(a.p, 'exposeSession', { ...card, harness: 'unknown', remove: true });
    await call(a.p, 'exposeSession', collision);
    expect((await call(a.p, 'sessionDirectory')).sessions).toEqual([expect.objectContaining(collision)]);
  });

  it('only advertises harness and session components accepted by the recipient parser', async () => {
    const a = await agent(org());
    const card = { session: 'x'.repeat(128), harness: 'hermes', card: '' };
    for (const field of ['harness', 'session']) {
      for (const value of ['.', '..', 'x'.repeat(129)]) {
        await expect(call(a.p, 'exposeSession', { ...card, [field]: value })).rejects.toThrow('invalid_request');
      }
    }
    await runInDurableObject(a.stub, (_, state) => {
      state.storage.sql.exec('INSERT INTO session_cards(agent_id,session,harness,card,updated_at) VALUES(?,?,?,?,?)',
        a.p.agentId, 'legacy', '..', 'old exposure', 1);
    });
    await call(a.p, 'exposeSession', card);
    expect((await call(a.p, 'sessionDirectory')).sessions.map((row: {target: string}) => row.target))
      .toEqual([`id:${a.p.agentId}/hermes/${card.session}`]);
  });

  it('paginates bounded cards and filters revoked authors and callers', async () => {
    const space = org(), a = await agent(space), b = await agent(space, 'bob', 'member', 3);
    for (let i = 0; i < 32; i++) await call(b.p, 'exposeSession', { session: `s-${i}`, harness: 'codex', card: '' });
    await expect(call(b.p, 'exposeSession', { session: 'overflow', harness: 'codex', card: '' })).rejects.toThrow('session_limit');
    await expect(call(b.p, 'exposeSession', { session: 's-0', harness: 'codex', card: 'x'.repeat(641) })).rejects.toThrow('invalid_request');
    await call(a.p, 'exposeSession', { session: 'admin', harness: 'claude', card: '' });
    const first = await call(a.p, 'sessionDirectory');
    expect(first.sessions).toHaveLength(32); expect(first.isDone).toBe(false);
    const second = await call(a.p, 'sessionDirectory', { cursor: first.continueCursor });
    expect(second.sessions).toHaveLength(1); expect(second.isDone).toBe(true);
    await call(a.p, 'revokeAgent', { deviceId: b.p.agentId });
    expect((await call(a.p, 'sessionDirectory')).sessions).toEqual([]);
    expect((await call(a.p, 'sessionDirectory', { cursor: first.continueCursor })).sessions).toHaveLength(1);
    await expect(call(b.p, 'sessionDirectory')).rejects.toThrow('revoked');
    await expect(call(b.p, 'exposeSession', { session: 'revoked', harness: 'codex', card: '' })).rejects.toThrow('revoked');
  });
});

describe('Workers boundary', () => {
  it.each(['/enroll', '/enroll/revoke', '/auth/github', '/v2/auth/poll', '/oauth/token', '/.well-known/oauth-authorization-server'])('keeps auth route %s behind configured authentication', async path => {
    const response = await handleRequest(new Request(`https://everett.test${path}`), env);
    expect(response.status).toBe(503);
    expect(await response.json()).toEqual({ error: 'authentication_unconfigured' });
  });
  it.each(['/enrollment', '/oauth', '/oauth-other/token'])('does not treat %s as an auth route', async path => {
    expect((await handleRequest(new Request(`https://everett.test${path}`), env)).status).toBe(404);
  });
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

describe('Bounded HTTP text', () => {
  it('decodes split UTF-8 while counting bytes and cancels oversized streams', async () => {
    const encoded = encoder.encode('😀');
    const body = new ReadableStream<Uint8Array>({ start(controller) {
      controller.enqueue(encoded.slice(0, 2)); controller.enqueue(encoded.slice(2)); controller.close();
    } });
    expect(await boundedText(body, 4)).toBe('😀');
    expect(body.locked).toBe(false);
    let cancelled = false;
    const oversized = new ReadableStream<Uint8Array>({
      start(controller) { controller.enqueue(encoded); },
      cancel() { cancelled = true; },
    });
    await expect(boundedText(oversized, 3)).rejects.toMatchObject({ code: 'body_too_large', status: 413 });
    expect(cancelled).toBe(true);
    expect(oversized.locked).toBe(false);
  });
  it('rejects missing bodies and incomplete UTF-8 rather than replacing it', async () => {
    await expect(boundedText(null, 4)).rejects.toMatchObject({ code: 'missing_body', status: 400 });
    const body = new ReadableStream<Uint8Array>({ start(controller) {
      controller.enqueue(new Uint8Array([0xf0, 0x9f])); controller.close();
    } });
    await expect(boundedText(body, 4)).rejects.toBeInstanceOf(TypeError);
    expect(body.locked).toBe(false);
  });
  it('times out and releases a stalled body without waiting for cancellation', async () => {
    let cancelled = false;
    const body = new ReadableStream<Uint8Array>({ cancel() {
      cancelled = true;
      return new Promise<void>(() => {});
    } });
    await expect(boundedText(body, 4, 1)).rejects.toMatchObject({ code: 'request_timeout', status: 408 });
    expect(cancelled).toBe(true);
    expect(body.locked).toBe(false);
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
    const status = () => call(a.p, 'status', { fromDevice: a.p.agentId, messageId: e.messageId });
    expect(await status()).toEqual({ state: 'delivered', queuedAt: expect.any(Number), claimedAt: null, repliedAt: null });
    const reply = observe(b, e, 'replied');
    await call(b.p, 'observe', reply);
    expect(await status()).toEqual({ state: 'replied', queuedAt: expect.any(Number), claimedAt: null, repliedAt: reply.at });
    const claim = observe(b, e, 'claimed');
    await call(b.p, 'observe', claim);
    await call(b.p, 'observe', observe(b, e, 'claimed'));
    expect(await status()).toEqual({ state: 'replied', queuedAt: expect.any(Number), claimedAt: claim.at, repliedAt: reply.at });
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

it('bounds a stalled identity probe without claiming a rejected operation', async () => {
  vi.useFakeTimers();
  try {
    const response = handleRequest(new Request('https://relay.test/v2/relay/whoami', {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}',
    }), env, { verify: () => new Promise(() => {}) });
    await vi.advanceTimersByTimeAsync(10000);
    const out = await response;
    expect(out.status).toBe(504);
    expect(out.headers.get('Cache-Control')).toBe('no-store');
    expect(await out.json()).toEqual({ error: 'request_timeout', outcome: 'unconfirmed', retry: 'reconcile_before_retry' });
  } finally { vi.useRealTimers(); }
});

describe('Cross-org named inbox and grant allowlist', () => {
  function decryptEnvelope(recipientKeySeed: number, e: Envelope) {
    const shared = x25519.getSharedSecret(secret(recipientKeySeed + 1), bytes(e.ephemeralKey));
    const key = hkdf(sha256, shared, encoder.encode('everett-relay-v2'), aadBytes(e), 32);
    return JSON.parse(new TextDecoder().decode(chacha20poly1305(key, bytes(e.nonce), aadBytes(e)).decrypt(bytes(e.ciphertext))));
  }

  function encryptedEnvelope(
    sender: Awaited<ReturnType<typeof agent>>,
    recipientDevice: Device,
    payload: unknown,
    overrides: Partial<Envelope> = {},
  ): Envelope {
    const now = Date.now();
    const e: Envelope = {
      owner: namespace(sender.p), recipientOwner: recipientDevice.owner, grantId: null,
      fromDevice: sender.p.agentId, toDevice: recipientDevice.deviceId,
      messageId: crypto.randomUUID(), idempotencyKey: crypto.randomUUID(), parent: null,
      hop: 1, createdAt: now, expiresAt: now + 60000,
      senderFingerprint: fingerprint(sender.d), recipientFingerprint: fingerprint(recipientDevice),
      ephemeralKey: hexString(x25519.getPublicKey(secret(101))), nonce: '11'.repeat(12), ciphertext: '', signature: '',
      ...overrides,
    };
    const shared = x25519.getSharedSecret(secret(101), bytes(recipientDevice.encryptionKey));
    const key = hkdf(sha256, shared, encoder.encode('everett-relay-v2'), aadBytes(e), 32);
    e.ciphertext = hexString(chacha20poly1305(key, bytes(e.nonce), aadBytes(e)).encrypt(encode(payload)));
    e.signature = sign(envelopeBytes(e), sender.n);
    return e;
  }

  it('delivers a private cross-org send-and-reply by provisional address between two accounts', async () => {
    const alice = await agent(org(), 'alice', 'admin', 1);
    const bob = await agent(org(), 'bob', 'admin', 5);
    const address = 'claude@wmm.10462';

    const registered = await call(bob.p, 'registerInbox', { address, harness: 'claude', session: 'inbox-b' });
    expect(registered).toEqual({
      address, owner: namespace(bob.p), deviceId: bob.p.agentId,
      harness: 'claude', session: 'inbox-b', status: 'active',
    });

    const grant = await call(bob.p, 'grantInbox', {
      address, senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId,
    });
    expect(grant).toMatchObject({
      grantId: expect.stringMatching(/^grant-[0-9a-f]{32}$/),
      address, owner: namespace(bob.p), deviceId: bob.p.agentId,
      harness: 'claude', session: 'inbox-b',
      senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId,
      status: 'active',
    });

    // Allowed sender resolves the private address over HTTP and receives the recipient bundle + exact inbox + grant.
    const resolveRes = await handleRequest(
      request('resolveInbox', { address }),
      env,
      { verify: async () => alice.p },
    );
    expect(resolveRes.status).toBe(200);
    const { result: resolved } = await resolveRes.json() as { result: { address: string; owner: string; device: Device; harness: string; session: string; grantId: string } };
    expect(resolved).toEqual({
      address, owner: namespace(bob.p), device: bob.d,
      harness: 'claude', session: 'inbox-b', grantId: grant.grantId,
    });
    validateDevice(resolved.device);

    const bobWatch = await watch(bob.p);
    const aliceWatch = await watch(alice.p);

    const reqPayload = {
      kind: 'inbox', session: resolved.session, harness: resolved.harness,
      text: 'Hello across orgs', from: 'inbox-a', from_harness: 'codex',
      from_card: 'Alice session', reply_to: '', hops: 1, external_untrusted: true,
    };
    const req = encryptedEnvelope(alice, resolved.device, reqPayload, { grantId: resolved.grantId });
    expect(await call(alice.p, 'enqueue', { envelope: req })).toEqual({ messageId: req.messageId, state: 'queued' });
    await vi.waitFor(() => expect(bobWatch.frames).toHaveLength(2));

    // Recipient pulls the queued cross-org message with untrusted provenance, exact inbox binding, and verified sender bundle.
    const bobPending = await call(bob.p, 'pending', { deviceId: bob.p.agentId });
    expect(bobPending.envelopes).toEqual([{
      envelope: req,
      trust: 'external-untrusted',
      inbox: { address, harness: 'claude', session: 'inbox-b' },
      senderDevice: alice.d,
    }]);
    validateDevice(bobPending.envelopes[0].senderDevice);
    verify(bobPending.envelopes[0].senderDevice.signingKey, req.signature, envelopeBytes(req));
    expect(decryptEnvelope(bob.n, req)).toEqual(reqPayload);

    // Recipient cannot reply before observing delivery.
    const replyPayload = {
      kind: 'inbox', session: 'inbox-a', harness: 'codex',
      text: 'Correlated reply from Bob', from: 'inbox-b', from_harness: 'claude',
      from_card: 'Bob session', reply_to: req.messageId, hops: 2, external_untrusted: true,
    };
    const reply = encryptedEnvelope(bob, bobPending.envelopes[0].senderDevice, replyPayload, {
      grantId: req.grantId,
      parent: { owner: req.owner, messageId: req.messageId },
      hop: 2,
      nonce: '22'.repeat(12),
    });
    await expect(call(bob.p, 'enqueue', { envelope: reply })).rejects.toThrow('invalid_parent_lineage');

    // Recipient observes delivery, then enqueues the correlated reply and observes 'replied'.
    await call(bob.p, 'observe', observe(bob, req, 'delivered'));
    expect((await call(bob.p, 'pending', { deviceId: bob.p.agentId })).envelopes).toEqual([]);
    expect(await call(alice.p, 'status', { fromDevice: alice.p.agentId, messageId: req.messageId })).toMatchObject({ state: 'delivered' });

    expect(await call(bob.p, 'enqueue', { envelope: reply })).toEqual({ messageId: reply.messageId, state: 'queued' });
    await vi.waitFor(() => expect(aliceWatch.frames).toHaveLength(2));
    const repliedObs = observe(bob, req, 'replied');
    await call(bob.p, 'observe', repliedObs);
    expect(await call(alice.p, 'status', { fromDevice: alice.p.agentId, messageId: req.messageId })).toMatchObject({
      state: 'replied', repliedAt: repliedObs.at,
    });

    // Sender receives the correlated reply, decrypts it, and observes delivery back to recipient.
    const alicePending = await call(alice.p, 'pending', { deviceId: alice.p.agentId });
    expect(alicePending.envelopes).toEqual([{
      envelope: reply, trust: 'external-untrusted', senderDevice: bob.d,
    }]);
    expect(decryptEnvelope(alice.n, reply)).toEqual(replyPayload);
    await call(alice.p, 'observe', observe(alice, reply, 'delivered'));
    expect((await call(alice.p, 'pending', { deviceId: alice.p.agentId })).envelopes).toEqual([]);
    expect(await call(bob.p, 'status', { fromDevice: bob.p.agentId, messageId: reply.messageId })).toMatchObject({ state: 'delivered' });
  });

  it('denies an unauthorized agent who knows the address without leaking existence or allowing hijacking', async () => {
    const alice = await agent(org(), 'alice', 'admin', 1);
    const bob = await agent(org(), 'bob', 'admin', 5);
    const eve = await agent(org(), 'eve', 'admin', 9);
    const address = 'claude@wmm.10462';

    await call(bob.p, 'registerInbox', { address, harness: 'claude', session: 'inbox-b' });
    const grant = await call(bob.p, 'grantInbox', {
      address, senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId,
    });

    // Eve gets the exact same denial whether the address exists or does not exist.
    const existingDenied = await handleRequest(request('resolveInbox', { address }), env, { verify: async () => eve.p });
    const absentDenied = await handleRequest(request('resolveInbox', { address: 'nobody@wmm.99999' }), env, { verify: async () => eve.p });
    expect(existingDenied.status).toBe(403);
    expect(absentDenied.status).toBe(403);
    expect(await existingDenied.json()).toEqual({ error: 'inbox_not_allowed' });
    expect(await absentDenied.json()).toEqual({ error: 'inbox_not_allowed' });

    // Eve cannot hijack the address, cannot send with no grant, guessed grant, or Alice's stolen grantId.
    await expect(call(eve.p, 'registerInbox', { address, harness: 'claude', session: 'stolen' })).rejects.toThrow('address_taken');
    await expect(call(eve.p, 'enqueue', { envelope: envelope(eve, bob) })).rejects.toThrow('cross_owner_not_supported');
    await expect(call(eve.p, 'enqueue', { envelope: envelope(eve, bob, { grantId: 'grant-00000000000000000000000000000000' }) })).rejects.toThrow('inbox_not_allowed');
    await expect(call(eve.p, 'enqueue', { envelope: envelope(eve, bob, { grantId: grant.grantId }) })).rejects.toThrow('inbox_not_allowed');
    expect((await call(bob.p, 'pending', { deviceId: bob.p.agentId })).envelopes).toEqual([]);
  });

  it('blocks pickup, observation, retries, and reply delivery when a grant is revoked after enqueue', async () => {
    const alice = await agent(org(), 'alice', 'admin', 1);
    const bob = await agent(org(), 'bob', 'admin', 5);
    const address = 'claude@wmm.10462';

    await call(bob.p, 'registerInbox', { address, harness: 'claude', session: 'inbox-b' });
    const grant = await call(bob.p, 'grantInbox', {
      address, senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId,
    });

    const msg = envelope(alice, bob, { grantId: grant.grantId });
    expect(await call(alice.p, 'enqueue', { envelope: msg })).toEqual({ messageId: msg.messageId, state: 'queued' });

    // Revoke the grant AFTER enqueue, BEFORE Bob picks up the message.
    expect(await call(bob.p, 'revokeInboxGrant', { grantId: grant.grantId })).toEqual({ grantId: grant.grantId, status: 'revoked' });

    // Pickup, signed observation, status, retries, and future address resolution are all blocked.
    expect((await call(bob.p, 'pending', { deviceId: bob.p.agentId })).envelopes).toEqual([]);
    await expect(call(bob.p, 'observe', observe(bob, msg, 'delivered'))).rejects.toThrow('inbox_not_allowed');
    await expect(call(alice.p, 'enqueue', { envelope: msg })).rejects.toThrow('inbox_not_allowed');
    await expect(call(alice.p, 'status', { fromDevice: alice.p.agentId, messageId: msg.messageId })).rejects.toThrow('inbox_not_allowed');
    await expect(call(alice.p, 'resolveInbox', { address })).rejects.toThrow('inbox_not_allowed');

    // A fresh grant creates a brand-new grantId; revoking after Bob queues a reply also blocks Alice's reply pickup.
    const freshGrant = await call(bob.p, 'grantInbox', {
      address, senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId,
    });
    expect(freshGrant.grantId).not.toBe(grant.grantId);
    const req2 = envelope(alice, bob, { grantId: freshGrant.grantId });
    await call(alice.p, 'enqueue', { envelope: req2 });
    await call(bob.p, 'observe', observe(bob, req2, 'delivered'));
    const reply2 = envelope(bob, alice, { grantId: freshGrant.grantId, parent: { owner: req2.owner, messageId: req2.messageId }, hop: 2 });
    await call(bob.p, 'enqueue', { envelope: reply2 });
    await call(bob.p, 'revokeInboxGrant', { grantId: freshGrant.grantId });
    expect((await call(alice.p, 'pending', { deviceId: alice.p.agentId })).envelopes).toEqual([]);
    await expect(call(alice.p, 'observe', observe(alice, reply2, 'delivered'))).rejects.toThrow('inbox_not_allowed');
  });

  it('rejects cross-grant replies, hop overflow, silent inbox rebinding, and post-removal hijacking', async () => {
    const alice = await agent(org(), 'alice', 'admin', 1);
    const bob = await agent(org(), 'bob', 'admin', 5);
    const eve = await agent(org(), 'eve', 'admin', 9);

    await call(bob.p, 'registerInbox', { address: 'claude@wmm.10462', harness: 'claude', session: 'inbox-1' });
    await call(bob.p, 'registerInbox', { address: 'codex@wmm.10462', harness: 'codex', session: 'inbox-2' });
    await expect(call(bob.p, 'registerInbox', { address: 'claude@wmm.10462', harness: 'claude', session: 'other' })).rejects.toThrow('inbox_binding_immutable');

    const g1 = await call(bob.p, 'grantInbox', { address: 'claude@wmm.10462', senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId });
    const g2 = await call(bob.p, 'grantInbox', { address: 'codex@wmm.10462', senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId });

    const req = envelope(alice, bob, { grantId: g1.grantId });
    await call(alice.p, 'enqueue', { envelope: req });
    await call(bob.p, 'observe', observe(bob, req, 'delivered'));

    // Cross-grant reply and hop > 3 are both rejected.
    await expect(call(bob.p, 'enqueue', {
      envelope: envelope(bob, alice, { grantId: g2.grantId, parent: { owner: req.owner, messageId: req.messageId }, hop: 2 }),
    })).rejects.toThrow('invalid_parent_lineage');
    await expect(call(bob.p, 'enqueue', {
      envelope: envelope(bob, alice, { grantId: g1.grantId, parent: { owner: req.owner, messageId: req.messageId }, hop: 4 }),
    })).rejects.toThrow('invalid_parent_lineage');

    // Removing the inbox revokes active grants and tombstones address ownership against hijacking.
    await call(bob.p, 'registerInbox', { address: 'claude@wmm.10462', harness: 'claude', session: 'inbox-1', remove: true });
    await expect(call(alice.p, 'resolveInbox', { address: 'claude@wmm.10462' })).rejects.toThrow('inbox_not_allowed');
    await expect(call(eve.p, 'registerInbox', { address: 'claude@wmm.10462', harness: 'claude', session: 'hijack' })).rejects.toThrow('address_taken');

    // Revoking the sender's device after enqueue also hides the queued cross-org message on pickup.
    const queuedBeforeRevoke = envelope(alice, bob, { grantId: g2.grantId });
    await call(alice.p, 'enqueue', { envelope: queuedBeforeRevoke });
    await call(alice.p, 'revokeAgent', { deviceId: alice.p.agentId });
    expect((await call(bob.p, 'pending', { deviceId: bob.p.agentId })).envelopes).toEqual([]);
  });

  it('binds a zone to the first owner and rejects other owners with zone_taken', async () => {
    const bob = await agent(org(), 'bob', 'admin', 5);
    const eve = await agent(org(), 'eve', 'admin', 9);

    for (const [local, harness] of [['claude', 'claude'], ['codex', 'codex'], ['ops', 'ops']] as const) {
      expect(await call(bob.p, 'registerInbox', {
        address: `${local}@wmm.10462`, harness, session: `inbox-${local}`,
      })).toMatchObject({ address: `${local}@wmm.10462`, status: 'active' });
    }

    await expect(call(eve.p, 'registerInbox', { address: 'other@wmm.10462', harness: 'claude', session: 'inbox-e' }))
      .rejects.toThrow('zone_taken');
    expect(await call(eve.p, 'registerInbox', { address: 'other@eve.zone', harness: 'claude', session: 'inbox-e' }))
      .toMatchObject({ address: 'other@eve.zone', status: 'active' });
    await expect(call(eve.p, 'registerInbox', { address: 'claude@wmm.10462', harness: 'claude', session: 'inbox-b' }))
      .rejects.toThrow('address_taken');
  });

  it('tombstones zone ownership after the last address is released', async () => {
    const bob = await agent(org(), 'bob', 'admin', 5);
    const eve = await agent(org(), 'eve', 'admin', 9);

    for (const [local, harness] of [['claude', 'claude'], ['codex', 'codex']] as const) {
      await call(bob.p, 'registerInbox', { address: `${local}@wmm.10462`, harness, session: `inbox-${local}` });
    }
    for (const [local, harness] of [['claude', 'claude'], ['codex', 'codex']] as const) {
      await call(bob.p, 'registerInbox', { address: `${local}@wmm.10462`, harness, session: `inbox-${local}`, remove: true });
    }

    await expect(call(eve.p, 'registerInbox', { address: 'fresh@wmm.10462', harness: 'claude', session: 'inbox-e' }))
      .rejects.toThrow('zone_taken');
    expect(await call(bob.p, 'registerInbox', { address: 'fresh@wmm.10462', harness: 'claude', session: 'inbox-f' }))
      .toMatchObject({ address: 'fresh@wmm.10462', status: 'active' });
  });

  it('returns zone_taken from the address directory for a foreign zone claim', async () => {
    const bob = await agent(org(), 'bob', 'admin', 5);
    const eve = await agent(org(), 'eve', 'admin', 9);
    const directory = env.ADDRESSES!.get(env.ADDRESSES!.idFromName('global'));

    await call(bob.p, 'registerInbox', { address: 'claude@wmm.10462', harness: 'claude', session: 'inbox-b' });
    expect(await directory.claim('other@wmm.10462', namespace(eve.p))).toEqual({ ok: false, error: 'zone_taken', status: 409 });
    expect(await directory.claim('other@eve.zone', namespace(eve.p))).toEqual({ ok: true });
  });

  it('keeps inbox bindings immutable under concurrent conflicting registrations', async () => {
    const bob = await agent(org(), 'bob', 'admin', 5);
    const address = 'claude@wmm.10462';

    const results = await Promise.allSettled([
      call(bob.p, 'registerInbox', { address, harness: 'claude', session: 'inbox-1' }),
      call(bob.p, 'registerInbox', { address, harness: 'claude', session: 'inbox-2' }),
    ]);
    const ok = results.filter((r): r is PromiseFulfilledResult<any> => r.status === 'fulfilled');
    const failed = results.filter((r): r is PromiseRejectedResult => r.status === 'rejected');
    expect(ok).toHaveLength(1);
    expect(failed).toHaveLength(1);
    expect(String(failed[0].reason)).toContain('inbox_binding_immutable');

    const resolved = await call(bob.p, 'resolveInbox', { address });
    expect(resolved.session).toBe(ok[0].value.session);
  });

  it('recovers cross-org retries past the 5-minute createdAt skew window after sender reservation', async () => {
    const alice = await agent(org(), 'alice', 'admin', 1);
    const bob = await agent(org(), 'bob', 'admin', 5);
    const address = 'claude@wmm.10462';
    await call(bob.p, 'registerInbox', { address, harness: 'claude', session: 'inbox-b' });
    const grant = await call(bob.p, 'grantInbox', {
      address, senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId,
    });

    const t0 = Date.now();
    const msg = envelope(alice, bob, { grantId: grant.grantId, createdAt: t0, expiresAt: t0 + 3_600_000 });
    expect(await call(alice.p, 'enqueue', { envelope: msg })).toEqual({ messageId: msg.messageId, state: 'queued' });

    // Simulate a late retry 6 minutes later (past the 5-minute createdAt skew check, still unexpired).
    vi.spyOn(Date, 'now').mockReturnValue(t0 + 360_000);
    expect(await call(alice.p, 'enqueue', { envelope: msg })).toEqual({ messageId: msg.messageId, state: 'queued' });
    await call(bob.p, 'observe', observe(bob, msg, 'delivered'));
    expect(await call(alice.p, 'enqueue', { envelope: msg })).toEqual({ messageId: msg.messageId, state: 'delivered' });
  });
});

describe('two-member shared threads', () => {
  it('allows authenticated members to create only from their bound inbox device', async () => {
    const alice = await agent(org(), 'alice', 'member', 71);
    const otherMember = await agent(alice.p.org!, 'other', 'member', 73);
    const bob = await agent(org(), 'bob', 'member', 75);
    const outsider = await agent(org(), 'outsider', 'member', 77);
    const suffix = crypto.randomUUID().replaceAll('-', '').slice(0, 8);
    const aliceAddress = `alice${suffix}@member-zone-a`;
    const bobAddress = `bob${suffix}@member-zone-b`;
    await call(alice.p, 'registerInbox', { address: aliceAddress, harness: 'claude', session: 'alice-session' });
    await call(bob.p, 'registerInbox', { address: bobAddress, harness: 'codex', session: 'bob-session' });
    await call(alice.p, 'grantInbox', {
      address: aliceAddress, senderOwner: namespace(bob.p), senderDeviceId: bob.p.agentId,
    });
    await call(bob.p, 'grantInbox', {
      address: bobAddress, senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId,
    });

    const created = await call(alice.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'member-create',
    });
    expect(created.members[0]).toMatchObject({
      address: aliceAddress, owner: namespace(alice.p), agentId: alice.p.agentId,
    });
    await expect(call(otherMember.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'wrong-device-create',
    })).rejects.toThrow('creator_inbox_must_match_device');
    await expect(call(outsider.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'unauthorized-create',
    })).rejects.toThrow('creator_inbox_must_match_device');
  });

  it('requires reciprocal fixed members, stores an ordered encrypted pair once, and retains it after revoke', async () => {
    const alice = await agent(org(), 'alice', 'admin', 31);
    const bob = await agent(org(), 'bob', 'admin', 41);
    const other = await agent(alice.p.org!, 'other', 'member', 51);
    const outsider = await agent(org(), 'eve', 'admin', 61);
    const suffix = crypto.randomUUID().replaceAll('-', '').slice(0, 8);
    const aliceAddress = `alice${suffix}@zone-a`;
    const bobAddress = `bob${suffix}@zone-b`;
    const sameOwnerAddress = `same${suffix}@zone-a`;
    await call(alice.p, 'registerInbox', { address: aliceAddress, harness: 'claude', session: 'alice-session' });
    await call(bob.p, 'registerInbox', { address: bobAddress, harness: 'codex', session: 'bob-session' });
    await call(alice.p, 'registerInbox', { address: sameOwnerAddress, harness: 'claude', session: 'same-session' });

    const grantForAlice = await call(alice.p, 'grantInbox', {
      address: aliceAddress, senderOwner: namespace(bob.p), senderDeviceId: bob.p.agentId,
    });
    await expect(call(alice.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'create-one-way',
    })).rejects.toThrow('inbox_not_allowed');
    await expect(call(alice.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: sameOwnerAddress, idempotencyKey: 'create-same-owner',
    })).rejects.toThrow('thread_members_must_be_distinct_owners');

    const grantForBob = await call(bob.p, 'grantInbox', {
      address: bobAddress, senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId,
    });
    const created = await call(alice.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'create-stable',
    });
    const threadRef = created.threadRef as { creatorOwner: string; threadId: string };
    const members = created.members as [ThreadMember, ThreadMember];
    expect(threadRef.creatorOwner).toBe(namespace(alice.p));
    expect(members.map(m => m.grantId)).toEqual([grantForAlice.grantId, grantForBob.grantId]);
    expect(members.map(m => [m.address, m.owner, m.agentId, m.harness, m.session, m.fingerprint])).toEqual([
      [aliceAddress, namespace(alice.p), alice.p.agentId, 'claude', 'alice-session', fingerprint(alice.d)],
      [bobAddress, namespace(bob.p), bob.p.agentId, 'codex', 'bob-session', fingerprint(bob.d)],
    ]);
    expect((await call(alice.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'create-stable',
    })).threadRef).toEqual(threadRef);
    const concurrentCreates = await Promise.all([1, 2].map(() => call(alice.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'create-concurrent',
    })));
    expect(concurrentCreates[0].threadRef).toEqual(concurrentCreates[1].threadRef);
    await expect(call(alice.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: sameOwnerAddress, idempotencyKey: 'create-stable',
    })).rejects.toThrow('idempotency_conflict');

    await expect(call(other.p, 'threadShow', { threadRef })).rejects.toThrow('thread_member_required');
    await expect(call(outsider.p, 'threadShow', { threadRef })).rejects.toThrow('thread_member_required');
    const spoofed = threadPost(alice, members, threadRef, 'spoof-key', 'not authored by caller');
    await expect(call(bob.p, 'threadSend', { threadRef, post: spoofed })).rejects.toThrow('thread_author_mismatch');

    const fromAlice = threadPost(alice, members, threadRef, 'alice-message', 'alpha private text');
    const fromBob = threadPost(bob, members, threadRef, 'bob-message', 'beta private text');
    const tampered = structuredClone(fromAlice);
    tampered.context.commitment = '00'.repeat(32);
    await expect(call(alice.p, 'threadSend', { threadRef, post: tampered })).rejects.toThrow('invalid_signature');
    const swapped = structuredClone(fromAlice);
    swapped.copies.reverse();
    await expect(call(alice.p, 'threadSend', { threadRef, post: swapped })).rejects.toThrow('thread_copy_routing_mismatch');
    const oversized = structuredClone(fromAlice);
    oversized.copies[0].ciphertext = '00'.repeat(40_000);
    await expect(call(alice.p, 'threadSend', { threadRef, post: oversized })).rejects.toThrow('invalid_request');
    const [storedAlice, storedBob] = await Promise.all([
      call(alice.p, 'threadSend', { threadRef, post: fromAlice }),
      call(bob.p, 'threadSend', { threadRef, post: fromBob }),
    ]);
    expect([storedAlice.seq, storedBob.seq].sort()).toEqual([1, 2]);
    expect(storedAlice.state).toBe('stored');
    expect(await call(alice.p, 'threadSend', { threadRef, post: fromAlice })).toMatchObject({
      state: 'stored', seq: storedAlice.seq, messageId: fromAlice.context.messageId,
    });
    const changedRetry = threadPost(alice, members, threadRef, 'alice-message', 'changed text');
    await expect(call(alice.p, 'threadSend', { threadRef, post: changedRetry })).rejects.toThrow('idempotency_conflict');

    const emptyPage = await call(alice.p, 'threadRead', { threadRef, afterSeq: 100, limit: 10 });
    expect(emptyPage).toMatchObject({ messages: [], nextAfterSeq: 100, hasMore: false });
    const firstPage = await call(alice.p, 'threadRead', { threadRef, afterSeq: 0, limit: 1 });
    expect(firstPage.messages).toHaveLength(1);
    expect(firstPage.hasMore).toBe(true);
    const secondPage = await call(alice.p, 'threadRead', {
      threadRef, afterSeq: firstPage.nextAfterSeq, limit: 1,
    });
    expect(secondPage.messages).toHaveLength(1);
    expect(secondPage.hasMore).toBe(false);
    await expect(call(alice.p, 'threadRead', { threadRef, afterSeq: 0, limit: 0 }))
      .rejects.toThrow('invalid_request');
    const readAsBob = await handleRequest(
      request('threadRead', { threadRef, afterSeq: 0, limit: 10 }),
      env,
      { verify: async () => bob.p },
    );
    expect(readAsBob.status).toBe(200);
    const bobHistory = (await readAsBob.json() as { result: { messages: Array<any> } }).result.messages;
    expect(bobHistory).toHaveLength(2);
    expect(bobHistory.every(message => !('copies' in message) && message.copy.recipient.owner === namespace(bob.p))).toBe(true);
    expect(bobHistory.map(message => openThreadCopy(bob, message.context, message.copy)).sort())
      .toEqual(['alpha private text', 'beta private text']);
    const aliceHistory = await call(alice.p, 'threadRead', { threadRef, afterSeq: 0, limit: 10 });
    expect(aliceHistory.messages.every((message: any) => message.copy.recipient.owner === namespace(alice.p))).toBe(true);
    expect(aliceHistory.messages.map((message: any) => openThreadCopy(alice, message.context, message.copy)).sort())
      .toEqual(['alpha private text', 'beta private text']);

    const storedRows = await runInDurableObject(alice.stub, (_instance, state) =>
      state.storage.sql.exec<{ post: string }>(
        'SELECT post FROM thread_messages WHERE thread_id=? ORDER BY seq', threadRef.threadId,
      ).toArray());
    expect(storedRows).toHaveLength(2);
    const storedJson = JSON.stringify(storedRows);
    expect(storedJson).not.toContain('alpha private text');
    expect(storedJson).not.toContain('beta private text');

    await call(alice.p, 'revokeInboxGrant', { grantId: grantForAlice.grantId });
    const replacement = await call(alice.p, 'grantInbox', {
      address: aliceAddress, senderOwner: namespace(bob.p), senderDeviceId: bob.p.agentId,
    });
    expect(replacement.grantId).not.toBe(grantForAlice.grantId);
    await expect(call(alice.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'create-stable',
    })).rejects.toThrow('inbox_not_allowed');
    await expect(call(alice.p, 'threadShow', { threadRef })).rejects.toThrow('inbox_not_allowed');
    await expect(call(alice.p, 'threadRead', { threadRef, afterSeq: 0, limit: 10 })).rejects.toThrow('inbox_not_allowed');
    await expect(call(bob.p, 'threadRead', { threadRef, afterSeq: 0, limit: 10 })).rejects.toThrow('inbox_not_allowed');
    await expect(call(bob.p, 'threadSend', { threadRef, post: fromBob })).rejects.toThrow('inbox_not_allowed');
    await expect(call(alice.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'create-stable',
    })).rejects.toThrow('inbox_not_allowed');
    const retained = await runInDurableObject(alice.stub, (_instance, state) =>
      state.storage.sql.exec<{ count: number }>(
        'SELECT COUNT(*) AS count FROM thread_messages WHERE thread_id=?', threadRef.threadId,
      ).toArray()[0].count);
    expect(retained).toBe(2);
  });

  it('scopes message idempotency by owner when two owners share a device ID', async () => {
    const alice = await agent(org(), 'alice', 'admin', 101);
    const bob = await agent(org(), 'bob', 'admin', 111);
    const previousBobId = bob.p.agentId;
    const sharedDeviceId = alice.p.agentId;
    bob.p.agentId = sharedDeviceId;
    bob.d = device(bob.p, bob.n);
    await runInDurableObject(bob.stub, (_instance, state) => {
      state.storage.sql.exec('UPDATE credentials SET agent_id=? WHERE agent_id=?', sharedDeviceId, previousBobId);
      state.storage.sql.exec('UPDATE enrollments SET agent_id=? WHERE agent_id=?', sharedDeviceId, previousBobId);
      state.storage.sql.exec(
        'UPDATE devices SET agent_id=?,bundle=? WHERE agent_id=?',
        sharedDeviceId, JSON.stringify(bob.d), previousBobId,
      );
    });

    const suffix = crypto.randomUUID().replaceAll('-', '').slice(0, 8);
    const aliceAddress = `alice${suffix}@zone-a`, bobAddress = `bob${suffix}@zone-b`;
    await call(alice.p, 'registerInbox', { address: aliceAddress, harness: 'claude', session: 'alice-session' });
    await call(bob.p, 'registerInbox', { address: bobAddress, harness: 'codex', session: 'bob-session' });
    await call(alice.p, 'grantInbox', {
      address: aliceAddress, senderOwner: namespace(bob.p), senderDeviceId: bob.p.agentId,
    });
    await call(bob.p, 'grantInbox', {
      address: bobAddress, senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId,
    });
    const created = await call(alice.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'same-device-create',
    });
    const threadRef = created.threadRef as { creatorOwner: string; threadId: string };
    const members = created.members as [ThreadMember, ThreadMember];
    expect(members[0].agentId).toBe(members[1].agentId);
    expect(members[0].owner).not.toBe(members[1].owner);

    const fromAlice = threadPost(alice, members, threadRef, 'same-cross-org-key', 'Alice text');
    const fromBob = threadPost(bob, members, threadRef, 'same-cross-org-key', 'Bob text');
    const [storedAlice, storedBob] = await Promise.all([
      call(alice.p, 'threadSend', { threadRef, post: fromAlice }),
      call(bob.p, 'threadSend', { threadRef, post: fromBob }),
    ]);
    expect([storedAlice.seq, storedBob.seq].sort()).toEqual([1, 2]);
    expect(await call(alice.p, 'threadSend', { threadRef, post: fromAlice })).toMatchObject({
      seq: storedAlice.seq, messageId: fromAlice.context.messageId,
    });
    expect(await call(bob.p, 'threadSend', { threadRef, post: fromBob })).toMatchObject({
      seq: storedBob.seq, messageId: fromBob.context.messageId,
    });
    const rows = await runInDurableObject(alice.stub, (_instance, state) =>
      state.storage.sql.exec<{ author_owner: string; author_device: string; idempotency_key: string }>(
        'SELECT author_owner,author_device,idempotency_key FROM thread_messages WHERE thread_id=? ORDER BY seq',
        threadRef.threadId,
      ).toArray());
    expect(rows.map(row => row.author_owner).sort()).toEqual(
      [members[0].owner, members[1].owner].sort(),
    );
    expect(rows.every(row =>
      row.author_device === sharedDeviceId && row.idempotency_key === 'same-cross-org-key',
    )).toBe(true);
  });

  it('denies both members after remote grant revocation completes and retains encrypted history', async () => {
    const alice = await agent(org(), 'alice', 'admin', 121);
    const bob = await agent(org(), 'bob', 'admin', 131);
    const suffix = crypto.randomUUID().replaceAll('-', '').slice(0, 8);
    const aliceAddress = `alice${suffix}@zone-a`, bobAddress = `bob${suffix}@zone-b`;
    await call(alice.p, 'registerInbox', { address: aliceAddress, harness: 'claude', session: 'alice-session' });
    await call(bob.p, 'registerInbox', { address: bobAddress, harness: 'codex', session: 'bob-session' });
    await call(alice.p, 'grantInbox', {
      address: aliceAddress, senderOwner: namespace(bob.p), senderDeviceId: bob.p.agentId,
    });
    const remoteGrant = await call(bob.p, 'grantInbox', {
      address: bobAddress, senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId,
    });
    const created = await call(alice.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'remote-revoke-create',
    });
    const threadRef = created.threadRef as { creatorOwner: string; threadId: string };
    const members = created.members as [ThreadMember, ThreadMember];
    const fromAlice = threadPost(alice, members, threadRef, 'alice-before-revoke', 'Alice retained text');
    const fromBob = threadPost(bob, members, threadRef, 'bob-before-revoke', 'Bob retained text');
    await call(alice.p, 'threadSend', { threadRef, post: fromAlice });
    await call(bob.p, 'threadSend', { threadRef, post: fromBob });

    await call(bob.p, 'revokeInboxGrant', { grantId: remoteGrant.grantId });
    const afterRevokeAlice = threadPost(alice, members, threadRef, 'alice-after-revoke', 'not stored');
    const afterRevokeBob = threadPost(bob, members, threadRef, 'bob-after-revoke', 'not stored');
    await expect(call(alice.p, 'threadSend', { threadRef, post: afterRevokeAlice })).rejects.toThrow('inbox_not_allowed');
    await expect(call(bob.p, 'threadSend', { threadRef, post: afterRevokeBob })).rejects.toThrow('inbox_not_allowed');
    await expect(call(alice.p, 'threadRead', { threadRef, afterSeq: 0, limit: 10 })).rejects.toThrow('inbox_not_allowed');
    await expect(call(bob.p, 'threadRead', { threadRef, afterSeq: 0, limit: 10 })).rejects.toThrow('inbox_not_allowed');
    const retained = await runInDurableObject(alice.stub, (_instance, state) =>
      state.storage.sql.exec<{ post: string }>(
        'SELECT post FROM thread_messages WHERE thread_id=? ORDER BY seq', threadRef.threadId,
      ).toArray());
    expect(retained).toHaveLength(2);
    expect(JSON.stringify(retained)).not.toContain('Alice retained text');
    expect(JSON.stringify(retained)).not.toContain('Bob retained text');
  });

  it('does not reactivate after member device keys rotate', async () => {
    const alice = await agent(org(), 'alice', 'admin', 71);
    const bob = await agent(org(), 'bob', 'admin', 81);
    const suffix = crypto.randomUUID().replaceAll('-', '').slice(0, 8);
    const aliceAddress = `alice${suffix}@zone-a`, bobAddress = `bob${suffix}@zone-b`;
    await call(alice.p, 'registerInbox', { address: aliceAddress, harness: 'claude', session: 'alice-session' });
    await call(bob.p, 'registerInbox', { address: bobAddress, harness: 'codex', session: 'bob-session' });
    await call(alice.p, 'grantInbox', { address: aliceAddress, senderOwner: namespace(bob.p), senderDeviceId: bob.p.agentId });
    await call(bob.p, 'grantInbox', { address: bobAddress, senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId });
    const created = await call(alice.p, 'threadCreate', { fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'rotate-create' });
    const replacementDevice = device(bob.p, 91);
    await call(bob.p, 'rotate', {
      device: replacementDevice,
      oldFingerprint: fingerprint(bob.d),
      signature: sign(rotationBytes(replacementDevice, fingerprint(bob.d)), bob.n),
    });
    await expect(call(alice.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'rotate-create',
    })).rejects.toThrow('device_key_changed');
    await expect(call(bob.p, 'threadShow', { threadRef: created.threadRef })).rejects.toThrow('device_key_changed');
  });

  it('denies shared-history access after an inbox or owner membership is revoked', async () => {
    const alice = await agent(org(), 'alice', 'admin', 101);
    const bob = await agent(org(), 'bob', 'admin', 111);
    const suffix = crypto.randomUUID().replaceAll('-', '').slice(0, 8);
    const aliceAddress = `alice${suffix}@zone-a`, bobAddress = `bob${suffix}@zone-b`;
    await call(alice.p, 'registerInbox', { address: aliceAddress, harness: 'claude', session: 'alice-session' });
    await call(bob.p, 'registerInbox', { address: bobAddress, harness: 'codex', session: 'bob-session' });
    await call(alice.p, 'grantInbox', { address: aliceAddress, senderOwner: namespace(bob.p), senderDeviceId: bob.p.agentId });
    await call(bob.p, 'grantInbox', { address: bobAddress, senderOwner: namespace(alice.p), senderDeviceId: alice.p.agentId });
    const created = await call(alice.p, 'threadCreate', {
      fromAddress: aliceAddress, toAddress: bobAddress, idempotencyKey: 'revoke-snapshot',
    });

    await call(alice.p, 'registerInbox', {
      address: aliceAddress, harness: 'claude', session: 'alice-session', remove: true,
    });
    await expect(call(bob.p, 'threadShow', { threadRef: created.threadRef })).rejects.toBeDefined();

    const carol = await agent(org(), 'carol', 'admin', 121);
    const dave = await agent(org(), 'dave', 'admin', 131);
    const secondSuffix = crypto.randomUUID().replaceAll('-', '').slice(0, 8);
    const carolAddress = `carol${secondSuffix}@zone-c`, daveAddress = `dave${secondSuffix}@zone-d`;
    await call(carol.p, 'registerInbox', { address: carolAddress, harness: 'claude', session: 'carol-session' });
    await call(dave.p, 'registerInbox', { address: daveAddress, harness: 'codex', session: 'dave-session' });
    await call(carol.p, 'grantInbox', { address: carolAddress, senderOwner: namespace(dave.p), senderDeviceId: dave.p.agentId });
    await call(dave.p, 'grantInbox', { address: daveAddress, senderOwner: namespace(carol.p), senderDeviceId: carol.p.agentId });
    const second = await call(carol.p, 'threadCreate', {
      fromAddress: carolAddress, toAddress: daveAddress, idempotencyKey: 'membership-snapshot',
    });
    await call(dave.p, 'revokeMember', { subject: dave.p.subject });
    await expect(call(carol.p, 'threadShow', { threadRef: second.threadRef })).rejects.toBeDefined();
  });
});
