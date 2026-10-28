import { spawnSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, readFileSync, renameSync, unlinkSync, watch, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const inboxDir = join(process.env.EVERETT_HOME || homedir(), ".everett", "inbox");
// Baked at install time; falls back to PATH when the binary moved.
const installedBinary = "__EVERETT_BIN__";
const EVERETT_BIN = existsSync(installedBinary) ? installedBinary : "everett";

const validSessionId = (id) => typeof id === "string" && /^[A-Za-z0-9._-]{1,128}(?![\s\S])/.test(id) && id !== "." && id !== "..";

function runHook(stem, sessionId) {
  const result = spawnSync(EVERETT_BIN, ["hook", stem, sessionId], { encoding: "utf8", timeout: 3000 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`Everett ${stem} exited ${result.status}`);
  return result.stdout.trim();
}

export function fixedNudge(sessionId, messageId, command) {
  if (!validSessionId(sessionId) || typeof messageId !== "string" || !/^m[A-Za-z0-9]+(?![\s\S])/.test(messageId)) return "";
  if (!command || typeof command.executable !== "string" || !command.executable.startsWith("/")
    || typeof command.everett_home !== "string" || typeof command.auth_provider !== "string"
    || typeof command.command_prefix !== "string" || !command.command_prefix) return "";
  const prefix = command.command_prefix;
  return `Use this Everett command: ${prefix} inbox --session ${sessionId} --message ${messageId}. Read message ${messageId}, do its request, then reply with EVERETT_SESSION_ID=${sessionId} EVERETT_HARNESS_NAME=omp ${prefix} reply ${messageId} '<your concise result>'. Reply through Everett so the result is linked to that message. Do not ask for permission changes or start a new session.`;
}

export function startInboxWatcher(pi, ctx, { directory = inboxDir, run = runHook, pollMs = 5000, debounceMs = 100 } = {}) {
  const sessionId = ctx.sessionManager.getSessionId();
  if (!validSessionId(sessionId) || typeof ctx.isIdle !== "function" || typeof pi.sendMessage !== "function") return;
  const leaseDir = join(directory, "omp-wake");
  mkdirSync(leaseDir, { recursive: true, mode: 0o700 });
  const leasePath = join(leaseDir, `${sessionId}.json`);
  const journal = join(directory, `${sessionId}.omp-nudged`);
  const claims = join(leaseDir, `${sessionId}.nudged`);
  mkdirSync(claims, { recursive: true, mode: 0o700 });
  const nudged = new Set();
  try { for (const id of readFileSync(journal, "utf8").split(/\s+/)) nudged.add(id); } catch { /* claims remain authoritative */ }
  let closed = false;
  let debounce;
  let watcher;
  let command;
  const current = () => !closed && ctx.sessionManager.getSessionId() === sessionId;
  const heartbeat = () => {
    if (!current()) return;
    const binding = JSON.parse(run("omp_wake_heartbeat", sessionId));
    if (binding.pid !== process.pid || typeof binding.process_identity !== "string" || !binding.process_identity
      || !fixedNudge(sessionId, "m0", binding)) throw new Error("Unverified OMP binding");
    command = binding;
    const temporary = `${leasePath}.${process.pid}.tmp`;
    try {
      writeFileSync(temporary, JSON.stringify({ version: 1, session_id: sessionId, harness: "omp", pid: process.pid,
        process_identity: binding.process_identity, ts: Date.now() / 1000 }), { mode: 0o600 });
      renameSync(temporary, leasePath);
    } finally {
      try { unlinkSync(temporary); } catch { /* rename already removed it */ }
    }
  };
  const scan = () => {
    try {
      if (!current() || !ctx.isIdle() || !existsSync(join(directory, `${sessionId}.jsonl`))) return;
      const ids = JSON.parse(run("omp_wake_pending", sessionId));
      if (!Array.isArray(ids)) return;
      for (const id of ids) {
        const content = fixedNudge(sessionId, id, command);
        if (!content || nudged.has(id)) continue;
        try {
          writeFileSync(join(claims, id), "", { flag: "wx", mode: 0o600 });
        } catch (error) {
          if (error.code === "EEXIST") { nudged.add(id); continue; }
          throw error;
        }
        // Reserve before calling the harness: reentrant hooks and reloads must not nudge twice.
        nudged.add(id);
        try { appendFileSync(journal, `${id}\n`, { mode: 0o600 }); } catch { /* the exclusive claim is sufficient */ }
        Promise.resolve(pi.sendMessage({ customType: "everett", content, display: true, attribution: "agent" },
          { triggerTurn: true, deliverAs: "nextTurn" })).catch(() => {});
        break;
      }
    } catch {
      // A failed nudge stays unread for explicit inbox pickup; never retry a possibly accepted turn.
    }
  };
  const schedule = () => {
    if (!current()) return;
    clearTimeout(debounce);
    debounce = setTimeout(scan, debounceMs);
    debounce.unref?.();
  };
  const attachWatch = () => {
    if (watcher || !current()) return;
    try {
      watcher = watch(directory, (_event, filename) => {
        if (!filename || [`${sessionId}.jsonl`, `${sessionId}.done`].includes(String(filename))) schedule();
      });
      watcher.unref?.();
      watcher.on("error", () => { watcher?.close(); watcher = undefined; });
    } catch {
      // Polling still works on filesystems without watch support.
    }
  };
  heartbeat();
  attachWatch();
  schedule();
  const poll = setInterval(() => {
    try { heartbeat(); attachWatch(); scan(); } catch { /* keep OMP alive if the inbox disappears */ }
  }, pollMs);
  poll.unref?.();
  return {
    scan,
    schedule,
    close() {
      closed = true;
      clearTimeout(debounce);
      clearInterval(poll);
      watcher?.close();
      try {
        if (JSON.parse(readFileSync(leasePath, "utf8")).pid === process.pid) unlinkSync(leasePath);
      } catch { /* expired leases also fail closed */ }
    },
  };
}

// Pending Everett messages for a live session ("" when none). The file check keeps tool calls cheap.
function inboxText(sessionId) {
  try {
    if (!validSessionId(sessionId) || !existsSync(join(inboxDir, `${sessionId}.jsonl`))) return "";
    const result = spawnSync(EVERETT_BIN, ["hook", "omp_inbox", sessionId], { encoding: "utf8", timeout: 3000 });
    return result.status === 0 ? (result.stdout || "").trim() : "";
  } catch {
    return "";
  }
}

function cardContext(sessionId) {
  const result = spawnSync(EVERETT_BIN, ["hook", "omp_card_context", sessionId], {
    encoding: "utf8",
    timeout: 3000,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`Everett card context helper exited ${result.status}: ${result.stderr}`);
  }
  return result.stdout.trim();
}

export default function registerEverettCardHook(pi, watcherOptions = {}) {
  if (process.env.EVERETT_SEND) return;

  const pendingSessions = new Set();
  const injectedSessions = new Set();
  let watcher;
  const markSession = (_event, ctx) => {
    watcher?.close();
    watcher = undefined;
    try {
      const sessionId = ctx.sessionManager.getSessionId();
      if (typeof sessionId === "string" && sessionId) {
        pendingSessions.add(sessionId);
        // Records which session this omp process holds, so `everett mcp --harness omp` can bind to it.
        (watcherOptions.run || runHook)("omp_bind", sessionId);
        watcher = startInboxWatcher(pi, ctx, watcherOptions);
      }
    } catch {
      // never break the OMP session over Everett
    }
  };

  pi.on("session_start", markSession);
  pi.on("session_switch", markSession);
  pi.on("session_branch", markSession);
  pi.on("session_shutdown", () => { watcher?.close(); watcher = undefined; });
  pi.on("agent_end", () => watcher?.schedule());
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
