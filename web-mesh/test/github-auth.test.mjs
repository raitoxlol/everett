import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';
import { createConsole } from '../server.mjs';

const origin = 'https://dashboard.example';
const issuer = 'https://issuer.example';
const config = {
  ENABLED: 'true', CONSOLE_AUTH: 'github', CONSOLE_ORIGIN: origin,
  SESSION_KEY: 'k'.repeat(64), CONSOLE_SECRET: 'fixture-old-secret'.repeat(2),
  MESH_ENDPOINT: issuer, MESH_TOKEN: 'emr_' + 'a'.repeat(64),
  MESH_OWNER: JSON.stringify([issuer, 'account', 'github:12345']),
  GITHUB_CLIENT_ID: 'fixture-app', GITHUB_CLIENT_SECRET: 'fixture-github-secret',
};
const request = (path, init) => new Request(origin + path, init);
const firstCookie = response => response.headers.getSetCookie()[0].split(';')[0];
const callback = (state, cookie, code = 'fixture-code') =>
  request('/api/github-callback?' + new URLSearchParams({ state, code }), { headers: { Cookie: cookie } });

function fixture(userId = 12345) {
  let revoked = false;
  let challenge;
  let exchanges = 0;
  const source = async (url, options) => {
    assert.equal(url, issuer + '/v1/mesh/snapshot');
    assert.equal(options.headers.Authorization, 'Bearer ' + config.MESH_TOKEN);
    assert.equal(options.headers['X-Everett-Console-Origin'], origin);
    if (revoked) return Response.json({error:'denied'}, {status:403});
    return Response.json({
      version:'everett.mesh.v1', source:'cloudflare', owner:config.MESH_OWNER,
      consoleOrigin:origin, serverNow:Date.now(), grantExpiresAt:Date.now()+60000,
      devices:[{deviceId:'my-device',name:'Mac',lastSeen:null,lastSeenConfidence:'unavailable'}],
      sessions:[], receipts:[], queue:{queued:0}, truncated:{devices:false,sessions:false,receipts:false},
    });
  };
  const github = async (url, options) => {
    if (url === 'https://github.com/login/oauth/access_token') {
      exchanges++;
      assert.equal(options.body.get('code'), 'fixture-code');
      assert.equal(options.body.get('client_secret'), config.GITHUB_CLIENT_SECRET);
      assert.equal(options.body.get('redirect_uri'), origin + '/api/github-callback');
      assert.equal(createHash('sha256').update(options.body.get('code_verifier')).digest('base64url'), challenge);
      return Response.json({access_token:'fixture-github-token'});
    }
    assert.equal(url, 'https://api.github.com/user');
    assert.equal(options.headers.Authorization, 'Bearer fixture-github-token');
    return Response.json({id:userId,login:'a-renamable-login'});
  };
  const app = createConsole(config, source, github);
  const start = async () => {
    const response = await app(request('/api/login'));
    assert.equal(response.status,303);
    const url = new URL(response.headers.get('location'));
    assert.equal(url.origin,'https://github.com');
    assert.equal(url.searchParams.get('code_challenge_method'),'S256');
    assert.equal(url.searchParams.get('scope'),'');
    challenge = url.searchParams.get('code_challenge');
    return { state:url.searchParams.get('state'), cookie:firstCookie(response), response };
  };
  return {app,start,revoke:()=>{revoked=true;},exchanges:()=>exchanges};
}

test('GitHub signs in the exact grant owner and Cloudflare still revokes the open session', async () => {
  const f = fixture();
  const unauthorized = await f.app(request('/api/mesh'));
  assert.equal(unauthorized.status,401);
  assert.equal((await unauthorized.json()).error,'github_sign_in_required');
  const { state, cookie, response } = await f.start();
  assert.match(response.headers.get('set-cookie'),/HttpOnly; Secure; SameSite=Lax/);
  const signIn = await f.app(callback(state,cookie));
  assert.equal(signIn.headers.get('location'),'/dashboard');
  const [session, cleared] = signIn.headers.getSetCookie();
  assert.match(session,/HttpOnly; SameSite=Strict; Max-Age=900; Secure/);
  assert.match(cleared,/__Host-everett-github-flow=;.*Max-Age=0/);
  const read = () => f.app(request('/api/mesh',{headers:{Cookie:firstCookie(signIn)}}));
  const authorized = await read();
  assert.equal(authorized.status,200);
  const body = await authorized.text();
  assert.ok(body.includes('my-device'));
  for (const value of [config.MESH_TOKEN, config.GITHUB_CLIENT_SECRET, 'fixture-github-token']) assert.ok(!body.includes(value));
  f.revoke();
  assert.equal((await read()).status,403);
});

test('rejects a different GitHub account before creating a dashboard session', async () => {
  const f = fixture(40000);
  const { state, cookie } = await f.start();
  const result = await f.app(callback(state,cookie));
  assert.equal(result.headers.get('location'),'/dashboard?auth=failed');
  assert.equal(result.headers.getSetCookie().length,1);
  assert.doesNotMatch(result.headers.get('set-cookie'),/__Host-everett-console=/);
});

test('rejects missing, mismatched, tampered or expired OAuth state before contacting GitHub', async t => {
  const f = fixture();
  const { state, cookie } = await f.start();
  for (const attempt of [
    callback(state,''),
    callback('z'.repeat(43),cookie),
    callback(state,cookie.replace(/.$/,match=>match==='0'?'1':'0')),
    request('/api/github-callback?state='+state+'&code=a&code=b',{headers:{Cookie:cookie}}),
  ]) {
    assert.equal((await f.app(attempt)).headers.get('location'),'/dashboard?auth=failed');
  }
  const future = Date.now()+301000;
  t.mock.method(Date,'now',()=>future);
  assert.equal((await f.app(callback(state,cookie))).headers.get('location'),'/dashboard?auth=failed');
  assert.equal(f.exchanges(),0);
});

test('GitHub mode rejects shared-secret login and requires an account grant', async () => {
  const f = fixture();
  assert.equal((await f.app(request('/api/login',{
    method:'POST', headers:{Origin:origin}, body:new URLSearchParams({secret:config.CONSOLE_SECRET}),
  }))).status,405);
  for (const owner of ['fixture', JSON.stringify([issuer,'org','github:12345']), JSON.stringify([origin,'account','github:12345'])]) {
    const misconfigured = createConsole({...config,MESH_OWNER:owner});
    assert.equal((await misconfigured(request('/api/login'))).status,503);
  }
});
