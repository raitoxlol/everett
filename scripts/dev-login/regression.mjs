import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import https from 'node:https';
import { X509Certificate, createPrivateKey } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { ensureTlsCertificate, seedSession, syncConvexFunctions } from './local-state.mjs';

const directory = path.dirname(fileURLToPath(import.meta.url));
const binary = '/opt/homebrew/bin/everett';
const cleanEnv = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(EVERETT_|CONVEX_|CLAUDE|CODEX|HERMES|PI_SESSION|GROK_SESSION|TYPESAFE_API_KEY|NODE_OPTIONS)/.test(key)));
const quote = value => `'${value.replaceAll("'", "'\\''")}'`;
function temporary(t) {
  const folder = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'everett-dev-login-test-')));
  t.after(() => fs.rmSync(folder, { recursive: true, force: true }));
  return folder;
}
function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: 'utf8', timeout: 30000, ...options });
  assert.equal(result.status, 0, result.error?.message || result.stderr || result.stdout);
  return result.stdout;
}
function write(file, contents) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, contents);
}

test('installed Everett discovers both dated session fixtures without a relay', t => {
  const folder = temporary(t);
  for (const name of ['alice-a', 'alice-b']) {
    const home = path.join(folder, name);
    const session = `review-${name}`;
    seedSession(home, session, folder);
    const listed = JSON.parse(run(binary, ['ls', '--json'], { env: { ...cleanEnv, HOME: home, EVERETT_HOME: home, EVERETT_ROUTER: 'local', EVERETT_NOTIFY: 'none' } }));
    assert.deepEqual(listed.map(entry => entry.id), [session]);
  }
});

test('smoke checks both real session IDs before requiring stack readiness', t => {
  const folder = temporary(t);
  for (const file of ['smoke.mjs', 'settings.mjs', 'local-state.mjs']) fs.copyFileSync(path.join(directory, file), path.join(folder, file));
  const dependencies = path.resolve(directory, '../../node_modules');
  fs.symlinkSync(dependencies, path.join(folder, 'node_modules'));
  const result = spawnSync(process.execPath, [path.join(folder, 'smoke.mjs')], { encoding: 'utf8', timeout: 30000, env: cleanEnv });
  assert.equal(result.status, 1, result.error?.message || result.stderr);
  assert.match(result.stdout, /PASS alice-a ls --json/);
  assert.match(result.stdout, /PASS alice-b ls --json/);
  assert.match(result.stdout, /FAIL Run up.sh first/);
  assert.ok(result.stdout.indexOf('PASS alice-b ls --json') < result.stdout.indexOf('FAIL Run up.sh first'));
  assert.doesNotMatch(result.stdout, /relay whoami/);
});

test('startup reconciles unchanged and changed lockfiles, skips issuer scripts, and stops on install failure', t => {
  const folder = temporary(t);
  const root = path.join(folder, 'repo');
  const issuer = path.join(root, 'scripts/dev-login');
  const bin = path.join(folder, 'bin');
  const cache = path.join(folder, 'npm-cache');
  const log = path.join(folder, 'installs.log');
  const npm = fs.realpathSync(run('which', ['npm']).trim());
  fs.mkdirSync(issuer, { recursive: true });
  fs.mkdirSync(bin);
  fs.copyFileSync(path.join(directory, 'up.sh'), path.join(issuer, 'up.sh'));
  write(path.join(bin, 'npm'), `#!/bin/sh\nprintf '%s\\n' "$*" >> "$INSTALL_LOG"\nif [ "\${INSTALL_FAIL:-}" = "$3" ]; then exit 23; fi\nexec ${quote(process.execPath)} ${quote(npm)} "$@" --offline --cache ${quote(cache)}\n`);
  write(path.join(bin, 'node'), `#!/bin/sh\ncase "$1" in */stack.mjs) printf 'stack reached\\n'; exit 0;; esac\nexec ${quote(process.execPath)} "$@"\n`);
  for (const name of ['npm', 'node']) fs.chmodSync(path.join(bin, name), 0o700);
  const env = { ...cleanEnv, PATH: `${bin}:${process.env.PATH}`, INSTALL_LOG: log };
  const packages = { convex: {}, 'oidc-provider': {} };
  for (const dependency of Object.keys(packages)) {
    for (const version of ['1.0.0', '2.0.0']) {
      const source = path.join(folder, `${dependency}-${version}`);
      write(path.join(source, 'package.json'), JSON.stringify({ name: dependency, version, scripts: { postinstall: `node -e "require('fs').writeFileSync('postinstall-ran', 'yes')"` } }));
      const packed = JSON.parse(run(process.execPath, [npm, 'pack', source, '--pack-destination', folder, '--json', '--offline', '--cache', cache], { env: cleanEnv }));
      packages[dependency][version] = path.join(folder, packed[0].filename);
    }
  }
  function lock(version) {
    for (const [target, dependency] of [[root, 'convex'], [issuer, 'oidc-provider']]) {
      write(path.join(target, 'package.json'), JSON.stringify({ name: `fixture-${dependency}`, version: '1.0.0', dependencies: { [dependency]: `file:${packages[dependency][version]}` } }));
      run(process.execPath, [npm, 'install', '--prefix', target, '--package-lock-only', '--ignore-scripts', '--no-audit', '--no-fund', '--offline', '--cache', cache], { env: cleanEnv });
    }
  }
  function startup(extra = {}) {
    return spawnSync('bash', [path.join(issuer, 'up.sh')], { encoding: 'utf8', timeout: 30000, env: { ...env, ...extra } });
  }
  function graph(version) {
    for (const [target, dependency] of [[root, 'convex'], [issuer, 'oidc-provider']]) {
      assert.equal(JSON.parse(fs.readFileSync(path.join(target, 'node_modules', dependency, 'package.json'))).version, version);
    }
    assert.equal(fs.readFileSync(path.join(root, 'node_modules/convex/postinstall-ran'), 'utf8'), 'yes');
    assert.equal(fs.existsSync(path.join(issuer, 'node_modules/oidc-provider/postinstall-ran')), false);
  }
  lock('1.0.0');
  let result = startup();
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /stack reached/);
  graph('1.0.0');
  write(path.join(root, 'node_modules/convex/stale-file'), 'stale');
  result = startup();
  assert.equal(result.status, 0, result.stderr);
  graph('1.0.0');
  assert.equal(fs.existsSync(path.join(root, 'node_modules/convex/stale-file')), false);
  lock('2.0.0');
  result = startup();
  assert.equal(result.status, 0, result.stderr);
  graph('2.0.0');
  for (const target of [root, issuer]) {
    result = startup({ INSTALL_FAIL: target });
    assert.equal(result.status, 23, result.stderr);
    assert.doesNotMatch(result.stdout, /stack reached/);
  }
  assert.equal(fs.readFileSync(log, 'utf8').trim().split('\n').length, 9);
});

test('copy removes deleted and renamed Convex modules while preserving sibling state', t => {
  const state = temporary(t);
  const root = path.join(state, 'source');
  const operator = path.join(state, 'operator');
  write(path.join(root, 'convex/old.ts'), 'old');
  write(path.join(root, 'convex/deleted.ts'), 'deleted');
  write(path.join(root, 'convex/_generated/api.ts'), 'generated');
  const preserved = ['data/convex.sqlite3', 'instance-secret', 'homes/alice-a/.everett/auth.json', 'operator/.env.local', 'operator-home/.config/state', 'tls-key.pem'];
  for (const file of preserved) write(path.join(state, file), `keep ${file}`);
  syncConvexFunctions(root, operator);
  assert.equal(fs.readFileSync(path.join(operator, 'convex/old.ts'), 'utf8'), 'old');
  fs.renameSync(path.join(root, 'convex/old.ts'), path.join(root, 'convex/renamed.ts'));
  fs.unlinkSync(path.join(root, 'convex/deleted.ts'));
  syncConvexFunctions(root, operator);
  assert.deepEqual(fs.readdirSync(path.join(operator, 'convex')), ['renamed.ts']);
  assert.equal(fs.readFileSync(path.join(operator, 'convex/renamed.ts'), 'utf8'), 'old');
  for (const file of preserved) assert.equal(fs.readFileSync(path.join(state, file), 'utf8'), `keep ${file}`);
  syncConvexFunctions(root, operator);
  assert.deepEqual(fs.readdirSync(path.join(operator, 'convex')), ['renamed.ts']);
});

function expiredCertificate(folder) {
  write(path.join(folder, 'index'), '');
  write(path.join(folder, 'serial'), '01');
  const config = path.join(folder, 'ca.cnf');
  write(config, `[ca]\ndefault_ca=local\n[local]\ndatabase=${folder}/index\nserial=${folder}/serial\nnew_certs_dir=${folder}\ndefault_md=sha256\npolicy=policy\nx509_extensions=ext\n[policy]\ncommonName=supplied\n[ext]\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,digitalSignature,keyEncipherment,keyCertSign\nextendedKeyUsage=serverAuth\n`);
  run('openssl', ['req', '-new', '-newkey', 'rsa:2048', '-nodes', '-subj', '/CN=localhost', '-keyout', path.join(folder, 'tls-key.pem'), '-out', path.join(folder, 'request.pem')]);
  run('openssl', ['ca', '-selfsign', '-batch', '-keyfile', path.join(folder, 'tls-key.pem'), '-in', path.join(folder, 'request.pem'), '-out', path.join(folder, 'tls-cert.pem'), '-config', config, '-startdate', '20000101000000Z', '-enddate', '20010101000000Z', '-notext']);
}

test('missing, expired, malformed, and mismatched TLS files renew; valid files are reused', t => {
  const folder = temporary(t);
  let tls = ensureTlsCertificate(folder);
  const valid = new X509Certificate(tls.cert);
  assert.equal(valid.checkHost('localhost'), 'localhost');
  assert.equal(valid.checkIP('127.0.0.1'), '127.0.0.1');
  assert.equal(valid.checkPrivateKey(createPrivateKey(tls.key)), true);
  assert.ok(Date.parse(valid.validTo) > Date.now());
  assert.deepEqual(ensureTlsCertificate(folder), tls);
  expiredCertificate(folder);
  const expired = new X509Certificate(fs.readFileSync(path.join(folder, 'tls-cert.pem')));
  assert.ok(Date.parse(expired.validTo) < Date.now());
  assert.equal(expired.checkPrivateKey(createPrivateKey(fs.readFileSync(path.join(folder, 'tls-key.pem')))), true);
  tls = ensureTlsCertificate(folder);
  assert.notEqual(new X509Certificate(tls.cert).fingerprint256, expired.fingerprint256);
  assert.ok(Date.parse(new X509Certificate(tls.cert).validTo) > Date.now());
  write(path.join(folder, 'tls-cert.pem'), 'broken certificate');
  tls = ensureTlsCertificate(folder);
  assert.equal(new X509Certificate(tls.cert).checkIP('127.0.0.1'), '127.0.0.1');
  const other = path.join(folder, 'other');
  fs.mkdirSync(other);
  const otherTls = ensureTlsCertificate(other);
  fs.writeFileSync(path.join(folder, 'tls-key.pem'), otherTls.key);
  tls = ensureTlsCertificate(folder);
  assert.equal(new X509Certificate(tls.cert).checkPrivateKey(createPrivateKey(tls.key)), true);
  assert.notDeepEqual(tls.key, otherTls.key);
});

test('renewed certificate serves loopback HTTPS with explicit Node CA trust', async t => {
  const folder = temporary(t);
  expiredCertificate(folder);
  const tls = ensureTlsCertificate(folder);
  const server = https.createServer(tls, (_request, response) => response.end('local TLS diagnostic'));
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  t.after(() => new Promise(resolve => server.close(resolve)));
  const result = await new Promise((resolve, reject) => {
    https.get(`https://127.0.0.1:${server.address().port}`, { ca: tls.cert, timeout: 2000 }, response => {
      let body = '';
      response.setEncoding('utf8');
      response.on('data', chunk => { body += chunk; });
      response.on('end', () => resolve({ status: response.statusCode, body }));
    }).once('error', reject).once('timeout', function () { this.destroy(new Error('TLS diagnostic timeout')); });
  });
  assert.deepEqual(result, { status: 200, body: 'local TLS diagnostic' });
});
