import type { AuthConfig } from "convex/server";

// The hosted provider is always declared: only JWTs signed by this deployment's own
// Convex Auth key verify against it, and every hosted identity is then rechecked
// against EVERETT_OWNER_GITHUB_ID (unset fails closed). Referencing that variable here
// would make `convex deploy` refuse generic-OIDC deployments that leave it unset.
export default {
  providers: [
    ...(process.env.CONVEX_SITE_URL ? [{ domain: process.env.CONVEX_SITE_URL, applicationID: "convex" }] : []),
    ...(process.env.OIDC_ISSUER && process.env.OIDC_CLIENT_ID ? [{
      domain: process.env.OIDC_ISSUER,
      applicationID: process.env.OIDC_CLIENT_ID,
    }] : []),
  ],
} satisfies AuthConfig;
