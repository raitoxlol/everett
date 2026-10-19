import GitHub from "@auth/core/providers/github";
import { convexAuth } from "@convex-dev/auth/server";
import { ConvexCredentials } from "@convex-dev/auth/providers/ConvexCredentials";
import { makeFunctionReference } from "convex/server";
import { checkGithub } from "./hosted";

if (process.env.AUTH_LOG_LEVEL === "DEBUG") throw new Error("Hosted auth must not log credential parameters; use AUTH_LOG_LEVEL=INFO");

export const { auth, signIn, signOut, store, isAuthenticated } = convexAuth({
  providers: [
    GitHub({ profile(profile) {
      checkGithub(profile);
      return { id: String(profile.id), name: profile.login };
    } }),
    ConvexCredentials({ id: "agent", authorize: async (params, ctx) => {
      if (typeof params.token !== "string") throw new Error("Agent token required");
      return await ctx.runQuery(makeFunctionReference<"query">("hosted:authenticateAgent"), { token: params.token });
    } }),
  ],
  jwt: { durationMs: 15 * 60_000, customClaims: async (ctx, { userId }) => {
    return await ctx.runQuery(makeFunctionReference<"query", { userId: string }, Record<string, string>>("hosted:claims"), { userId });
  } },
  callbacks: { redirect: async ({ redirectTo }) => {
    const url = new URL(redirectTo, process.env.CONVEX_SITE_URL);
    if (url.origin !== process.env.CONVEX_SITE_URL || url.pathname !== "/enroll") throw new Error("Invalid auth redirect");
    return url.toString();
  } },
});
