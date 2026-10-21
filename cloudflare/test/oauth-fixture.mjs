#!/usr/bin/env node
// Local actual-Worker OAuth fixture. Never imported into deployed source.
import { constants, closeSync, existsSync, fstatSync, lstatSync, mkdirSync, openSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { createHash, randomBytes } from 'node:crypto';
import { resolve, join } from 'node:path';
import { serve } from './native-fixture.mjs';

const [mode, directory, port = '18839', resource = 'http://127.0.0.1:18840/mcp'] = process.argv.slice(2);
const home = directory && resolve(directory);
const clientId = 'https://client.everett.test/client.json', redirectUri = 'https://client.everett.test/callback';
function check(condition, stage) { if (!condition) throw new Error('fixture_' + stage); }
function loopback(value) {
  const url = new URL(value);
  check(url.protocol === 'http:' && url.hostname === '127.0.0.1' && url.port && !url.username && !url.password && !url.hash && !url.search, 'target');
  return url;
}
function privateHome() {
  check(Boolean(home), 'directory');
  mkdirSync(home, { mode: 0o700, recursive: true });
  const info = lstatSync(home);
  check(info.isDirectory() && info.uid === process.getuid() && (info.mode & 0o777) === 0o700, 'directory_mode');
}
function readPrivate(name) {
  const fd = openSync(join(home, name), constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const info = fstatSync(fd);
    check(info.isFile() && info.uid === process.getuid() && (info.mode & 0o777) === 0o600 && info.size < 16384, 'file_mode');
    return readFileSync(fd, 'utf8');
  } finally { closeSync(fd); }
}
function writePrivate(name, value, replace = false) {
  const path = join(home, name);
  if (replace) readPrivate(name);
  const temporary = replace ? path + '.tmp-' + randomBytes(8).toString('hex') : path;
  writeFileSync(temporary, value, { mode: 0o600, flag: 'wx' });
  if (replace) renameSync(temporary, path);
}
function config() {
  const value = JSON.parse(readPrivate('fixture.json'));
  check(loopback(value.issuer).origin === value.issuer && loopback(value.resource).pathname === '/mcp', 'config');
  check(value.clientId === clientId && value.redirectUri === redirectUri, 'config_client');
  return value;
}
async function request(c, path, fields, authorization) {
  const headers = new Headers();
  if (fields) headers.set('Content-Type', 'application/x-www-form-urlencoded');
  if (authorization) headers.set('Authorization', 'Bearer ' + authorization);
  return fetch(c.issuer + path, { method: fields ? 'POST' : 'GET', headers,
    ...(fields ? { body: new URLSearchParams(fields) } : {}), redirect: 'manual', signal: AbortSignal.timeout(10000) });
}
async function json(response, expected = 200) { check(response.status === expected, 'http_' + response.status); return response.json(); }
async function ownerBrowser(c) {
  const cookies = new Map();
  async function browser(path, init = {}) {
    const headers = new Headers(init.headers);
    headers.set('Cookie', [...cookies].map(([name, value]) => `${name}=${value}`).join('; '));
    if (init.method === 'POST') headers.set('Origin', c.issuer);
    const response = await fetch(c.issuer + path, { ...init, headers, redirect: 'manual', signal: AbortSignal.timeout(10000) });
    for (const cookie of response.headers.getSetCookie()) {
      const pair = cookie.split(';', 1)[0], at = pair.indexOf('='); cookies.set(pair.slice(0, at), pair.slice(at + 1));
    }
    return response;
  }
  const start = await browser('/auth/github?return_to=%2Fenroll'); check(start.status === 302, 'login_start');
  const location = new URL(start.headers.get('Location'));
  check(location.origin === 'https://github.com' && location.searchParams.get('state'), 'login_redirect');
  const callback = await browser('/auth/callback/github?' + new URLSearchParams({ state: location.searchParams.get('state'), code: 'synthetic-oauth-fixture-code' }));
  check(callback.status === 303, 'login_callback');
  return browser;
}
async function csrf(browser, path) {
  const page = await browser(path); check(page.status === 200, 'owner_page');
  const value = (await page.text()).match(/name=["']csrf["'][^>]*value=["']([^"']+)["']/)?.[1];
  check(value, 'csrf'); return value;
}
async function introspect(c, token) {
  return json(await request(c, '/oauth/introspect', { token }, readPrivate('introspection.secret')));
}
async function saveTokens(c, tokens, replace) {
  check(/^eva_[a-f0-9]{64}$/.test(tokens.access_token ?? '') && /^evr_[a-f0-9]{64}$/.test(tokens.refresh_token ?? '') && tokens.token_type === 'Bearer', 'token_shape');
  const identity = await introspect(c, tokens.access_token);
  check(identity.active === true && identity.iss === c.issuer && identity.aud === c.resource && /^agent-[a-f0-9]{32}$/.test(identity.everett_agent ?? ''), 'identity');
  writePrivate('tokens.json', JSON.stringify({ ...tokens, agent_id: identity.everett_agent }), replace);
  writePrivate('access.token', tokens.access_token, replace);
  console.log(JSON.stringify({ status: replace ? 'refreshed' : 'issued', agent_id: identity.everett_agent, active: true, issuer: c.issuer, resource: c.resource }));
}

try {
  privateHome();
  if (mode === 'serve') {
    check(Number.isInteger(Number(port)) && Number(port) >= 1024 && Number(port) <= 65535, 'port');
    check(loopback(resource).pathname === '/mcp', 'resource');
    const c = { issuer: 'http://127.0.0.1:' + port, clientId, redirectUri, resource };
    const introspectionSecret = randomBytes(24).toString('hex');
    writePrivate('introspection.secret', introspectionSecret);
    writePrivate('fixture.json', JSON.stringify(c));
    console.log('Private fixture config and introspection secret saved mode0600; values suppressed.');
    await serve(port, { ...c, introspectionSecret });
  } else {
    const c = config();
    if (mode === 'issue') {
      check(!existsSync(join(home, 'tokens.json')) && !existsSync(join(home, 'access.token')), 'already_issued');
      const metadata = await json(await request(c, '/.well-known/oauth-authorization-server'));
      check(metadata.issuer === c.issuer && metadata.token_endpoint === c.issuer + '/oauth/token', 'metadata');
      const browser = await ownerBrowser(c), verifier = randomBytes(32).toString('hex'), state = randomBytes(16).toString('hex');
      const parameters = new URLSearchParams({ client_id: clientId, redirect_uri: redirectUri, resource: c.resource,
        response_type: 'code', scope: 'everett', code_challenge_method: 'S256', code_challenge: createHash('sha256').update(verifier).digest('base64url'), state });
      const response = await browser('/oauth/authorize?' + parameters); check(response.status === 302, 'authorize');
      const enrollment = new URL(response.headers.get('Location'), c.issuer);
      check(enrollment.origin === c.issuer && enrollment.pathname === '/enroll' && enrollment.searchParams.get('request'), 'consent_location');
      const approved = await browser('/oauth/consent', { method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({ csrf: await csrf(browser, enrollment.pathname + enrollment.search), request_id: enrollment.searchParams.get('request'), decision: 'approve', name: 'synthetic-gateway-agent' }) });
      check(approved.status === 303, 'consent');
      const callback = new URL(approved.headers.get('Location'));
      check(callback.origin + callback.pathname === redirectUri && callback.searchParams.get('state') === state && callback.searchParams.get('iss') === c.issuer, 'callback');
      const tokens = await json(await request(c, '/oauth/token', { grant_type: 'authorization_code', code: callback.searchParams.get('code'),
        client_id: clientId, redirect_uri: redirectUri, resource: c.resource, code_verifier: verifier }));
      await saveTokens(c, tokens, false);
    } else if (mode === 'refresh') {
      const previous = JSON.parse(readPrivate('tokens.json'));
      const tokens = await json(await request(c, '/oauth/token', { grant_type: 'refresh_token', client_id: clientId, resource: c.resource, refresh_token: previous.refresh_token }));
      await saveTokens(c, tokens, true);
    } else if (mode === 'check') {
      const identity = await introspect(c, readPrivate('access.token'));
      console.log(JSON.stringify({ active: identity.active, ...(identity.active ? { agent_id: identity.everett_agent, issuer: identity.iss, resource: identity.aud } : {}) }));
    } else if (mode === 'revoke' || mode === 'revoke-agent') {
      const tokens = JSON.parse(readPrivate('tokens.json'));
      if (mode === 'revoke') await json(await request(c, '/oauth/revoke', { client_id: clientId, token: tokens.refresh_token }));
      else {
        const browser = await ownerBrowser(c);
        const result = await browser('/enroll/revoke', { method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
          body: new URLSearchParams({ csrf: await csrf(browser, '/enroll'), agent_id: tokens.agent_id }) });
        check(result.status === 303, 'owner_revoke');
      }
      check((await introspect(c, tokens.access_token)).active === false, 'revocation');
      const refresh = await request(c, '/oauth/token', { grant_type: 'refresh_token', client_id: clientId, refresh_token: tokens.refresh_token });
      check(refresh.status === 400, 'revoked_refresh');
      console.log(JSON.stringify({ status: 'revoked', agent_id: tokens.agent_id, active: false, refresh_http: refresh.status }));
    } else throw new Error('fixture_usage');
  }
} catch (error) {
  console.error('Local OAuth fixture failed (' + (error instanceof Error && /^fixture_[a-z_0-9]+$/.test(error.message) ? error.message : 'runtime') + '). Credential and response values suppressed.');
  process.exitCode = 1;
}
