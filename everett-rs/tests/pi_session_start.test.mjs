import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { EventEmitter } from "node:events";
import os from "node:os";
import path from "node:path";
import { PassThrough } from "node:stream";
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { after, test } from "node:test";

const fixtures = [];
const extensionSource = process.env.PI_EXTENSION_SOURCE_REF
  ? execFileSync("git", ["show", `${process.env.PI_EXTENSION_SOURCE_REF}:everett-rs/src/pi_session_start.ts`], { encoding: "utf8" })
  : await readFile(process.env.PI_EXTENSION_SOURCE ?? new URL("../src/pi_session_start.ts", import.meta.url), "utf8");

after(async () => {
  await Promise.all(fixtures.map((dir) => rm(dir, { recursive: true, force: true })));
});

async function fixture(tools, options = {}) {
  const dir = await mkdtemp(path.join(os.tmpdir(), "everett-pi-test-"));
  fixtures.push(dir);
  const executable = path.join(dir, "everett");
  const extension = path.join(dir, "everett.mjs");
  const bindLog = path.join(dir, "binds.jsonl");
  const requestLog = path.join(dir, "requests.jsonl");
  const packageDir = path.join(dir, "node_modules", "typebox");
  await mkdir(packageDir, { recursive: true });
  await writeFile(path.join(packageDir, "package.json"), JSON.stringify({ name: "typebox", type: "module", exports: "./index.js" }));
  await writeFile(path.join(packageDir, "index.js"), "export const Type = { Unsafe: (schema) => schema };\n");
  await writeFile(extension, extensionSource.replaceAll('"__EVERETT_BIN__"', JSON.stringify(executable)));
  await writeFile(executable, `#!/usr/bin/env node
const fs = require("node:fs");
const readline = require("node:readline");
const tools = ${JSON.stringify(tools)};
const options = ${JSON.stringify(options)};
const bindLog = ${JSON.stringify(bindLog)};
const requestLog = ${JSON.stringify(requestLog)};
if (process.argv[2] === "hook" && process.argv[3] === "pi_bind") {
  fs.appendFileSync(bindLog, JSON.stringify({ sessionId: process.argv[4] }) + String.fromCharCode(10));
  process.exit(0);
}
if (process.argv[2] !== "mcp") process.exit(2);
readline.createInterface({ input: process.stdin }).on("line", (line) => {
  const request = JSON.parse(line);
  if (request.method === "notifications/initialized") {
    return;
  }
  if (request.id === undefined) return;
  fs.appendFileSync(requestLog, JSON.stringify({ method: request.method, sessionId: process.env.EVERETT_SESSION_ID || "bootstrap" }) + String.fromCharCode(10));
  let result;
  if (request.method === "initialize") {
    result = { protocolVersion: "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "fixture", version: "1" } };
    const send = () => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + String.fromCharCode(10));
    setTimeout(send, process.env.EVERETT_SESSION_ID ? (options.delaySessionInitializeMs || 0) : 0);
    return;
  } else if (request.method === "tools/list") {
    result = { tools };
  } else if (request.method === "tools/call") {
    if (options.exitOnCall) process.exit(1);
    if (options.holdCall) return;
    result = options.failCall
      ? { isError: true, content: [{ type: "text", text: "fixture tool failed" }] }
      : { content: [{ type: "text", text: process.env.EVERETT_SESSION_ID + ":" + request.params.name }] };
  } else {
    process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { code: -32601, message: "method not found" } }) + String.fromCharCode(10));
    return;
  }
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + String.fromCharCode(10));
});
`);
  await chmod(executable, 0o755);
  return { dir, extension, bindLog, requestLog };
}

async function waitForLog(file, needle) {
  const deadline = Date.now() + 3000;
  while (Date.now() < deadline) {
    try {
      if ((await readFile(file, "utf8")).includes(needle)) return;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error(`timed out waiting for ${needle} in ${file}`);
}

function piApi() {
  const events = new Map();
  const tools = new Map();
  return {
    events,
    tools,
    api: {
      on: (name, handler) => events.set(name, handler),
      registerTool: (tool) => tools.set(tool.name, tool),
    },
  };
}

async function loadExtension(fx, pi) {
  const extension = await import(pathToFileURL(fx.extension));
  await extension.default(pi);
}

test("Pi tools bridge lists schemas and forwards a session-bound call", async () => {
  const fx = await fixture([{ name: "everett_whoami", description: "Read caller identity", inputSchema: { type: "object", properties: {} } }]);
  const pi = piApi();
  await loadExtension(fx, pi.api);
  assert.deepEqual([...pi.tools.keys()], ["everett_whoami"]);
  await pi.events.get("session_start")({}, { sessionManager: { getSessionId: () => "pi-fixture-session" } });
  try {
    const result = await pi.tools.get("everett_whoami").execute("call-1", {});
    assert.equal(result.content[0].text, "pi-fixture-session:everett_whoami");
    assert.equal((await readFile(fx.bindLog, "utf8")).trim(), JSON.stringify({ sessionId: "pi-fixture-session" }));
  } finally {
    await pi.events.get("session_shutdown")();
  }
});

test("Pi bridge registers no tools when Everett returns an empty list", async () => {
  const fx = await fixture([]);
  const pi = piApi();
  await loadExtension(fx, pi.api);
  assert.equal(pi.tools.size, 0);
  assert.equal(pi.events.has("session_start"), false);
});

test("Pi bridge reports MCP tool errors to the caller", async () => {
  const fx = await fixture([{ name: "everett_read", inputSchema: { type: "object", properties: {} } }], { failCall: true });
  const pi = piApi();
  await loadExtension(fx, pi.api);
  await pi.events.get("session_start")({}, { sessionManager: { getSessionId: () => "pi-error-session" } });
  try {
    await assert.rejects(pi.tools.get("everett_read").execute("call-1", {}), /fixture tool failed/);
  } finally {
    await pi.events.get("session_shutdown")();
  }
});

test("Pi bridge rebinds on session start and disconnects when the new session id is invalid", async () => {
  const fx = await fixture([{ name: "everett_whoami", inputSchema: { type: "object", properties: {} } }]);
  const pi = piApi();
  await loadExtension(fx, pi.api);
  const start = pi.events.get("session_start");
  const tool = pi.tools.get("everett_whoami");
  try {
    for (const [reason, sessionId] of [["startup", "session-a"], ["new", "session-b"], ["resume", "session-c"], ["fork", "session-d"]]) {
      await start({ type: "session_start", reason }, { sessionManager: { getSessionId: () => sessionId } });
      const result = await tool.execute("call-1", {});
      assert.match(result.content[0].text, new RegExp(`^${sessionId}:`));
    }
    await start({ type: "session_start", reason: "resume" }, { sessionManager: { getSessionId: () => "" } });
    await assert.rejects(tool.execute("call-invalid", {}), /not connected to this Pi session/);
  } finally {
    await pi.events.get("session_shutdown")();
  }
});

test("Pi bridge handles an abrupt MCP exit during a tool call", async () => {
  const fx = await fixture([{ name: "everett_read", inputSchema: { type: "object", properties: {} } }], { exitOnCall: true });
  const pi = piApi();
  await loadExtension(fx, pi.api);
  await pi.events.get("session_start")({}, { sessionManager: { getSessionId: () => "pi-epipe-session" } });
  try {
    await assert.rejects(pi.tools.get("everett_read").execute("call-epipe", {}), /Everett MCP server exited/);
  } finally {
    await pi.events.get("session_shutdown")();
  }
});

test("Pi bridge aborts a pending MCP call", async () => {
  const fx = await fixture([{ name: "everett_read", inputSchema: { type: "object", properties: {} } }], { holdCall: true });
  const pi = piApi();
  await loadExtension(fx, pi.api);
  await pi.events.get("session_start")({}, { sessionManager: { getSessionId: () => "pi-abort-session" } });
  try {
    const controller = new AbortController();
    const call = pi.tools.get("everett_read").execute("call-abort", {}, controller.signal);
    controller.abort();
    await assert.rejects(call, /aborted/);
  } finally {
    await pi.events.get("session_shutdown")();
  }
});

test("Pi MCP client rejects pending requests when stdin emits EPIPE", async () => {
  const fx = await fixture([]);
  const extension = await import(pathToFileURL(fx.extension));
  const child = new EventEmitter();
  child.stdin = new PassThrough();
  child.stdout = new PassThrough();
  child.exitCode = 0;
  child.signalCode = null;
  child.kill = () => true;
  const client = new extension.EverettMcpClient({}, () => child);
  try {
    const pending = client.request("tools/call", { name: "everett_read" });
    child.stdin.emit("error", Object.assign(new Error("write failed"), { code: "EPIPE" }));
    await assert.rejects(pending, /write failed/);
    assert.equal(client.closed, true);
  } finally {
    await client.close();
    child.stdin.destroy();
    child.stdout.destroy();
  }
});

test("Pi shutdown aborts a session connection that is still initializing", async () => {
  const fx = await fixture([{ name: "everett_read", inputSchema: { type: "object", properties: {} } }], { delaySessionInitializeMs: 500 });
  const pi = piApi();
  await loadExtension(fx, pi.api);
  const start = pi.events.get("session_start")({}, { sessionManager: { getSessionId: () => "pi-pending-session" } });
  try {
    await waitForLog(fx.requestLog, '"sessionId":"pi-pending-session"');
    await pi.events.get("session_shutdown")();
    await start;
    await assert.rejects(pi.tools.get("everett_read").execute("call-after-shutdown", {}), /not connected to this Pi session/);
  } finally {
    await pi.events.get("session_shutdown")();
  }
});

test("Pi runtimes with native MCP registration retain the existing path", async () => {
  const fx = await fixture([]);
  const calls = [];
  const events = new Map();
  const pi = {
    on: (name, handler) => events.set(name, handler),
    registerMcpServer: (...args) => calls.push(["register", ...args]),
    unregisterMcpServer: (...args) => {
      calls.push(["unregister", ...args]);
      throw new Error("not registered yet");
    },
  };
  await loadExtension(fx, pi);
  events.get("session_start")({}, { sessionManager: { getSessionId: () => "pi-native-session" } });
  assert.equal(calls[0][0], "unregister");
  assert.equal(calls[1][0], "register");
  assert.equal(calls[1][1], "everett");
  assert.equal(calls[1][2].env.EVERETT_SESSION_ID, "pi-native-session");
});
