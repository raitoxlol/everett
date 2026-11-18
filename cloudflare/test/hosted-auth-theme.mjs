// Score the theme in HTML served by the real Worker with synthetic providers.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { join } from 'node:path';

const socket = createServer().listen(0, '127.0.0.1');
await once(socket, 'listening');
const { port } = socket.address();
await new Promise((resolve, reject) => socket.close(error => error ? reject(error) : resolve()));
const issuer = `http://127.0.0.1:${port}`;
const clientId = 'https://client.everett.test/client.json';
const redirectUri = 'https://client.everett.test/callback';
const resource = 'http://127.0.0.1:18840/mcp';
const worker = spawn(process.execPath, ['--input-type=module', '-e', `
  import { serve } from './test/native-fixture.mjs';
  await serve(${port}, ${JSON.stringify({ clientId, redirectUri, resource, introspectionSecret: 'synthetic-theme-test-value-only-0000' })});
`], { cwd: new URL('../', import.meta.url), stdio: ['ignore', 'pipe', 'inherit'] });
const finished = once(worker, 'exit');
const ready = AbortSignal.timeout(10000);
const cookies = new Map();

async function request(path, status, { fields, json, anonymous = false } = {}) {
  const headers = new Headers();
  if (!anonymous) headers.set('Cookie', [...cookies].map(pair => pair.join('=')).join('; '));
  if (fields) {
    headers.set('Origin', issuer);
    headers.set('Content-Type', 'application/x-www-form-urlencoded');
  }
  if (json) headers.set('Content-Type', 'application/json');
  const response = await fetch(issuer + path, {
    method: fields || json ? 'POST' : 'GET',
    body: fields ? new URLSearchParams(fields) : json && JSON.stringify(json),
    headers, redirect: 'manual', signal: AbortSignal.timeout(5000),
  });
  assert.equal(response.status, status, new URL(response.url).pathname);
  if (!anonymous) {
    for (const cookie of response.headers.getSetCookie()) {
      const [pair] = cookie.split(';', 1);
      cookies.set(pair.slice(0, pair.indexOf('=')), pair.slice(pair.indexOf('=') + 1));
    }
  }
  return response;
}

let score = 0;
async function page(name, path, clues, options) {
  const response = await request(path, 200, options);
  const html = await response.text();
  assert.match(response.headers.get('Content-Type'), /^text\/html/);
  for (const clue of clues) assert.ok(html.includes(clue), `${name} is missing ${clue}`);

  const style = html.match(/<style>([\s\S]*?)<\/style>/)?.[1] ?? '';
  const brandCues = [
    /name="theme-color" content="#0e0c0b"/i.test(html),
    /--bone:\s*#ede6dc\b/i.test(style),
    /--sun:\s*#ff5a1f\b/i.test(style),
    /"Geist"\s*,/.test(style),
    /"Geist Mono"\s*,/.test(style),
  ];
  const matched = brandCues.filter(Boolean).length;
  console.log(`${name}: ${matched}/${brandCues.length}`);
  score += matched;

  if (process.env.EVERETT_AUTH_THEME_PROOF_DIR) {
    await writeFile(join(process.env.EVERETT_AUTH_THEME_PROOF_DIR, name + '.json'),
      JSON.stringify({ html, csp: response.headers.get('Content-Security-Policy') }), { mode: 0o600 });
  }
  return html;
}
const csrf = html => {
  const value = html.match(/name="csrf" value="([^"]+)"/)?.[1];
  assert.ok(value);
  return value;
};

try {
  await Promise.race([
    (async () => {
      let output = '';
      for await (const chunk of worker.stdout) {
        output += chunk;
        if (output.includes('SYNTHETIC LOCAL FIXTURE ready:')) return;
      }
      throw new Error('Local Worker stopped before it was ready');
    })(),
    new Promise((_, reject) => ready.addEventListener('abort', () => reject(ready.reason), { once: true })),
  ]);

  const device = await (await request('/v2/auth/device', 200, {
    json: { name: 'Studio Mac mini with a long device name for layout review' }, anonymous: true,
  })).json();
  assert.match(device.user_code, /^[A-F0-9]{12}$/);
  const enrollment = '/enroll?code=' + device.user_code;
  await page('sign-in', enrollment, ['Sign in to Everett', 'Continue with GitHub', encodeURIComponent(enrollment)], { anonymous: true });
  const start = await request('/auth/github?return_to=' + encodeURIComponent(enrollment), 302);
  const provider = new URL(start.headers.get('Location'));
  assert.equal(provider.origin, 'https://github.com');
  await request('/auth/callback/github?' + new URLSearchParams({
    state: provider.searchParams.get('state'), code: 'synthetic-theme-fixture',
  }), 303);

  const pending = await page('device', enrollment, ['Studio Mac mini', 'Full code', device.user_code, 'Approve this agent', 'raitoxlol']);
  await request('/enroll/decision', 303, { fields: { csrf: csrf(pending), user_code: device.user_code, decision: 'approve' } });
  const connected = await page('connected', enrollment, ['Connected', 'return to your terminal', 'Revoke', device.user_code]);
  const agentId = connected.match(/name="agent_id" value="(agent-[a-f0-9]{32})"/)?.[1];
  assert.ok(agentId);
  await request('/enroll/revoke', 303, { fields: { csrf: csrf(connected), agent_id: agentId } });
  await page('revoked', enrollment, ['Credential revoked', device.user_code]);

  const params = new URLSearchParams({
    client_id: clientId, redirect_uri: redirectUri, resource, response_type: 'code', scope: 'everett',
    code_challenge_method: 'S256', code_challenge: createHash('sha256').update('v'.repeat(64)).digest('base64url'),
    state: 'synthetic-theme-state',
  });
  const authorization = await request('/oauth/authorize?' + params, 302);
  const consent = new URL(authorization.headers.get('Location'), issuer);
  assert.equal(consent.origin, issuer);
  await page('consent', consent.pathname + consent.search, ['Connect Synthetic gateway acceptance', clientId, resource, redirectUri, 'Approve connection', 'raitoxlol']);

  console.log(score);
} finally {
  worker.kill();
  await finished;
}
