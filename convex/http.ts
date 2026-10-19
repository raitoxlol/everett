import { httpRouter, httpActionGeneric as httpAction } from "convex/server";
import { sha256 } from "@noble/hashes/sha256";
import { auth } from "./auth";
import { enrollmentPage } from "./enrollmentPage";
import * as oauth from "./oauth";

const http = httpRouter();
auth.addHttpRoutes(http);
http.route({ path: "/enroll", method: "GET", handler: httpAction(async () => {
  const cloud = process.env.CONVEX_CLOUD_URL;
  if (!cloud) return new Response("Deployment URL is not configured", { status: 503 });
  const page = enrollmentPage(cloud);
  const script = page.split("<script>")[1].split("</script>")[0];
  const digest = btoa(String.fromCharCode(...sha256(new TextEncoder().encode(script))));
  return new Response(page, { headers: {
    "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store",
    "Content-Security-Policy": `default-src 'none'; script-src 'sha256-${digest}'; style-src 'unsafe-inline'; connect-src ${cloud}; base-uri 'none'; frame-ancestors 'none'; form-action 'none'`,
    "Referrer-Policy": "no-referrer", "X-Content-Type-Options": "nosniff",
  } });
}) });
http.route({ path: "/.well-known/oauth-authorization-server", method: "GET", handler: oauth.authorizationServerMetadata });
http.route({ path: "/oauth/authorize", method: "GET", handler: oauth.authorize });
http.route({ path: "/oauth/token", method: "POST", handler: oauth.token });
http.route({ path: "/oauth/register", method: "POST", handler: oauth.registerClient });
http.route({ path: "/oauth/introspect", method: "POST", handler: oauth.introspectToken });
http.route({ path: "/oauth/revoke", method: "POST", handler: oauth.revoke });
export default http;
