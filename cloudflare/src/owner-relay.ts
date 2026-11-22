import { DurableObject } from 'cloudflare:workers';
import { z, ZodError } from 'zod';
import {
  CROSS_OWNER_MAX_HOPS, DEVICE_LABEL_LIMIT, deviceSchema, envelopeSchema, observationSchema, identitySchema, principalSchema,
  namespace, identifier, inboxAddress, validateDevice, fingerprint, rotationBytes, verify,
  envelopeBytes, observationBytes, digest, RelayError,
  THREAD_MAX_REQUEST_BYTES, threadCopyBytes, threadPairBytes, threadPostSchema, threadRefSchema,
  threadMemberSchema,
  type Device, type Envelope, type Observation, type Principal, type Identity, type MessageState, type Operation, type RelayResult,
  type ThreadMember, type ThreadPost,
} from './protocol';
import type { AddressDirectory } from './address-directory';

type OwnerRelayEnv = Cloudflare.Env & {
  OWNERS: DurableObjectNamespace<OwnerRelay>;
  ADDRESSES?: DurableObjectNamespace<AddressDirectory>;
};
type Credential = { agent_id: string; subject: string; status: string };
type Member = { subject: string; role: 'admin' | 'member'; status: string };
type InboxAddress = { address: string; agent_id: string; harness: string; session: string; status: string; created_at: number; updated_at: number };
type InboxGrant = { grant_id: string; address: string; recipient_agent_id: string; harness: string; session: string; sender_owner: string; sender_agent_id: string; status: string; created_at: number; revoked_at: number | null };
type Attempt = { message_id: string; from_device: string; to_device: string; idempotency_key: string; digest: string; state: MessageState; expires_at: number; metadata: string };
type Metadata = Omit<Envelope, 'ciphertext' | 'signature' | 'ephemeralKey' | 'nonce' | 'createdAt'> & {
  queuedAt?: number;
  claimedAt?: number;
  repliedAt?: number;
};
type EnrollmentResult = { ok: true; value: { owner: string; deviceId: string } } | { ok: false; error: string; status: number };
type RpcResult<T> = { ok: true; value: T } | { ok: false; error: string; status: number };
const empty = z.strictObject({});
// Matches the local inbox ID grammar used by peer::target for both address components.
const addressPart = identifier.max(128).refine(value => value !== '.' && value !== '..');
const deviceIdArgs = z.strictObject({ deviceId: identifier });
const stateRank: Record<MessageState, number> = { queued: 0, expired: 0, delivered: 1, claimed: 2, replied: 3 };
const watcherSchema = principalSchema.pick({ agentId: true, expiresAt: true }).extend({ fingerprint: z.string().regex(/^[0-9a-f]{64}$/) });
const inboxChanged = JSON.stringify({ type: 'inbox_changed' });

const meshLimit = { devices: 256, sessions: 256, receipts: 100 } as const;

export class OwnerRelay extends DurableObject<OwnerRelayEnv> {
  constructor(ctx: DurableObjectState, env: OwnerRelayEnv) {
    super(ctx, env);
    ctx.storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS owner (singleton INTEGER PRIMARY KEY CHECK(singleton=1), namespace TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS members (subject TEXT PRIMARY KEY, role TEXT NOT NULL, status TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS credentials (agent_id TEXT PRIMARY KEY, subject TEXT NOT NULL, status TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS enrollments (event_id TEXT PRIMARY KEY, identity TEXT NOT NULL, role TEXT NOT NULL, agent_id TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS devices (agent_id TEXT PRIMARY KEY, bundle TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS session_cards (seq INTEGER PRIMARY KEY AUTOINCREMENT, agent_id TEXT NOT NULL, session TEXT NOT NULL, harness TEXT NOT NULL, card TEXT NOT NULL, updated_at INTEGER NOT NULL, UNIQUE(agent_id,session));
      CREATE TABLE IF NOT EXISTS inbox_addresses (address TEXT PRIMARY KEY, agent_id TEXT NOT NULL, harness TEXT NOT NULL, session TEXT NOT NULL, status TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS inbox_grants (grant_id TEXT PRIMARY KEY, address TEXT NOT NULL, recipient_agent_id TEXT NOT NULL, harness TEXT NOT NULL, session TEXT NOT NULL, sender_owner TEXT NOT NULL, sender_agent_id TEXT NOT NULL, status TEXT NOT NULL, created_at INTEGER NOT NULL, revoked_at INTEGER);
      CREATE INDEX IF NOT EXISTS inbox_grant_lookup ON inbox_grants(address, sender_owner, sender_agent_id, status);
      CREATE TABLE IF NOT EXISTS threads (thread_id TEXT PRIMARY KEY, creator_device TEXT NOT NULL, create_key TEXT NOT NULL, create_digest TEXT NOT NULL, members TEXT NOT NULL, created_at INTEGER NOT NULL, UNIQUE(creator_device,create_key));
      CREATE TABLE IF NOT EXISTS thread_messages (thread_id TEXT NOT NULL, seq INTEGER NOT NULL, message_id TEXT NOT NULL UNIQUE, author_owner TEXT NOT NULL, author_device TEXT NOT NULL, idempotency_key TEXT NOT NULL, digest TEXT NOT NULL, post TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY(thread_id,seq), UNIQUE(thread_id,author_owner,author_device,idempotency_key));
      CREATE INDEX IF NOT EXISTS thread_message_history ON thread_messages(thread_id,seq);
      CREATE TABLE IF NOT EXISTS attempts (message_id TEXT PRIMARY KEY, from_device TEXT NOT NULL, idempotency_key TEXT NOT NULL, digest TEXT NOT NULL, state TEXT NOT NULL, expires_at INTEGER NOT NULL, to_device TEXT NOT NULL, metadata TEXT NOT NULL, UNIQUE(from_device, idempotency_key));
      CREATE INDEX IF NOT EXISTS attempt_recipient ON attempts(to_device, state, expires_at);
      CREATE TABLE IF NOT EXISTS messages (seq INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT NOT NULL UNIQUE, to_device TEXT NOT NULL, expires_at INTEGER NOT NULL, envelope TEXT NOT NULL);
      CREATE INDEX IF NOT EXISTS message_recipient ON messages(to_device,seq);
      CREATE INDEX IF NOT EXISTS message_expiry ON messages(expires_at);
      CREATE TABLE IF NOT EXISTS mesh_seen(agent_id TEXT PRIMARY KEY, at INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS mesh_receipts(seq INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT NOT NULL, device_id TEXT NOT NULL, state TEXT NOT NULL, at INTEGER NOT NULL, UNIQUE(message_id,state));
      CREATE TABLE IF NOT EXISTS rates (scope TEXT PRIMARY KEY, window INTEGER NOT NULL, count INTEGER NOT NULL);
    `);
  }

  #row<T extends Record<string, SqlStorageValue>>(sql: string, ...args: SqlStorageValue[]) {
    return this.ctx.storage.sql.exec<T>(sql, ...args).toArray()[0];
  }
  #owner() { return this.#row<{ namespace: string }>('SELECT namespace FROM owner WHERE singleton=1')?.namespace; }

  async fetch(request: Request): Promise<Response> {
    if (request.method !== 'GET' || new URL(request.url).pathname !== '/watch' || request.headers.get('Upgrade')?.toLowerCase() !== 'websocket') return Response.json({ error: 'not_found' }, { status: 404 });
    try {
      const p = principalSchema.parse(JSON.parse(decodeURIComponent(request.headers.get('X-Everett-Principal') ?? 'null')));
      this.#authorize(p);
      const device = this.#activeDevice(p.agentId);
      this.#rate(JSON.stringify(['watch', p.agentId]), 12, Date.now());
      if (this.ctx.getWebSockets(p.agentId).filter(socket => socket.readyState === WebSocket.OPEN).length >= 2) throw new RelayError('connection_limit', 429);
      const pair = new WebSocketPair();
      this.ctx.acceptWebSocket(pair[1], [p.agentId]);
      pair[1].serializeAttachment({ agentId: p.agentId, expiresAt: p.expiresAt, fingerprint: fingerprint(device) });
      pair[1].send(inboxChanged);
      return new Response(null, { status: 101, webSocket: pair[0] });
    } catch (error) {
      return Response.json({ error: error instanceof RelayError ? error.code : 'invalid_request' }, { status: error instanceof RelayError ? error.status : 400 });
    }
  }

  #watcherActive(socket: WebSocket): boolean {
    try {
      const bound = watcherSchema.parse(socket.deserializeAttachment());
      if (bound.expiresAt <= Date.now()) throw new RelayError('invalid_token', 401);
      if (fingerprint(this.#activeDevice(bound.agentId)) !== bound.fingerprint) throw new RelayError('device_key_changed');
      return true;
    } catch {
      socket.close(1008, 'notification authorization expired or changed');
      return false;
    }
  }

  #notify(device: string) {
    for (const socket of this.ctx.getWebSockets(device)) {
      if (!this.#watcherActive(socket)) continue;
      try { socket.send(inboxChanged); } catch { socket.close(1001, 'reconnect'); }
    }
  }

  #closeInvalidWatchers() { for (const socket of this.ctx.getWebSockets()) this.#watcherActive(socket); }

  webSocketMessage(socket: WebSocket, _message: string | ArrayBuffer) { socket.close(1008, 'notifications are server-only'); }
  webSocketClose(socket: WebSocket, _code: number, _reason: string, _wasClean: boolean) { socket.close(1000, ''); }
  webSocketError(socket: WebSocket, _error: unknown) { socket.close(1011, 'reconnect'); }

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

  async meshSnapshot(input: Principal) {
    const p = principalSchema.parse(input);
    this.#authorize(p);
    const now = Date.now();
    const devices = this.ctx.storage.sql.exec<{ agent_id: string; at: number | null }>("SELECT d.agent_id,s.at FROM devices d JOIN credentials c ON c.agent_id=d.agent_id JOIN members m ON m.subject=c.subject LEFT JOIN mesh_seen s ON s.agent_id=d.agent_id WHERE c.subject=? AND c.status='active' AND m.status='active' ORDER BY d.agent_id LIMIT ?", p.subject, meshLimit.devices + 1).toArray();
    const deviceCap = devices.length > meshLimit.devices;
    const displayedDevices = devices.slice(0, meshLimit.devices);
    const lastDeviceId = displayedDevices.at(-1)?.agent_id ?? null;
    const activeSessions = "FROM session_cards s JOIN devices d ON d.agent_id=s.agent_id JOIN credentials c ON c.agent_id=s.agent_id WHERE c.subject=? AND c.status='active'";
    const sessions = this.ctx.storage.sql.exec<{ agent_id: string; session: string; harness: string; updated_at: number }>(
      `SELECT s.agent_id,s.session,s.harness,s.updated_at ${activeSessions} AND s.agent_id<=? ORDER BY s.updated_at DESC,s.seq DESC LIMIT ?`,
      p.subject, lastDeviceId, meshLimit.sessions + 1,
    ).toArray();
    const omittedSessions = deviceCap && !!this.#row(
      `SELECT 1 ${activeSessions} AND s.agent_id>? LIMIT 1`, p.subject, lastDeviceId,
    );
    const queue = this.#row<{ queued: number }>("SELECT count(*) queued FROM attempts a JOIN credentials f ON f.agent_id=a.from_device JOIN credentials t ON t.agent_id=a.to_device WHERE f.subject=? AND t.subject=? AND f.status='active' AND t.status='active' AND a.state='queued' AND a.expires_at>?", p.subject,p.subject,now)!;
    const receipts = this.ctx.storage.sql.exec<{ message_id: string; device_id: string; state: string; at: number }>("SELECT r.message_id,r.device_id,r.state,r.at FROM mesh_receipts r JOIN attempts a ON a.message_id=r.message_id JOIN credentials f ON f.agent_id=a.from_device JOIN credentials t ON t.agent_id=a.to_device WHERE f.subject=? AND t.subject=? AND f.status='active' AND t.status='active' ORDER BY r.seq DESC LIMIT ?",p.subject,p.subject,meshLimit.receipts + 1).toArray();
    return {
      version: 'everett.mesh.v1' as const, source: 'cloudflare' as const, owner: namespace(p), serverNow: now,
      devices: displayedDevices.map(d => ({ deviceId:d.agent_id,lastSeen:d.at,lastSeenConfidence:d.at === null ? 'unavailable' : 'server-observed-request' })),
      sessions: sessions.slice(0,meshLimit.sessions).map(s => ({ deviceId:s.agent_id,sessionId:s.session,harness:s.harness,lastSeen:s.updated_at,lastSeenConfidence:'server-observed-exposure',selfReported:{ status:null,confidence:'unavailable',observedAt:null } })),
      queue, receipts:receipts.slice(0,meshLimit.receipts).map(r => ({ messageId:r.message_id,deviceId:r.device_id,state:r.state,at:r.at,confidence:'signed-device-observation' })),
      truncated: { devices:deviceCap,sessions:sessions.length>meshLimit.sessions || omittedSessions,receipts:receipts.length>meshLimit.receipts },
    };
  }
  async revokeCredential(identityInput: Identity, agentIdInput: string): Promise<boolean> {
    const identity = identitySchema.safeParse(identityInput), agent = identifier.safeParse(agentIdInput);
    if (!identity.success || !agent.success || this.#owner() !== namespace(identity.data)) return false;
    const member = this.#row<Member>('SELECT * FROM members WHERE subject=?', identity.data.subject);
    const credential = this.#row<Credential>('SELECT * FROM credentials WHERE agent_id=?', agent.data);
    if (member?.status !== 'active' || member.role !== 'admin' || credential?.subject !== identity.data.subject) return false;
    this.ctx.storage.sql.exec("UPDATE credentials SET status='revoked' WHERE agent_id=?", agent.data);
    this.#closeInvalidWatchers();
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
  #ownerStub(targetOwner: string) { return this.env.OWNERS.get(this.env.OWNERS.idFromName(targetOwner)); }
  #addresses() {
    if (!this.env.ADDRESSES) throw new RelayError('address_directory_unavailable', 503);
    return this.env.ADDRESSES.get(this.env.ADDRESSES.idFromName('global'));
  }
  #touchSeen(agentId: string) {
    this.ctx.storage.sql.exec('INSERT INTO mesh_seen VALUES(?,?) ON CONFLICT(agent_id) DO UPDATE SET at=excluded.at', agentId, Date.now());
  }
  #activeGrant(grantId: string, senderOwner: string, senderDeviceId: string, recipientDeviceId: string) {
    const grant = this.#row<InboxGrant>('SELECT * FROM inbox_grants WHERE grant_id=?', grantId);
    if (
      !grant
      || grant.status !== 'active'
      || grant.sender_owner !== senderOwner
      || grant.sender_agent_id !== senderDeviceId
      || grant.recipient_agent_id !== recipientDeviceId
    ) throw new RelayError('inbox_not_allowed', 403);
    const inbox = this.#row<InboxAddress>('SELECT * FROM inbox_addresses WHERE address=?', grant.address);
    if (
      !inbox
      || inbox.status !== 'active'
      || inbox.agent_id !== grant.recipient_agent_id
      || inbox.harness !== grant.harness
      || inbox.session !== grant.session
    ) throw new RelayError('inbox_not_allowed', 403);
    return { grant, inbox };
  }
  #assertReplyCorrelation(
    orig: Attempt | undefined,
    origMeta: Metadata | undefined,
    e: Pick<Envelope, 'owner' | 'recipientOwner' | 'grantId' | 'fromDevice' | 'toDevice' | 'parent' | 'hop' | 'senderFingerprint' | 'recipientFingerprint'>,
    expectedOwner: string,
  ) {
    if (
      !orig
      || !origMeta
      || origMeta.parent !== null
      || !e.parent
      || e.parent.owner !== expectedOwner
      || orig.from_device !== e.toDevice
      || orig.to_device !== e.fromDevice
      || origMeta.owner !== expectedOwner
      || origMeta.recipientOwner !== e.owner
      || origMeta.grantId === null
      || origMeta.grantId !== e.grantId
      || origMeta.recipientFingerprint !== e.senderFingerprint
      || origMeta.senderFingerprint !== e.recipientFingerprint
      || e.hop <= origMeta.hop
      || e.hop > CROSS_OWNER_MAX_HOPS
    ) throw new RelayError('invalid_parent_lineage');
  }
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

  async resolveCrossOwnerInbox(senderOwnerInput: string, senderDeviceIdInput: string, addressInput: string): Promise<RpcResult<{
    address: string; owner: string; device: Device; harness: string; session: string; grantId: string;
  }>> {
    try {
      const senderOwner = z.string().min(1).max(4096).parse(senderOwnerInput);
      const senderDeviceId = identifier.parse(senderDeviceIdInput);
      const address = inboxAddress.parse(addressInput);
      const owner = this.#owner();
      if (!owner || owner === senderOwner) throw new RelayError('inbox_not_allowed', 403);
      const grant = this.#row<InboxGrant>(
        "SELECT * FROM inbox_grants WHERE address=? AND sender_owner=? AND sender_agent_id=? AND status='active' ORDER BY created_at DESC LIMIT 1",
        address, senderOwner, senderDeviceId,
      );
      if (!grant) throw new RelayError('inbox_not_allowed', 403);
      const { inbox } = this.#activeGrant(grant.grant_id, senderOwner, senderDeviceId, grant.recipient_agent_id);
      const device = this.#activeDevice(inbox.agent_id);
      return { ok: true, value: { address, owner, device, harness: inbox.harness, session: inbox.session, grantId: grant.grant_id } };
    } catch (error) {
      if (error instanceof RelayError) return { ok: false, error: error.code === 'device_unavailable' ? 'inbox_not_allowed' : error.code, status: error.status };
      if (error instanceof ZodError) return { ok: false, error: 'invalid_request', status: 400 };
      throw error;
    }
  }

  async verifyActiveDevice(agentIdInput: string, expectedFingerprint: string): Promise<RpcResult<Device>> {
    try {
      const agentId = identifier.parse(agentIdInput);
      const device = this.#activeDevice(agentId);
      if (fingerprint(device) !== expectedFingerprint) throw new RelayError('device_key_changed');
      return { ok: true, value: device };
    } catch (error) {
      if (error instanceof RelayError) return { ok: false, error: error.code, status: error.status };
      if (error instanceof ZodError) return { ok: false, error: 'invalid_request', status: 400 };
      throw error;
    }
  }

  async verifyCrossOwnerReplySender(
    grantIdInput: string,
    recipientDeviceInput: string,
    senderOwnerInput: string,
    senderDeviceInput: string,
    expectedFingerprint: string,
  ): Promise<RpcResult<Device>> {
    try {
      const grantId = identifier.parse(grantIdInput);
      const recipientDeviceId = identifier.parse(recipientDeviceInput);
      const senderOwner = z.string().min(1).max(4096).parse(senderOwnerInput);
      const senderDeviceId = identifier.parse(senderDeviceInput);
      this.#activeGrant(grantId, senderOwner, senderDeviceId, recipientDeviceId);
      const device = this.#activeDevice(recipientDeviceId);
      if (fingerprint(device) !== expectedFingerprint) throw new RelayError('device_key_changed');
      return { ok: true, value: device };
    } catch (error) {
      if (error instanceof RelayError) return { ok: false, error: error.code, status: error.status };
      if (error instanceof ZodError) return { ok: false, error: 'invalid_request', status: 400 };
      throw error;
    }
  }

  async verifyCrossOwnerAccess(eInput: Pick<Envelope, 'owner' | 'recipientOwner' | 'grantId' | 'fromDevice' | 'toDevice' | 'parent' | 'hop' | 'senderFingerprint' | 'recipientFingerprint'>): Promise<RpcResult<null>> {
    try {
      const owner = this.#owner();
      if (!owner || eInput.recipientOwner !== owner || eInput.owner === owner || eInput.grantId === null) throw new RelayError('cross_owner_not_supported');
      const recipient = this.#activeDevice(eInput.toDevice);
      if (fingerprint(recipient) !== eInput.recipientFingerprint) throw new RelayError('device_key_changed');
      if (eInput.parent === null) {
        this.#activeGrant(eInput.grantId, eInput.owner, eInput.fromDevice, eInput.toDevice);
      } else {
        const orig = this.#attempt(eInput.parent.messageId);
        const origMeta: Metadata | undefined = orig && JSON.parse(orig.metadata);
        this.#assertReplyCorrelation(orig, origMeta, eInput, owner);
      }
      return { ok: true, value: null };
    } catch (error) {
      if (error instanceof RelayError) return { ok: false, error: error.code, status: error.status };
      if (error instanceof ZodError) return { ok: false, error: 'invalid_request', status: 400 };
      throw error;
    }
  }

  async receiveCrossOwnerEnvelope(eInput: Envelope, senderDeviceInput: Device): Promise<RpcResult<{ messageId: string; state: MessageState }>> {
    try {
      const e = envelopeSchema.parse(eInput);
      const sender = deviceSchema.parse(senderDeviceInput);
      const now = Date.now();
      const result = this.ctx.storage.transactionSync(() => {
        const owner = this.#owner();
        if (!owner || e.recipientOwner !== owner || e.owner === owner || e.grantId === null) throw new RelayError('cross_owner_not_supported');
        validateDevice(sender);
        if (sender.owner !== e.owner || sender.deviceId !== e.fromDevice || fingerprint(sender) !== e.senderFingerprint) throw new RelayError('device_key_changed');
        const recipient = this.#activeDevice(e.toDevice);
        if (fingerprint(recipient) !== e.recipientFingerprint) throw new RelayError('device_key_changed');
        verify(sender.signingKey, e.signature, envelopeBytes(e));
        if (e.parent === null) {
          if (e.hop !== 1) throw new RelayError('invalid_root_hop', 400);
          this.#activeGrant(e.grantId, e.owner, e.fromDevice, e.toDevice);
        } else {
          const orig = this.#attempt(e.parent.messageId);
          const origMeta: Metadata | undefined = orig && JSON.parse(orig.metadata);
          this.#assertReplyCorrelation(orig, origMeta, e, owner);
        }
        const requestDigest = digest(envelopeBytes(e));
        const old = this.#row<Attempt>('SELECT * FROM attempts WHERE from_device=? AND idempotency_key=?', e.fromDevice, e.idempotencyKey);
        if (old) {
          if (old.digest !== requestDigest) throw new RelayError('idempotency_conflict', 409);
          return { messageId: old.message_id, state: this.#state(old, now), newlyQueued: false };
        }
        if (e.expiresAt <= now || Math.abs(e.createdAt - now) > 300000 || e.expiresAt <= e.createdAt || e.expiresAt - e.createdAt > 86400000) throw new RelayError('invalid_ttl', 400);
        if (this.#attempt(e.messageId)) throw new RelayError('message_id_reused', 409);
        const queued = this.#row<{ n: number }>("SELECT count(*) AS n FROM attempts WHERE to_device=? AND state='queued' AND expires_at>?", e.toDevice, now)!.n;
        if (queued >= 256) throw new RelayError('recipient_queue_full', 429);
        const { ciphertext, signature, ephemeralKey, nonce, createdAt, ...metadata } = e;
        this.ctx.storage.sql.exec('INSERT INTO attempts VALUES(?,?,?,?,?,?,?,?)', e.messageId, e.fromDevice, e.idempotencyKey, requestDigest, 'queued', e.expiresAt, e.toDevice, JSON.stringify({ ...metadata, queuedAt: now }));
        this.ctx.storage.sql.exec('INSERT INTO messages(message_id,to_device,expires_at,envelope) VALUES(?,?,?,?)', e.messageId, e.toDevice, e.expiresAt, JSON.stringify(e));
        return { messageId: e.messageId, state: 'queued' as MessageState, newlyQueued: true };
      });
      if (result.newlyQueued) {
        await this.#scheduleExpiry();
        this.#notify(e.toDevice);
      }
      return { ok: true, value: { messageId: result.messageId, state: result.state } };
    } catch (error) {
      if (error instanceof RelayError) return { ok: false, error: error.code, status: error.status };
      if (error instanceof ZodError) return { ok: false, error: 'invalid_request', status: 400 };
      throw error;
    }
  }

  async verifyAndRecordCrossOwnerObservation(recipientOwnerInput: string, oInput: Observation, recipientDeviceInput: Device): Promise<RpcResult<null>> {
    try {
      const recipientOwner = z.string().min(1).max(4096).parse(recipientOwnerInput);
      const o = observationSchema.parse(oInput);
      const recipient = deviceSchema.parse(recipientDeviceInput);
      const now = Date.now();
      this.ctx.storage.transactionSync(() => {
        const owner = this.#owner();
        if (!owner || o.senderOwner !== owner || recipientOwner === owner) throw new RelayError('cross_owner_not_supported');
        const a = this.#attempt(o.messageId);
        const e: Metadata | undefined = a && JSON.parse(a.metadata);
        if (!a || !e || e.owner !== owner || e.recipientOwner !== recipientOwner || e.toDevice !== o.deviceId || e.grantId === null) {
          throw new RelayError('message_not_for_agent');
        }
        const sender = this.#activeDevice(e.fromDevice);
        if (fingerprint(sender) !== e.senderFingerprint) throw new RelayError('device_key_changed');
        if (e.parent !== null) {
          this.#activeGrant(e.grantId, recipientOwner, o.deviceId, e.fromDevice);
        }
        validateDevice(recipient);
        if (recipient.owner !== recipientOwner || recipient.deviceId !== o.deviceId || fingerprint(recipient) !== e.recipientFingerprint) {
          throw new RelayError('device_key_changed');
        }
        if (Math.abs(now - o.at) > 300000) throw new RelayError('stale_observation', 400);
        verify(recipient.signingKey, o.signature, observationBytes(recipientOwner, o));
        if (this.#state(a, now) === 'expired') throw new RelayError('message_expired');
        if (a.state === 'queued' && o.state !== 'delivered') throw new RelayError('delivery_required');
        const state = stateRank[o.state] > stateRank[a.state] ? o.state : a.state;
        const newClaim = o.state === 'claimed' && e.claimedAt === undefined;
        const newReply = o.state === 'replied' && e.repliedAt === undefined;
        if (newClaim) e.claimedAt = o.at;
        if (newReply) e.repliedAt = o.at;
        if (state !== a.state || newClaim || newReply) {
          this.ctx.storage.sql.exec('UPDATE attempts SET state=?, metadata=? WHERE message_id=?', state, JSON.stringify(e), o.messageId);
        }
      });
      return { ok: true, value: null };
    } catch (error) {
      if (error instanceof RelayError) return { ok: false, error: error.code, status: error.status };
      if (error instanceof ZodError) return { ok: false, error: 'invalid_request', status: 400 };
      throw error;
    }
  }

  async #registerInbox(p: Principal, args: unknown) {
    const auth = this.#authorize(p), owner = namespace(p), now = Date.now();
    this.#activeDevice(p.agentId);
    const a = z.strictObject({
      address: inboxAddress,
      deviceId: identifier.optional(),
      harness: addressPart,
      session: addressPart,
      remove: z.boolean().optional(),
    }).parse(args);
    const targetDevice = a.deviceId ?? p.agentId;
    if (targetDevice !== p.agentId && !auth.admin) throw new RelayError('admin_required');
    if (a.remove) {
      const existing = this.#row<InboxAddress>('SELECT * FROM inbox_addresses WHERE address=?', a.address);
      if (existing) {
        if (existing.agent_id !== p.agentId && !auth.admin) throw new RelayError('admin_required');
        if (existing.agent_id !== targetDevice || existing.harness !== a.harness || existing.session !== a.session) {
          throw new RelayError('inbox_binding_immutable', 409);
        }
      }
      await this.#addresses().release(a.address, owner);
      this.ctx.storage.transactionSync(() => {
        if (existing) {
          this.ctx.storage.sql.exec("UPDATE inbox_addresses SET status='removed', updated_at=? WHERE address=?", now, a.address);
          this.ctx.storage.sql.exec("UPDATE inbox_grants SET status='revoked', revoked_at=? WHERE address=? AND status='active'", now, a.address);
        }
        this.#touchSeen(p.agentId);
      });
      return { address: a.address, owner, deviceId: targetDevice, harness: a.harness, session: a.session, status: 'removed' as const };
    }
    this.#activeDevice(targetDevice);
    const existing = this.#row<InboxAddress>('SELECT * FROM inbox_addresses WHERE address=?', a.address);
    if (existing?.status === 'active' && (existing.agent_id !== targetDevice || existing.harness !== a.harness || existing.session !== a.session)) {
      throw new RelayError('inbox_binding_immutable', 409);
    }
    const claimed = await this.#addresses().claim(a.address, owner);
    if (!claimed.ok) throw new RelayError(claimed.error, claimed.status);
    this.ctx.storage.transactionSync(() => {
      const latest = this.#row<InboxAddress>('SELECT * FROM inbox_addresses WHERE address=?', a.address);
      if (latest?.status === 'active' && (latest.agent_id !== targetDevice || latest.harness !== a.harness || latest.session !== a.session)) {
        throw new RelayError('inbox_binding_immutable', 409);
      }
      this.ctx.storage.sql.exec(
        "INSERT INTO inbox_addresses(address,agent_id,harness,session,status,created_at,updated_at) VALUES(?,?,?,?,'active',?,?) ON CONFLICT(address) DO UPDATE SET agent_id=excluded.agent_id,harness=excluded.harness,session=excluded.session,status='active',updated_at=excluded.updated_at",
        a.address, targetDevice, a.harness, a.session, now, now,
      );
      this.#touchSeen(p.agentId);
    });
    return { address: a.address, owner, deviceId: targetDevice, harness: a.harness, session: a.session, status: 'active' as const };
  }

  async #resolveInbox(p: Principal, args: unknown) {
    this.#authorize(p);
    this.#activeDevice(p.agentId);
    const owner = namespace(p);
    const { address } = z.strictObject({ address: inboxAddress }).parse(args);
    const targetOwner = await this.#addresses().lookup(address);
    if (!targetOwner) throw new RelayError('inbox_not_allowed', 403);
    if (targetOwner === owner) {
      const inbox = this.#row<InboxAddress>('SELECT * FROM inbox_addresses WHERE address=?', address);
      if (!inbox || inbox.status !== 'active') throw new RelayError('inbox_not_allowed', 403);
      const device = this.#activeDevice(inbox.agent_id);
      this.#touchSeen(p.agentId);
      return { address, owner, device, harness: inbox.harness, session: inbox.session, grantId: null };
    }
    const resolved = await this.#ownerStub(targetOwner).resolveCrossOwnerInbox(owner, p.agentId, address);
    if (!resolved.ok) throw new RelayError(resolved.error, resolved.status);
    this.#touchSeen(p.agentId);
    return resolved.value;
  }

  #threadLocalMember(member: ThreadMember, other: ThreadMember): Device {
    if (member.owner !== this.#owner()) throw new RelayError('thread_owner_mismatch');
    const device = this.#activeDevice(member.agentId);
    validateDevice(member.device);
    if (
      device.owner !== member.owner || device.deviceId !== member.agentId
      || fingerprint(device) !== member.fingerprint || fingerprint(member.device) !== member.fingerprint
    ) throw new RelayError('device_key_changed');
    const inbox = this.#row<InboxAddress>('SELECT * FROM inbox_addresses WHERE address=?', member.address);
    if (
      !inbox || inbox.status !== 'active' || inbox.agent_id !== member.agentId
      || inbox.harness !== member.harness || inbox.session !== member.session
    ) throw new RelayError('thread_binding_changed');
    const { grant } = this.#activeGrant(member.grantId, other.owner, other.agentId, member.agentId);
    if (
      grant.address !== member.address || grant.harness !== member.harness
      || grant.session !== member.session
    ) throw new RelayError('thread_grant_changed');
    return device;
  }

  async #revalidateCreatorThreadMembers(members: [ThreadMember, ThreadMember]) {
    const localDevice = this.#threadLocalMember(members[0], members[1]);
    const remote = await this.#ownerStub(members[1].owner).verifyThreadAccess(members[0], members[1]);
    if (!remote.ok) throw new RelayError(remote.error, remote.status);
    return { localDevice, remoteDevice: remote.value };
  }

  // Internal owner-to-owner RPC only. Public HTTP operations never accept a Principal here.
  async verifyThreadAccess(creatorInput: unknown, peerInput: unknown): Promise<RpcResult<Device>> {
    try {
      const creator = threadMemberSchema.parse(creatorInput);
      const peer = threadMemberSchema.parse(peerInput);
      const owner = this.#owner();
      if (!owner || peer.owner !== owner || creator.owner === owner || creator.owner === peer.owner) {
        throw new RelayError('thread_owner_mismatch');
      }
      const device = this.#activeDevice(peer.agentId);
      validateDevice(peer.device);
      if (
        device.owner !== peer.owner || device.deviceId !== peer.agentId
        || fingerprint(device) !== peer.fingerprint || fingerprint(peer.device) !== peer.fingerprint
      ) throw new RelayError('device_key_changed');
      const { grant } = this.#activeGrant(peer.grantId, creator.owner, creator.agentId, peer.agentId);
      const inbox = this.#row<InboxAddress>('SELECT * FROM inbox_addresses WHERE address=?', peer.address);
      if (
        grant.address !== peer.address || grant.harness !== peer.harness || grant.session !== peer.session
        || !inbox || inbox.status !== 'active' || inbox.agent_id !== peer.agentId
        || inbox.harness !== peer.harness || inbox.session !== peer.session
      ) throw new RelayError('thread_binding_changed');
      return { ok: true, value: device };
    } catch (error) {
      if (error instanceof RelayError) return { ok: false, error: error.code, status: error.status };
      if (error instanceof ZodError) return { ok: false, error: 'invalid_request', status: 400 };
      throw error;
    }
  }

  // This RPC is called only by the authenticated member owner's relay after #authorize.
  async threadFromOwnerRelay(principalInput: Principal, opInput: Operation, args: unknown): Promise<RpcResult<unknown>> {
    try {
      const p = principalSchema.parse(principalInput);
      const owner = this.#owner();
      const op = z.enum(['threadShow', 'threadSend', 'threadRead']).parse(opInput);
      const { threadRef } = z.strictObject({ threadRef: threadRefSchema }).passthrough().parse(args);
      if (!owner || threadRef.creatorOwner !== owner || namespace(p) === owner || p.expiresAt <= Date.now()) {
        throw new RelayError('thread_owner_mismatch');
      }
      return { ok: true, value: await this.#threadOperation(p, op, args, true) };
    } catch (error) {
      if (error instanceof RelayError) return { ok: false, error: error.code, status: error.status };
      if (error instanceof ZodError) return { ok: false, error: 'invalid_request', status: 400 };
      throw error;
    }
  }

  async #threadCreate(p: Principal, args: unknown) {
    const owner = namespace(p);
    this.#authorize(p);
    this.#activeDevice(p.agentId);
    this.#rate(JSON.stringify(['agent', p.agentId]), 120, Date.now());
    this.#rate(JSON.stringify(['account', p.issuer, p.subject]), 120, Date.now());
    this.#rate('org', 600, Date.now());
    this.#rate(JSON.stringify(['thread-create', p.agentId]), 10, Date.now());
    const a = z.strictObject({
      fromAddress: inboxAddress,
      toAddress: inboxAddress,
      idempotencyKey: identifier,
    }).parse(args);
    const createDigest = digest(new TextEncoder().encode(JSON.stringify([a.fromAddress, a.toAddress])));
    const previous = this.#row<{ thread_id: string; create_digest: string; members: string; created_at: number }>(
      'SELECT thread_id,create_digest,members,created_at FROM threads WHERE creator_device=? AND create_key=?',
      p.agentId, a.idempotencyKey,
    );
    if (previous) {
      if (previous.create_digest !== createDigest) throw new RelayError('idempotency_conflict', 409);
      const members = z.tuple([threadMemberSchema, threadMemberSchema]).parse(JSON.parse(previous.members));
      if (
        members[0].owner !== owner || members[0].agentId !== p.agentId
        || members[0].address !== a.fromAddress || members[1].address !== a.toAddress
      ) throw new RelayError('thread_corrupt', 500);
      await this.#revalidateCreatorThreadMembers(members);
      return {
        threadRef: { creatorOwner: owner, threadId: previous.thread_id },
        members,
        createdAt: previous.created_at,
        idempotencyKey: a.idempotencyKey,
      };
    }
    const localDevice = this.#activeDevice(p.agentId);
    const localInbox = this.#row<InboxAddress>('SELECT * FROM inbox_addresses WHERE address=?', a.fromAddress);
    if (
      !localInbox || localInbox.status !== 'active' || localInbox.agent_id !== p.agentId
    ) throw new RelayError('creator_inbox_must_match_device');
    const targetOwner = await this.#addresses().lookup(a.toAddress);
    if (!targetOwner) throw new RelayError('inbox_not_allowed');
    if (targetOwner === owner) throw new RelayError('thread_members_must_be_distinct_owners', 400);
    const resolved = await this.#ownerStub(targetOwner).resolveCrossOwnerInbox(owner, p.agentId, a.toAddress);
    if (!resolved.ok) throw new RelayError(resolved.error, resolved.status);
    const remote = resolved.value;
    if (remote.owner !== targetOwner || remote.address !== a.toAddress || remote.device.owner !== targetOwner) {
      throw new RelayError('inbox_binding_changed');
    }
    const grant = this.#row<InboxGrant>(
      "SELECT * FROM inbox_grants WHERE address=? AND sender_owner=? AND sender_agent_id=? AND recipient_agent_id=? AND status='active' ORDER BY created_at DESC LIMIT 1",
      a.fromAddress, targetOwner, remote.device.deviceId, p.agentId,
    );
    if (!grant) throw new RelayError('reciprocal_grant_required');
    const checked = this.#activeGrant(grant.grant_id, targetOwner, remote.device.deviceId, p.agentId);
    if (
      checked.inbox.address !== a.fromAddress || checked.inbox.harness !== localInbox.harness
      || checked.inbox.session !== localInbox.session
    ) throw new RelayError('thread_binding_changed');
    const members: [ThreadMember, ThreadMember] = [
      threadMemberSchema.parse({
        address: a.fromAddress, owner, agentId: p.agentId, harness: localInbox.harness,
        session: localInbox.session, fingerprint: fingerprint(localDevice), grantId: grant.grant_id, device: localDevice,
      }),
      threadMemberSchema.parse({
        address: remote.address, owner: remote.owner, agentId: remote.device.deviceId,
        harness: remote.harness, session: remote.session, fingerprint: fingerprint(remote.device),
        grantId: remote.grantId, device: remote.device,
      }),
    ];
    const remoteCheck = await this.#ownerStub(targetOwner).verifyThreadAccess(members[0], members[1]);
    if (!remoteCheck.ok) throw new RelayError(remoteCheck.error, remoteCheck.status);
    const now = Date.now(), threadId = crypto.randomUUID();
    const result = this.ctx.storage.transactionSync(() => {
      const existing = this.#row<{ thread_id: string; create_digest: string; members: string; created_at: number }>(
        'SELECT thread_id,create_digest,members,created_at FROM threads WHERE creator_device=? AND create_key=?',
        p.agentId, a.idempotencyKey,
      );
      if (existing) {
        if (existing.create_digest !== createDigest) throw new RelayError('idempotency_conflict', 409);
        return {
          output: {
            threadRef: { creatorOwner: owner, threadId: existing.thread_id },
            members: JSON.parse(existing.members),
            createdAt: existing.created_at,
            idempotencyKey: a.idempotencyKey,
          },
          existingMembers: existing.members,
        };
      }
      this.#threadLocalMember(members[0], members[1]);
      this.ctx.storage.sql.exec(
        'INSERT INTO threads(thread_id,creator_device,create_key,create_digest,members,created_at) VALUES(?,?,?,?,?,?)',
        threadId, p.agentId, a.idempotencyKey, createDigest, JSON.stringify(members), now,
      );
      return {
        output: { threadRef: { creatorOwner: owner, threadId }, members, createdAt: now, idempotencyKey: a.idempotencyKey },
        existingMembers: null,
      };
    });
    if (result.existingMembers) {
      const members = z.tuple([threadMemberSchema, threadMemberSchema]).parse(JSON.parse(result.existingMembers));
      if (
        members[0].owner !== owner || members[0].agentId !== p.agentId
        || members[0].address !== a.fromAddress || members[1].address !== a.toAddress
      ) throw new RelayError('thread_corrupt', 500);
      await this.#revalidateCreatorThreadMembers(members);
    }
    return result.output;
  }

  async #threadOperation(p: Principal, op: 'threadShow' | 'threadSend' | 'threadRead', args: unknown, callerValidated = false) {
    if (!callerValidated) this.#authorize(p);
    const threadRef = threadRefSchema.parse((args as { threadRef?: unknown } | null)?.threadRef);
    if (op === 'threadShow') z.strictObject({ threadRef: threadRefSchema }).parse(args);
    if (threadRef.creatorOwner !== this.#owner()) throw new RelayError('thread_not_found', 404);
    const row = this.#row<{ members: string; created_at: number }>(
      'SELECT members,created_at FROM threads WHERE thread_id=?', threadRef.threadId,
    );
    if (!row) throw new RelayError('thread_not_found', 404);
    const members = z.tuple([threadMemberSchema, threadMemberSchema]).parse(JSON.parse(row.members));
    if (members[0].owner === members[1].owner || members[0].owner !== threadRef.creatorOwner) throw new RelayError('thread_corrupt', 500);
    const callerOwner = namespace(p);
    const callerIndex = members.findIndex(m => m.owner === callerOwner && m.agentId === p.agentId);
    if (callerIndex < 0) throw new RelayError('thread_member_required');
    if (callerValidated && callerOwner === this.#owner()) throw new RelayError('thread_owner_mismatch');
    const localIndex = members.findIndex(m => m.owner === this.#owner());
    const peerIndex = 1 - localIndex;
    const localMember = members[localIndex], peerMember = members[peerIndex];
    const now = Date.now();
    this.#rate(JSON.stringify(['agent', p.agentId]), 120, now);
    this.#rate(JSON.stringify(['account', p.issuer, p.subject]), 120, now);
    this.#rate('org', 600, now);
    this.#rate(JSON.stringify(['thread', op, callerOwner, p.agentId]), op === 'threadRead' ? 60 : 30, now);
    const localDevice = this.#threadLocalMember(localMember, peerMember);
    const peerCheck = await this.#ownerStub(peerMember.owner).verifyThreadAccess(localMember, peerMember);
    if (!peerCheck.ok) throw new RelayError(peerCheck.error, peerCheck.status);
    const callerMember = members[callerIndex];
    if (
      callerValidated && (callerMember.owner !== callerOwner || callerMember.agentId !== p.agentId)
    ) throw new RelayError('thread_member_required');

    if (op === 'threadShow') {
      return this.ctx.storage.transactionSync(() => {
        this.#threadLocalMember(localMember, peerMember);
        const latest = this.#row<{ last_seq: number | null }>(
          'SELECT MAX(seq) AS last_seq FROM thread_messages WHERE thread_id=?', threadRef.threadId,
        );
        return { threadRef, members, createdAt: row.created_at, lastSeq: latest?.last_seq ?? 0, state: 'active' };
      });
    }
    if (op === 'threadRead') {
      const a = z.strictObject({
        threadRef: threadRefSchema,
        afterSeq: z.number().int().safe().nonnegative().default(0),
        limit: z.number().int().min(1).max(32).default(20),
      }).parse(args);
      return this.ctx.storage.transactionSync(() => {
        this.#threadLocalMember(localMember, peerMember);
        const rows = this.ctx.storage.sql.exec<{ seq: number; post: string; created_at: number }>(
          'SELECT seq,post,created_at FROM thread_messages WHERE thread_id=? AND seq>? ORDER BY seq LIMIT ?',
          threadRef.threadId, a.afterSeq, a.limit + 1,
        ).toArray();
        const hasMore = rows.length > a.limit;
        const visible = rows.slice(0, a.limit).map(message => {
          const post = threadPostSchema.parse(JSON.parse(message.post));
          const copy = post.copies.find(candidate => {
            const recipient = candidate.recipient;
            return recipient.owner === callerMember.owner && recipient.agentId === callerMember.agentId;
          });
          if (!copy) throw new RelayError('thread_corrupt', 500);
          return { seq: message.seq, context: post.context, copy, pairSignature: post.pairSignature, createdAt: message.created_at };
        });
        return {
          threadRef, members, messages: visible,
          nextAfterSeq: visible.length ? visible[visible.length - 1].seq : a.afterSeq,
          hasMore,
        };
      });
    }

    const a = z.strictObject({ threadRef: threadRefSchema, post: threadPostSchema }).parse(args);
    const post = a.post;
    const requestBytes = new TextEncoder().encode(JSON.stringify(args)).length;
    if (requestBytes > THREAD_MAX_REQUEST_BYTES) throw new RelayError('thread_request_too_large', 413);
    const author = members.find(m => m.owner === callerOwner && m.agentId === p.agentId);
    if (!author || post.context.creatorOwner !== threadRef.creatorOwner || post.context.threadId !== threadRef.threadId) {
      throw new RelayError('thread_member_required');
    }
    if (
      post.context.authorOwner !== author.owner || post.context.authorDeviceId !== author.agentId
      || post.context.authorFingerprint !== author.fingerprint
    ) throw new RelayError('thread_author_mismatch');
    for (let i = 0; i < 2; i++) {
      const expected = members[i], copy = post.copies[i];
      if (
        copy.recipient.address !== expected.address || copy.recipient.owner !== expected.owner
        || copy.recipient.agentId !== expected.agentId || copy.recipient.harness !== expected.harness
        || copy.recipient.session !== expected.session || copy.recipient.fingerprint !== expected.fingerprint
        || copy.recipient.grantId !== expected.grantId
      ) throw new RelayError('thread_copy_routing_mismatch');
      validateDevice(copy.recipient.device);
      if (
        copy.recipient.device.owner !== expected.owner
        || copy.recipient.device.deviceId !== expected.agentId
        || fingerprint(copy.recipient.device) !== expected.fingerprint
      ) throw new RelayError('thread_copy_routing_mismatch');
    }
    const authorDevice = author.owner === this.#owner() ? localDevice : peerCheck.value;
    if (fingerprint(authorDevice) !== author.fingerprint) throw new RelayError('device_key_changed');
    for (const copy of post.copies) verify(authorDevice.signingKey, copy.signature, threadCopyBytes(post.context, copy));
    verify(authorDevice.signingKey, post.pairSignature, threadPairBytes(post.context, post.copies));
    const requestDigest = digest(new TextEncoder().encode(JSON.stringify(post)));
    return this.ctx.storage.transactionSync(() => {
      this.#threadLocalMember(localMember, peerMember);
      const previous = this.#row<{ seq: number; message_id: string; digest: string }>(
        'SELECT seq,message_id,digest FROM thread_messages WHERE thread_id=? AND author_owner=? AND author_device=? AND idempotency_key=?',
        threadRef.threadId, author.owner, author.agentId, post.context.idempotencyKey,
      );
      if (previous) {
        if (previous.digest !== requestDigest || previous.message_id !== post.context.messageId) {
          throw new RelayError('idempotency_conflict', 409);
        }
        return { state: 'stored' as const, threadRef, seq: previous.seq, messageId: previous.message_id, idempotencyKey: post.context.idempotencyKey };
      }
      if (this.#row('SELECT message_id FROM thread_messages WHERE message_id=?', post.context.messageId)) {
        throw new RelayError('message_id_conflict', 409);
      }
      const seq = (this.#row<{ last_seq: number | null }>(
        'SELECT MAX(seq) AS last_seq FROM thread_messages WHERE thread_id=?', threadRef.threadId,
      )?.last_seq ?? 0) + 1;
      this.ctx.storage.sql.exec(
        'INSERT INTO thread_messages(thread_id,seq,message_id,author_owner,author_device,idempotency_key,digest,post,created_at) VALUES(?,?,?,?,?,?,?,?,?)',
        threadRef.threadId, seq, post.context.messageId, author.owner, author.agentId, post.context.idempotencyKey,
        requestDigest, JSON.stringify(post), now,
      );
      return { state: 'stored' as const, threadRef, seq, messageId: post.context.messageId, idempotencyKey: post.context.idempotencyKey };
    });
  }

  async #enqueueCrossOwner(p: Principal, e: Envelope) {
    this.#authorize(p);
    const now = Date.now(), owner = namespace(p);
    this.#own(p, e.fromDevice);
    if (e.owner !== owner) throw new RelayError('wrong_owner');
    if (e.grantId === null) throw new RelayError('cross_owner_not_supported');
    const sender = this.#activeDevice(e.fromDevice);
    if (fingerprint(sender) !== e.senderFingerprint) throw new RelayError('device_key_changed');
    verify(sender.signingKey, e.signature, envelopeBytes(e));
    const requestDigest = digest(envelopeBytes(e));
    const reserved = this.ctx.storage.transactionSync(() => {
      const old = this.#row<Attempt>('SELECT * FROM attempts WHERE from_device=? AND idempotency_key=?', p.agentId, e.idempotencyKey);
      if (old) {
        if (old.digest !== requestDigest) throw new RelayError('idempotency_conflict', 409);
        if (e.parent !== null) this.#activeGrant(e.grantId!, e.recipientOwner, e.toDevice, e.fromDevice);
        return { existing: old, created: false };
      }
      if (e.expiresAt <= now || Math.abs(e.createdAt - now) > 300000 || e.expiresAt <= e.createdAt || e.expiresAt - e.createdAt > 86400000) throw new RelayError('invalid_ttl', 400);
      if (this.#attempt(e.messageId)) throw new RelayError('message_id_reused', 409);
      if (e.parent === null) {
        if (e.hop !== 1) throw new RelayError('invalid_root_hop', 400);
      } else {
        const parent = this.#attempt(e.parent.messageId);
        const parentMeta: Metadata | undefined = parent && JSON.parse(parent.metadata);
        if (
          !parent
          || !parentMeta
          || parentMeta.parent !== null
          || parentMeta.owner !== e.parent.owner
          || parentMeta.owner !== e.recipientOwner
          || parentMeta.toDevice !== p.agentId
          || parentMeta.fromDevice !== e.toDevice
          || parentMeta.grantId !== e.grantId
          || parentMeta.senderFingerprint !== e.recipientFingerprint
          || !['delivered', 'claimed', 'replied'].includes(parent.state)
          || e.hop <= parentMeta.hop
          || e.hop > CROSS_OWNER_MAX_HOPS
        ) throw new RelayError('invalid_parent_lineage');
        this.#activeGrant(e.grantId!, e.recipientOwner, e.toDevice, e.fromDevice);
      }
      this.#rate(JSON.stringify(['agent', p.agentId]), 120, now);
      this.#rate(JSON.stringify(['account', p.issuer, p.subject]), 120, now);
      this.#rate('org', 600, now);
      const { ciphertext, signature, ephemeralKey, nonce, createdAt, ...metadata } = e;
      this.ctx.storage.sql.exec(
        'INSERT INTO attempts VALUES(?,?,?,?,?,?,?,?)',
        e.messageId, p.agentId, e.idempotencyKey, requestDigest, 'queued', e.expiresAt, e.toDevice, JSON.stringify({ ...metadata, queuedAt: now }),
      );
      return { existing: null as Attempt | null, created: true };
    });
    if (reserved.existing && this.#state(reserved.existing, now) !== 'queued') {
      const check = await this.#ownerStub(e.recipientOwner).verifyCrossOwnerAccess(e);
      if (!check.ok) throw new RelayError(check.error, check.status);
      this.#touchSeen(p.agentId);
      return { messageId: reserved.existing.message_id, state: this.#state(reserved.existing, now) };
    }
    const delivered = await this.#ownerStub(e.recipientOwner).receiveCrossOwnerEnvelope(e, sender);
    if (!delivered.ok) {
      if (reserved.created) {
        this.ctx.storage.sql.exec("DELETE FROM attempts WHERE message_id=? AND state='queued'", e.messageId);
      }
      throw new RelayError(delivered.error, delivered.status);
    }
    return this.ctx.storage.transactionSync(() => {
      const current = this.#attempt(e.messageId);
      if (current && stateRank[delivered.value.state] > stateRank[current.state]) {
        this.ctx.storage.sql.exec('UPDATE attempts SET state=? WHERE message_id=?', delivered.value.state, e.messageId);
      }
      this.#touchSeen(p.agentId);
      const finalState = current && stateRank[current.state] > stateRank[delivered.value.state]
        ? this.#state(current, now)
        : delivered.value.state;
      return { messageId: delivered.value.messageId, state: finalState };
    });
  }

  async #pending(p: Principal, args: unknown) {
    this.#authorize(p);
    const now = Date.now(), owner = namespace(p);
    const { deviceId, cursor } = z.strictObject({ deviceId: identifier, cursor: z.string().regex(/^(0|[1-9][0-9]*)$/).max(16).nullable().optional() }).parse(args);
    this.#own(p, deviceId); this.#activeDevice(p.agentId);
    const after = cursor == null ? 0 : Number(cursor);
    if (!Number.isSafeInteger(after)) throw new RelayError('invalid_cursor', 400);
    const rows = this.ctx.storage.sql.exec<{ seq: number; envelope: string }>('SELECT seq,envelope FROM messages WHERE to_device=? AND seq>? ORDER BY seq LIMIT 33', p.agentId, after).toArray();
    const envelopes: unknown[] = [];
    for (const row of rows.slice(0, 32)) {
      const envelope: Envelope = JSON.parse(row.envelope);
      if (envelope.expiresAt <= now) continue;
      if (envelope.owner === owner) {
        try { this.#route(envelope); envelopes.push({ envelope, trust: 'same-org' }); }
        catch (error) { if (!(error instanceof RelayError)) throw error; }
        continue;
      }
      try {
        if (envelope.recipientOwner !== owner || envelope.grantId === null) continue;
        const recipient = this.#activeDevice(envelope.toDevice);
        if (fingerprint(recipient) !== envelope.recipientFingerprint) continue;
        if (envelope.parent === null) {
          const { grant } = this.#activeGrant(envelope.grantId, envelope.owner, envelope.fromDevice, envelope.toDevice);
          const senderCheck = await this.#ownerStub(envelope.owner).verifyActiveDevice(envelope.fromDevice, envelope.senderFingerprint);
          if (!senderCheck.ok) continue;
          envelopes.push({
            envelope,
            trust: 'external-untrusted',
            inbox: { address: grant.address, harness: grant.harness, session: grant.session },
            senderDevice: senderCheck.value,
          });
        } else {
          const orig = this.#attempt(envelope.parent.messageId);
          const origMeta: Metadata | undefined = orig && JSON.parse(orig.metadata);
          this.#assertReplyCorrelation(orig, origMeta, envelope, owner);
          const senderCheck = await this.#ownerStub(envelope.owner).verifyCrossOwnerReplySender(
            envelope.grantId, envelope.fromDevice, owner, envelope.toDevice, envelope.senderFingerprint,
          );
          if (!senderCheck.ok) continue;
          envelopes.push({ envelope, trust: 'external-untrusted', senderDevice: senderCheck.value });
        }
      } catch (error) {
        if (!(error instanceof RelayError)) throw error;
      }
    }
    this.#touchSeen(p.agentId);
    return { serverNow: now, envelopes, isDone: rows.length <= 32, continueCursor: String(rows.slice(0, 32).at(-1)?.seq ?? after) };
  }

  async #observeCrossOwner(p: Principal, o: Observation) {
    this.#authorize(p);
    const now = Date.now(), owner = namespace(p);
    this.#own(p, o.deviceId);
    const recipient = this.#activeDevice(p.agentId);
    if (Math.abs(now - o.at) > 300000) throw new RelayError('stale_observation', 400);
    verify(recipient.signingKey, o.signature, observationBytes(owner, o));
    const a = this.#attempt(o.messageId), e: Metadata | undefined = a && JSON.parse(a.metadata);
    if (!a || !e || e.toDevice !== p.agentId || e.owner !== o.senderOwner || e.recipientOwner !== owner || e.grantId === null) {
      throw new RelayError('message_not_for_agent');
    }
    if (fingerprint(recipient) !== e.recipientFingerprint) throw new RelayError('device_key_changed');
    if (e.parent === null) {
      this.#activeGrant(e.grantId, e.owner, e.fromDevice, e.toDevice);
    } else {
      const orig = this.#attempt(e.parent.messageId);
      const origMeta: Metadata | undefined = orig && JSON.parse(orig.metadata);
      this.#assertReplyCorrelation(orig, origMeta, e, owner);
    }
    if (this.#state(a, now) === 'expired') throw new RelayError('message_expired');
    if (a.state === 'queued' && o.state !== 'delivered') throw new RelayError('delivery_required');
    const recorded = await this.#ownerStub(o.senderOwner).verifyAndRecordCrossOwnerObservation(owner, o, recipient);
    if (!recorded.ok) throw new RelayError(recorded.error, recorded.status);
    this.ctx.storage.transactionSync(() => {
      const latest = this.#attempt(o.messageId);
      const meta: Metadata | undefined = latest && JSON.parse(latest.metadata);
      if (!latest || !meta) throw new RelayError('message_not_for_agent');
      const state = stateRank[o.state] > stateRank[latest.state] ? o.state : latest.state;
      const newClaim = o.state === 'claimed' && meta.claimedAt === undefined;
      const newReply = o.state === 'replied' && meta.repliedAt === undefined;
      if (newClaim) meta.claimedAt = o.at;
      if (newReply) meta.repliedAt = o.at;
      if (state !== latest.state || newClaim || newReply) {
        this.ctx.storage.sql.exec('UPDATE attempts SET state=?, metadata=? WHERE message_id=?', state, JSON.stringify(meta), o.messageId);
      }
      if (state !== latest.state) {
        this.ctx.storage.sql.exec('INSERT OR IGNORE INTO mesh_receipts(message_id,device_id,state,at) VALUES(?,?,?,?)', o.messageId, o.deviceId, o.state, o.at);
        this.ctx.storage.sql.exec('DELETE FROM mesh_receipts WHERE seq <= (SELECT max(seq)-2000 FROM mesh_receipts)');
      }
      this.ctx.storage.sql.exec('DELETE FROM messages WHERE message_id=?', o.messageId);
      this.#touchSeen(p.agentId);
    });
    await this.#scheduleExpiry();
    return null;
  }

  async #status(p: Principal, args: unknown) {
    this.#authorize(p);
    const now = Date.now(), owner = namespace(p);
    const { fromDevice, messageId } = z.strictObject({ fromDevice: identifier, messageId: identifier }).parse(args);
    this.#own(p, fromDevice);
    const sender = this.#activeDevice(p.agentId);
    const a = this.#attempt(messageId);
    if (!a || a.from_device !== p.agentId) throw new RelayError('message_not_for_agent');
    const e: Metadata = JSON.parse(a.metadata);
    if (e.recipientOwner === owner) {
      this.#route(e);
    } else {
      if (e.grantId === null) throw new RelayError('cross_owner_not_supported');
      if (fingerprint(sender) !== e.senderFingerprint) throw new RelayError('device_key_changed');
      if (e.parent !== null) this.#activeGrant(e.grantId, e.recipientOwner, e.toDevice, e.fromDevice);
      const check = await this.#ownerStub(e.recipientOwner).verifyCrossOwnerAccess(e);
      if (!check.ok) throw new RelayError(check.error, check.status);
    }
    this.#touchSeen(p.agentId);
    return { state: this.#state(a, now), queuedAt: e.queuedAt ?? null, claimedAt: e.claimedAt ?? null, repliedAt: e.repliedAt ?? null };
  }

  async relay(principalInput: Principal, op: Operation, args: unknown) {
    try {
      const p = principalSchema.parse(principalInput);
      const owner = namespace(p);
      if (op === 'threadCreate') return { ok: true, value: await this.#threadCreate(p, args) } satisfies RelayResult;
      if (op === 'threadShow' || op === 'threadSend' || op === 'threadRead') {
        const { threadRef } = z.strictObject({ threadRef: threadRefSchema }).passthrough().parse(args);
        if (threadRef.creatorOwner === owner) {
          return { ok: true, value: await this.#threadOperation(p, op, args) } satisfies RelayResult;
        }
        this.#authorize(p);
        this.#activeDevice(p.agentId);
        const result = await this.#ownerStub(threadRef.creatorOwner).threadFromOwnerRelay(p, op, args);
        const response = 'value' in result
          ? { ok: true as const, value: result.value }
          : { ok: false as const, error: result.error, status: result.status };
        result[Symbol.dispose]();
        return response;
      }
      if (op === 'registerInbox') return { ok: true, value: await this.#registerInbox(p, args) } satisfies RelayResult;
      if (op === 'resolveInbox') return { ok: true, value: await this.#resolveInbox(p, args) } satisfies RelayResult;
      if (op === 'pending') return { ok: true, value: await this.#pending(p, args) } satisfies RelayResult;
      if (op === 'status') return { ok: true, value: await this.#status(p, args) } satisfies RelayResult;
      if (op === 'enqueue') {
        const { envelope: e } = z.strictObject({ envelope: envelopeSchema }).parse(args);
        if (e.recipientOwner !== owner) return { ok: true, value: await this.#enqueueCrossOwner(p, e) } satisfies RelayResult;
      }
      if (op === 'accept' || op === 'observe') {
        const o = op === 'accept'
          ? observationSchema.parse({ ...z.strictObject(observationSchema.omit({ state: true }).shape).parse(args), state: 'delivered' })
          : observationSchema.parse(args);
        if (o.senderOwner !== owner) return { ok: true, value: await this.#observeCrossOwner(p, o) } satisfies RelayResult;
      }
      const value = await this.ctx.storage.transaction(async () => {
        const result = this.#execute(p, op, args);
        this.#touchSeen(p.agentId);
        if (op === 'enqueue' || op === 'accept' || op === 'observe') await this.#scheduleExpiry();
        return result;
      });
      if (op === 'enqueue') {
        const queued = z.object({ messageId: identifier }).parse(value);
        const attempt = this.#attempt(queued.messageId);
        if (attempt?.state === 'queued' && attempt.expires_at > Date.now()) this.#notify(attempt.to_device);
      }
      if (op === 'revokeAgent' || op === 'revokeMember' || op === 'rotate') this.#closeInvalidWatchers();
      return { ok: true, value } satisfies RelayResult;
    } catch (error) {
      if (error instanceof RelayError) return { ok: false, error: error.code, status: error.status } satisfies RelayResult;
      if (error instanceof ZodError) return { ok: false, error: 'invalid_request', status: 400 } satisfies RelayResult;
      throw error;
    }
  }

  #execute(p: Principal, op: Operation, args: unknown) {
    const auth = this.#authorize(p), now = Date.now(), owner = namespace(p);
    switch (op) {
      case 'exposeSession': {
        this.#activeDevice(p.agentId);
        const a = z.strictObject({ session: identifier, harness: identifier, card: z.string().max(640), remove: z.boolean().optional() }).parse(args);
        const old = this.#row<{ harness: string }>('SELECT harness FROM session_cards WHERE agent_id=? AND session=?', p.agentId, a.session);
        // Older CLI clients send "unknown" for an unqualified removal by the owning device.
        if (old && old.harness !== a.harness && !(a.remove && a.harness === 'unknown')) throw new RelayError('session_harness_conflict', 409);
        if (a.remove) { this.ctx.storage.sql.exec('DELETE FROM session_cards WHERE agent_id=? AND session=?', p.agentId, a.session); return null; }
        addressPart.parse(a.session);
        addressPart.parse(a.harness);
        const count = this.#row<{ n: number }>('SELECT COUNT(*) AS n FROM session_cards WHERE agent_id=?', p.agentId)!.n;
        if (!old && count >= 32) throw new RelayError('session_limit', 409);
        this.ctx.storage.sql.exec('INSERT INTO session_cards(agent_id,session,harness,card,updated_at) VALUES(?,?,?,?,?) ON CONFLICT(agent_id,session) DO UPDATE SET card=excluded.card,updated_at=excluded.updated_at', p.agentId, a.session, a.harness, a.card, now);
        return null;
      }
      case 'sessionDirectory': {
        this.#activeDevice(p.agentId);
        const { cursor } = z.strictObject({ cursor: z.string().regex(/^[0-9]{1,16}$/).nullable().optional() }).parse(args);
        const rows = this.ctx.storage.sql.exec<{ seq: number; agent_id: string; session: string; harness: string; card: string; updated_at: number }>('SELECT * FROM session_cards WHERE seq>? ORDER BY seq LIMIT 33', Number(cursor ?? 0)).toArray();
        const sessions = [];
        for (const row of rows.slice(0, 32)) {
          if (!addressPart.safeParse(row.session).success || !addressPart.safeParse(row.harness).success) continue;
          try { this.#activeDevice(row.agent_id); }
          catch (error) { if (error instanceof RelayError) continue; throw error; }
          const credential = this.#row<Credential>('SELECT * FROM credentials WHERE agent_id=?', row.agent_id)!;
          sessions.push({ owner, author: credential.subject, agent: row.agent_id, session: row.session,
            harness: row.harness, card: row.card, updated_at: row.updated_at,
            target: `id:${row.agent_id}/${row.harness}/${row.session}` });
        }
        return { sessions, isDone: rows.length <= 32, continueCursor: rows.length ? String(rows[Math.min(rows.length, 32) - 1].seq) : cursor ?? '' };
      }
      case 'grantInbox': {
        this.#activeDevice(p.agentId);
        const a = z.strictObject({
          address: inboxAddress,
          senderOwner: z.string().min(1).max(4096),
          senderDeviceId: identifier,
        }).parse(args);
        if (a.senderOwner === owner) throw new RelayError('same_owner_grant_not_needed', 400);
        const inbox = this.#row<InboxAddress>('SELECT * FROM inbox_addresses WHERE address=?', a.address);
        if (!inbox || inbox.status !== 'active') throw new RelayError('unknown_inbox', 404);
        if (inbox.agent_id !== p.agentId && !auth.admin) throw new RelayError('admin_required');
        this.#activeDevice(inbox.agent_id);
        const existing = this.#row<InboxGrant>(
          "SELECT * FROM inbox_grants WHERE address=? AND recipient_agent_id=? AND harness=? AND session=? AND sender_owner=? AND sender_agent_id=? AND status='active'",
          a.address, inbox.agent_id, inbox.harness, inbox.session, a.senderOwner, a.senderDeviceId,
        );
        const grantId = existing?.grant_id ?? `grant-${crypto.randomUUID().replaceAll('-', '')}`;
        if (!existing) {
          this.ctx.storage.sql.exec(
            "INSERT INTO inbox_grants(grant_id,address,recipient_agent_id,harness,session,sender_owner,sender_agent_id,status,created_at,revoked_at) VALUES(?,?,?,?,?,?,?,'active',?,NULL)",
            grantId, a.address, inbox.agent_id, inbox.harness, inbox.session, a.senderOwner, a.senderDeviceId, now,
          );
        }
        return {
          grantId,
          address: a.address,
          owner,
          deviceId: inbox.agent_id,
          harness: inbox.harness,
          session: inbox.session,
          senderOwner: a.senderOwner,
          senderDeviceId: a.senderDeviceId,
          status: 'active' as const,
        };
      }
      case 'revokeInboxGrant': {
        this.#activeDevice(p.agentId);
        const { grantId } = z.strictObject({ grantId: identifier }).parse(args);
        const grant = this.#row<InboxGrant>('SELECT * FROM inbox_grants WHERE grant_id=?', grantId);
        if (!grant) throw new RelayError('unknown_grant');
        if (grant.recipient_agent_id !== p.agentId && !auth.admin) throw new RelayError('admin_required');
        if (grant.status !== 'revoked') {
          this.ctx.storage.sql.exec("UPDATE inbox_grants SET status='revoked', revoked_at=? WHERE grant_id=?", now, grantId);
        }
        return { grantId, status: 'revoked' as const };
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
        this.ctx.storage.sql.exec('INSERT INTO attempts VALUES(?,?,?,?,?,?,?,?)', e.messageId, p.agentId, e.idempotencyKey, requestDigest, 'queued', e.expiresAt, e.toDevice, JSON.stringify({ ...metadata, queuedAt: now }));
        this.ctx.storage.sql.exec('INSERT INTO messages(message_id,to_device,expires_at,envelope) VALUES(?,?,?,?)', e.messageId, e.toDevice, e.expiresAt, JSON.stringify(e));
        return { messageId: e.messageId, state: 'queued' };
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
        const state = stateRank[o.state] > stateRank[a.state] ? o.state : a.state;
        const newClaim = o.state === 'claimed' && e.claimedAt === undefined;
        const newReply = o.state === 'replied' && e.repliedAt === undefined;
        if (newClaim) e.claimedAt = o.at;
        if (newReply) e.repliedAt = o.at;
        if (state !== a.state || newClaim || newReply) {
          this.ctx.storage.sql.exec('UPDATE attempts SET state=?, metadata=? WHERE message_id=?', state, JSON.stringify(e), o.messageId);
        }
        if (state !== a.state) {
          this.ctx.storage.sql.exec('INSERT OR IGNORE INTO mesh_receipts(message_id,device_id,state,at) VALUES(?,?,?,?)', o.messageId, o.deviceId, o.state, o.at);
          this.ctx.storage.sql.exec('DELETE FROM mesh_receipts WHERE seq <= (SELECT max(seq)-2000 FROM mesh_receipts)');
        }
        this.ctx.storage.sql.exec('DELETE FROM messages WHERE message_id=?', o.messageId);
        return null;
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
