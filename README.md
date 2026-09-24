# Everett

Everett is one layer above all your coding-agent sessions. It sees every Claude Code, Codex, OMP, Pi, Hermes Agent, and Grok CLI session on your machine (including the ones T3 Code drives), routes a new request to the session it belongs to, and delivers it there.

It is built mainly for agents: one agent can hand work to the right parallel session, and every session starts from the same shared core. Humans get the same commands.

Named after Hugh Everett (many worlds): every session branches from one origin but keeps its own history.

## Quickstart (60 seconds)

```bash
pipx install everett-sessions   # or, from a checkout: pipx install .
everett doctor                  # what Everett can see on this machine
everett ls                      # recent sessions, newest first
everett route "add retries to the upload client"
everett install-hooks           # prints the hook snippets; add --apply to merge them
```

Requires Python 3.10+ and no third-party packages. Everett reads the session stores the harnesses already write: `~/.claude/projects`, `~/.codex/sessions`, `~/.omp/agent/sessions`, `~/.pi/agent/sessions`, `~/.grok/sessions`, Hermes' `state.db` files, and T3 Code's `~/.t3/userdata/state.sqlite` (both databases opened read-only).

## Demo

This is example output (paths and ids are shortened):

```text
$ everett ls
● claude  2m  ~/src/api        API: adding retry/backoff to upload client; next: integration test
○ codex  40m  ~/src/web        Web: dark-mode toggle in settings; state: styles done, wiring store
○ omp     3h  ~/src/infra      Infra: Terraform module for the staging bucket

$ everett route "the upload retries should cap at 5"
SESSION  (choice=s0, confidence=0.84, router=local)
  → [claude] ~/src/api — API: adding retry/backoff to upload client
  cd ~/src/api && claude --resume 3f1c… 'the upload retries should cap at 5'

$ everett send "the upload retries should cap at 5"
SENT  [claude] ~/src/api — API: adding retry/backoff to upload client
Done. MAX_RETRIES is now 5 and the backoff test covers the cap.
```

`●` means running (its id is in a live process, or its file changed in the last 10 minutes).

## Commands

| Command | What it does |
|---|---|
| `everett ls [--json] [--all] [--harness H]` | Recent sessions from every harness, one line each. `--all` includes scripted runs. |
| `everett route "<text>" [--router local\|jev] [--json]` | Picks the session a request continues: `SESSION`, `NEW`, or `ASK`, with a confidence and the resume command. It never sends. |
| `everett send "<text>" [--dry-run] [--timeout S] [--router …]` | Routes the request, waits for that session to go idle (up to 2 min), resumes it headless, and prints the reply. `NEW` and `ASK` send nothing. |
| `everett send --to <id-prefix\|name> "<text>"` | Skips routing and delivers to one session. The target is an exact id, a unique id prefix, the card's name (`Kairos: …` → `kairos`), or the project folder name. If more than one session matches, Everett lists them and sends nothing. |
| `everett send --spawn [--dir D] [--harness H] "<text>"` | When routing says `NEW` with confidence ≥ 0.6, starts a new headless session and prints its reply and new session id. The directory defaults to the folder of the best-matching session, else the current one. |
| `everett cards` | Card coverage per harness: agent-written, automatic, missing. |
| `everett install-hooks [--claude] [--codex] [--omp] [--grok] [--apply]` | Prints the hook registrations. `--apply` backs up the file, then merges idempotently. |
| `everett mcp` | Runs the stdio MCP server (harnesses start it; see below). |
| `everett install-mcp [--claude] [--codex] [--omp] [--apply]` | Prints or registers the MCP server for each harness. |
| `everett doctor` | Python version, session stores, hooks, card coverage, router, vault. |
| `everett learn "<fact>" [--project P] [--scope global\|project]` | Pushes a fact to the shared core inbox. Secrets are rejected. |
| `everett trunk merge [--llm claude\|codex\|none] [--dry-run]` | Distills the inbox into the shared core. |
| `everett core [show\|edit-path\|history] [--project P]` | Shows the core a new session in this folder receives, the file to edit, or past merges. |
| `everett trunk [view] [--dry-run]` | Writes the session list as a Markdown note into your notes vault (optional). |

Global option: `--hours N` sets the look-back window (default 72).

### How delivery works

| Harness | Resume (send) | New session (--spawn) |
|---|---|---|
| Claude Code | `claude --resume <id> --print <text>` | `claude --session-id <new uuid> --print <text>` |
| Codex | `codex exec resume <id> <text>` | `codex exec --skip-git-repo-check -o <file> <text>` |
| OMP | `omp -r <id> -p <text>` | `omp -p <text>` |
| Pi | `pi --session <file> -p <text>` | `pi -p <text>` |
| Hermes Agent | `hermes -p <profile> chat --resume <id> -Q -q <text>` | `hermes chat -Q -q <text>` |
| Grok CLI | `grok --resume <id> -p <text>` | `grok --session-id <new uuid> -p <text>` |
| T3 Code | list/route only (see below) | not supported |

The harness appends the request and the reply to that session's history. Hermes sessions that belong to a chat platform (Telegram, Discord, and so on) are listed and routable, but `send` refuses them, because a CLI resume would not reach that chat. Scripted Hermes runs (cron, oneshot, webhook) are hidden like other automated runs. Grok sessions are read from `~/.grok/sessions/<url-encoded cwd>/<id>/` (`summary.json` for id, folder, and title; `prompt_history.jsonl` for the typed requests). A Grok session counts as running while `~/.grok/active_sessions.json` names it with a live process id. Headless `grok -p` runs are hidden like other scripted runs.

T3 Code is a desktop GUI that runs Codex, Claude Code, and Grok underneath. Each T3 thread is a normal session of one of those harnesses, so Everett does not list T3 threads separately: it marks the matching session with source `t3code` (`[t3code]` in `ls`), and the thread title becomes its headline when the session has none. `send` refuses these sessions, because T3 keeps its own resume point and a CLI resume would fork the thread; continue them in T3 Code.

Sessions that Everett spawns are recorded in `~/.everett/spawned.jsonl`, so they show in `ls` even though they ran headless.

Safety rails for agents that call `send`:
- **Hop guard.** Each delivery sets `EVERETT_HOPS` in the harness environment. A `send` from inside a session that is already 3 hops deep is refused (exit 7), which stops ping-pong loops between agents.
- **No self-send.** When the caller's session id is known (`EVERETT_SESSION_ID`, or the harness's own session variable), Everett refuses to send to that session.
- **Spawning is explicit.** `NEW` starts a session only with `--spawn`.

Exit codes: 2 bad input, 3 no Jev key with `--router jev`, 4 session stayed busy, 5 harness did not start or timed out, 6 harness failed, 7 hop limit reached.

## Routing

Everett has two routers with the same output:

- **local** (default without a key): BM25 lexical scoring of the request against each session's card, title, first and last request, and project folder name, weighted by recency. It needs no network. A clear lead gives `SESSION`, a tie gives `ASK`, and no overlap gives `NEW`.
- **jev** (default when a key is set): the [Jev](https://typesafe.ai) System One model makes a typed choice among the sessions. A confidence below 0.6 gives `ASK`.

The Jev key is read from, in order: `TYPESAFE_API_KEY`, `typesafe_api_key` in `~/.everett/config.toml`, then `TYPESAFE_API_KEY` in `~/.hermes/.env`. Force a router with `--router local|jev`, or set `router = "local"` in the config.

## Session cards

A card is a note of 50 words or fewer, kept at `~/.everett/cards/<session-id>.md`, that says what a session is about, its state, and the next step. Routing and `ls` prefer a card over the raw transcript text. A card is ignored if it is more than 24 hours older than the session's last activity.

- **Agent cards**: the SessionStart hook tells each new session where its card lives and asks the agent to write it in its first working reply.
- **Automatic cards**: when the agent has not written one, the Stop hook writes a deterministic fallback from the transcript (no LLM, no network). Automatic cards start with `<!-- everett:auto -->`, and hooks never overwrite an agent-written card.

Hooks stay silent for `everett send` resumes (`EVERETT_SEND=1`) and for scripted Claude runs.

### Installing the hooks

```bash
everett install-hooks            # print the snippets for all harnesses
everett install-hooks --apply    # back up, then merge into the real config files
```

- **Claude Code**: adds `SessionStart` and `Stop` entries to `~/.claude/settings.json`.
- **Codex** (0.155+): adds `SessionStart` and `Stop` entries to `~/.codex/hooks.json`. Codex needs `hooks = true` in `~/.codex/config.toml` and asks you to trust new hooks the first time they run.
- **Grok CLI**: writes `~/.grok/hooks/everett.json` with a `Stop` hook for automatic cards. Grok ignores `SessionStart` output, so Grok sessions get no card instruction or shared core at start. Grok also runs the hooks in `~/.claude/settings.json`; Everett's Claude hooks do nothing there: they need `session_id` and `transcript_path`, and Grok's documented hook input uses camelCase `sessionId` with no transcript path. `--grok` is included by default when `~/.grok` exists.
- **OMP**: writes `~/.omp/agent/extensions/everett.ts`, which re-exports Everett's extension. OMP loads that folder at startup. You can also pass it per launch: `omp --hook=<path printed by install-hooks>`.

`--apply` writes a backup (`<file>.everett-bak-<timestamp>`) before any change. It never removes other hooks and never adds a second Everett entry. The printed commands use the Python interpreter and the hook paths of the install you ran them from.

## Shared core: one mind, many sessions

Parallel sessions each build their own context and drift apart. The shared core is the part they all have in common. Their context and actions differ, but the core is the same.

1. **Push.** Any session (or you) runs `everett learn "Kairos deploys from main; staging is auto"`. The fact is checked by a secret filter (key and token patterns plus an entropy check), capped at 500 characters, and appended to `~/.everett/core/inbox.jsonl` with the time, session id, harness, folder, and project.
2. **Merge (slow path).** `everett trunk merge` distills the inbox into `~/.everett/core/core.md` (global) and `~/.everett/core/projects/<project>.md`, at most 300 words each. It deduplicates, lets newer facts win over contradicted ones (noting the old value), and drops one-off items. With `--llm claude` or `--llm codex`, a headless `claude -p` or `codex exec` run does the merge under a strict prompt; invalid or secret-bearing output changes nothing. `--llm none` is a deterministic append and dedupe with no LLM. Before writing, the previous core files go to `~/.everett/core/history/<time>/`, and the processed inbox is archived there too. If a vault is configured, the merged core is mirrored to `<vault>/<vault_dir>/Core.md`.
3. **Pull.** The SessionStart hooks (Claude Code, Codex, OMP) add the global core plus the current project's core (at most 350 words in total) to each new session's context, with one line telling the agent to run `everett learn` when it finds something other sessions should know. The hooks read local files only, take about 40 ms, and stay silent on any error.

The project is the enclosing git repository's folder name, else the folder name. Your home folder is not a project. Default merge engine: `merge_llm` in the config, else `claude`.

## For agents: MCP

Everett is mostly used by agents. `everett mcp` is a stdio MCP server (JSON-RPC 2.0, protocol versions 2025-06-18, 2025-03-26, and 2024-11-05) written with the standard library only, so every MCP-capable harness gets Everett as native tools:

| Tool | Arguments | Returns |
|---|---|---|
| `everett_ls` | `hours?`, `harness?` | Sessions with their cards (and `source`, such as `t3code`); your own session is marked `you`. |
| `everett_route` | `text`, `router?` | `SESSION` / `NEW` / `ASK`, confidence, candidate. Never sends. |
| `everett_send` | `text`, `to?`, `spawn?`, `dir?`, `harness?`, `timeout?`, `session_id?` | The other session's reply. |
| `everett_learn` | `fact`, `project?`, `scope?` | Queues a fact for the shared core (secret-filtered). |
| `everett_core` | `project?` | The shared core this session's project sees. |
| `everett_card` | `what`, `state`, `next`, `session_id?` | Writes the calling session's card. |
| `everett_whoami` | none | The caller's session id, harness, hop count, and how they were detected. |

Register it:

```bash
everett install-mcp            # print the registration for Claude Code, Codex, and OMP
everett install-mcp --apply    # back up, then register (idempotent)
```

- **Claude Code**: `claude mcp add --scope user everett -- <python> -m everett mcp`, or `--apply` adds `mcpServers.everett` to `~/.claude.json`.
- **Codex**: an `[mcp_servers.everett]` block in `~/.codex/config.toml`.
- **OMP**: `mcpServers.everett` in `~/.omp/agent/mcp.json`. OMP also imports Claude Code's servers.

The registration uses the Python interpreter and package path of the install you ran it from.

**Caller identity.** Everett reads the calling session from the environment the harness gives the server: `CLAUDE_CODE_SESSION_ID` (Claude Code), `CODEX_THREAD_ID` (Codex), `HERMES_SESSION_ID` (Hermes), `PI_SESSION_FILE` (Pi), or `GROK_SESSION_ID` (Grok documents it for hooks; for MCP servers it is unverified). `EVERETT_SESSION_ID` overrides all of them. OMP exposes none. When nothing is detected, agents pass `session_id` to `everett_send` and `everett_card`. An MCP server starts once per session, so after a harness switches sessions in place (for example `/clear`), pass `session_id` explicitly.

**Safety.**
- The server never sends to the caller's own session.
- `EVERETT_HOPS` travels through every delivery, and a request that has already been forwarded 3 times is refused, so two agents cannot ping-pong.
- A new session starts only with `spawn: true`.
- `everett_learn` runs the secret filter.
- Every call is logged to `~/.everett/mcp.log` (tool, arguments clipped to 200 characters, result status, duration). Replies are not logged.

Tool failures come back as `isError: true` with a message written for the agent. Malformed requests get standard JSON-RPC error codes.

## Configuration

Everything is optional. `~/.everett/config.toml`:

```toml
vault = "~/Notes"             # enables `everett trunk`
vault_dir = "Everett"         # folder inside the vault (default "Everett")
default_harness = "claude"    # used in NEW suggestions: claude | codex | omp
router = "local"              # local | jev
typesafe_api_key = "…"        # Jev key ([jev] api_key also works)
merge_llm = "claude"          # trunk merge engine: claude | codex | none
```

Environment overrides: `EVERETT_VAULT`, `EVERETT_VAULT_DIR`, `EVERETT_HARNESS`, `EVERETT_ROUTER`, `TYPESAFE_API_KEY`, and `EVERETT_HOME` (the root used in place of `~`).

## Privacy

Everything stays on your machine. Everett reads the harness session files and writes only under `~/.everett/` (plus the vault note if you configure one, and the harness config files if you run `install-hooks --apply` or `install-mcp --apply`). The only network call Everett makes is to Jev, and only when a Jev key is configured. Use `--router local` to keep routing offline. `send`, `--spawn`, and `trunk merge --llm claude|codex` run your installed harness CLI, which talks to its own model provider as usual. The shared core is plain Markdown in `~/.everett/core/`.

Everett reads only the first and last 64 KB of each session file.

## Development

```bash
python3 -m unittest discover -s tests -v
bin/everett ls          # run from a checkout without installing
```

Tests run against a temporary HOME and never read or write your real session stores.

## License

MIT. See [LICENSE](LICENSE).
