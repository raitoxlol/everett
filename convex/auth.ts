import GitHub from "@auth/core/providers/github";
import { convexAuth } from "@convex-dev/auth/server";
import { ConvexCredentials } from "@convex-dev/auth/providers/ConvexCredentials";
import { makeFunctionReference } from "convex/server";
import { agentCredential, checkGithub } from "./hosted";

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
    const credential = await ctx.db.query("credentials").withIndex("by_auth_user", q => q.eq("authUserId", userId)).unique();
    if (!credential) return {};
    const p = await agentCredential(ctx, userId);
    return { everett_agent: p.credential.agentId, everett_org: p.credential.subject };
  } },
  callbacks: { redirect: async ({ redirectTo }) => {
    const url = new URL(redirectTo, process.env.CONVEX_SITE_URL);
    if (url.origin !== process.env.CONVEX_SITE_URL || url.pathname !== "/enroll") throw new Error("Invalid auth redirect");
    return url.toString();
  } },
});
