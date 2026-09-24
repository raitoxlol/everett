import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const contextScript = fileURLToPath(new URL("./omp_card_context.py", import.meta.url));

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
  pi.on("before_agent_start", (event, ctx) => {
    const sessionId = ctx.sessionManager.getSessionId();
    if (!sessionId || !pendingSessions.has(sessionId) || injectedSessions.has(sessionId)) return;

    let context;
    try {
      context = cardContext(sessionId);
    } catch {
      return; // never break the OMP session over a card
    }
    if (!context) return;
    injectedSessions.add(sessionId);

    const systemPrompt = Array.isArray(event.systemPrompt)
      ? event.systemPrompt
      : event.systemPrompt
        ? [event.systemPrompt]
        : [];
    return { systemPrompt: [...systemPrompt, context] };
  });
}
