import type { OwnerRelay } from './owner-relay';
import { RelayError } from './protocol';

export interface HostedAuthEnv {
  OWNERS: DurableObjectNamespace<OwnerRelay>;
  EVERETT_ISSUER?: string;
  EVERETT_PUBLIC_HOSTS?: string;
  AUTH_GITHUB_ID?: string;
  AUTH_GITHUB_SECRET?: string;
  EVERETT_OWNER_GITHUB_ID?: string;
  EVERETT_OWNER_GITHUB_LOGIN?: string;
  EVERETT_ENROLLMENT?: string;
  EVERETT_OAUTH_REDIRECT_URIS?: string;
  EVERETT_OAUTH_RESOURCES?: string;
  EVERETT_OAUTH_CLIENT_IDS?: string;
  EVERETT_OAUTH_INTROSPECTION_SECRET?: string;
}
export type EnrollmentMode = 'owner' | 'public';
export interface HostedConfig {
  issuer: string; publicHosts: string[]; secureCookies: boolean; githubId: string; githubSecret: string;
  ownerId: string; ownerLogin: string; enrollment: EnrollmentMode; redirects: string[]; resources: string[];
  clients: string[]; introspectionSecret: string;
}
const list = (raw?: string) => [...new Set((raw ?? '').split(',').map(v => v.trim()).filter(Boolean))];
export function exactUrl(raw: string, loopback = false): string {
  let url: URL;
  try { url = new URL(raw); } catch { throw new RelayError('authentication_unconfigured', 503); }
  const local = loopback && url.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname);
  if ((!local && url.protocol !== 'https:') || url.username || url.password || url.hash || url.href !== raw || raw.length > 2048) throw new RelayError('authentication_unconfigured', 503);
  return raw;
}
export function exactOrigin(raw: string, loopback = false): string {
  const parsed = new URL(exactUrl(`${raw}/`, loopback));
  if (parsed.origin !== raw || parsed.pathname !== '/' || parsed.search) throw new RelayError('authentication_unconfigured', 503);
  return raw;
}
export function hostedConfig(env: HostedAuthEnv): HostedConfig {
  const issuer = exactOrigin(env.EVERETT_ISSUER ?? '', true);
  const extra = list(env.EVERETT_PUBLIC_HOSTS).map(v => exactOrigin(v, true));
  const publicHosts = [...new Set([issuer, ...extra])];
  if (publicHosts.length > 16) throw new RelayError('authentication_unconfigured', 503);
  const ownerId = env.EVERETT_OWNER_GITHUB_ID ?? '', ownerLogin = env.EVERETT_OWNER_GITHUB_LOGIN ?? '';
  const githubId = env.AUTH_GITHUB_ID ?? '', githubSecret = env.AUTH_GITHUB_SECRET ?? '';
  const enrollment = env.EVERETT_ENROLLMENT ?? 'owner';
  if (!/^[1-9][0-9]{0,19}$/.test(ownerId) || !/^[A-Za-z0-9-]{1,39}$/.test(ownerLogin) || !githubId || githubId.length > 256 || !githubSecret || githubSecret.length > 1024) throw new RelayError('authentication_unconfigured', 503);
  if (enrollment !== 'owner' && enrollment !== 'public') throw new RelayError('authentication_unconfigured', 503);
  const redirects = list(env.EVERETT_OAUTH_REDIRECT_URIS).map(v => exactUrl(v));
  const resources = list(env.EVERETT_OAUTH_RESOURCES).map(v => exactUrl(v, true));
  const clients = list(env.EVERETT_OAUTH_CLIENT_IDS).map(v => exactUrl(v));
  for (const raw of clients) {
    const u = new URL(raw), host = u.hostname.toLowerCase();
    if (u.pathname === '/' || host === 'localhost' || host.endsWith('.localhost') || host.endsWith('.local') || /^[0-9.]+$/.test(host) || host.includes(':')) throw new RelayError('authentication_unconfigured', 503);
  }
  if (redirects.length > 16 || resources.length > 16 || clients.length > 16) throw new RelayError('authentication_unconfigured', 503);
  return { issuer, publicHosts, secureCookies: new URL(issuer).protocol === 'https:', githubId, githubSecret, ownerId, ownerLogin, enrollment,
    redirects, resources, clients, introspectionSecret: env.EVERETT_OAUTH_INTROSPECTION_SECRET ?? '' };
}
export function requirePublicHost(url: URL, c: HostedConfig): string {
  if (!c.publicHosts.includes(url.origin)) throw new RelayError('origin_denied', 403);
  return url.origin;
}
export function requireOAuth(c: HostedConfig) {
  if (!c.redirects.length || !c.resources.length || !c.clients.length || c.introspectionSecret.length < 32 || c.introspectionSecret.length > 1024) throw new RelayError('oauth_unconfigured', 503);
}
export const hostedAuthPaths = (path: string) => path === '/enroll' || path.startsWith('/enroll/') || path.startsWith('/auth/') || path.startsWith('/v2/auth/') || path.startsWith('/oauth/') || path === '/.well-known/oauth-authorization-server';
