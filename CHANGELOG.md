# Changelog

## 1.1.0 — 2026-09-25

First public release. It includes the 1.0.0 work prepared on 2026-09-24, which was never published on its own.

### Sessions
- Lists Claude Code, Codex, OMP, Pi, Hermes Agent, and Grok CLI sessions from the stores those harnesses already write. Everett never writes to them. It reads only the first and last 64 KB of transcript files and opens SQLite stores with `mode=ro`.
- Hermes: `~/.hermes/state.db` and `~/.hermes/profiles/*/state.db`. The profile name is kept. Chat-platform sessions (Telegram, Discord, and so on) are list/route only. Cron, oneshot, and webhook runs are hidden as automated.
- Pi: `~/.pi/agent/sessions/<cwd-slug>/*.jsonl`, in the same format as OMP.
- Grok CLI: `~/.grok/sessions/<url-encoded cwd>/<id>/`. Everett reads `summary.json` for the id, folder, title, and `session_kind`, and `prompt_history.jsonl` for the typed requests, falling back to `<user_query>` blocks in `chat_history.jsonl`. A session is running while `~/.grok/active_sessions.json` names it with a live pid. Headless runs are hidden as automated.
- T3 Code: each thread runs Codex, Claude Code, or Grok underneath, and `~/.t3/userdata/state.sqlite` names the underlying session. Those sessions are marked source `t3code` (`[t3code]` in `ls`) and take the thread title when they have none, so nothing is listed twice. Codex sessions started by T3 are also recognized by their `t3code_desktop` originator.
- `everett ls [--json] [--all] [--harness H]`. `--harness` filters before the 40-session cap.

### Routing and delivery
- Local router (default without a Jev key): BM25 over each session's card, title, first and last requests, and project folder, weighted by recency. The Jev router is used when a key is set. Both return `SESSION` / `NEW` / `ASK` with a confidence.
- `everett send` waits for the target to go idle (up to 2 minutes), then resumes it headless and prints the reply. `--to <id-prefix|name>` skips routing. `--spawn` starts a new headless session on a confident `NEW` and records it in `~/.everett/spawned.jsonl`.
- Resume and spawn commands: `claude --resume` / `--session-id`, `codex exec resume` / `codex exec`, `omp -r`, `pi --session`, `hermes -p <profile> chat --resume`, and `grok --resume <id> -p` / `grok --session-id <uuid> -p`. `send` refuses T3 Code sessions, because T3 keeps its own resume point and a CLI resume would fork the thread.
- Safety: a hop guard (`EVERETT_HOPS`, max 3, exit 7), refusal to send to the calling session, and spawning only on request.

### Cards and shared core
- Session cards in `~/.everett/cards/`: the SessionStart hook asks the agent to write one, and the Stop hook writes a deterministic fallback. `everett install-hooks [--claude] [--codex] [--omp] [--grok] [--apply]` backs up the config first and merges idempotently. Grok gets the Stop hook only (`~/.grok/hooks/everett.json`), because Grok ignores `SessionStart` output.
- Shared core: `everett learn` (secret-filtered, capped at 500 characters), `everett trunk merge [--llm claude|codex|none]` (capped at 300 words, with history snapshots), and `everett core`. SessionStart hooks for Claude Code, Codex, and OMP inject the global and project core, at most 350 words.

### For agents
- `everett mcp`: a stdio MCP server built on the standard library, with the tools `everett_ls`, `everett_route`, `everett_send`, `everett_learn`, `everett_core`, `everett_card`, and `everett_whoami`. Calls are logged to `~/.everett/mcp.log`. `everett install-mcp` registers it for Claude Code, Codex, and OMP.
- Caller identity comes from `CLAUDE_CODE_SESSION_ID`, `CODEX_THREAD_ID`, `HERMES_SESSION_ID`, `PI_SESSION_FILE`, or `GROK_SESSION_ID`, and `EVERETT_SESSION_ID` overrides them all.

### Setup
- `~/.everett/config.toml` (`vault`, `vault_dir`, `default_harness`, `router`, `merge_llm`, Jev key), with environment overrides. No vault path is hardcoded.
- `everett doctor` shows the Python version, session stores (including Grok and T3 Code), hooks, MCP registration, card coverage, router, and vault.
- `pipx install everett-sessions`, `python -m everett`, and `everett --version`. No runtime dependencies. MIT license.

### Verified
Live round trips on throwaway sessions (2026-09-24):
- Claude Code `send` replied in 10.3 s. Codex `send` replied in 43.8 s, including the idle wait; an earlier attempt was correctly refused as busy. OMP `send` replied in 60.5 s.
- `spawn('claude', ...)` replied in 5.6 s, and the new session appeared in `everett ls`.
- Not live-checked: the Hermes, Pi, and Grok resume and spawn commands, which come from each CLI's `--help` and docs. Pi was not installed on the test machine. The Grok and T3 Code listings were checked read-only against real stores.

## 0.x — 2026-09-23

- Session registry for Claude Code, Codex, and OMP; Jev router; `send` with idle wait; session cards with SessionStart and Stop hooks; automatic fallback cards; `everett cards`; vault trunk note.
