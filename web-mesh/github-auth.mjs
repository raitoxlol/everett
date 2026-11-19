import { createHash, randomBytes } from 'node:crypto';

const flowAge = 300;
const callbackPath = '/api/github-callback';
const base64url = /^[A-Za-z0-9_-]{43}$/;

export function githubOwner(config) {
  const owner = JSON.parse(config.MESH_OWNER);
  if (!Array.isArray(owner) || owner.length !== 3 || owner[0] !== config.MESH_ENDPOINT ||
      owner[1] !== 'account' || typeof owner[2] !== 'string' || !/^github:[1-9]\d*$/.test(owner[2])) {
    throw Error('GitHub login requires an account-bound Everett read grant');
  }
  return owner[2];
}

export function readCookie(request, name) {
  const matches = (request.headers.get('Cookie') ?? '').split(';').map(cookie => cookie.trim())
    .filter(cookie => cookie.startsWith(name + '='));
  return matches.length === 1 ? matches[0].slice(name.length + 1) : '';
}

export function githubRoutes(config, { mac, equal, fetchJson, issueSession, reply }) {
  const callback = config.CONSOLE_ORIGIN + callbackPath;
  const flowName = '__Host-everett-github-flow';
  const flowCookie = (value, age = flowAge) =>
    `${flowName}=${value}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=${age}`;
  const signature = value => mac('github-flow:' + value);

  function start() {
    const state = randomBytes(32).toString('base64url');
    const verifier = randomBytes(32).toString('base64url');
    const value = `${Date.now() + flowAge * 1000}.${state}.${verifier}`;
    const url = new URL('https://github.com/login/oauth/authorize');
    url.search = new URLSearchParams({
      client_id: config.GITHUB_CLIENT_ID,
      redirect_uri: callback,
      scope: '',
      state,
      code_challenge: createHash('sha256').update(verifier).digest('base64url'),
      code_challenge_method: 'S256',
    });
    return reply(null, 303, { Location: url.href, 'Set-Cookie': flowCookie(value + '.' + signature(value)) });
  }

  async function finish(request, url) {
    const reject = () => reply(null, 303, { Location: '/dashboard?auth=failed', 'Set-Cookie': flowCookie('', 0) });
    const parts = readCookie(request, flowName).split('.');
    const [expiry = '', state = '', verifier = '', signed = ''] = parts;
    const code = url.searchParams.get('code') ?? '';
    const returnedState = url.searchParams.get('state') ?? '';
    const now = Date.now();
    if (parts.length !== 4 || !/^\d{13}$/.test(expiry) || Number(expiry) <= now ||
        Number(expiry) > now + flowAge * 1000 || !base64url.test(state) || !base64url.test(verifier) ||
        !base64url.test(returnedState) || !equal(state, returnedState) ||
        !equal(signed, signature(parts.slice(0, 3).join('.'))) ||
        url.searchParams.getAll('state').length !== 1 || url.searchParams.getAll('code').length !== 1 ||
        !/^[A-Za-z0-9_-]{1,256}$/.test(code)) return reject();

    try {
      const exchange = await fetchJson('https://github.com/login/oauth/access_token', {
        method: 'POST',
        headers: { Accept: 'application/json', 'Content-Type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({
          client_id: config.GITHUB_CLIENT_ID, client_secret: config.GITHUB_CLIENT_SECRET,
          code, code_verifier: verifier, redirect_uri: callback,
        }),
      });
      if (typeof exchange.access_token !== 'string' || !/^[A-Za-z0-9_-]{1,512}$/.test(exchange.access_token)) return reject();
      const user = await fetchJson('https://api.github.com/user', {
        headers: {
          Authorization: 'Bearer ' + exchange.access_token,
          Accept: 'application/vnd.github+json',
          'User-Agent': 'Everett Dashboard',
        },
      });
      if (!Number.isSafeInteger(user.id) || 'github:' + user.id !== config.githubOwner) return reject();

      const response = reply(null, 303, { Location: '/dashboard', 'Set-Cookie': issueSession() });
      response.headers.append('Set-Cookie', flowCookie('', 0));
      return response;
    } catch {
      return reject();
    }
  }

  return { start, finish };
}
