import { DurableObject } from 'cloudflare:workers';
import { z, ZodError } from 'zod';
import { inboxAddress, RelayError } from './protocol';

type ClaimResult = { ok: true } | { ok: false; error: string; status: number };

const ownerSchema = z.string().min(1).max(4096);

export class AddressDirectory extends DurableObject {
  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    ctx.storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS addresses (
        address TEXT PRIMARY KEY,
        owner TEXT NOT NULL,
        status TEXT NOT NULL,
        updated_at INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS zones (
        zone TEXT PRIMARY KEY,
        owner TEXT NOT NULL,
        status TEXT NOT NULL,
        updated_at INTEGER NOT NULL
      );
      INSERT OR IGNORE INTO zones(zone, owner, status, updated_at)
        SELECT substr(address, instr(address, '@') + 1), owner, 'active', updated_at
        FROM addresses WHERE status='active' ORDER BY updated_at;
      INSERT OR IGNORE INTO zones(zone, owner, status, updated_at)
        SELECT substr(address, instr(address, '@') + 1), owner, 'removed', updated_at
        FROM addresses ORDER BY updated_at;
    `);
  }

  #row<T extends Record<string, SqlStorageValue>>(sql: string, ...args: SqlStorageValue[]) {
    return this.ctx.storage.sql.exec<T>(sql, ...args).toArray()[0];
  }

  async claim(addressInput: string, ownerInput: string): Promise<ClaimResult> {
    try {
      const address = inboxAddress.parse(addressInput);
      const owner = ownerSchema.parse(ownerInput);
      const now = Date.now();
      this.ctx.storage.transactionSync(() => {
        const existing = this.#row<{ owner: string; status: string }>('SELECT owner, status FROM addresses WHERE address=?', address);
        if (existing && existing.owner !== owner) throw new RelayError('address_taken', 409);
        const zone = address.slice(address.indexOf('@') + 1);
        const zoneRow = this.#row<{ owner: string }>('SELECT owner FROM zones WHERE zone=?', zone);
        if (zoneRow && zoneRow.owner !== owner) throw new RelayError('zone_taken', 409);
        this.ctx.storage.sql.exec(
          'INSERT INTO addresses(address, owner, status, updated_at) VALUES(?,?,?,?) ON CONFLICT(address) DO UPDATE SET status=excluded.status, updated_at=excluded.updated_at',
          address, owner, 'active', now,
        );
        this.ctx.storage.sql.exec(
          "INSERT INTO zones(zone, owner, status, updated_at) VALUES(?,?,?,?) ON CONFLICT(zone) DO UPDATE SET status='active', updated_at=excluded.updated_at",
          zone, owner, 'active', now,
        );
      });
      return { ok: true };
    } catch (error) {
      if (error instanceof RelayError) return { ok: false, error: error.code, status: error.status };
      if (error instanceof ZodError) return { ok: false, error: 'invalid_request', status: 400 };
      throw error;
    }
  }

  async release(addressInput: string, ownerInput: string): Promise<boolean> {
    const address = inboxAddress.safeParse(addressInput);
    const owner = ownerSchema.safeParse(ownerInput);
    if (!address.success || !owner.success) return false;
    const existing = this.#row<{ owner: string }>('SELECT owner FROM addresses WHERE address=?', address.data);
    if (!existing || existing.owner !== owner.data) return false;
    const now = Date.now();
    this.ctx.storage.transactionSync(() => {
      this.ctx.storage.sql.exec("UPDATE addresses SET status='removed', updated_at=? WHERE address=?", now, address.data);
      const zone = address.data.slice(address.data.indexOf('@') + 1);
      const remaining = this.#row<{ address: string }>(
        "SELECT address FROM addresses WHERE owner=? AND status='active' AND substr(address, instr(address, '@') + 1)=?",
        owner.data, zone,
      );
      if (!remaining) this.ctx.storage.sql.exec("UPDATE zones SET status='removed', updated_at=? WHERE zone=? AND owner=?", now, zone, owner.data);
    });
    return true;
  }

  async lookup(addressInput: string): Promise<string | null> {
    const address = inboxAddress.safeParse(addressInput);
    if (!address.success) return null;
    const row = this.#row<{ owner: string; status: string }>('SELECT owner, status FROM addresses WHERE address=?', address.data);
    return row?.status === 'active' ? row.owner : null;
  }
}
