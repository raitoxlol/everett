import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { createServer } from "node:http";
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rm, unlink, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

const everettBin = process.env.EVERETT_BIN ?? path.resolve("everett-rs/target/debug/everett");
const piSdkEntry = process.env.PI_SDK_ENTRY ?? "/home/ubuntu/pi-runtime-0731/node_modules/@mariozechner/pi-coding-agent/dist/index.js";
const resultPath = process.env.PI_RUNTIME_RESULT ?? "/home/ubuntu/pi-runtime-0731/pi-bridge-evidence/actual-pi-runtime.json";
const profile = await mkdtemp(path.join(os.tmpdir(), "everett-pi-runtime-"));
const credentialPath = path.join(profile, ".everett", "cloudflare-auth.json");
process.env.HOME = profile;
process.env.PI_CODING_AGENT_DIR = path.join(profile, ".pi", "agent");
process.env.PI_OFFLINE = "1";
process.env.EVERETT_NOTIFY = "none";
const { createAgentSession, SessionManager } = await import(pathToFileURL(piSdkEntry).href);
const localVerifierRequests = [];
let session;
let server;

try {
  server = createServer((request, response) => {
    localVerifierRequests.push(`${request.method} ${request.url}`);
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ result: { owner: "fixture-owner", deviceId: "fixture-agent", role: "member" } }));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const issuer = `http://127.0.0.1:${server.address().port}`;
  await mkdir(path.dirname(credentialPath), { recursive: true, mode: 0o700 });
  const credential = JSON.stringify({
    issuer,
    owner: "fixture-owner",
    agent_id: "fixture-agent",
    device_name: "Fixture",
    token: `evt_${randomBytes(32).toString("hex")}`,
  });
  await writeFile(credentialPath, `${credential.slice(0, -1)},"expires_at":18446744073709551615}`, { mode: 0o600, flag: "wx" });

  const env = {
    ...process.env,
    HOME: profile,
    PI_CODING_AGENT_DIR: process.env.PI_CODING_AGENT_DIR,
    PI_OFFLINE: process.env.PI_OFFLINE,
    EVERETT_NOTIFY: "none",
  };
  execFileSync(everettBin, ["install-hooks", "--pi", "--apply"], { env, stdio: "inherit" });
  ({ session } = await createAgentSession({
    cwd: process.cwd(),
    agentDir: env.PI_CODING_AGENT_DIR,
    sessionManager: SessionManager.inMemory(),
    noTools: "builtin",
  }));
  await session.bindExtensions({});
  const everettTools = session.getAllTools().map((tool) => tool.name).filter((name) => name.startsWith("everett_"));
  const tool = session.agent.state.tools.find((candidate) => candidate.name === "everett_whoami");
  assert.ok(tool, `Pi did not activate everett_whoami; found ${JSON.stringify(everettTools)}`);
  const call = await tool.execute("pi-runtime-smoke", {}, undefined, undefined);
  const read = JSON.parse(call.content.find((item) => item.type === "text").text);
  assert.equal(read.harness, "pi");
  assert.equal(read.source, "env EVERETT_SESSION_ID");
  assert.equal(read.identity.account.status, "verified");
  assert.equal(read.identity.client.id, "fixture-agent");
  assert.deepEqual(localVerifierRequests, ["POST /v2/relay/whoami"]);

  const result = {
    piSdkEntry,
    piVersion: "0.73.1",
    exposedEverettTools: everettTools,
    execution: "registered Pi AgentTool.execute; no model call",
    read,
    localVerifierRequests,
    syntheticCredentialRemovedAfterRun: true,
  };
  await mkdir(path.dirname(resultPath), { recursive: true });
  await writeFile(resultPath, `${JSON.stringify(result, null, 2)}\n`, { mode: 0o600 });
  console.log(JSON.stringify({ resultPath, ...result }));
} finally {
  if (session) {
    await session.extensionRunner.emit({ type: "session_shutdown", reason: "quit" });
    await session.dispose();
  }
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  await unlink(credentialPath).catch(() => {});
  await rm(profile, { recursive: true, force: true });
}
