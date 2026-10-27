import { DurableObject } from 'cloudflare:workers';
import { z, ZodError } from 'zod';
import type { Env, Operation } from './index';
import {
  DEVICE_LABEL_LIMIT, deviceSchema, envelopeSchema, observationSchema, identitySchema, principalSchema,
  namespace, identifier, validateDevice, fingerprint, rotationBytes, verify,
  envelopeBytes, observationBytes, digest, RelayError,
  type Device, type Envelope, type Principal, type Identity, type MessageState,
} from './protocol';

type Credential = { agent_id: string; subject: string; status: string };
type Member = { subject: string; role: 'admin' | 'member'; status: string };
type Attempt = { message_id: string; from_device: string; idempotency_key: string; digest: string; state: MessageState; expires_at: number; metadata: string };
type Metadata = Omit<Envelope, 'ciphertext' | 'signature' | 'ephemeralKey' | 'nonce' | 'createdAt'>;
type EnrollmentResult = { ok: true; value: { owner: string; deviceId: string } } | { ok: false; error: string; status: number };
export type RelayResult = { ok: true; value: unknown } | { ok: false; error: string; status: number };
const empty = z.strictObject({});
const deviceIdArgs = z.strictObject({ deviceId: identifier });
const stateRank: Record<MessageState, number> = { queued: 0, expired: 0, delivered: 1, claimed: 2, replied: 3 };

export class OwnerRelay extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS owner (singleton INTEGER PRIMARY KEY CHECK(singleton=1), namespace TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS members (subject TEXT PRIMARY KEY, role TEXT NOT NULL, status TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS credentials (agent_id TEXT PRIMARY KEY, subject TEXT NOT NULL, status TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS enrollments (event_id TEXT PRIMARY KEY, identity TEXT NOT NULL, role TEXT NOT NULL, agent_id TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS devices (agent_id TEXT PRIMARY KEY, bundle TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS session_cards (seq INTEGER PRIMARY KEY AUTOINCREMENT, agent_id TEXT NOT NULL, session TEXT NOT NULL, harness TEXT NOT NULL, card TEXT NOT NULL, updated_at INTEGER NOT NULL, UNIQUE(agent_id,session));
      CREATE TABLE IF NOT EXISTS attempts (message_id TEXT PRIMARY KEY, from_device TEXT NOT NULL, idempotency_key TEXT NOT NULL, digest TEXT NOT NULL, state TEXT NOT NULL, expires_at INTEGER NOT NULL, to_device TEXT NOT NULL, metadata TEXT NOT NULL, UNIQUE(from_device, idempotency_key));
      CREATE INDEX IF NOT EXISTS attempt_recipient ON attempts(to_device, state, expires_at);
      CREATE TABLE IF NOT EXISTS messages (seq INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT NOT NULL UNIQUE, to_device TEXT NOT NULL, expires_at INTEGER NOT NULL, envelope TEXT NOT NULL);
      CREATE INDEX IF NOT EXISTS message_recipient ON messages(to_device,seq);
      CREATE INDEX IF NOT EXISTS message_expiry ON messages(expires_at);
      CREATE TABLE IF NOT EXISTS rates (scope TEXT PRIMARY KEY, window INTEGER NOT NULL, count INTEGER NOT NULL);
    `);
  }

  #row<T extends Record<string, SqlStorageValue>>(sql: string, ...args: SqlStorageValue[]) {
    return this.ctx.storage.sql.exec<T>(sql, ...args).toArray()[0];
  }
  #owner() { return this.#row<{ namespace: string }>('SELECT namespace FROM owner WHERE singleton=1')?.namespace; }

  // Only an operator-controlled enrollment Worker may hold a namespace binding and call this RPC.
  // There is no public HTTP path; the event must come after owner consent and verified identity.
  async enroll(identityInput: Identity, roleInput: 'admin' | 'member', eventIdInput: string): Promise<EnrollmentResult> {
    try { return { ok: true, value: this.#enrollVerified(identityInput, roleInput, eventIdInput) }; }
    catch (error) {
      if (error instanceof RelayError) return { ok: false, error: error.code, status: error.status };
      if (error instanceof ZodError) return { ok: false, error: 'invalid_request', status: 400 };
      throw error;
    }
  }

  #enrollVerified(identityInput: Identity, roleInput: 'admin' | 'member', eventIdInput: string) {
    const identity = identitySchema.parse(identityInput), owner = namespace(identity);
    const role = z.enum(['admin', 'member']).parse(roleInput), eventId = identifier.parse(eventIdInput);
    return this.ctx.storage.transactionSync(() => {
      const bound = this.#owner();
      if (bound && bound !== owner) throw new RelayError('Owner namespace is immutable', 409);
      const previous = this.#row<{ identity: string; role: string; agent_id: string }>('SELECT * FROM enrollments WHERE event_id=?', eventId);
      if (previous && (previous.identity !== JSON.stringify(identity) || previous.role !== role)) throw new RelayError('Enrollment event binding is immutable', 409);
      const member = this.#row<Member>('SELECT * FROM members WHERE subject=?', identity.subject);
      if (member?.status === 'revoked') throw new RelayError('Membership tombstone cannot be reused', 409);
      if (member && member.role !== role) throw new RelayError('Role changes require a separate control-plane operation', 409);
      if (previous) {
        const credential = this.#row<Credential>('SELECT * FROM credentials WHERE agent_id=?', previous.agent_id)!;
        if (credential.status !== 'active') throw new RelayError('Credential tombstone cannot be reused', 409);
        return { owner, deviceId: previous.agent_id };
      }
      const agentId = `agent-${crypto.randomUUID().replaceAll('-', '')}`;
      this.ctx.storage.sql.exec('INSERT OR IGNORE INTO owner VALUES(1,?)', owner);
      this.ctx.storage.sql.exec('INSERT OR IGNORE INTO members VALUES(?,?,?)', identity.subject, role, 'active');
      this.ctx.storage.sql.exec('INSERT INTO credentials VALUES(?,?,?)', agentId, identity.subject, 'active');
      this.ctx.storage.sql.exec('INSERT INTO enrollments VALUES(?,?,?,?)', eventId, JSON.stringify(identity), role, agentId);
      return { owner, deviceId: agentId };
    });
  }

  async credentialActive(identityInput: Identity, agentIdInput: string): Promise<boolean> {
    try {
      const identity = identitySchema.parse(identityInput), agentId = identifier.parse(agentIdInput);
      this.#authorize({ ...identity, agentId, role: 'member', expiresAt: Date.now() + 1000 });
      return true;
    } catch (error) {
      if (error instanceof RelayError || error instanceof ZodError) return false;
      throw error;
    }
  }

  async activeCredentialIds(principalInput: Principal): Promise<{ ok: true; value: string[] } | { ok: false; error: string; status: number }> {
    try {
      const p = principalSchema.parse(principalInput);
      this.#authorize(p);
      const rows = this.ctx.storage.sql.exec<{ agent_id: string }>(
        "SELECT c.agent_id FROM credentials c JOIN members m ON c.subject=m.subject WHERE c.status='active' AND m.status='active' ORDER BY c.agent_id LIMIT ?", DEVICE_LABEL_LIMIT + 1);
      return { ok: true, value: rows.toArray().map(row => row.agent_id) };
    } catch (error) {
      if (error instanceof RelayError) return { ok: false, error: error.code, status: error.status };
      if (error instanceof ZodError) return { ok: false, error: 'invalid_request', status: 400 };
      throw error;
    }
  }

  async revokeCredential(identityInput: Identity, agentIdInput: string): Promise<boolean> {
    const identity = identitySchema.safeParse(identityInput), agent = identifier.safeParse(agentIdInput);
    if (!identity.success || !agent.success || this.#owner() !== namespace(identity.data)) return false;
    const member = this.#row<Member>('SELECT * FROM members WHERE subject=?', identity.data.subject);
    const credential = this.#row<Credential>('SELECT * FROM credentials WHERE agent_id=?', agent.data);
    if (member?.status !== 'active' || member.role !== 'admin' || credential?.subject !== identity.data.subject) return false;
    this.ctx.storage.sql.exec("UPDATE credentials SET status='revoked' WHERE agent_id=?", agent.data);
    return true;
  }

  #authorize(p: Principal) {
    if (p.expiresAt <= Date.now()) throw new RelayError('invalid_token', 401);
    if (this.#owner() !== namespace(p)) throw new RelayError('wrong_owner');
    const member = this.#row<Member>('SELECT * FROM members WHERE subject=?', p.subject);
    const credential = this.#row<Credential>('SELECT * FROM credentials WHERE agent_id=?', p.agentId);
    if (!member || member.status !== 'active' || !credential || credential.status !== 'active' || credential.subject !== p.subject) throw new RelayError('credential_or_membership_revoked');
    return { member, credential, admin: member.role === 'admin' && p.role === 'admin' };
  }
  #activeDevice(agentId: string): Device {
    identifier.parse(agentId);
    const credential = this.#row<Credential>('SELECT * FROM credentials WHERE agent_id=?', agentId);
    const member = credential && this.#row<Member>('SELECT * FROM members WHERE subject=?', credential.subject);
    const device = this.#row<{ bundle: string }>('SELECT bundle FROM devices WHERE agent_id=?', agentId);
    if (!credential || credential.status !== 'active' || !member || member.status !== 'active' || !device) throw new RelayError('device_unavailable');
    return JSON.parse(device.bundle);
  }
  #own(p: Principal, agentId: string) { if (p.agentId !== agentId) throw new RelayError('agent_mismatch'); }
  #attempt(messageId: string) { return this.#row<Attempt>('SELECT * FROM attempts WHERE message_id=?', messageId); }
  #route(e: Pick<Envelope, 'owner' | 'recipientOwner' | 'grantId' | 'fromDevice' | 'toDevice' | 'senderFingerprint' | 'recipientFingerprint'>) {
    if (e.owner !== this.#owner() || e.recipientOwner !== e.owner || e.grantId !== null) throw new RelayError('cross_owner_not_supported');
    const sender = this.#activeDevice(e.fromDevice), recipient = this.#activeDevice(e.toDevice);
    if (fingerprint(sender) !== e.senderFingerprint || fingerprint(recipient) !== e.recipientFingerprint) throw new RelayError('device_key_changed');
    return { sender, recipient };
  }
  #rate(scope: string, limit: number, now: number) {
    const window = Math.floor(now / 60000);
    const old = this.#row<{ window: number; count: number }>('SELECT * FROM rates WHERE scope=?', scope);
    const count = old?.window === window ? old.count : 0;
    if (count >= limit) throw new RelayError('rate_limit', 429);
    this.ctx.storage.sql.exec('INSERT INTO rates VALUES(?,?,?) ON CONFLICT(scope) DO UPDATE SET window=excluded.window,count=excluded.count', scope, window, count + 1);
  }

  async relay(principalInput: Principal, op: Operation, args: unknown): Promise<string> {
    try {
      const p = principalSchema.parse(principalInput);
      const value = await this.ctx.storage.transaction(async () => {
        const result = this.#execute(p, op, args);
        if (op === 'enqueue' || op === 'accept' || op === 'observe') await this.#scheduleExpiry();
        return result;
      });
      return JSON.stringify({ ok: true, value });
    } catch (error) {
      if (error instanceof RelayError) return JSON.stringify({ ok: false, error: error.code, status: error.status });
      if (error instanceof ZodError) return JSON.stringify({ ok: false, error: 'invalid_request', status: 400 });
      throw error;
    }
  }

  #execute(p: Principal, op: Operation, args: unknown): unknown {
    const auth = this.#authorize(p), now = Date.now(), owner = namespace(p);
    switch (op) {
      case 'exposeSession': {
        this.#activeDevice(p.agentId);
        const a = z.strictObject({ session: identifier, harness: identifier, card: z.string().max(640), remove: z.boolean().optional() }).parse(args);
        if (a.remove) { this.ctx.storage.sql.exec('DELETE FROM session_cards WHERE agent_id=? AND session=?', p.agentId, a.session); return null; }
        const old = this.#row('SELECT seq FROM session_cards WHERE agent_id=? AND session=?', p.agentId, a.session);
        const count = this.#row<{ n: number }>('SELECT COUNT(*) AS n FROM session_cards WHERE agent_id=?', p.agentId)!.n;
        if (!old && count >= 32) throw new RelayError('session_limit', 409);
        this.ctx.storage.sql.exec('INSERT INTO session_cards(agent_id,session,harness,card,updated_at) VALUES(?,?,?,?,?) ON CONFLICT(agent_id,session) DO UPDATE SET harness=excluded.harness,card=excluded.card,updated_at=excluded.updated_at', p.agentId, a.session, a.harness, a.card, now);
        return null;
      }
      case 'sessionDirectory': {
        this.#activeDevice(p.agentId);
        const { cursor } = z.strictObject({ cursor: z.string().regex(/^[0-9]{1,16}$/).nullable().optional() }).parse(args);
        const rows = this.ctx.storage.sql.exec<{ seq: number; agent_id: string; session: string; harness: string; card: string; updated_at: number }>('SELECT * FROM session_cards WHERE seq>? ORDER BY seq LIMIT 33', Number(cursor ?? 0)).toArray();
        const sessions = [];
        for (const row of rows.slice(0, 32)) {
          try { this.#activeDevice(row.agent_id); }
          catch (error) { if (error instanceof RelayError) continue; throw error; }
          const credential = this.#row<Credential>('SELECT * FROM credentials WHERE agent_id=?', row.agent_id)!;
          sessions.push({ owner, author: credential.subject, agent: row.agent_id, session: row.session,
            harness: row.harness, card: row.card, updated_at: row.updated_at,
            target: `id:${row.agent_id}/${row.harness}/${row.session}` });
        }
        return { sessions, isDone: rows.length <= 32, continueCursor: rows.length ? String(rows[Math.min(rows.length, 32) - 1].seq) : cursor ?? '' };
      }
      case 'whoami': empty.parse(args); return { owner, deviceId: p.agentId, role: auth.admin ? 'admin' : 'member' };
      case 'register': {
        const { device } = z.strictObject({ device: deviceSchema }).parse(args);
        this.#own(p, device.deviceId);
        if (device.owner !== owner) throw new RelayError('wrong_owner');
        validateDevice(device);
        const old = this.#row<{ bundle: string }>('SELECT bundle FROM devices WHERE agent_id=?', p.agentId);
        if (old && fingerprint(JSON.parse(old.bundle)) !== fingerprint(device)) throw new RelayError('signed_rotation_required');
        this.ctx.storage.sql.exec('INSERT OR IGNORE INTO devices VALUES(?,?)', p.agentId, JSON.stringify(device));
        return null;
      }
      case 'device': { const { deviceId } = deviceIdArgs.parse(args); this.#activeDevice(p.agentId); return this.#activeDevice(deviceId); }
      case 'devices': {
        empty.parse(args); this.#activeDevice(p.agentId);
        const result: Device[] = [];
        for (const row of this.ctx.storage.sql.exec<{ agent_id: string }>('SELECT agent_id FROM devices ORDER BY agent_id LIMIT 256')) {
          try { result.push(this.#activeDevice(row.agent_id)); } catch (error) { if (!(error instanceof RelayError)) throw error; }
        }
        return result;
      }
      case 'credentials': {
        empty.parse(args);
        const rows = auth.admin ? this.ctx.storage.sql.exec<Credential>('SELECT * FROM credentials ORDER BY agent_id LIMIT 257').toArray() : [auth.credential];
        return { credentials: rows.slice(0, 256).map(r => ({ agentId: r.agent_id, subject: r.subject, status: r.status })), truncated: rows.length > 256 };
      }
      case 'memberships': {
        empty.parse(args); if (!auth.admin) throw new RelayError('admin_required');
        const rows = this.ctx.storage.sql.exec<Member>('SELECT * FROM members ORDER BY subject LIMIT 257').toArray();
        return { memberships: rows.slice(0, 256), truncated: rows.length > 256 };
      }
      case 'revokeAgent': {
        const { deviceId } = deviceIdArgs.parse(args); this.#activeDevice(p.agentId);
        if (deviceId !== p.agentId && !auth.admin) throw new RelayError('admin_required');
        if (!this.#row('SELECT agent_id FROM credentials WHERE agent_id=?', deviceId)) throw new RelayError('unknown_agent');
        this.ctx.storage.sql.exec("UPDATE credentials SET status='revoked' WHERE agent_id=?", deviceId); return null;
      }
      case 'revokeMember': {
        const { subject } = z.strictObject({ subject: z.string().min(1).max(256) }).parse(args);
        this.#activeDevice(p.agentId); if (!auth.admin) throw new RelayError('admin_required');
        if (!this.#row('SELECT subject FROM members WHERE subject=?', subject)) throw new RelayError('unknown_member');
        this.ctx.storage.sql.exec("UPDATE members SET status='revoked' WHERE subject=?", subject); return null;
      }
      case 'rotate': {
        const { device, oldFingerprint, signature } = z.strictObject({ device: deviceSchema, oldFingerprint: z.string().regex(/^[0-9a-f]{64}$/), signature: z.string().regex(/^[0-9a-f]{128}$/) }).parse(args);
        this.#own(p, device.deviceId); if (device.owner !== owner) throw new RelayError('wrong_owner');
        const old = this.#activeDevice(p.agentId); validateDevice(device);
        if (fingerprint(old) !== oldFingerprint) throw new RelayError('old_fingerprint_mismatch');
        verify(old.signingKey, signature, rotationBytes(device, oldFingerprint));
        this.ctx.storage.sql.exec('UPDATE devices SET bundle=? WHERE agent_id=?', JSON.stringify(device), p.agentId);
        return { fingerprint: fingerprint(device) };
      }
      case 'enqueue': {
        const { envelope: e } = z.strictObject({ envelope: envelopeSchema }).parse(args);
        this.#own(p, e.fromDevice); if (e.owner !== owner) throw new RelayError('wrong_owner');
        const { sender } = this.#route(e); verify(sender.signingKey, e.signature, envelopeBytes(e));
        const requestDigest = digest(envelopeBytes(e));
        const old = this.#row<Attempt>('SELECT * FROM attempts WHERE from_device=? AND idempotency_key=?', p.agentId, e.idempotencyKey);
        if (old) {
          if (old.digest !== requestDigest) throw new RelayError('idempotency_conflict', 409);
          return { messageId: old.message_id, state: this.#state(old, now) };
        }
        if (e.expiresAt <= now || Math.abs(e.createdAt - now) > 300000 || e.expiresAt <= e.createdAt || e.expiresAt - e.createdAt > 86400000) throw new RelayError('invalid_ttl', 400);
        if (this.#attempt(e.messageId)) throw new RelayError('message_id_reused', 409);
        if (e.parent === null) {
          if (e.hop !== 1) throw new RelayError('invalid_root_hop', 400);
        } else {
          if (e.parent.owner !== owner) throw new RelayError('cross_owner_not_supported');
          const parent = this.#attempt(e.parent.messageId), metadata: Metadata | undefined = parent && JSON.parse(parent.metadata);
          if (!parent || !metadata || metadata.toDevice !== p.agentId || !['delivered', 'claimed', 'replied'].includes(parent.state) || e.hop <= metadata.hop) throw new RelayError('invalid_parent_lineage');
        }
        const queued = this.#row<{ n: number }>("SELECT count(*) AS n FROM attempts WHERE to_device=? AND state='queued' AND expires_at>?", e.toDevice, now)!.n;
        if (queued >= 256) throw new RelayError('recipient_queue_full', 429);
        this.#rate(JSON.stringify(['agent', p.agentId]), 120, now);
        this.#rate(JSON.stringify(['account', p.issuer, p.subject]), 120, now);
        this.#rate('org', 600, now);
        const { ciphertext, signature, ephemeralKey, nonce, createdAt, ...metadata } = e;
        this.ctx.storage.sql.exec('INSERT INTO attempts VALUES(?,?,?,?,?,?,?,?)', e.messageId, p.agentId, e.idempotencyKey, requestDigest, 'queued', e.expiresAt, e.toDevice, JSON.stringify(metadata));
        this.ctx.storage.sql.exec('INSERT INTO messages(message_id,to_device,expires_at,envelope) VALUES(?,?,?,?)', e.messageId, e.toDevice, e.expiresAt, JSON.stringify(e));
        return { messageId: e.messageId, state: 'queued' };
      }
      case 'pending': {
        const { deviceId, cursor } = z.strictObject({ deviceId: identifier, cursor: z.string().regex(/^(0|[1-9][0-9]*)$/).max(16).nullable().optional() }).parse(args);
        this.#own(p, deviceId); this.#activeDevice(p.agentId);
        const after = cursor == null ? 0 : Number(cursor);
        if (!Number.isSafeInteger(after)) throw new RelayError('invalid_cursor', 400);
        const rows = this.ctx.storage.sql.exec<{ seq: number; envelope: string }>('SELECT seq,envelope FROM messages WHERE to_device=? AND seq>? ORDER BY seq LIMIT 33', p.agentId, after).toArray();
        const envelopes = [];
        for (const row of rows.slice(0, 32)) {
          const envelope: Envelope = JSON.parse(row.envelope);
          if (envelope.expiresAt <= now) continue;
          try { this.#route(envelope); envelopes.push({ envelope, trust: 'same-org' }); }
          catch (error) { if (!(error instanceof RelayError)) throw error; }
        }
        return { serverNow: now, envelopes, isDone: rows.length <= 32, continueCursor: String(rows.slice(0, 32).at(-1)?.seq ?? after) };
      }
      case 'accept': case 'observe': {
        const o = op === 'accept' ? observationSchema.parse({ ...z.strictObject(observationSchema.omit({ state: true }).shape).parse(args), state: 'delivered' }) : observationSchema.parse(args);
        this.#own(p, o.deviceId);
        if (o.senderOwner !== owner) throw new RelayError('cross_owner_not_supported');
        const recipient = this.#activeDevice(p.agentId);
        if (Math.abs(now - o.at) > 300000) throw new RelayError('stale_observation', 400);
        verify(recipient.signingKey, o.signature, observationBytes(owner, o));
        const a = this.#attempt(o.messageId), e: Metadata | undefined = a && JSON.parse(a.metadata);
        if (!a || !e || e.toDevice !== p.agentId) throw new RelayError('message_not_for_agent');
        this.#route(e);
        if (this.#state(a, now) === 'expired') throw new RelayError('message_expired');
        if (a.state === 'queued' && o.state !== 'delivered') throw new RelayError('delivery_required');
        if (stateRank[o.state] > stateRank[a.state]) this.ctx.storage.sql.exec('UPDATE attempts SET state=? WHERE message_id=?', o.state, o.messageId);
        this.ctx.storage.sql.exec('DELETE FROM messages WHERE message_id=?', o.messageId);
        return null;
      }
      case 'status': {
        const { fromDevice, messageId } = z.strictObject({ fromDevice: identifier, messageId: identifier }).parse(args);
        this.#own(p, fromDevice); this.#activeDevice(p.agentId);
        const a = this.#attempt(messageId);
        if (!a || a.from_device !== p.agentId) throw new RelayError('message_not_for_agent');
        this.#route(JSON.parse(a.metadata)); return { state: this.#state(a, now) };
      }
      default: throw new RelayError('unsupported_operation', 400);
    }
  }

  #state(a: Attempt, now: number): MessageState { return a.state === 'queued' && a.expires_at <= now ? 'expired' : a.state; }
  async #scheduleExpiry() {
    const next = this.#row<{ expires_at: number }>('SELECT min(expires_at) AS expires_at FROM messages');
    if (next?.expires_at != null) await this.ctx.storage.setAlarm(Math.max(Date.now() + 1, next.expires_at));
    else await this.ctx.storage.deleteAlarm();
  }
  async alarm() {
    const now = Date.now();
    await this.ctx.storage.transaction(async () => {
      const rows = this.ctx.storage.sql.exec<{ message_id: string }>('SELECT message_id FROM messages WHERE expires_at<=? LIMIT 64', now).toArray();
      for (const row of rows) {
        this.ctx.storage.sql.exec("UPDATE attempts SET state='expired' WHERE message_id=? AND state='queued'", row.message_id);
        this.ctx.storage.sql.exec('DELETE FROM messages WHERE message_id=?', row.message_id);
      }
      await this.#scheduleExpiry();
    });
  }
}
