import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const contextScript = fileURLToPath(new URL("./omp_card_context.py", import.meta.url));
const inboxScript = fileURLToPath(new URL("./omp_inbox.py", import.meta.url));
const inboxDir = join(process.env.EVERETT_HOME || homedir(), ".everett", "inbox");

// Pending Everett messages for a live session ("" when none). The file check keeps tool calls cheap.
function inboxText(sessionId) {
  try {
    if (!/^[A-Za-z0-9._-]{1,128}$/.test(sessionId) || !existsSync(join(inboxDir, `${sessionId}.jsonl`))) return "";
    const result = spawnSync("python3", [inboxScript, sessionId], { encoding: "utf8", timeout: 3000 });
    return result.status === 0 ? (result.stdout || "").trim() : "";
  } catch {
    return "";
  }
}

function cardContext(sessionId) {
  const result = spawnSync("python3", [contextScript, sessionId], {
    encoding: "utf8",
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`Everett card context helper exited ${result.status}: ${result.stderr}`);
  }
  return result.stdout.trim();
}

export default function registerEverettCardHook(pi) {
  if (process.env.EVERETT_SEND) return;

  const pendingSessions = new Set();
  const injectedSessions = new Set();
  const markSession = (_event, ctx) => {
    const sessionId = ctx.sessionManager.getSessionId();
    if (typeof sessionId === "string" && sessionId) pendingSessions.add(sessionId);
  };

  pi.on("session_start", markSession);
  pi.on("session_switch", markSession);
  pi.on("session_branch", markSession);
  // Live delivery mid-turn: after each tool result, steer pending Everett messages into the turn.
  pi.on("tool_result", (_event, ctx) => {
    try {
      const sessionId = ctx.sessionManager.getSessionId();
      const text = sessionId ? inboxText(sessionId) : "";
      if (!text) return;
      Promise.resolve(pi.sendMessage({ customType: "everett", content: text, display: true },
        { deliverAs: "steer" })).catch(() => {});
    } catch {
      // never break the OMP session over Everett
    }
  });

  pi.on("before_agent_start", (event, ctx) => {
    const sessionId = ctx.sessionManager.getSessionId();
    if (!sessionId) return;
    const parts = [];
    if (pendingSessions.has(sessionId) && !injectedSessions.has(sessionId)) {
      try {
        const context = cardContext(sessionId);
        if (context) parts.push(context);
      } catch {
        // never break the OMP session over a card
      }
      injectedSessions.add(sessionId);
    }
    const messages = inboxText(sessionId); // live delivery at the next turn
    if (messages) parts.push(messages);
    if (!parts.length) return;
    const context = parts.join("\n\n");

    const systemPrompt = Array.isArray(event.systemPrompt)
      ? event.systemPrompt
      : event.systemPrompt
        ? [event.systemPrompt]
        : [];
    return { systemPrompt: [...systemPrompt, context] };
  });
}
