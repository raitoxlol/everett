import { httpRouter, httpActionGeneric as httpAction } from "convex/server";
import { sha256 } from "@noble/hashes/sha256";
import { auth } from "./auth";
import { enrollmentPage } from "./enrollmentPage";

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
export default http;
