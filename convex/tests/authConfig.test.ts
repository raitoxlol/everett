import { afterEach, describe, expect, test, vi } from "vitest";

const original = process.env;
// Convex evaluates auth.config.ts with an environment that throws on reads of unset variables.
function deployment(variables: Record<string, string>) {
  process.env = new Proxy({} as NodeJS.ProcessEnv, {
    get: (_, name) => {
      if (typeof name !== "string" || !(name in variables)) throw new Error(`Environment variable ${String(name)} is used in auth config file but its value was not set.`);
      return variables[name];
    },
  });
}
async function providers() {
  vi.resetModules();
  return (await import("../auth.config")).default.providers;
}
afterEach(() => { process.env = original; });

describe("auth config", () => {
  const site = "https://hosted.convex.site";
  const hosted = { domain: site, applicationID: "convex" };
  test("owner-hosted deployments need no generic OIDC variables", async () => {
    deployment({ CONVEX_SITE_URL: site, EVERETT_OWNER_GITHUB_ID: "12345" });
    expect(await providers()).toEqual([hosted]);
  });
  test("generic OIDC deployments need no owner variable", async () => {
    deployment({ CONVEX_SITE_URL: site, OIDC_ISSUER: "https://login.example", OIDC_CLIENT_ID: "everett-relay" });
    expect(await providers()).toEqual([hosted, { domain: "https://login.example", applicationID: "everett-relay" }]);
  });
});
