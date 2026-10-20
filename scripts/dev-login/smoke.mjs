import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import https from 'node:https';
import { spawn } from 'node:child_process';
import { ConvexHttpClient } from 'convex/browser';
import { accounts, backend, clientId, issuer, relay, state } from './settings.mjs';
import { seedSession } from './local-state.mjs';

process.umask(0o077);
const binary = process.env.EVERETT_VERIFY_BINARY || 'everett';
const runId = new Date().toISOString().replaceAll(/[:.]/g, '-');
const sessions = { 'alice-a': `dev-login-a-${runId}`, 'alice-b': `dev-login-b-${runId}` };
const evidence = path.join(state, 'runs', runId);
fs.mkdirSync(evidence, { recursive: true });
const failures = [];
const listeners = new Set();
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const output = [];
function note(line) { output.push(line); console.log(line); }
function envFor(name, extra = {}) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('EVERETT_') && !key.startsWith('CONVEX_') && !/^(CLAUDE|CODEX|HERMES|PI_SESSION|GROK_SESSION|TYPESAFE_API_KEY|NODE_OPTIONS)/.test(key)));
  const home = path.join(state, 'homes', name);
  return { ...env, HOME: home, EVERETT_HOME: home, EVERETT_NOTIFY: 'none', EVERETT_ROUTER: 'local', EVERETT_AUTH_ISSUER: issuer, EVERETT_AUTH_CLIENT_ID: clientId, EVERETT_CONVEX_URL: relay, EVERETT_RELAY_CA_FILE: path.join(state, 'ca.pem'), ...extra };
}
function command(name, args, extra = {}, approve = false) {
  return new Promise((resolve, reject) => {
    const child = spawn(binary, args, { env: envFor(name, extra), stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '', stderr = '', approval, approvalError;
    const timer = setTimeout(() => child.kill('SIGTERM'), 30000);
    child.stdout.on('data', chunk => { stdout += chunk; });
    child.stderr.on('data', chunk => {
      stderr += chunk;
      const code = stderr.match(/enter code\s+([A-Z0-9-]+)/)?.[1];
      if (approve && code && !approval) {
        approval = fetch(`${issuer}/approve`, { method: 'POST', body: new URLSearchParams({ user_code: code, account: name }), signal: AbortSignal.timeout(5000) })
          .then(async response => { if (!response.ok) throw new Error(`Approval HTTP ${response.status}: ${await response.text()}`); })
          .catch(error => { approvalError = error; child.kill('SIGTERM'); });
      }
    });
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('close', async (code, signal) => {
      clearTimeout(timer);
      await approval;
      const label = `${name} ${args.join(' ')}`;
      const file = `${name}-${args.slice(0, 3).join('-').replaceAll(/[^a-zA-Z0-9.-]/g, '_')}-${Date.now()}.txt`;
      fs.writeFileSync(path.join(evidence, file), `$ ${binary} ${args.join(' ')}\nexit=${code ?? signal}\nstdout:\n${stdout}\nstderr:\n${stderr}`);
      if (approvalError) return reject(approvalError);
      resolve({ code, signal, stdout, stderr, label });
    });
  });
}
function check(result, validate = () => {}) {
  try {
    assert.equal(result.code, 0, result.stderr.trim() || `exit ${result.code ?? result.signal}`);
    validate(result.stdout);
    note(`PASS ${result.label}`);
    return true;
  } catch (error) {
    const detail = result.code !== 0 ? result.stderr.trim() || `exit ${result.code ?? result.signal}` : error.message;
    failures.push(result.label);
    note(`FAIL ${result.label}\n  ${detail}`);
    return false;
  }
}
async function must(name, args, extra, validate) {
  const result = await command(name, args, extra);
  if (!check(result, validate)) throw new Error(`Journey stopped at ${result.label}`);
  return JSON.parse(result.stdout);
}
function identity(value, name) {
  assert.equal(value.deviceId, name);
  assert.equal(value.owner, JSON.stringify([issuer, 'org', 'local-alice']));
  assert.equal(value.role, accounts[name].everett_role);
}

async function backendCheck(name) {
  const authPath = path.join(state, 'homes', name, '.everett/auth.json');
  const auth = JSON.parse(fs.readFileSync(authPath));
  const client = new ConvexHttpClient(backend);
  client.setAuth(auth.id_token);
  identity(await client.query('relay:whoami', {}), name);
  assert.equal(fs.statSync(authPath).mode & 0o777, 0o600);
  const claims = JSON.parse(Buffer.from(auth.id_token.split('.')[1], 'base64url'));
  for (const key of ['everett_agent', 'everett_org', 'everett_role']) assert.equal(claims[key], accounts[name][key]);
  assert.equal(claims.iss, issuer);
  assert.equal(claims.sub, name);
  assert.equal(claims.aud, clientId);
  assert.ok(claims.exp > Date.now() / 1000);
  auth.expires_at = 0;
  fs.writeFileSync(authPath, JSON.stringify(auth), { mode: 0o600 });
  const refresh = await command(name, ['whoami', '--json']);
  assert.equal(refresh.code, 0, refresh.stderr);
  assert.equal(JSON.parse(refresh.stdout).sub, name);
  const refreshed = JSON.parse(fs.readFileSync(authPath));
  assert.ok(refreshed.expires_at > Date.now() / 1000, 'Native client must persist refreshed credentials');
  assert.ok(refreshed.id_token, 'Refresh must return a signed ID token');
  client.setAuth(refreshed.id_token);
  identity(await client.query('relay:whoami', {}), name);
  const [header, payload, signature] = refreshed.id_token.split('.');
  const tampered = JSON.parse(Buffer.from(payload, 'base64url'));
  tampered.everett_role = tampered.everett_role === 'admin' ? 'member' : 'admin';
  client.setAuth(`${header}.${Buffer.from(JSON.stringify(tampered)).toString('base64url')}.${signature}`);
  await assert.rejects(client.query('relay:whoami', {}));
  note(`PASS ${name} backend diagnostic: verified JWT/claims/enrollment, refresh ID token, tamper rejection, credentials mode 0600`);
}

async function device(name) {
  const login = await command(name, ['login', '--device', name, '--open=false', '--force', '--json'], {}, true);
  if (!check(login, value => { const who = JSON.parse(value); assert.equal(who.signed_in, true); assert.equal(who.sub, name); assert.equal(who.device_name, name); })) return false;
  const whoami = await command(name, ['whoami', '--json']);
  if (!check(whoami, value => { const who = JSON.parse(value); assert.equal(who.signed_in, true); assert.equal(who.sub, name); })) return false;
  try { await backendCheck(name); }
  catch (error) { failures.push(`${name} backend diagnostic`); note(`FAIL ${name} backend diagnostic: ${error.message}`); }
  let ready = true;
  for (const operation of ['whoami', 'register', 'devices']) {
    const result = await command(name, ['relay', operation]);
    ready = check(result, value => {
      const data = JSON.parse(value);
      if (operation === 'whoami') identity(data, name);
      if (operation === 'register') { assert.equal(data.device.deviceId, name); assert.match(data.fingerprint, /^[a-f0-9]{64}$/); }
      if (operation === 'devices') { assert.ok(Array.isArray(data)); assert.ok(data.some(device => device.deviceId === name)); }
    }) && ready;
  }
  return ready;
}

function listen(name) {
  const fd = fs.openSync(path.join(evidence, `${name}-listener.log`), 'a', 0o600);
  const child = spawn(binary, ['relay'], { env: envFor(name), stdio: ['ignore', fd, fd] });
  fs.closeSync(fd);
  child.once('error', error => note(`FAIL ${name} listener: ${error.message}`));
  listeners.add(child);
}
async function waitMessage(name, session, text) {
  for (let n = 0; n < 30; n++) {
    const result = await command(name, ['inbox', '--session', session, '--peek', '--json']);
    assert.equal(result.code, 0, result.stderr);
    const found = JSON.parse(result.stdout).messages.find(message => message.text === text);
    if (found) return found;
    await delay(500);
  }
  throw new Error(`No ${JSON.stringify(text)} in ${name}/${session}`);
}
async function exchange() {
  const a = await must('alice-a', ['relay', 'register']);
  const b = await must('alice-b', ['relay', 'register']);
  await must('alice-a', ['peer', 'add-relay', 'alice-b', '--device', 'alice-b', '--fingerprint', b.fingerprint]);
  await must('alice-b', ['peer', 'add-relay', 'alice-a', '--device', 'alice-a', '--fingerprint', a.fingerprint]);
  const aSession = sessions['alice-a'], bSession = sessions['alice-b'];
  const sender = { EVERETT_SESSION_ID: aSession, EVERETT_HARNESS_NAME: 'codex' };
  const sent = await must('alice-a', ['send', 'Hello from A', '--to', `alice-b/codex/${bSession}`, '--idempotency-key', `local-${runId}`, '--json'], sender, value => assert.ok(JSON.parse(value).message_id));
  await must('alice-a', ['peer', 'status', 'alice-b', sent.message_id]);
  const client = new ConvexHttpClient(backend);
  client.setAuth(JSON.parse(fs.readFileSync(path.join(state, 'homes/alice-b/.everett/auth.json'))).id_token);
  const pending = await client.query('relay:pending', { deviceId: 'alice-b' });
  const envelope = pending.envelopes.find(row => row.envelope.messageId === sent.message_id)?.envelope;
  assert.ok(envelope, 'Backend must queue the real encrypted envelope');
  assert.match(envelope.ciphertext, /^[0-9a-f]+$/);
  assert.equal(envelope.signature.length, 128);
  assert.ok(!JSON.stringify(envelope).includes('Hello from A'), 'Backend must not receive plaintext');
  note('PASS backend queued signed ciphertext only; recipient has not run yet');
  for (const name of Object.keys(sessions)) listen(name);
  const received = await waitMessage('alice-b', bSession, 'Hello from A');
  assert.equal(received.relay_message_id, sent.message_id);
  assert.notEqual(received.id, sent.message_id);
  await must('alice-b', ['inbox', '--session', bSession, '--json']);
  const reply = await command('alice-b', ['reply', received.id, 'Hello back from B'], { EVERETT_SESSION_ID: bSession, EVERETT_HARNESS_NAME: 'codex' });
  if (!check(reply)) throw new Error('Reply failed');
  await waitMessage('alice-a', aSession, 'Hello back from B');
  await must('alice-a', ['inbox', '--session', aSession, '--json']);
  for (let n = 0; n < 30; n++) {
    const result = await command('alice-a', ['peer', 'status', 'alice-b', sent.message_id]);
    assert.equal(result.code, 0, result.stderr);
    if (JSON.parse(result.stdout).state === 'replied') { note('PASS relay A -> B pickup -> reply -> A inbox; signed status replied'); return; }
    await delay(500);
  }
  throw new Error('Signed status never reached replied');
}

try {
  for (const [name, session] of Object.entries(sessions)) {
    seedSession(path.join(state, 'homes', name), session, state);
    await must(name, ['ls', '--json'], {}, value => assert.ok(JSON.parse(value).some(entry => entry.id === session), `Missing seeded session ${session}`));
  }
  assert.ok(fs.existsSync(path.join(state, 'stack.pid')), 'Run up.sh first');
  assert.equal(await (await fetch(`${issuer}/health`, { signal: AbortSignal.timeout(1000) })).text(), 'everett-dev-login');
  const version = await command('alice-a', ['--version']);
  assert.match(version.stdout.trim(), /^everett \d+\.\d+\.\d+/);
  note(`PASS binary ${binary}: ${version.stdout.trim()}`);
  const ready = await Promise.all(Object.keys(accounts).map(device));
  const http = await command('alice-a', ['relay', 'whoami'], { EVERETT_CONVEX_URL: backend });
  assert.notEqual(http.code, 0);
  assert.match(http.stderr, /HTTPS origin/);
  note(`PASS HTTP rejection reproduced: ${http.stderr.trim()}`);
  await must('alice-a', ['relay', 'whoami'], { EVERETT_RELAY_CA_FILE: '' }, value => identity(JSON.parse(value), 'alice-a'));
  note('PASS native relay trusts the explicit config-file CA without an environment override');
  const started = Date.now();
  const configFile = path.join(state, 'homes/alice-a/.everett/config.toml');
  const config = fs.readFileSync(configFile, 'utf8');
  let untrusted;
  try {
    fs.writeFileSync(configFile, config.replace(/^ca_file = .*$/m, ''));
    untrusted = await command('alice-a', ['relay', 'whoami'], { EVERETT_RELAY_CA_FILE: '' });
  } finally {
    fs.writeFileSync(configFile, config);
  }
  assert.notEqual(untrusted.code, 0);
  assert.match(untrusted.stderr, /TLS\/certificate verification failed.*UnknownIssuer/s);
  assert.ok(Date.now() - started < 10000, 'TLS failure must not wait for the 20-second request deadline');
  note(`PASS native untrusted CA rejected clearly in ${Date.now() - started}ms`);
  const tls = await new Promise((resolve, reject) => {
    https.get(`${relay}/version`, { ca: fs.readFileSync(path.join(state, 'ca.pem')), timeout: 2000 }, response => {
      response.resume(); response.once('end', () => resolve(response.statusCode));
    }).once('error', reject).once('timeout', function () { this.destroy(new Error('TLS diagnostic timeout')); });
  });
  assert.equal(tls, 200);
  note('PASS TLS proxy /version with explicit local CA trust');
  if (ready.every(Boolean)) await exchange();
  else note('SKIP message exchange: installed binary could not complete relay identity/registration/devices');
} catch (error) {
  failures.push(error.message);
  note(`FAIL ${error.message}`);
} finally {
  for (const child of listeners) child.kill('SIGTERM');
  note(`SUMMARY ${failures.length ? 'FAIL' : 'PASS'}: ${failures.length} failed checks; real output in ${evidence}`);
  fs.writeFileSync(path.join(evidence, 'summary.txt'), output.join('\n') + '\n');
  fs.writeFileSync(path.join(state, 'last-smoke.txt'), output.join('\n') + '\n');
  process.exitCode = failures.length ? 1 : 0;
}
