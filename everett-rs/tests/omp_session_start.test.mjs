import assert from "node:assert/strict";
import { appendFileSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import fs from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import { spawnSync } from "node:child_process";
import register, { fixedNudge, startInboxWatcher } from "../src/omp_session_start.mjs";

function helperCommand(settings) {
  const quote = (value) => `'${value.replaceAll("'", "'\\''")}'`;
  return { ...settings, command_prefix: `EVERETT_HOME=${quote(settings.everett_home)} EVERETT_AUTH_PROVIDER=${quote(settings.auth_provider)} ${quote(settings.executable)}` };
}

const command = helperCommand({ executable: "/tmp/Everett bin/'quoted'/everett", everett_home: "/tmp/Everett home/'profile'", auth_provider: "custom 'provider'", process_identity: "test-incarnation", pid: process.pid });

function fixture(t, options = {}) {
  const directory = mkdtempSync(join(tmpdir(), "everett-omp-wake-"));
  const state = { sid: "session-1", idle: true, ids: ["m123"], messages: [], reads: 0 };
  const ctx = { sessionManager: { getSessionId: () => state.sid }, isIdle: () => state.idle };
  const pi = { sendMessage(message, options) { state.messages.push({ message, options }); state.onSend?.(); } };
  const run = (stem) => {
    if (stem === "omp_wake_heartbeat") return JSON.stringify(command);
    state.reads++;
    return JSON.stringify(state.ids);
  };
  writeFileSync(join(directory, "session-1.jsonl"), JSON.stringify({ id: "m123", text: "PRIVATE sender text $(attack)" }) + "\n");
  const start = () => startInboxWatcher(pi, ctx, { directory, run, pollMs: 60_000, debounceMs: 60_000, ...options });
  let watcher = start();
  t.after(() => { watcher?.close(); rmSync(directory, { recursive: true, force: true }); });
  return { directory, state, ctx, pi, run, get watcher() { return watcher; }, restart() { watcher.close(); watcher = start(); } };
}

test("one fixed nudge per ID with triggerTurn and agent attribution, without sender text", (t) => {
  const f = fixture(t);
  f.watcher.scan();
  f.watcher.scan();
  f.state.ids.push("m456");
  f.watcher.scan();
  f.watcher.scan();
  assert.deepEqual(f.state.messages.map(({ message }) => message.content), [fixedNudge("session-1", "m123", command), fixedNudge("session-1", "m456", command)]);
  for (const { message, options } of f.state.messages) {
    assert.equal(message.attribution, "agent");
    assert.equal(options.triggerTurn, true);
    assert.equal(options.deliverAs, "nextTurn");
    assert.match(message.content, /inbox --session session-1 --message m/);
    assert.match(message.content, /reply m\w+ '<your concise result>'/);
    assert.doesNotMatch(message.content, /PRIVATE|sender|attack/);
  }
  assert.equal(existsSync(join(f.directory, "session-1.done")), false);
});

test("busy agent gets no nudge or reservation; idle scan delivers once", (t) => {
  const f = fixture(t);
  f.state.idle = false;
  f.watcher.scan();
  assert.equal(f.state.messages.length, 0);
  assert.equal(f.state.reads, 0);
  assert.equal(existsSync(join(f.directory, "session-1.omp-nudged")), false);
  f.state.idle = true;
  f.watcher.scan();
  assert.equal(f.state.messages.length, 1);
});

test("nudge journal prevents re-delivery after extension restart", (t) => {
  const f = fixture(t);
  f.watcher.scan();
  f.restart();
  f.watcher.scan();
  assert.equal(f.state.messages.length, 1);
  assert.equal(readFileSync(join(f.directory, "session-1.omp-nudged"), "utf8"), "m123\n");
});

test("overlapping watchers reserve each message only once", (t) => {
  const f = fixture(t);
  const other = startInboxWatcher(f.pi, f.ctx, {
    directory: f.directory,
    run: f.run,
    pollMs: 60_000,
    debounceMs: 60_000,
  });
  t.after(() => other.close());
  f.watcher.scan();
  other.scan();
  assert.equal(f.state.messages.length, 1);
});

test("session change and shutdown stop scans and remove capability", (t) => {
  const f = fixture(t);
  const path = join(f.directory, "omp-wake/session-1.json");
  const lease = JSON.parse(readFileSync(path, "utf8"));
  assert.equal(lease.version, 1);
  assert.equal(lease.pid, process.pid);
  assert.equal(lease.session_id, "session-1");
  f.state.sid = "session-2";
  f.watcher.scan();
  assert.equal(f.state.messages.length, 0);
  f.watcher.close();
  assert.equal(existsSync(path), false);
  f.state.sid = "session-1";
  f.watcher.scan();
  assert.equal(f.state.messages.length, 0);
});

test("unsafe IDs cannot enter a command", () => {
  for (const command_prefix of [undefined, "", {}]) assert.equal(fixedNudge("session-1", "m123", { ...command, command_prefix }), "");
  for (const sid of [".", "..", "$(attack)", "session;echo bad"]) assert.equal(fixedNudge(sid, "m123", command), "");
  for (const mid of ["m123;echo bad", "relay-abc", "PRIVATE sender text", {}]) assert.equal(fixedNudge("session-1", mid, command), "");
  for (const end of ["\n", "\r", "\u2028", "\u2029"]) {
    assert.equal(fixedNudge("session-1" + end, "m123", command), "");
    assert.equal(fixedNudge("session-1", "m123" + end, command), "");
  }
});

test("both commands quote the absolute executable and preserve home, auth and reply identity", (t) => {
  const directory = mkdtempSync(join(tmpdir(), "Everett bin 'quoted' "));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const executable = join(directory, "everett");
  writeFileSync(executable, "#!/bin/sh\n/usr/bin/env\n/usr/bin/printf 'arg=%s\\n' \"$@\"\n", { mode: 0o700 });
  const content = fixedNudge("session-1", "m123", helperCommand({ ...command, executable }));
  const commands = [content.split("Use this Everett command: ")[1].split(". Read message")[0],
    content.split("then reply with ")[1].split(". Reply through")[0]];
  for (const [index, text] of commands.entries()) {
    const shell = spawnSync("/bin/sh", ["-c", text], { encoding: "utf8", env: { PATH: "/usr/bin:/bin" } });
    assert.equal(shell.status, 0, shell.stderr);
    assert.ok(shell.stdout.includes("EVERETT_HOME=" + command.everett_home));
    assert.ok(shell.stdout.includes("EVERETT_AUTH_PROVIDER=" + command.auth_provider));
    if (index === 1) {
      assert.ok(shell.stdout.includes("EVERETT_SESSION_ID=session-1"));
      assert.ok(shell.stdout.includes("EVERETT_HARNESS_NAME=omp"));
    }
    assert.ok(shell.stdout.includes(index === 0 ? "arg=inbox" : "arg=reply"));
  }
});

test("malformed helper output and send failures cannot crash or retry a possibly delivered turn", (t) => {
  const f = fixture(t);
  f.state.ids = {};
  assert.doesNotThrow(() => f.watcher.scan());
  f.state.ids = ["m123"];
  let attempts = 0;
  f.pi.sendMessage = () => { attempts++; throw new Error("send failed"); };
  assert.doesNotThrow(() => f.watcher.scan());
  assert.doesNotThrow(() => f.watcher.scan());
  f.restart();
  f.watcher.scan();
  assert.equal(attempts, 1);
});

async function sent(f, action) {
  let timer;
  try {
    await Promise.race([
      new Promise((resolve) => { f.state.onSend = resolve; action(); }),
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("watcher did not deliver within 2 seconds")), 2000); }),
    ]);
  } finally { clearTimeout(timer); }
}

test("fs.watch picks up an inbox change with debouncing", async (t) => {
  const f = fixture(t, { debounceMs: 10 });
  f.state.ids = [];
  await new Promise((resolve) => setTimeout(resolve, 80));
  assert.ok(f.state.reads > 0, "initial empty scan settled");
  assert.equal(f.state.messages.length, 0);
  f.state.ids = ["m456"];
  await sent(f, () => appendFileSync(join(f.directory, "session-1.jsonl"), "\n"));
  assert.equal(f.state.messages.length, 1);
  assert.match(f.state.messages[0].message.content, /--message m456/);
});

test("poll fallback wakes an idle session even without an inbox file event", async (t) => {
  const f = fixture(t, { pollMs: 15 });
  await sent(f, () => {});
  assert.equal(f.state.messages.length, 1);
});

test("a failed journal append after the exclusive claim still attempts one nudge across reloads", (t) => {
  const f = fixture(t);
  fs.mkdirSync(join(f.directory, "session-1.omp-nudged"));
  f.watcher.scan();
  assert.equal(f.state.messages.length, 1);
  assert.ok(existsSync(join(f.directory, "omp-wake/session-1.nudged/m123")));
  f.restart();
  f.watcher.scan();
  assert.equal(f.state.messages.length, 1);
});

test("initial lease publication failure leaves no watcher that can send on later events", async (t) => {
  const f = fixture(t);
  f.watcher.close();
  fs.mkdirSync(join(f.directory, "omp-wake/session-1.json"));
  const original = fs.watch;
  const watches = [];
  t.mock.method(fs, "watch", (...args) => { const w = original(...args); watches.push(w); return w; });
  syncBuiltinESMExports();
  t.after(() => { for (const w of watches) w.close(); t.mock.restoreAll(); syncBuiltinESMExports(); });
  assert.throws(() => startInboxWatcher(f.pi, f.ctx, { directory: f.directory, run: f.run, debounceMs: 10 }));
  appendFileSync(join(f.directory, "session-1.jsonl"), "\n");
  await new Promise((resolve) => setTimeout(resolve, 80));
  assert.equal(f.state.messages.length, 0);
  assert.equal(watches.length, 0);
});

test("registered start, switch, branch and shutdown replace and close watchers and timers", async (t) => {
  const f = fixture(t);
  f.watcher.close();
  f.state.ids = [];
  const handlers = new Map();
  f.pi.on = (event, handler) => handlers.set(event, handler);
  register(f.pi, { directory: f.directory, run: f.run, pollMs: 15, debounceMs: 10 });
  handlers.get("session_start")({}, f.ctx);
  assert.ok(existsSync(join(f.directory, "omp-wake/session-1.json")));
  for (const [event, sid] of [["session_switch", "session-2"], ["session_branch", "session-3"]]) {
    const previous = f.state.sid;
    f.state.sid = sid;
    writeFileSync(join(f.directory, sid + ".jsonl"), "\n");
    handlers.get(event)({}, f.ctx);
    assert.equal(existsSync(join(f.directory, "omp-wake", previous + ".json")), false);
    assert.ok(existsSync(join(f.directory, "omp-wake", sid + ".json")));
  }
  await new Promise((resolve) => setTimeout(resolve, 60));
  handlers.get("session_shutdown")({}, f.ctx);
  const reads = f.state.reads;
  assert.equal(existsSync(join(f.directory, "omp-wake/session-3.json")), false);
  f.state.ids = ["m789"];
  appendFileSync(join(f.directory, "session-3.jsonl"), "\n");
  handlers.get("agent_end")({}, f.ctx);
  await new Promise((resolve) => setTimeout(resolve, 80));
  assert.equal(f.state.reads, reads, "shutdown cancels timers and file events");
  assert.equal(f.state.messages.length, 0);
});
