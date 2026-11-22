import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createTestHarness } from 'wrangler';
import { FIXTURE_ACCOUNTS, FIXTURE_ISSUER } from './shared-threads-data.mjs';

const testDir = path.dirname(fileURLToPath(import.meta.url));
const cloudflareDir = path.resolve(testDir, '..');
const repoDir = path.resolve(cloudflareDir, '..');
const manifest = path.join(repoDir, 'everett-rs', 'Cargo.toml');
const metadata = spawnSync('cargo', ['metadata', '--no-deps', '--format-version', '1', '--manifest-path', manifest], {
  cwd: repoDir,
  encoding: 'utf8',
});
if (metadata.status !== 0) throw new Error(metadata.stderr || 'cargo metadata failed');
const targetDir = JSON.parse(metadata.stdout).target_directory;
const binary = process.env.EVERETT_BIN
  ? path.resolve(process.env.EVERETT_BIN)
  : path.join(targetDir, 'debug', process.platform === 'win32' ? 'everett.exe' : 'everett');
if (!process.env.EVERETT_BIN || !existsSync(binary)) {
  const build = spawnSync('cargo', ['build', '--locked', '--manifest-path', manifest, '--bin', 'everett'], {
    cwd: repoDir,
    stdio: 'inherit',
  });
  if (build.status !== 0) throw new Error('cargo build --bin everett failed');
}
if (!existsSync(binary)) throw new Error('native Everett CLI binary was not produced');

const harness = createTestHarness({
  root: cloudflareDir,
  workers: [
    {
      configPath: './wrangler.jsonc',
      bindingOverrides: { AUTHENTICATOR: 'shared-threads-test-auth' },
    },
    {
      config: {
        name: 'shared-threads-test-auth',
        main: './test/shared-threads-auth.mjs',
        compatibility_date: '2026-10-21',
        workers_dev: false,
        send_metrics: false,
      },
    },
  ],
});

const root = await mkdtemp(path.join(tmpdir(), 'everett-shared-threads-'));
const ownerFor = principal => JSON.stringify([principal.issuer, 'org', principal.org]);
const accounts = {};
const demoLines = [];

function demoLog(line) {
  demoLines.push(line);
  console.log(line);
}

function printableCommand(args) {
  const printable = args.map(arg =>
    /^[A-Za-z0-9_./:@=+-]+$/.test(arg) ? arg : JSON.stringify(arg));
  return `$ everett ${printable.join(' ')}`;
}

function commandEnv(home, baseUrl) {
  return {
    PATH: process.env.PATH ?? '/usr/bin:/bin',
    HOME: home,
    EVERETT_HOME: home,
    EVERETT_NOTIFY: 'none',
    EVERETT_AUTH_PROVIDER: 'cloudflare',
    EVERETT_AUTH_ISSUER: FIXTURE_ISSUER,
    EVERETT_AUTH_BASE_URL: baseUrl,
  };
}

async function createAccount(name, baseUrl, principal) {
  const fixture = FIXTURE_ACCOUNTS[name];
  const home = path.join(root, name);
  const everettDir = path.join(home, '.everett');
  await mkdir(everettDir, { recursive: true, mode: 0o700 });
  await writeFile(path.join(everettDir, 'config.toml'), '[auth]\nprovider = "cloudflare"\n', { mode: 0o600 });
  await writeFile(path.join(everettDir, 'cloudflare-auth.json'), JSON.stringify({
    issuer: FIXTURE_ISSUER,
    base_url: baseUrl,
    agent_id: principal.agentId,
    owner: ownerFor(fixture.principal),
    device_name: `shared-thread-${name}`,
    token: fixture.token,
    expires_at: Date.now() + 3_600_000,
  }), { mode: 0o600 });
  return {
    home,
    env: commandEnv(home, baseUrl),
    owner: ownerFor(fixture.principal),
    agentId: principal.agentId,
  };
}

function run(account, args) {
  demoLog(printableCommand(args));
  const result = spawnSync(binary, args, {
    cwd: account.home,
    env: account.env,
    encoding: 'utf8',
    timeout: 30_000,
    maxBuffer: 2 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`everett ${args.join(' ')} failed (exit ${result.status}): ${result.stderr}`);
  }
  return JSON.parse(result.stdout);
}

function expectDenied(account, args) {
  demoLog(printableCommand(args));
  const result = spawnSync(binary, args, {
    cwd: account.home,
    env: account.env,
    encoding: 'utf8',
    timeout: 30_000,
    maxBuffer: 2 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  assert.notEqual(result.status, 0, `everett ${args.join(' ')} unexpectedly succeeded`);
  assert.match(result.stderr, /inbox_not_allowed/);
  demoLog('DENIED as expected: inbox_not_allowed');
}

try {
  demoLog('Shared-thread CLI demo (synthetic accounts; local Worker only)');
  const { url } = await harness.listen();
  const worker = harness.getWorker('everett-cloudflare-local');
  const env = await worker.getEnv();
  const authWorker = harness.getWorker('shared-threads-test-auth');
  async function enroll(name) {
    const fixture = FIXTURE_ACCOUNTS[name];
    const identity = fixture.principal;
    const owner = ownerFor(identity);
    const stub = env.OWNERS.get(env.OWNERS.idFromName(owner));
    const result = await stub.enroll(identity, 'member', crypto.randomUUID());
    assert.equal(result.ok, true, `synthetic ${name} enrollment failed`);
    const principal = {
      ...identity,
      agentId: result.value.deviceId,
      role: 'member',
      expiresAt: Date.now() + 3_600_000,
    };
    const response = await authWorker.fetch('https://test-auth.internal/__test/set-principal', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token: fixture.token, principal }),
    });
    assert.equal(response.status, 200, `synthetic ${name} token setup failed`);
    return principal;
  }
  const alicePrincipal = await enroll('alice');
  const bobPrincipal = await enroll('bob');
  assert.equal(alicePrincipal.role, 'member');
  assert.equal(bobPrincipal.role, 'member');
  accounts.alice = await createAccount('alice', url.origin, alicePrincipal);
  accounts.bob = await createAccount('bob', url.origin, bobPrincipal);

  const aliceDevice = run(accounts.alice, ['relay', 'register']).device;
  const bobDevice = run(accounts.bob, ['relay', 'register']).device;
  assert.equal(aliceDevice.deviceId, accounts.alice.agentId);
  assert.equal(bobDevice.deviceId, accounts.bob.agentId);

  const aliceAddress = 'alice@thread-zone-a';
  const bobAddress = 'bob@thread-zone-b';
  run(accounts.alice, ['relay', 'register-inbox', aliceAddress, '--harness', 'claude', '--session', 'alice-session']);
  run(accounts.bob, ['relay', 'register-inbox', bobAddress, '--harness', 'codex', '--session', 'bob-session']);

  run(accounts.alice, [
    'relay', 'grant-inbox', aliceAddress, '--sender-owner', accounts.bob.owner, '--sender-device', bobDevice.deviceId,
  ]);
  const grantForBob = run(accounts.bob, [
    'relay', 'grant-inbox', bobAddress, '--sender-owner', accounts.alice.owner, '--sender-device', aliceDevice.deviceId,
  ]);
  const created = run(accounts.alice, [
    'thread', 'create', '--from', aliceAddress, '--to', bobAddress, '--idempotency-key', 'local-worker-create',
  ]);
  const threadRef = created.threadRef;
  assert.equal(threadRef.creatorOwner, accounts.alice.owner);
  const shown = run(accounts.alice, [
    'thread', 'show', '--creator-owner', threadRef.creatorOwner, '--thread-id', threadRef.threadId,
  ]);
  assert.deepEqual(shown.threadRef, threadRef);

  const aliceSendArgs = [
    'thread', 'send', '--creator-owner', threadRef.creatorOwner, '--thread-id', threadRef.threadId,
    'alice synthetic message', '--idempotency-key', 'alice-message',
  ];
  const aliceStored = run(accounts.alice, aliceSendArgs);
  const aliceRetry = run(accounts.alice, aliceSendArgs);
  assert.equal(aliceRetry.seq, aliceStored.seq);
  assert.equal(aliceRetry.messageId, aliceStored.messageId);
  demoLog(`RETRY same idempotency key: seq=${aliceRetry.seq}, messageId=${aliceRetry.messageId}`);
  const bobStored = run(accounts.bob, [
    'thread', 'send', '--creator-owner', threadRef.creatorOwner, '--thread-id', threadRef.threadId,
    'bob synthetic reply', '--idempotency-key', 'bob-message',
  ]);
  assert.equal(aliceStored.seq, 1);
  assert.equal(bobStored.seq, 2);
  const aliceHistory = run(accounts.alice, [
    'thread', 'read', '--creator-owner', threadRef.creatorOwner, '--thread-id', threadRef.threadId,
  ]);
  const bobHistory = run(accounts.bob, [
    'thread', 'read', '--creator-owner', threadRef.creatorOwner, '--thread-id', threadRef.threadId,
  ]);
  for (const history of [aliceHistory, bobHistory]) {
    assert.deepEqual(
      history.messages.map(message => message.text).sort(),
      ['alice synthetic message', 'bob synthetic reply'].sort(),
    );
  }
  demoLog(`ALICE decrypted history: ${JSON.stringify(aliceHistory.messages.map(({ seq, text }) => ({ seq, text })))}`);
  demoLog(`BOB decrypted history: ${JSON.stringify(bobHistory.messages.map(({ seq, text }) => ({ seq, text })))}`);

  const ownerRelay = harness.getWorker('everett-cloudflare-local');
  const storage = await ownerRelay.getDurableObjectStorage('OWNERS', { name: accounts.alice.owner });
  const storedRows = await storage.exec(
    'SELECT seq, post FROM thread_messages WHERE thread_id=? ORDER BY seq',
    threadRef.threadId,
  );
  assert.equal(storedRows.length, 2);
  assert.deepEqual(storedRows.map(row => row.seq), [1, 2]);
  const storedJson = JSON.stringify(storedRows);
  assert.doesNotMatch(storedJson, /alice synthetic message|bob synthetic reply/);
  const ciphertextCopies = storedRows.reduce((count, row) => count + JSON.parse(row.post).copies.length, 0);
  assert.equal(ciphertextCopies, 4);
  demoLog(`CREATOR DO storage: ${storedRows.length} encrypted message rows, ${ciphertextCopies} ciphertext copies, no plaintext`);

  run(accounts.bob, ['relay', 'revoke-inbox-grant', grantForBob.grantId]);
  demoLog('REVOKE acknowledged by remote member owner');
  expectDenied(accounts.alice, [
    'thread', 'send', '--creator-owner', threadRef.creatorOwner, '--thread-id', threadRef.threadId,
    'post after revoke', '--idempotency-key', 'post-after-revoke',
  ]);
  expectDenied(accounts.bob, [
    'thread', 'send', '--creator-owner', threadRef.creatorOwner, '--thread-id', threadRef.threadId,
    'post after revoke', '--idempotency-key', 'bob-post-after-revoke',
  ]);
  expectDenied(accounts.alice, [
    'thread', 'read', '--creator-owner', threadRef.creatorOwner, '--thread-id', threadRef.threadId,
  ]);
  expectDenied(accounts.bob, [
    'thread', 'read', '--creator-owner', threadRef.creatorOwner, '--thread-id', threadRef.threadId,
  ]);
  const retainedRows = await storage.exec(
    'SELECT seq, post FROM thread_messages WHERE thread_id=? ORDER BY seq',
    threadRef.threadId,
  );
  assert.equal(retainedRows.length, 2);
  assert.deepEqual(retainedRows.map(row => row.seq), [1, 2]);
  demoLog(`AFTER REVOKE: retained ${retainedRows.length} encrypted rows / ${retainedRows.reduce((count, row) => count + JSON.parse(row.post).copies.length, 0)} ciphertext copies`);

  demoLog('PASS native Everett CLI ↔ local Cloudflare Worker demo');
  if (process.env.SHARED_THREADS_DEMO_LOG) {
    await writeFile(
      process.env.SHARED_THREADS_DEMO_LOG,
      demoLines.join(String.fromCharCode(10)) + String.fromCharCode(10),
      { mode: 0o600 },
    );
  }
} finally {
  await harness.close();
  await rm(root, { recursive: true, force: true });
}
