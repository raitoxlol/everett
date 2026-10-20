import type { AuthConfig } from "convex/server";

// The hosted provider is always declared: only JWTs signed by this deployment's own
// Convex Auth key verify against it, and every hosted identity is then rechecked
// against EVERETT_OWNER_GITHUB_ID (unset fails closed). Convex throws when an auth
// config reads an unset variable, so hosted deployments that set no OIDC_* would fail
// to push; read those as optional.
const optional = (name: string) => {
  try { return process.env[name]; } catch { return undefined; }
};
const oidcIssuer = optional("OIDC_ISSUER");
const oidcClientId = optional("OIDC_CLIENT_ID");

export default {
  providers: [
    ...(process.env.CONVEX_SITE_URL ? [{ domain: process.env.CONVEX_SITE_URL, applicationID: "convex" }] : []),
    ...(oidcIssuer && oidcClientId ? [{ domain: oidcIssuer, applicationID: oidcClientId }] : []),
  ],
} satisfies AuthConfig;
