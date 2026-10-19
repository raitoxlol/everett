import type { AuthConfig } from "convex/server";

export default {
  providers: [
    ...(process.env.EVERETT_OWNER_GITHUB_ID && process.env.CONVEX_SITE_URL ? [{ domain: process.env.CONVEX_SITE_URL, applicationID: "convex" }] : []),
    ...(process.env.OIDC_ISSUER && process.env.OIDC_CLIENT_ID ? [{
    domain: process.env.OIDC_ISSUER!,
    applicationID: process.env.OIDC_CLIENT_ID!,
    }] : []),
  ],
} satisfies AuthConfig;
