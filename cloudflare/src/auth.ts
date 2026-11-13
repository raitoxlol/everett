import { principalSchema, RelayError, type Principal } from './protocol';
import { boundedText } from './http-body';

export interface PrincipalVerifier {
  verify(request: Request): Promise<Principal>;
}

export function bindingVerifier(binding?: Fetcher): PrincipalVerifier {
  return {
    async verify(request) {
      if (!binding) throw new RelayError('authentication_unconfigured', 503);
      const authorization = request.headers.get('Authorization');
      if (!authorization?.startsWith('Bearer ') || authorization.length > 16384) throw new RelayError('sign_in_required', 401);
      const response = await binding.fetch('https://everett-auth.internal/verify-agent', {
        method: 'POST', headers: { Authorization: authorization }, redirect: 'manual',
      });
      if (response.status === 401 || response.status === 403) throw new RelayError('invalid_token', 401);
      if (!response.ok) throw new RelayError('authentication_unavailable', 503);
      const text = await boundedText(response.body, 16384);
      const principal = principalSchema.parse(JSON.parse(text));
      if (principal.expiresAt <= Date.now()) throw new RelayError('invalid_token', 401);
      return principal;
    },
  };
}
