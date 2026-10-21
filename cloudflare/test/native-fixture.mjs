#!/usr/bin/env node
// Local native acceptance fixture. Never imported by src or deployed to Cloudflare.
// Production Worker/HostedAuth/OwnerRelay run unchanged; only outbound GitHub is synthetic.
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const require = createRequire(import.meta.url);
const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const dependencies = [require.resolve('wrangler/package.json')];
const [mode, target, agentId] = process.argv.slice(2);

function loopback(value) {
  const url = new URL(value);
  if (url.protocol !== 'http:' || url.hostname !== '127.0.0.1' || !url.port || url.username || url.password || url.hash) throw new Error('fixture_target');
  return url;
}

async function browserAction(value, decision) {
  const url = loopback(value), origin = url.origin;
  if (decision === 'revoke') {
    if (url.href !== origin + '/' || !/^agent-[a-f0-9]{32}$/.test(agentId ?? '')) throw new Error('fixture_revoke_target');
  } else if (url.pathname !== '/enroll' || !/^[A-F0-9]{12}$/.test(url.searchParams.get('code') ?? '')) throw new Error('fixture_enrollment_target');
  const cookies = new Map();
  async function request(path, init = {}) {
    const headers = new Headers(init.headers);
    headers.set('Cookie', [...cookies].map(([name, value]) => `${name}=${value}`).join('; '));
    if (init.method === 'POST') headers.set('Origin', origin);
    const response = await fetch(origin + path, { ...init, headers, redirect: 'manual', signal: AbortSignal.timeout(10000) });
    for (const cookie of response.headers.getSetCookie()) {
      const pair = cookie.split(';', 1)[0], split = pair.indexOf('=');
      cookies.set(pair.slice(0, split), pair.slice(split + 1));
    }
    return response;
  }
  const returnTo = decision === 'revoke' ? '/enroll' : url.pathname + url.search;
  const start = await request('/auth/github?return_to=' + encodeURIComponent(returnTo));
  if (start.status !== 302) throw new Error('fixture_login_start');
  const provider = new URL(start.headers.get('Location'));
  if (provider.origin !== 'https://github.com' || !provider.searchParams.get('state')) throw new Error('fixture_provider_redirect');
  // Do not visit GitHub: only this local Worker's outbound service supplies a fake identity.
  const callback = await request('/auth/callback/github?' + new URLSearchParams({ state: provider.searchParams.get('state'), code: 'synthetic-fixture-code' }));
  if (callback.status !== 303) throw new Error('fixture_login_callback');
  const page = await request(returnTo), html = await page.text();
  const csrf = html.match(/name=["']csrf["'][^>]*value=["']([^"']+)["']/)?.[1];
  if (page.status !== 200 || !csrf) throw new Error('fixture_owner_page');
  const fields = decision === 'revoke' ? { csrf, agent_id: agentId } : { csrf, user_code: url.searchParams.get('code'), decision };
  const result = await request(decision === 'revoke' ? '/enroll/revoke' : '/enroll/decision', {
    method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams(fields),
  });
  if (result.status !== 303) throw new Error('fixture_owner_decision');
  console.log(`Synthetic local browser ${decision} completed. Credential values were not displayed.`);
}

export async function serve(value = '18789', oauth = null) {
  const port = Number(value);
  if (!Number.isInteger(port) || port < 1024 || port > 65535) throw new Error('fixture_port');
  const { Miniflare, Log, LogLevel, convertV4MiniflareOptions } = require(require.resolve('miniflare', { paths: dependencies }));
  const { build } = require(require.resolve('esbuild', { paths: dependencies }));
  const bundle = await build({ entryPoints: [resolve(packageRoot, 'src/index.ts')], bundle: true, write: false,
    format: 'esm', platform: 'browser', target: 'es2022', external: ['cloudflare:workers'], logLevel: 'silent' });
  const issuer = `http://127.0.0.1:${port}`;
  const mf = new Miniflare(convertV4MiniflareOptions({ host: '127.0.0.1', port, modules: true, script: bundle.outputFiles[0].text,
    compatibilityDate: '2026-10-21', log: new Log(LogLevel.NONE),
    handleUncaughtError() { console.error('Local fixture runtime error; request data suppressed.'); },
    durableObjects: { OWNERS: { className: 'OwnerRelay', useSQLite: true }, AUTH_STATE: { className: 'HostedAuth', useSQLite: true } },
    bindings: { EVERETT_ISSUER: issuer, AUTH_GITHUB_ID: 'synthetic-fixture-client', AUTH_GITHUB_SECRET: 'synthetic-fixture-only',
      EVERETT_OWNER_GITHUB_ID: '282002453', EVERETT_OWNER_GITHUB_LOGIN: 'raitoxlol',
      ...(oauth ? { EVERETT_OAUTH_CLIENT_IDS: oauth.clientId, EVERETT_OAUTH_REDIRECT_URIS: oauth.redirectUri,
        EVERETT_OAUTH_RESOURCES: oauth.resource, EVERETT_OAUTH_INTROSPECTION_SECRET: oauth.introspectionSecret } : {}) },
    outboundService(request) {
      const url = new URL(request.url);
      if (url.href === 'https://github.com/login/oauth/access_token' && request.method === 'POST') return Response.json({
        access_token: 'synthetic-fixture-provider-only', token_type: 'bearer', expires_in: 28800,
        refresh_token: 'synthetic-fixture-refresh-only', refresh_token_expires_in: 15811200,
      });
      if (url.href === 'https://api.github.com/user' && request.method === 'GET') return Response.json({ id: 282002453, login: 'raitoxlol' });
      if (oauth && url.href === oauth.clientId && request.method === 'GET') return Response.json({
        client_id: oauth.clientId, client_name: 'Synthetic gateway acceptance', redirect_uris: [oauth.redirectUri],
        token_endpoint_auth_method: 'none', grant_types: ['authorization_code', 'refresh_token'], response_types: ['code'],
      });
      return Response.json({ error: 'fixture_outbound_denied' }, { status: 502 });
    },
  }));
  try {
    await mf.ready;
    console.log(`SYNTHETIC LOCAL FIXTURE ready: ${issuer}`);
    console.log('Synthetic providers only; public accounts and physical-device acceptance remain unverified. State is temporary; Ctrl-C stops it.');
    await new Promise(resolve => { process.once('SIGINT', resolve); process.once('SIGTERM', resolve); });
  } finally { await mf.dispose(); }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (mode === 'serve') await serve(target);
    else if (mode === 'approve' || mode === 'deny' || mode === 'revoke') await browserAction(target, mode);
    else throw new Error('fixture_usage');
  } catch (error) {
    console.error('Local fixture failed (' + (error instanceof Error && /^fixture_[a-z_]+$/.test(error.message) ? error.message : 'runtime') + '). No credential or request values logged.');
    process.exitCode = 1;
  }
}
