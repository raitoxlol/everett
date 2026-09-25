# Changelog

## 1.2.0 — 2026-09-25

Install: `brew install raitoxlol/tap/everett` or `pipx install git+https://github.com/raitoxlol/everett`

### Onboarding: Jev key + nightly merge
- `everett onboard` adds a "Smarter routing (optional)" step (curses TUI, plain fallback, and `--yes`), between MCP and backfill: two lines explain that Jev (typesafe.ai) picks the right session when many are running, and that without it Everett uses local matching.
- Paste a key or skip. If a key is already found (`TYPESAFE_API_KEY`, `~/.everett/config.toml`, or `~/.hermes/.env`), the screen shows "Jev key found ✓ (source)" and defaults to skip.
- A pasted key is validated with one test route call (5 s timeout); on success it's saved, on failure you're offered "keep it anyway" or "skip". The key is masked while typing (curses `*`, plain `getpass`) and is never echoed or logged — only the saved/validated status appears in the summary.
- The key is written as `typesafe_api_key` in `~/.everett/config.toml` (created `chmod 600`; every other key and `[section]` is preserved). `--yes --jev-key-env VAR` reads the key from an env var for scripted setup.
- The same step carries an optional "merge shared memory nightly" toggle (see below); `--yes --schedule-merge` opts in non-interactively. The onboarding step count and progress rail updated (6 → 7 steps).

### Nightly auto-merge for the shared core
- `everett trunk schedule [--at HH:MM] [--remove] [--llm claude|codex|none]` prints (default) or, with `--apply`, installs/removes a macOS LaunchAgent (`~/Library/LaunchAgents/dev.everett.core-merge.plist`) that runs `everett trunk merge --llm <x>` nightly (default 04:00, default `claude`). `everett trunk merge` already skips cleanly when the inbox is empty. Logs go to `~/.everett/logs/merge.log`.
- `--apply` writes the plist and runs `launchctl bootstrap`; `--remove --apply` runs `launchctl bootout` and deletes the plist. Every path and the launchctl call are injectable (`trunk_schedule.install/remove(path=, runner=)`), so tests never touch the real `~/Library` or `launchctl`.
- `everett doctor` reports whether nightly merge is scheduled.

## 1.1.0 — 2026-09-25

First public release. It includes the 1.0.0 work prepared on 2026-09-24, which was never published on its own.

### Sessions
- Lists Claude Code, Codex, OMP, Pi, Hermes Agent, and Grok CLI sessions from the stores those harnesses already write. Everett never writes to them. It reads only the first and last 64 KB of transcript files and opens SQLite stores with `mode=ro`.
- Hermes: `~/.hermes/state.db` and `~/.hermes/profiles/*/state.db`. The profile name is kept. Chat-platform sessions (Telegram, Discord, and so on) are list/route only. Cron, oneshot, and webhook runs are hidden as automated.
- Pi: `~/.pi/agent/sessions/<cwd-slug>/*.jsonl`, in the same format as OMP.
- Grok CLI: `~/.grok/sessions/<url-encoded cwd>/<id>/`. Everett reads `summary.json` for the id, folder, title, and `session_kind`, and `prompt_history.jsonl` for the typed requests, falling back to `<user_query>` blocks in `chat_history.jsonl`. A session is running while `~/.grok/active_sessions.json` names it with a live pid. Headless runs are hidden as automated.
- T3 Code: each thread runs Codex, Claude Code, or Grok underneath, and `~/.t3/userdata/state.sqlite` names the underlying session. Those sessions are marked source `t3code` (`[t3code]` in `ls`) and take the thread title when they have none, so nothing is listed twice. Codex sessions started by T3 are also recognized by their `t3code_desktop` originator.
- `everett ls [--json] [--all] [--harness H]`. `--harness` filters before the 40-session cap.

### Jev first
- `everett_send` (and `everett send`) route by default: omit `to` and Everett picks the session (jev when a key is configured, else the local matcher) instead of an agent eyeballing `everett_ls` and sending straight `to` an id. `to` is now documented as for a named session or a reply only. The MCP server instructions and the `everett_ls` / `everett_route` / `everett_send` tool descriptions say so explicitly, and `everett_ls`'s says it is for overview, not for picking a send target.
- The `everett_send` response and the `everett send` CLI report the routing decision (router `jev`/`local`, chosen session, confidence): the CLI prints `routed by jev → [claude] ~/src/api — ... (0.87)` before delivering. On an `ASK` decision nothing is sent; the response now carries `candidates` (up to 3 close sessions) instead of a single `suggested` guess.

### Routing and delivery
- Local router (default without a Jev key): BM25 over each session's card, title, first and last requests, and project folder, weighted by recency. The Jev router is used when a key is set. Both return `SESSION` / `NEW` / `ASK` with a confidence.
- `everett send` waits for the target to go idle (up to 2 minutes), then resumes it headless and prints the reply. `--to <id-prefix|name>` skips routing. `--spawn` starts a new headless session on a confident `NEW` and records it in `~/.everett/spawned.jsonl`.
- Resume and spawn commands: `claude --resume` / `--session-id`, `codex exec resume` / `codex exec`, `omp -r`, `pi --session`, `hermes -p <profile> chat --resume`, and `grok --resume <id> -p` / `grok --session-id <uuid> -p`. `send` refuses T3 Code sessions, because T3 keeps its own resume point and a CLI resume would fork the thread.
- Safety: a hop guard (`EVERETT_HOPS`, max 3, exit 7), refusal to send to the calling session, and spawning only on request.

### Live delivery
- Running sessions are reachable. `send --mode auto` (the default) puts a request for a session with a live harness process into `~/.everett/inbox/<session-id>.jsonl` instead of refusing it or forking it with a headless resume; idle sessions are still resumed headless. `--mode resume|inbox` forces a path. T3 Code sessions now take inbox delivery instead of being refused.
- The session's own hooks inject pending messages (at most 5 per hook, 6,000 characters) marked as coming from Everett with the sender's session and card: Claude Code and Codex at `UserPromptSubmit` and `PostToolUse` (`hookSpecificOutput.additionalContext`; the Codex contract checked against the 0.155.1 binary's embedded schemas), Grok at `PostToolUse` only (it discards `UserPromptSubmit` context), and OMP through its extension (`before_agent_start`, plus `tool_result` steering). About 40 ms per hook, silent on every error.
- Replies: `everett reply <id> "<text>"` or `everett_send(reply_to=…)` deliver to the sender's inbox. `send --wait S` / `everett_send(wait=S)` wait for the reply. `everett inbox` and the `everett_inbox` MCP tool read an inbox directly (for Pi, Hermes, and humans). Reply chains stop at 3 hops.
- `install-hooks` adds the new hooks (backup first, idempotent). `install-mcp --grok` registers the MCP server in `~/.grok/config.toml`. `everett onboard` offers the new hooks.
- Not used: Claude Code's own `<cross-session-message>` socket channel, which has no public way to send.

### Events
- `everett event done|blocked|needs-input|info "<msg>" [--project P]` and `everett_event`. Stored in `~/.everett/events.jsonl`, with each session's current state (and since when) in `~/.everett/state/`. `everett ls` flags blocked and waiting sessions (`⚠ blocked 32h: waiting on staging key`), `everett_ls` returns `state` for each session, and `everett events [--since 24h]` lists them.
- Stop hooks detect events deterministically from the turn's last assistant message: a closing question or an ask ("need you to", "should I", "please confirm") → `needs-input`; "blocked" / "stuck" / "waiting on" / "can't proceed" (not negated, not asked) → `blocked`; otherwise `done`. Repeats of the current state within 10 minutes are dropped.
- `everett subscribe <session|project>` / `everett_subscribe` route events into the subscriber's inbox. `blocked` and `needs-input` notify the human: a macOS notification by default, plus `notify_command` (config or `EVERETT_NOTIFY_COMMAND`) for your own channel, and one escalation after `escalate_minutes` (default 30). `everett events --check` runs the escalation from cron.

### Cards and shared core
- Session cards in `~/.everett/cards/`: the SessionStart hook asks the agent to write one, and the Stop hook writes a deterministic fallback. `everett install-hooks [--claude] [--codex] [--omp] [--grok] [--apply]` backs up the config first and merges idempotently. Grok gets the Stop hook only (`~/.grok/hooks/everett.json`), because Grok ignores `SessionStart` output.
- Shared core: `everett learn` (secret-filtered, capped at 500 characters), `everett trunk merge [--llm claude|codex|none]` (capped at 300 words, with history snapshots), and `everett core`. SessionStart hooks for Claude Code, Codex, and OMP inject the global and project core, at most 350 words.

### For agents
- `everett mcp`: a stdio MCP server built on the standard library, with the tools `everett_ls`, `everett_route`, `everett_send`, `everett_learn`, `everett_core`, `everett_card`, and `everett_whoami`. Calls are logged to `~/.everett/mcp.log`. `everett install-mcp` registers it for Claude Code, Codex, and OMP.
- Caller identity comes from `CLAUDE_CODE_SESSION_ID`, `CODEX_THREAD_ID`, `HERMES_SESSION_ID`, `PI_SESSION_FILE`, or `GROK_SESSION_ID`, and `EVERETT_SESSION_ID` overrides them all.

### Setup
- `everett onboard`: a friendly first-time-setup TUI (curses, with a plain-prompt fallback when not a TTY) -- welcome, detected harnesses, per-hook toggles with the exact files that will change, MCP registration toggles, and an optional card-backfill step (deterministic AUTO cards, no LLM calls) with a progress bar. Nothing is written before the final confirm. `--yes [--span-days N] [--no-backfill] [--no-mcp]` runs it non-interactively.
- `~/.everett/config.toml` (`vault`, `vault_dir`, `default_harness`, `router`, `merge_llm`, Jev key), with environment overrides. No vault path is hardcoded.
- `everett doctor` shows the Python version, session stores (including Grok and T3 Code), hooks, MCP registration, card coverage, router, and vault.
- Install with `pipx install git+https://github.com/raitoxlol/everett` (the package is not on PyPI), `python -m everett`, and `everett --version`. No runtime dependencies. MIT license.

### Verified
Live round trips on throwaway sessions (2026-09-24):
- Claude Code `send` replied in 10.3 s. Codex `send` replied in 43.8 s, including the idle wait; an earlier attempt was correctly refused as busy. OMP `send` replied in 60.5 s.
- `spawn('claude', ...)` replied in 5.6 s, and the new session appeared in `everett ls`.
- Not live-checked: the Hermes, Pi, and Grok resume and spawn commands, which come from each CLI's `--help` and docs. Pi was not installed on the test machine. The Grok and T3 Code listings were checked read-only against real stores.

## 0.x — 2026-09-23

- Session registry for Claude Code, Codex, and OMP; Jev router; `send` with idle wait; session cards with SessionStart and Stop hooks; automatic fallback cards; `everett cards`; vault trunk note.
