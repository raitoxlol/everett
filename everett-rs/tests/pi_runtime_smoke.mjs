import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { createServer } from "node:http";
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, unlink, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const everettBin = process.env.EVERETT_BIN ? path.resolve(process.env.EVERETT_BIN) : path.join(repoRoot, "everett-rs", "target", "debug", "everett");
if (!process.env.PI_SDK_ENTRY) {
  throw new Error("PI_SDK_ENTRY is required; set it to the installed Pi SDK entry file, e.g. PI_SDK_ENTRY=/path/to/node_modules/@mariozechner/pi-coding-agent/dist/index.js");
}
const piSdkEntry = path.resolve(process.env.PI_SDK_ENTRY);
const resultPath = process.env.PI_RUNTIME_RESULT ? path.resolve(process.env.PI_RUNTIME_RESULT) : undefined;
async function readPiPackageVersion(entry) {
  let directory = path.dirname(entry);
  while (true) {
    try {
      const metadata = JSON.parse(await readFile(path.join(directory, "package.json"), "utf8"));
      if (metadata.name === "@mariozechner/pi-coding-agent" && typeof metadata.version === "string") return metadata.version;
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    const parent = path.dirname(directory);
    if (parent === directory) break;
    directory = parent;
  }
  throw new Error(`Could not find @mariozechner/pi-coding-agent package metadata above PI_SDK_ENTRY=${entry}`);
}
const piVersion = await readPiPackageVersion(piSdkEntry);
const tempRoot = await mkdtemp(path.join(os.tmpdir(), "everett-pi-runtime-"));
const profile = path.join(tempRoot, "home");
const workingDirectory = path.join(tempRoot, "work");
await mkdir(profile, { mode: 0o700 });
await mkdir(workingDirectory, { mode: 0o700 });
const credentialPath = path.join(profile, ".everett", "cloudflare-auth.json");
const sanitizedEnvironment = {
  PATH: process.env.PATH ?? "/usr/bin:/bin",
  TMPDIR: process.env.TMPDIR ?? os.tmpdir(),
  LANG: process.env.LANG ?? "C.UTF-8",
  LC_ALL: process.env.LC_ALL ?? "C.UTF-8",
  TERM: process.env.TERM ?? "dumb",
  HOME: profile,
  PI_CODING_AGENT_DIR: path.join(profile, ".pi", "agent"),
  PI_OFFLINE: "1",
  EVERETT_NOTIFY: "none",
};
for (const name of Object.keys(process.env)) delete process.env[name];
Object.assign(process.env, sanitizedEnvironment);
process.chdir(workingDirectory);
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
    ...sanitizedEnvironment,
  };
  execFileSync(everettBin, ["install-hooks", "--pi", "--apply"], { env, cwd: workingDirectory, stdio: "inherit" });
  ({ session } = await createAgentSession({
    cwd: workingDirectory,
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
    piPackage: "@mariozechner/pi-coding-agent",
    piVersion,
    exposedEverettTools: everettTools,
    execution: "registered Pi AgentTool.execute; no model call",
    read,
    localVerifierRequests,
    syntheticCredentialRemovedAfterRun: true,
  };
  if (resultPath) {
    await mkdir(path.dirname(resultPath), { recursive: true });
    await writeFile(resultPath, `${JSON.stringify(result, null, 2)}\n`, { mode: 0o600 });
    console.log(`Pi runtime result saved to ${resultPath}`);
  } else {
    console.log(JSON.stringify(result));
  }
} finally {
  if (session) {
    await session.extensionRunner.emit({ type: "session_shutdown", reason: "quit" });
    await session.dispose();
  }
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  await unlink(credentialPath).catch(() => {});
  await rm(tempRoot, { recursive: true, force: true });
}
