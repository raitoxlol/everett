import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";

// Baked at install time; falls back to PATH when the binary moved.
const EVERETT_BIN = existsSync("__EVERETT_BIN__") ? "__EVERETT_BIN__" : "everett";

// Pi starts MCP servers without the session id, so this extension registers Everett's server
// per session with EVERETT_SESSION_ID set, and records the binding for `everett mcp --harness pi`.
export default function registerEverett(pi) {
  if (process.env.EVERETT_SEND) return;

  const bind = (_event, ctx) => {
    try {
      const sessionId = ctx.sessionManager.getSessionId();
      if (typeof sessionId !== "string" || !sessionId) return;
      spawnSync(EVERETT_BIN, ["hook", "pi_bind", sessionId], { timeout: 3000 });
      try {
        pi.unregisterMcpServer("everett");
      } catch {
        // not registered yet
      }
      pi.registerMcpServer("everett", {
        command: EVERETT_BIN,
        args: ["mcp"],
        env: { EVERETT_SESSION_ID: sessionId, EVERETT_HARNESS_NAME: "pi" },
      });
    } catch {
      // never break the Pi session over Everett
    }
  };

  pi.on("session_start", bind);
  pi.on("session_switch", bind);
  pi.on("session_branch", bind);
}
