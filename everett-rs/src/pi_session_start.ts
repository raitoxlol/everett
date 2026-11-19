import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { createInterface } from "node:readline";

const EVERETT_BIN = existsSync("__EVERETT_BIN__") ? "__EVERETT_BIN__" : "everett";
const MCP_PROTOCOL_VERSION = "2025-06-18";
const MCP_REQUEST_TIMEOUT_MS = 10_000;

export class EverettMcpClient {
  constructor(env = process.env, spawnChild = spawn) {
    this.child = spawnChild(EVERETT_BIN, ["mcp"], { env, stdio: ["pipe", "pipe", "ignore"] });
    this.pending = new Map();
    this.nextId = 1;
    this.closed = false;
    this.closePromise = undefined;
    this.lines = createInterface({ input: this.child.stdout });
    this.lines.on("line", (line) => {
      let message;
      try {
        message = JSON.parse(line);
      } catch {
        return;
      }
      if (message?.id === undefined) return;
      const request = this.pending.get(String(message.id));
      if (!request) return;
      if (message.error) request.reject(new Error(message.error.message ?? JSON.stringify(message.error)));
      else request.resolve(message.result);
    });
    this.child.stdin.on("error", (error) => this.fail(error));
    this.child.on("error", (error) => this.fail(error));
    this.child.on("close", (code, signal) => {
      this.fail(new Error(`Everett MCP server exited (${signal ?? code})`));
    });
  }

  fail(error) {
    this.closed = true;
    for (const request of [...this.pending.values()]) request.reject(error);
  }

  request(method, params = {}, { timeout = 0, signal } = {}) {
    if (this.closed) return Promise.reject(new Error("Everett MCP server is not running"));
    const id = this.nextId++;
    const key = String(id);
    return new Promise((resolve, reject) => {
      let timer;
      const finish = (callback) => {
        if (!this.pending.delete(key)) return;
        clearTimeout(timer);
        signal?.removeEventListener("abort", onAbort);
        callback();
      };
      const request = {
        resolve: (result) => finish(() => resolve(result)),
        reject: (error) => finish(() => reject(error)),
      };
      const onAbort = () => {
        request.reject(new Error("Everett MCP request was aborted"));
        void this.close().catch(() => {});
      };
      this.pending.set(key, request);
      if (signal?.aborted) {
        request.reject(new Error("Everett MCP request was aborted"));
        return;
      }
      signal?.addEventListener("abort", onAbort, { once: true });
      if (timeout > 0) timer = setTimeout(() => request.reject(new Error(`${method} timed out`)), timeout);
      try {
        this.child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`, (error) => {
          if (error) request.reject(error);
        });
      } catch (error) {
        request.reject(error);
      }
    });
  }

  notify(method, params = {}) {
    if (this.closed) return;
    this.child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method, params })}\n`);
  }

  async callTool(name, arguments_, signal) {
    const result = await this.request("tools/call", { name, arguments: arguments_ }, { signal });
    const content = Array.isArray(result?.content) ? result.content : [];
    if (result?.isError) {
      const message = content.filter((item) => item.type === "text").map((item) => item.text).join("\n");
      throw new Error(message || `Everett tool ${name} failed`);
    }
    return {
      content: content.filter((item) => item.type === "text" || item.type === "image"),
      details: result?.structuredContent,
    };
  }

  async close() {
    if (this.closePromise) return this.closePromise;
    if (!this.closed) this.fail(new Error("Everett MCP server closed"));
    this.lines.close();
    if (this.child.exitCode !== null || this.child.signalCode !== null) return;
    this.closePromise = new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.child.kill("SIGKILL");
        resolve();
      }, 1000);
      this.child.once("close", () => {
        clearTimeout(timer);
        resolve();
      });
      this.child.kill("SIGTERM");
    });
    await this.closePromise;
  }
}

async function connectMcpServer(env = process.env, signal) {
  const client = new EverettMcpClient(env);
  try {
    await client.request("initialize", {
      protocolVersion: MCP_PROTOCOL_VERSION,
      capabilities: {},
      clientInfo: { name: "everett-pi", version: "1.0.0" },
    }, { timeout: MCP_REQUEST_TIMEOUT_MS, signal });
    client.notify("notifications/initialized");
    return client;
  } catch (error) {
    await client.close();
    throw error;
  }
}

function sessionIdFromContext(ctx) {
  const sessionId = ctx?.sessionManager?.getSessionId?.();
  return typeof sessionId === "string" && sessionId ? sessionId : undefined;
}

function bindSession(sessionId) {
  spawnSync(EVERETT_BIN, ["hook", "pi_bind", sessionId], { timeout: 3000, stdio: "ignore" });
}

function report(message, error) {
  console.error(`[Everett Pi] ${message}: ${error instanceof Error ? error.message : String(error)}`);
}

function registerNativeMcpServer(pi) {
  const bind = (_event, ctx) => {
    try {
      const sessionId = sessionIdFromContext(ctx);
      if (!sessionId) return;
      bindSession(sessionId);
      try {
        pi.unregisterMcpServer?.("everett");
      } catch {
        // not registered yet
      }
      pi.registerMcpServer("everett", {
        command: EVERETT_BIN,
        args: ["mcp"],
        env: { EVERETT_SESSION_ID: sessionId, EVERETT_HARNESS_NAME: "pi" },
      });
    } catch (error) {
      report("could not register MCP server", error);
    }
  };

  pi.on("session_start", bind);
  pi.on("session_switch", bind);
  pi.on("session_branch", bind);
}

async function registerToolBridge(pi) {
  let client;
  let listed;
  try {
    client = await connectMcpServer();
    listed = await client.request("tools/list", {}, { timeout: MCP_REQUEST_TIMEOUT_MS });
  } catch (error) {
    report("could not discover Everett tools", error);
    return;
  } finally {
    await client?.close();
  }

  const tools = Array.isArray(listed?.tools) ? listed.tools.filter((tool) => typeof tool?.name === "string" && tool.name) : [];
  if (tools.length === 0) return;

  let Type;
  try {
    ({ Type } = await import("typebox"));
  } catch (error) {
    report("could not load Pi tool schemas", error);
    return;
  }

  let generation = 0;
  let activeClient;
  let connecting;
  let connectionController;
  let boundSessionId;
  const getSessionClient = async () => {
    while (connecting) {
      const pendingConnection = connecting;
      const sessionClient = await pendingConnection;
      if (sessionClient && !sessionClient.closed) return sessionClient;
      if (connecting === pendingConnection) {
        connecting = undefined;
        connectionController = undefined;
      }
    }
    if (activeClient && !activeClient.closed) return activeClient;
    await activeClient?.close();
    activeClient = undefined;
    const sessionId = boundSessionId;
    if (!sessionId) return undefined;

    const currentGeneration = generation;
    const controller = new AbortController();
    connectionController = controller;
    const pendingConnection = (async () => {
      try {
        const sessionClient = await connectMcpServer({
          ...process.env,
          EVERETT_SESSION_ID: sessionId,
          EVERETT_HARNESS_NAME: "pi",
        }, controller.signal);
        if (currentGeneration !== generation || sessionId !== boundSessionId) {
          await sessionClient.close();
          return undefined;
        }
        activeClient = sessionClient;
        return sessionClient;
      } catch (error) {
        if (currentGeneration === generation && !controller.signal.aborted) report("could not reconnect to Everett", error);
        return undefined;
      }
    })();
    connecting = pendingConnection;
    try {
      return await pendingConnection;
    } finally {
      if (connecting === pendingConnection) {
        connecting = undefined;
        connectionController = undefined;
      }
    }
  };
  const start = async (_event, ctx) => {
    const sessionId = sessionIdFromContext(ctx);
    const currentGeneration = ++generation;
    const previousClient = activeClient;
    const previousConnection = connecting;
    const previousController = connectionController;
    activeClient = undefined;
    connecting = undefined;
    connectionController = undefined;
    previousController?.abort();
    if (!sessionId) {
      boundSessionId = undefined;
      await previousClient?.close();
      await previousConnection;
      return;
    }

    boundSessionId = sessionId;
    const controller = new AbortController();
    connectionController = controller;
    connecting = (async () => {
      try {
        await previousClient?.close();
        await previousConnection;
        if (currentGeneration !== generation) return undefined;
        bindSession(sessionId);
        const sessionClient = await connectMcpServer({
          ...process.env,
          EVERETT_SESSION_ID: sessionId,
          EVERETT_HARNESS_NAME: "pi",
        }, controller.signal);
        if (currentGeneration !== generation) {
          await sessionClient.close();
          return undefined;
        }
        activeClient = sessionClient;
        return sessionClient;
      } catch (error) {
        if (currentGeneration === generation && !controller.signal.aborted) report("could not connect to Everett", error);
        return undefined;
      }
    })();
    await connecting;
    if (currentGeneration === generation) {
      connecting = undefined;
      connectionController = undefined;
    }
  };
  const shutdown = async () => {
    generation += 1;
    boundSessionId = undefined;
    const previousClient = activeClient;
    const previousConnection = connecting;
    const previousController = connectionController;
    activeClient = undefined;
    connecting = undefined;
    connectionController = undefined;
    previousController?.abort();
    await previousClient?.close();
    await previousConnection;
  };

  pi.on("session_start", start);
  pi.on("session_shutdown", shutdown);
  for (const tool of tools) {
    try {
      pi.registerTool({
        name: tool.name,
        label: tool.name,
        description: typeof tool.description === "string" ? tool.description : tool.name,
        parameters: Type.Unsafe(tool.inputSchema ?? { type: "object", properties: {} }),
        execute: async (_toolCallId, params, signal) => {
          const sessionClient = await getSessionClient();
          if (!sessionClient) throw new Error("Everett MCP is not connected to this Pi session");
          return sessionClient.callTool(tool.name, params, signal);
        },
      });
    } catch (error) {
      report(`could not register ${tool.name}`, error);
    }
  }
}

export default async function registerEverett(pi) {
  if (process.env.EVERETT_SEND) return;
  if (typeof pi.registerMcpServer === "function") {
    registerNativeMcpServer(pi);
    return;
  }
  if (typeof pi.registerTool === "function") await registerToolBridge(pi);
}
