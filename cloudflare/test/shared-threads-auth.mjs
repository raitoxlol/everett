import { FIXTURE_ACCOUNTS } from './shared-threads-data.mjs';

const accountsByToken = new Map();

export default {
  async fetch(request) {
    const path = new URL(request.url).pathname;
    if (request.method === 'POST' && path === '/__test/set-principal') {
      const { token, principal } = await request.json();
      if (!Object.values(FIXTURE_ACCOUNTS).some(account => account.token === token)) {
        return Response.json({ error: 'invalid_test_account' }, { status: 403 });
      }
      accountsByToken.set(token, principal);
      return Response.json({ ok: true });
    }
    if (request.method !== 'POST' || path !== '/verify-agent') return Response.json({ error: 'not_found' }, { status: 404 });
    const authorization = request.headers.get('Authorization') ?? '';
    const principal = authorization.startsWith('Bearer ')
      ? accountsByToken.get(authorization.slice(7))
      : undefined;
    if (!principal) return Response.json({ error: 'invalid_token' }, { status: 401 });
    return Response.json({ ...principal, expiresAt: Date.now() + 3_600_000 });
  },
};
