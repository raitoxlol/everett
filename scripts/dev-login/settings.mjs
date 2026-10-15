import { fileURLToPath } from 'node:url';
import path from 'node:path';

export const directory = path.dirname(fileURLToPath(import.meta.url));
export const root = path.resolve(directory, '../..');
export const state = path.join(directory, '.state');
export const issuer = 'http://127.0.0.1:4444';
export const clientId = 'everett-relay';
export const backend = 'http://127.0.0.1:3210';
export const relay = 'https://127.0.0.1:4445';
export const accounts = {
  'alice-a': { sub: 'alice-a', email: 'alice-a@localhost', name: 'Alice A', everett_agent: 'alice-a', everett_org: 'local-alice', everett_role: 'admin' },
  'alice-b': { sub: 'alice-b', email: 'alice-b@localhost', name: 'Alice B', everett_agent: 'alice-b', everett_org: 'local-alice', everett_role: 'member' },
};
