import type { AuthConfig } from "convex/server";

export default {
  providers: [{
    domain: process.env.OIDC_ISSUER!,
    applicationID: process.env.OIDC_CLIENT_ID!,
  }],
} satisfies AuthConfig;
