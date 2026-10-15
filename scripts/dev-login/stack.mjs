import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import https from 'node:https';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { accounts, backend, clientId, directory, issuer, relay, root, state } from './settings.mjs';

process.umask(0o077);
fs.mkdirSync(state, { recursive: true });
const pidFile = path.join(state, 'stack.pid');
const children = new Set();
let proxy;
let stopping = false;
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const quote = value => `'${value.replaceAll("'", "'\\''")}'`;
const operator = path.join(state, 'operator');
const operatorHome = path.join(state, 'operator-home');
const cli = path.join(root, 'node_modules/convex/bin/main.js');
const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('CONVEX_') && !key.startsWith('EVERETT_') && !['NODE_OPTIONS', 'VERCEL', 'SENTRY_DSN'].includes(key)));
Object.assign(env, { HOME: operatorHome, CI: '1', DO_NOT_TRACK: '1', DISABLE_BEACON: '1', SENTRY_DSN: '' });

async function shutdown(code) {
  if (stopping) return;
  stopping = true;
  proxy?.close();
  for (const child of children) child.kill('SIGTERM');
  await Promise.race([Promise.all([...children].map(child => new Promise(resolve => child.once('exit', resolve)))), delay(4000)]);
  for (const child of children) child.kill('SIGKILL');
  if (fs.existsSync(pidFile) && fs.readFileSync(pidFile, 'utf8').trim() === String(process.pid)) {
    fs.unlinkSync(pidFile);
    fs.rmSync(path.join(state, 'ready.json'), { force: true });
  }
  process.exit(code);
}
process.on('SIGINT', () => shutdown(0));
process.on('SIGTERM', () => shutdown(0));

function start(command, args, log, options = {}) {
  const fd = fs.openSync(path.join(state, log), 'a', 0o600);
  const child = spawn(command, args, { cwd: operator, env, stdio: ['ignore', fd, fd], ...options });
  fs.closeSync(fd);
  children.add(child);
  child.once('exit', (code, signal) => {
    children.delete(child);
    if (!stopping) {
      console.error(`FAIL ${log} process exited (${code ?? signal}). Read ${path.join(state, log)}`);
      void shutdown(1);
    }
  });
  child.once('error', error => { console.error(error.message); void shutdown(1); });
  return child;
}

async function run(command, args, log) {
  const fd = fs.openSync(path.join(state, log), 'a', 0o600);
  const child = spawn(command, args, { cwd: operator, env, stdio: ['ignore', fd, fd] });
  fs.closeSync(fd);
  children.add(child);
  const timer = setTimeout(() => child.kill('SIGTERM'), 120000);
  try {
    const code = await new Promise((resolve, reject) => { child.once('exit', resolve); child.once('error', reject); });
    if (code !== 0) throw new Error(`${command === process.execPath ? 'convex' : command} ${args[command === process.execPath ? 1 : 0]} failed (${code}). Read ${path.join(state, log)}. If it asks for cloud login, stop here and run npx convex login once in a private terminal; this stack never selects cloud.`);
  } finally {
    clearTimeout(timer);
    children.delete(child);
  }
}

async function waitFor(url) {
  for (let n = 0; n < 100; n++) {
    try { if ((await fetch(url, { signal: AbortSignal.timeout(1000) })).ok) return; } catch {}
    await delay(200);
  }
  throw new Error(`Service did not become ready at ${url}`);
}

async function freePort(port) {
  const server = net.createServer();
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(port, '127.0.0.1', resolve); });
  await new Promise(resolve => server.close(resolve));
}

async function backendBinary() {
  const version = 'precompiled-2026-10-06-a3538c6';
  const platforms = {
    'darwin-arm64': ['aarch64-apple-darwin', 'f89a9dc8189148b0aa67b50dbcb202ce429b541df83e36cab4438c2b7955d5c2'],
    'darwin-x64': ['x86_64-apple-darwin', 'a9465ddf5535392a3ae9bd5f9aab49e5067927f2b27e8fdb45fe1317d7b42f6d'],
    'linux-arm64': ['aarch64-unknown-linux-gnu', 'd3bf1d8799d91cecabe144ee2c96348c903ec3f5f88199f375004647b037cc35'],
    'linux-x64': ['x86_64-unknown-linux-gnu', 'eba2eeab62632e0c153e83f9f1c5ada9e98834e2db3386e32f825a3a97f8c093'],
  };
  const platform = platforms[`${process.platform}-${process.arch}`];
  if (!platform) throw new Error(`Unsupported local backend platform ${process.platform}/${process.arch}`);
  const [target, digest] = platform;
  const destination = path.join(state, 'backend', version);
  fs.mkdirSync(destination, { recursive: true });
  const archive = path.join(destination, 'backend.zip');
  if (!fs.existsSync(archive)) {
    console.log(`Download pinned Convex backend ${version} (${target})`);
    const response = await fetch(`https://github.com/get-convex/convex-backend/releases/download/${version}/convex-local-backend-${target}.zip`, { signal: AbortSignal.timeout(180000) });
    if (!response.ok) throw new Error(`Backend download HTTP ${response.status}`);
    fs.writeFileSync(archive, Buffer.from(await response.arrayBuffer()));
  }
  if (createHash('sha256').update(fs.readFileSync(archive)).digest('hex') !== digest) throw new Error(`Backend archive checksum mismatch at ${archive}`);
  const binary = path.join(destination, 'convex-local-backend');
  await run('unzip', ['-oq', archive, '-d', destination], 'install.log');
  fs.chmodSync(binary, 0o700);
  return binary;
}

function tlsProxy() {
  const key = path.join(state, 'tls-key.pem');
  const cert = path.join(state, 'tls-cert.pem');
  if (!fs.existsSync(key) || !fs.existsSync(cert)) {
    const config = path.join(state, 'openssl.cnf');
    fs.writeFileSync(config, '[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN=localhost\n[ext]\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,digitalSignature,keyEncipherment,keyCertSign\nextendedKeyUsage=serverAuth\n');
    const result = spawnSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '30', '-keyout', key, '-out', cert, '-config', config], { stdio: 'ignore' });
    if (result.status !== 0) throw new Error('openssl failed to create the local TLS certificate');
  }
  const server = https.createServer({ key: fs.readFileSync(key), cert: fs.readFileSync(cert) }, (req, res) => {
    const upstream = http.request(`${backend}${req.url}`, { method: req.method, headers: req.headers }, incoming => {
      res.writeHead(incoming.statusCode, incoming.headers);
      incoming.pipe(res);
    });
    upstream.on('error', () => { res.writeHead(502); res.end('Local backend unavailable'); });
    req.pipe(upstream);
  });
  server.on('tlsClientError', error => fs.appendFileSync(path.join(state, 'tls-errors.log'), `${new Date().toISOString()} ${error.message}\n`));
  server.on('upgrade', (req, socket, head) => {
    const upstream = http.request(`${backend}${req.url}`, { headers: req.headers });
    upstream.on('upgrade', (response, remote, remoteHead) => {
      socket.write(`HTTP/1.1 101 Switching Protocols\r\n${Object.entries(response.headers).map(([key, value]) => `${key}: ${value}\r\n`).join('')}\r\n`);
      if (head.length) remote.write(head);
      if (remoteHead.length) socket.write(remoteHead);
      socket.on('error', () => remote.destroy());
      remote.on('error', () => socket.destroy());
      socket.on('close', () => remote.destroy());
      remote.pipe(socket).pipe(remote);
    });
    upstream.on('error', () => socket.destroy());
    upstream.on('response', () => socket.destroy());
    upstream.end();
  });
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(4445, '127.0.0.1', () => resolve(server));
  });
}

try {
  if (fs.existsSync(pidFile)) {
    const pid = Number(fs.readFileSync(pidFile, 'utf8').trim());
    try { process.kill(pid, 0); throw new Error(`Stack PID ${pid} is active. Use down.sh first.`); }
    catch (error) { if (error.code !== 'ESRCH') throw error; }
    fs.unlinkSync(pidFile);
  }
  fs.writeFileSync(pidFile, `${process.pid}\n`, { flag: 'wx' });
  for (const port of [3210, 3211, 4444, 4445]) await freePort(port);
  for (const folder of [operator, operatorHome, path.join(state, 'data')]) fs.mkdirSync(folder, { recursive: true });
  fs.cpSync(path.join(root, 'convex'), path.join(operator, 'convex'), { recursive: true, filter: source => !source.split(path.sep).includes('_generated') });
  for (const file of ['package.json', 'tsconfig.json']) fs.copyFileSync(path.join(root, file), path.join(operator, file));
  if (!fs.existsSync(path.join(operator, 'node_modules'))) fs.symlinkSync(path.join(root, 'node_modules'), path.join(operator, 'node_modules'));
  const binary = await backendBinary();
  const secretPath = path.join(state, 'instance-secret');
  if (!fs.existsSync(secretPath)) fs.writeFileSync(secretPath, randomBytes(32).toString('hex'), { flag: 'wx' });
  const secret = fs.readFileSync(secretPath, 'utf8').trim();
  const keyResult = spawnSync(binary, ['keygen', 'admin-key', '--instance-name', 'everett-dev-login', '--instance-secret', secret], { encoding: 'utf8', env });
  if (keyResult.status !== 0) throw new Error('Convex admin-key generation failed');
  fs.writeFileSync(path.join(operator, '.env.local'), `CONVEX_SELF_HOSTED_URL=${backend}\nCONVEX_SELF_HOSTED_ADMIN_KEY=${keyResult.stdout.trim()}\n`, { mode: 0o600 });
  start(process.execPath, [path.join(directory, 'issuer.mjs')], 'issuer.log');
  start(binary, ['--interface', '127.0.0.1', '--port', '3210', '--site-proxy-port', '3211', '--instance-name', 'everett-dev-login', '--instance-secret', secret, '--disable-beacon', '--local-storage', path.join(state, 'data/storage'), path.join(state, 'data/convex.sqlite3')], 'backend.log');
  await waitFor(`${issuer}/health`);
  await waitFor(`${backend}/version`);
  console.log('PASS local issuer and Convex backend ready; no account required');
  await run(process.execPath, [cli, 'env', 'set', 'OIDC_ISSUER', issuer], 'convex.log');
  await run(process.execPath, [cli, 'env', 'set', 'OIDC_CLIENT_ID', clientId], 'convex.log');
  await run(process.execPath, [cli, 'dev', '--once', '--typecheck', 'enable'], 'convex.log');
  for (const account of Object.values(accounts)) {
    const args = { issuer, subject: account.sub, org: account.everett_org, agentId: account.everett_agent, role: account.everett_role };
    await run(process.execPath, [cli, 'run', 'relay:provisionIdentity', JSON.stringify(args)], 'convex.log');
  }
  proxy = await tlsProxy();
  for (const name of Object.keys(accounts)) {
    const home = path.join(state, 'homes', name);
    fs.mkdirSync(path.join(home, '.everett'), { recursive: true });
    fs.writeFileSync(path.join(home, '.everett/config.toml'), `[auth]\nissuer = "${issuer}"\nclient_id = "${clientId}"\n[relay]\nurl = "${relay}"\n`);
    const variables = { EVERETT_HOME: home, EVERETT_AUTH_ISSUER: issuer, EVERETT_AUTH_CLIENT_ID: clientId, EVERETT_CONVEX_URL: relay, EVERETT_NOTIFY: 'none' };
    fs.writeFileSync(path.join(state, `${name}.env`), `${Object.entries(variables).map(([key, value]) => `export ${key}=${quote(value)}`).join('\n')}\n`);
    console.log(`\n${name}:\n${Object.entries(variables).map(([key, value]) => `export ${key}=${quote(value)}`).join('\n')}`);
    console.log(`Config: ${path.join(home, '.everett/config.toml')}\n[auth]\nissuer = "${issuer}"\nclient_id = "${clientId}"\n[relay]\nurl = "${relay}"`);
  }
  console.log(`\nPASS both operator identities provisioned. TLS diagnostic proxy ${relay}`);
  console.log('NOTE Everett 1.6.0 requires HTTPS and uses public WebPKI roots. The self-signed local proxy may block relay; smoke.sh reports the real result.');
  console.log(`Run ${path.join(directory, 'smoke.sh')} in another terminal. Ctrl-C or down.sh stops services and preserves data.`);
  fs.writeFileSync(path.join(state, 'ready.json'), JSON.stringify({ pid: process.pid, issuer, relay, backend }));
  await new Promise(() => {});
} catch (error) {
  console.error(`FAIL ${error.message}`);
  await shutdown(1);
}
