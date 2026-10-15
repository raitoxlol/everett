import http from 'node:http';
import { generateKeyPairSync, randomBytes } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import Provider from 'oidc-provider';
import { accounts, clientId, issuer, state } from './settings.mjs';

process.umask(0o077);
fs.mkdirSync(state, { recursive: true });
const keyFile = path.join(state, 'issuer-key.json');
if (!fs.existsSync(keyFile)) {
  const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const key = { ...privateKey.export({ format: 'jwk' }), kid: randomBytes(16).toString('hex'), use: 'sig', alg: 'RS256' };
  fs.writeFileSync(keyFile, JSON.stringify(key), { flag: 'wx', mode: 0o600 });
}
const provider = new Provider(issuer, {
  jwks: { keys: [JSON.parse(fs.readFileSync(keyFile))] },
  clients: [{ client_id: clientId, token_endpoint_auth_method: 'none', grant_types: ['urn:ietf:params:oauth:grant-type:device_code', 'refresh_token'], response_types: [], redirect_uris: [] }],
  features: { devInteractions: { enabled: false }, deviceFlow: { enabled: true }, revocation: { enabled: true } },
  claims: { openid: ['sub', 'everett_agent', 'everett_org', 'everett_role'], email: ['email'], profile: ['name'] },
  findAccount: async (_ctx, id) => accounts[id] && { accountId: id, claims: async () => accounts[id] },
  issueRefreshToken: async () => true,
  expiresWithSession: async () => false,
  ttl: { AccessToken: 300, IdToken: 300, DeviceCode: 300, Grant: 86400, RefreshToken: 86400 },
  cookies: { keys: [randomBytes(32).toString('hex')] },
});

const form = `<!doctype html><html><head><title>Everett local device approval</title></head><body>
<h1>Approve a local Everett device</h1><p>Development only. Choose the identity for this device.</p>
<form method="post" action="/approve"><label>Device code <input name="user_code" required></label>
<label>Identity <select name="account"><option>alice-a</option><option>alice-b</option></select></label>
<button>Approve</button></form></body></html>`;

async function approve(userCode, account) {
  if (!Object.hasOwn(accounts, account)) throw new Error('Choose alice-a or alice-b');
  const code = await provider.DeviceCode.findByUserCode(userCode.replace(/[-\s]/g, '').toUpperCase());
  if (!code || code.isExpired || code.consumed || code.accountId) throw new Error('Invalid, expired, or already approved code');
  const grant = new provider.Grant({ accountId: account, clientId });
  grant.addOIDCScope('openid email profile offline_access');
  Object.assign(code, { accountId: account, grantId: await grant.save(), scope: 'openid email profile offline_access', authTime: Math.floor(Date.now() / 1000) });
  await code.save();
}

const callback = provider.callback();
http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url, issuer);
    if (url.pathname === '/health') return res.end('everett-dev-login');
    if ((url.pathname === '/device' || url.pathname === '/approve') && req.method === 'GET') {
      res.setHeader('Content-Type', 'text/html; charset=utf-8');
      const userCode = (url.searchParams.get('user_code') ?? '').replace(/[^A-Z0-9-]/gi, '').slice(0, 20);
      return res.end(form.replace('name="user_code"', `name="user_code" value="${userCode}"`));
    }
    if (url.pathname === '/approve' && req.method === 'POST') {
      if (req.headers.origin && req.headers.origin !== issuer) throw new Error('Foreign origin denied');
      let body = '';
      for await (const chunk of req) {
        body += chunk;
        if (body.length > 2048) throw new Error('Body too large');
      }
      const params = new URLSearchParams(body);
      await approve(params.get('user_code') ?? '', params.get('account'));
      return res.end('Approved. Return to the Everett terminal.\n');
    }
    callback(req, res);
  } catch (error) {
    res.writeHead(400, { 'Content-Type': 'text/plain' });
    res.end(`${error.message}\n`);
  }
}).listen(4444, '127.0.0.1', () => console.log(`Local OIDC issuer ${issuer}`));
