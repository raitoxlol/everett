// Owner-only hosted mode against the running local stack: real Convex Auth sign-in for a
// simulated GitHub owner and intruder, real `everett login`/relay with per-agent tokens.
// GitHub's own OAuth exchange is the one step replaced: auth:store records the callback
// exactly as @convex-dev/auth does after GitHub returns a profile.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { createPublicKey, generateKeyPairSync, randomBytes } from 'node:crypto';
import { ConvexHttpClient } from 'convex/browser';
import { backend, relay, root, state } from './settings.mjs';

process.umask(0o077);
const binary = process.env.EVERETT_VERIFY_BINARY || 'everett';
const runId = new Date().toISOString().replaceAll(/[:.]/g, '-');
const evidence = path.join(state, 'hosted-runs', runId);
fs.mkdirSync(evidence, { recursive: true });
const operator = path.join(state, 'operator');
const cli = path.join(root, 'node_modules/convex/bin/main.js');
const caFile = path.join(state, 'ca.pem');
const OWNER = { id: '424242', login: 'dev-owner' };
const INTRUDER = { id: '999999', login: 'dev-intruder' };
const secrets = new Set();
const failures = [];
const output = [];
const note = line => { output.push(line); console.log(line); };
const remember = value => { if (typeof value === 'string' && value.length >= 16) secrets.add(value); return value; };

function convex(args, input) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('CONVEX_') && !key.startsWith('EVERETT_')));
  const result = spawnSync(process.execPath, [cli, ...args], { cwd: operator, env: { ...env, HOME: path.join(state, 'operator-home'), CI: '1' }, input, encoding: 'utf8', timeout: 120000 });
  // Function results can carry auth codes; keep stdout in memory, log only the status.
  fs.appendFileSync(path.join(evidence, 'convex-cli.log'), `convex ${args[0]} ${args[1] ?? ''} exit=${result.status}\n${result.stderr}\n`);
  if (result.status !== 0) throw new Error(`convex ${args.slice(0, 2).join(' ')} failed (${result.status}); see ${evidence}/convex-cli.log`);
  return result.stdout;
}
const store = args => { const out = convex(['run', 'auth:store', JSON.stringify({ args })]).trim(); return out ? JSON.parse(out) : null; };

function ownerMode() {
  const keyFile = path.join(state, 'hosted-jwt-key.pem');
  if (!fs.existsSync(keyFile)) {
    const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
    fs.writeFileSync(keyFile, privateKey.export({ type: 'pkcs8', format: 'pem' }), { mode: 0o600, flag: 'wx' });
  }
  const pem = fs.readFileSync(keyFile, 'utf8');
  const jwk = createPublicKey(pem).export({ format: 'jwk' });
  remember(pem.split('\n').slice(1, -2).join(''));
  const file = path.join(state, 'hosted.env');
  const vars = {
    EVERETT_OWNER_GITHUB_ID: OWNER.id, EVERETT_OWNER_GITHUB_LOGIN: OWNER.login, AUTH_LOG_LEVEL: 'INFO',
    SITE_URL: 'http://127.0.0.1:3211', JWT_PRIVATE_KEY: pem.trimEnd().replaceAll('\n', ' '), JWKS: JSON.stringify({ keys: [{ use: 'sig', ...jwk }] }),
  };
  fs.writeFileSync(file, Object.entries(vars).map(([key, value]) => `${key}='${value}'`).join('\n') + '\n', { mode: 0o600 });
  try { convex(['env', 'set', '--force', '--from-file', file]); } finally { fs.rmSync(file); }
}

async function githubSession(profile) {
  const verifier = store({ type: 'verifier' });
  const signature = randomBytes(32).toString('hex');
  store({ type: 'verifierSignature', verifier, signature });
  const code = store({ type: 'userOAuth', provider: 'github', providerAccountId: profile.id, profile: { name: profile.login }, signature });
  const result = await new ConvexHttpClient(backend).action('auth:signIn', { provider: 'github', params: { code }, verifier });
  assert.ok(result?.tokens?.token, 'Convex Auth must mint a hosted session JWT');
  remember(result.tokens.token); remember(result.tokens.refreshToken);
  const client = new ConvexHttpClient(backend);
  client.setAuth(result.tokens.token);
  return client;
}

const children = new Set();
const homeOf = name => path.join(state, 'hosted-homes', runId, name);
function envFor(name) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('EVERETT_') && !key.startsWith('CONVEX_') && !/^(CLAUDE|CODEX|HERMES|PI_SESSION|GROK_SESSION|TYPESAFE_API_KEY|NODE_OPTIONS)/.test(key)));
  return { ...env, HOME: homeOf(name), EVERETT_HOME: homeOf(name), EVERETT_NOTIFY: 'none', EVERETT_ROUTER: 'local', EVERETT_HOSTED_URL: relay, EVERETT_RELAY_CA_FILE: caFile };
}
const transcripts = [];
function everett(name, args, onStderr) {
  fs.mkdirSync(homeOf(name), { recursive: true });
  return new Promise((resolve, reject) => {
    const child = spawn(binary, args, { env: envFor(name), stdio: ['ignore', 'pipe', 'pipe'] });
    children.add(child);
    let stdout = '', stderr = '';
    const timer = setTimeout(() => child.kill('SIGTERM'), 60000);
    child.stdout.on('data', chunk => { stdout += chunk; });
    child.stderr.on('data', chunk => { stderr += chunk; onStderr?.(stderr); });
    child.once('error', reject);
    child.once('close', code => {
      clearTimeout(timer); children.delete(child);
      const text = `$ everett ${args.join(' ')}\nexit=${code}\nstdout:\n${stdout}\nstderr:\n${stderr}\n`;
      transcripts.push(text);
      fs.appendFileSync(path.join(evidence, `${name}.txt`), text);
      resolve({ code, stdout, stderr, label: `${name} ${args.join(' ')}` });
    });
  });
}
async function step(label, body) {
  try { await body(); note(`PASS ${label}`); }
  catch (error) { failures.push(label); note(`FAIL ${label}\n  ${error.message}`); }
}
const ok = result => { assert.equal(result.code, 0, `${result.label}: ${result.stderr.trim()}`); return result.stdout ? JSON.parse(result.stdout) : null; };

async function enroll(name, owner) {
  let approval;
  const login = await everett(name, ['login', '--device', name, '--open=false', '--json'], stderr => {
    const code = stderr.match(/approve code ([A-F0-9]+)/)?.[1];
    if (code && !approval) approval = owner.mutation('hosted:approve', { code, approve: true }).then(() => null, error => error);
  });
  const approvalError = await approval;
  if (approvalError) throw approvalError;
  const who = ok(login);
  assert.match(who.name, /^agent-[a-f0-9]{32}$/);
  const token = JSON.parse(fs.readFileSync(path.join(homeOf(name), '.everett/agent-token.json'), 'utf8')).token;
  remember(token); for (const part of token.split('.')) remember(part);
  const auth = JSON.parse(fs.readFileSync(path.join(homeOf(name), '.everett/auth.json'), 'utf8'));
  remember(auth.access_token); remember(auth.id_token);
  assert.equal(fs.statSync(path.join(homeOf(name), '.everett/agent-token.json')).mode & 0o777, 0o600);
  return who.name;
}

let ownerSet = false;
try {
  assert.ok(fs.existsSync(path.join(state, 'ready.json')), 'Run up.sh first');
  ownerMode(); ownerSet = true;
  note(`PASS owner allowlist configured on the local backend (GitHub ID ${OWNER.id}); generic OIDC now denied`);
  const owner = await githubSession(OWNER);
  const intruder = await githubSession(INTRUDER);
  note('PASS real Convex Auth sessions minted for the owner and a non-owner GitHub account');

  const agents = {};
  const preexisting = new Set((await owner.query('hosted:credentials', {})).credentials.map(c => c.agentId));
  await step('owner approves two independent agents through real everett login', async () => {
    agents.a = await enroll('hosted-a', owner);
    agents.b = await enroll('hosted-b', owner);
    assert.notEqual(agents.a, agents.b);
    for (const name of ['hosted-a', 'hosted-b']) {
      ok(await everett(name, ['relay', 'register']));
      assert.equal(ok(await everett(name, ['relay', 'whoami'])).deviceId, name === 'hosted-a' ? agents.a : agents.b);
    }
  });

  await step('non-owner, agent, generic OIDC and anonymous callers cannot approve, inspect, list or revoke', async () => {
    let pending;
    const waiting = everett('hosted-c', ['login', '--device', 'hosted-c', '--open=false', '--json'], stderr => {
      pending ??= stderr.match(/approve code ([A-F0-9]+)/)?.[1];
    });
    for (let n = 0; n < 50 && !pending; n++) await new Promise(r => setTimeout(r, 200));
    assert.ok(pending, 'login must print an approval code');
    const agentClient = new ConvexHttpClient(backend);
    agentClient.setAuth(JSON.parse(fs.readFileSync(path.join(homeOf('hosted-a'), '.everett/auth.json'), 'utf8')).access_token);
    const callers = [['non-owner GitHub', intruder], ['enrolled agent', agentClient], ['anonymous', new ConvexHttpClient(backend)]];
    const alice = path.join(state, 'homes/alice-a/.everett/auth.json');
    const aliceAuth = fs.existsSync(alice) ? JSON.parse(fs.readFileSync(alice, 'utf8')) : null;
    if (aliceAuth?.expires_at > Date.now() / 1000 + 30) {
      const generic = new ConvexHttpClient(backend);
      generic.setAuth(remember(aliceAuth.id_token));
      callers.push(['generic OIDC', generic]);
    } else note('  generic OIDC caller skipped: no fresh alice-a token (run smoke.sh first)');
    for (const [who, client] of callers) {
      await assert.rejects(client.mutation('hosted:approve', { code: pending, approve: true }), /Owner only|sign in required|denies external|Hosted/i, `${who} approve`);
      await assert.rejects(client.query('hosted:inspect', { code: pending }), undefined, `${who} inspect`);
      await assert.rejects(client.query('hosted:credentials', {}), undefined, `${who} list`);
      await assert.rejects(client.mutation('hosted:revoke', { agentId: agents.b }), undefined, `${who} revoke`);
    }
    assert.equal((await owner.query('hosted:inspect', { code: pending })).state, 'pending', 'denied approvals must not change the request');
    await owner.mutation('hosted:approve', { code: pending, approve: false });
    const denied = await waiting;
    assert.notEqual(denied.code, 0);
    assert.match(denied.stderr, /denied or expired/);
    const listed = await owner.query('hosted:credentials', {});
    assert.deepEqual(listed.credentials.map(c => c.agentId).filter(id => !preexisting.has(id)).sort(), [agents.a, agents.b].sort(), 'only the two owner-approved agents gained credentials');
  });

  await step('revoking agent A denies its still-valid JWT on the next request; agent B keeps working', async () => {
    const auth = JSON.parse(fs.readFileSync(path.join(homeOf('hosted-a'), '.everett/auth.json'), 'utf8'));
    assert.ok(auth.expires_at - Date.now() / 1000 > 300, 'cached agent JWT is still valid before revocation');
    assert.equal(ok(await everett('hosted-a', ['relay', 'whoami'])).deviceId, agents.a, 'agent A works with its cached JWT before revocation');
    await owner.mutation('hosted:revoke', { agentId: agents.a });
    const started = Date.now();
    const after = await everett('hosted-a', ['relay', 'whoami']);
    assert.notEqual(after.code, 0, 'revoked agent must fail with its cached JWT');
    assert.match(after.stderr, /Convex rejected the request/);
    assert.equal(JSON.parse(fs.readFileSync(path.join(homeOf('hosted-a'), '.everett/auth.json'), 'utf8')).id_token, auth.id_token, 'same cached JWT was presented');
    note(`  revoked JWT rejected on the first request after revoke (${Date.now() - started}ms incl. process start)`);
    const fresh = await everett('hosted-a', ['login', '--force', '--json']);
    assert.notEqual(fresh.code, 0, 'revoked agent token must not mint a fresh session');
    assert.equal(ok(await everett('hosted-b', ['relay', 'whoami'])).deviceId, agents.b);
    const rows = (await owner.query('hosted:credentials', {})).credentials;
    assert.equal(rows.find(c => c.agentId === agents.a)?.status, 'revoked');
    assert.equal(rows.find(c => c.agentId === agents.b)?.status, 'active');
  });

  await step(`no token, JWT, refresh token or signing key appears in CLI output or stack logs (${secrets.size} secrets checked)`, async () => {
    const files = [];
    const walk = dir => { for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) { if (!['backend', 'data', 'operator', 'node_modules', 'operator-home'].includes(entry.name)) walk(full); }
      else if (/\.(log|txt)$/.test(entry.name)) files.push(full);
    } };
    walk(state);
    files.push(...['backend.log', 'convex.log', 'issuer.log'].map(f => path.join(state, f)).filter(f => fs.existsSync(f)));
    const corpora = [...new Set(files)].map(f => [f, fs.readFileSync(f, 'utf8')]).concat(transcripts.map((t, i) => [`transcript ${i}`, t]));
    for (const secret of secrets) for (const [where, text] of corpora) assert.ok(!text.includes(secret), `secret material leaked into ${where}`);
    note(`  scanned ${corpora.length} files/transcripts`);
  });
} catch (error) {
  failures.push(error.message);
  note(`FAIL ${error.message}`);
} finally {
  for (const child of children) child.kill('SIGTERM');
  if (ownerSet) {
    try { convex(['env', 'remove', 'EVERETT_OWNER_GITHUB_ID']); note('PASS owner allowlist removed again; generic OIDC smoke mode restored'); }
    catch (error) { failures.push('restore'); note(`FAIL restore generic mode: ${error.message}`); }
  }
  note(`SUMMARY ${failures.length ? 'FAIL' : 'PASS'}: ${failures.length} failed checks; real output in ${evidence}`);
  fs.writeFileSync(path.join(evidence, 'summary.txt'), output.join('\n') + '\n');
  fs.writeFileSync(path.join(state, 'last-hosted-smoke.txt'), output.join('\n') + '\n');
  process.exitCode = failures.length ? 1 : 0;
}
