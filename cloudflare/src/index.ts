import { ZodError, z } from 'zod';
import { bindingVerifier, boundedText, type PrincipalVerifier } from './auth';
import { namespace, principalSchema, RelayError } from './protocol';
import { OwnerRelay, type RelayResult } from './owner-relay';
import { HostedAuth } from './hosted-auth';
export { OwnerRelay, HostedAuth };
export interface Env {
  OWNERS: DurableObjectNamespace<OwnerRelay>;
  AUTHENTICATOR?: Fetcher;
  AUTH_STATE?: DurableObjectNamespace<HostedAuth>;
  EVERETT_ISSUER?: string;
}
export const operations = ['whoami', 'register', 'device', 'devices', 'exposeSession', 'sessionDirectory', 'credentials', 'memberships', 'revokeAgent', 'revokeMember', 'rotate', 'enqueue', 'pending', 'accept', 'observe', 'status'] as const;
export type Operation = typeof operations[number];

function hostedVerifier(env: Env): PrincipalVerifier {
  if (!env.EVERETT_ISSUER || !env.AUTH_STATE) return bindingVerifier(env.AUTHENTICATOR);
  return { async verify(request) {
    const bearer = request.headers.get('Authorization');
    if (!bearer || !/^Bearer evt_[0-9a-f]{64}$/.test(bearer)) throw new RelayError('invalid_token', 401);
    const auth = env.AUTH_STATE!.get(env.AUTH_STATE!.idFromName(env.EVERETT_ISSUER!));
    const principal = await auth.verifyAgent(bearer.slice(7));
    if (!principal) throw new RelayError('invalid_token', 401);
    return principal;
  } };
}

export async function handleRequest(request: Request, env: Env, verifier: PrincipalVerifier = hostedVerifier(env)): Promise<Response> {
  const url = new URL(request.url);
  if (request.method === 'GET' && url.pathname === '/health') return Response.json({ backend: 'cloudflare', status: env.AUTHENTICATOR || env.EVERETT_ISSUER ? 'configuration-present' : 'authentication-unconfigured' });
  if (url.pathname === '/.well-known/oauth-authorization-server' || url.pathname === '/enroll' ||
    ['/auth/', '/enroll/', '/v2/auth/', '/oauth/'].some(prefix => url.pathname.startsWith(prefix))) {
    if (!env.EVERETT_ISSUER || !env.AUTH_STATE) return Response.json({ error: 'authentication_unconfigured' }, { status: 503, headers: { 'Cache-Control': 'no-store' } });
    return env.AUTH_STATE.get(env.AUTH_STATE.idFromName(env.EVERETT_ISSUER)).fetch(new Request(request, { redirect: 'manual' }));
  }
  const op = url.pathname.match(/^\/v2\/relay\/([A-Za-z]+)$/)?.[1];
  const metadata = op === 'deviceLabels' || op === 'renameDevice';
  if (!op || (!metadata && !operations.includes(op as Operation))) return Response.json({ error: 'not_found' }, { status: 404 });
  if (request.method !== 'POST') return Response.json({ error: 'method_not_allowed' }, { status: 405, headers: { Allow: 'POST' } });
  try {
    const p = principalSchema.parse(await verifier.verify(request));
    if (p.expiresAt <= Date.now()) throw new RelayError('invalid_token', 401);
    if (!request.headers.get('Content-Type')?.startsWith('application/json')) throw new RelayError('json_required', 415);
    const args: unknown = JSON.parse(await boundedText(request.body, 150000));
    const stub = env.OWNERS.get(env.OWNERS.idFromName(namespace(p)));
    if (metadata) {
      if (!env.AUTH_STATE || !env.EVERETT_ISSUER) throw new RelayError('metadata_unavailable', 404);
      const name = op === 'renameDevice'
        ? z.strictObject({ name: z.string().trim().min(1).max(100).regex(/^[^\x00-\x1f\x7f]+$/) }).parse(args).name
        : (z.strictObject({}).parse(args), undefined);
      const auth = env.AUTH_STATE.get(env.AUTH_STATE.idFromName(env.EVERETT_ISSUER));
      const result = await auth.relayDeviceLabels(p, name);
      if (!result.ok) return Response.json({ error: result.error }, { status: result.status, headers: { 'Cache-Control': 'no-store' } });
      return Response.json({ result: result.value }, { headers: { 'Cache-Control': 'no-store' } });
    }
    const result: RelayResult = JSON.parse(await stub.relay(p, op as Operation, args));
    if (!result.ok) return Response.json({ error: result.error }, { status: result.status, headers: { 'Cache-Control': 'no-store' } });
    return Response.json({ result: result.value }, { headers: { 'Cache-Control': 'no-store' } });
  } catch (error) {
    const known = error instanceof RelayError;
    const validation = error instanceof ZodError || error instanceof SyntaxError || error instanceof TypeError;
    return Response.json({ error: known ? error.code : validation ? 'invalid_request' : 'service_unavailable' }, {
      status: known ? error.status : validation ? 400 : 503,
      headers: { 'Cache-Control': 'no-store' },
    });
  }
}
export default {
  fetch(request: Request, env: Env) { return handleRequest(request, env); },
} satisfies ExportedHandler<Env>;
