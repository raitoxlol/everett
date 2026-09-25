# Everett

Everett is one layer above all your coding-agent sessions. It sees every Claude Code, Codex, OMP, Pi, Hermes Agent, and Grok CLI session on your machine (including the ones T3 Code drives), routes a new request to the session it belongs to, and delivers it there.

It is built mainly for agents: one agent can hand work to the right parallel session, even one that is running right now, and every session starts from the same shared core. Everett also knows which sessions are done, blocked, or waiting on you, and tells you. Humans get the same commands.

Named after Hugh Everett (many worlds): every session branches from one origin but keeps its own history.

## Quickstart: `everett onboard`

New to Everett? One command walks you through setup:

```bash
brew install raitoxlol/tap/everett
everett onboard                 # friendly terminal setup wizard
```

Other ways to install (all need Python 3.10+; Everett has no third-party dependencies):

```bash
pipx install git+https://github.com/raitoxlol/everett
uv tool install git+https://github.com/raitoxlol/everett
pip install git+https://github.com/raitoxlol/everett   # inside a virtualenv
git clone https://github.com/raitoxlol/everett && cd everett && pipx install .   # from source
```

Then run `everett onboard`.

It is a six-step terminal UI (welcome, detect, hooks, MCP, backfill, confirm) with a title bar,
step indicator, and progress rail so you always know where you are, plus a live "Try this" panel
with the exact first commands at the end: what Everett found on this machine, which hooks to
install (and in which files -- a backup is made of each one first), whether to register the MCP
server, and an optional "make cards for my recent sessions" backfill step -- pick the day span
with an inline `‹ 3 days ›` stepper and watch a per-step checklist tick off with a progress bar as
it runs. Nothing is written until you confirm at the end; `q` quits at any point with no changes.
Not a TTY (or curses fails to start)? It falls back to the same steps as plain yes/no prompts. For
scripts and CI, skip the UI entirely:

```bash
everett onboard --yes                # apply every default, non-interactively
everett onboard --yes --span-days 7  # backfill cards for the last 7 days (default 3, 1-30)
everett onboard --yes --no-backfill  # skip card backfill
everett onboard --yes --no-mcp       # skip MCP registration
```

## Quickstart (60 seconds)

```bash
brew install raitoxlol/tap/everett   # or any method above
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
| `everett send "<text>" [--dry-run] [--timeout S] [--router …]` | Routes the request (jev when a key is configured, else local) and delivers it, printing `routed by jev → [claude] ~/src/api — … (0.87)` before sending. A session with a live harness process gets it in its inbox, injected at its next turn or tool call (see [Live delivery](#live-delivery)). An idle one is resumed headless after it goes quiet (up to 2 min), and the reply is printed. `NEW` and `ASK` send nothing (`ASK` prints its candidates; resend with `--to`). |
| `everett send … [--mode auto\|resume\|inbox] [--wait S]` | `--mode` forces headless resume or inbox delivery (default `auto`). `--wait S` waits up to S seconds for an inbox reply; without it the reply arrives in your inbox later. |
| `everett reply <message-id> "<text>"` | Answers an Everett message; the reply goes to the sender's inbox. |
| `everett inbox [--session S] [--peek] [--json]` | Shows the messages waiting for this session (or the human inbox when run outside a session) and marks them delivered. |
| `everett event done\|blocked\|needs-input\|info "<msg>" [--project P]` | Records what this session is doing (see [Events](#events)). |
| `everett events [--since 24h] [--session S] [--json] [--check]` | Recent events across sessions. `--check` also runs the blocked-too-long escalation. |
| `everett subscribe <session\|project> [--as S] [--remove]` | Delivers another session's (or a project's) events to your inbox. |
| `everett send --to <id-prefix\|name> "<text>"` | Skips routing and delivers to one session. The target is an exact id, a unique id prefix, the card's name (`Kairos: …` → `kairos`), or the project folder name. If more than one session matches, Everett lists them and sends nothing. |
| `everett send --spawn [--dir D] [--harness H] "<text>"` | When routing says `NEW` with confidence ≥ 0.6, starts a new headless session and prints its reply and new session id. The directory defaults to the folder of the best-matching session, else the current one. |
| `everett cards` | Card coverage per harness: agent-written, automatic, missing. |
| `everett onboard [--yes] [--span-days N] [--no-backfill] [--no-mcp]` | Friendly first-time setup TUI (welcome, detect, hooks, MCP, optional card backfill, summary); `--yes` runs it non-interactively for scripts. |
| `everett install-hooks [--claude] [--codex] [--omp] [--grok] [--apply]` | Prints the hook registrations. `--apply` backs up the file, then merges idempotently. |
| `everett mcp` | Runs the stdio MCP server (harnesses start it; see below). |
| `everett install-mcp [--claude] [--codex] [--omp] [--grok] [--apply]` | Prints or registers the MCP server for each harness. |
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

## Live delivery

Headless resume only works on a session nobody has open. When a session is running, Everett delivers into it instead: the request goes to the session's inbox, and the session's own hooks inject it at its next turn or, if it is busy, right after its next tool call.

1. **Which path.** `send --mode auto` (the default) uses the inbox when a harness process is attached to the target: Everett's hooks recorded a live process for it (`~/.everett/inbox/live/<id>.json`), or its id is on a running harness command line. T3 Code sessions always use the inbox, because a CLI resume would fork the thread. Everything else is resumed headless, as before. `--mode resume` and `--mode inbox` force a path.
2. **Inbox.** `~/.everett/inbox/<session-id>.jsonl`, one message per line: `id`, `from` (the sender's session id, or `human` from a terminal), `from_harness`, `from_card`, `text`, `ts`, `reply_to`, `hops`, `kind` (`message`, `reply`, or `event`). Delivered ids go to `<session-id>.done`. Undelivered messages expire after 7 days.
3. **Injection.** The hook adds at most 5 messages (2,000 characters each, 6,000 in total) as context, marked as coming from Everett with the sender's session and card, and marks exactly those delivered. The rest arrive at the next hook. The hook reads local files only, takes about 40 ms, and prints nothing on any error.
4. **Replies.** The receiving agent answers with `everett_send(reply_to="<id>", text=…)` or `everett reply <id> "<text>"`. The reply lands in the sender's inbox and is injected there the same way. A sender can instead block on it: `everett send --wait 120 …` or `everett_send(wait=120)`. `everett inbox` / `everett_inbox` read an inbox directly.
5. **Safety.** A thread can go back and forth at most 3 hops (exit 7), the calling session is never a target, and headless `everett send` runs (`EVERETT_SEND=1`) never consume inbox messages.

| Harness | Next turn | Mid-task | How |
|---|---|---|---|
| Claude Code | `UserPromptSubmit` | `PostToolUse` | `hookSpecificOutput.additionalContext` |
| Codex (0.155+) | `UserPromptSubmit` | `PostToolUse` | the same contract (checked against the hook schemas embedded in the 0.155.1 binary) |
| Grok CLI | no | `PostToolUse` | Grok discards an allowing `UserPromptSubmit` hook's context, so messages wait for the next tool call. A Grok session that runs no tool before stopping reads them with `everett_inbox`, or at its next tool call. |
| OMP | `before_agent_start` | `tool_result` (steered in with `pi.sendMessage(…, {deliverAs: "steer"})`) | the Everett extension |
| Pi, Hermes | no hooks | no hooks | messages wait in the inbox; the agent reads them with `everett_inbox` |

Grok also runs the hooks in `~/.claude/settings.json`. Everett's Claude delivery hook recognizes Grok's camelCase input and leaves a Grok session's messages alone on `UserPromptSubmit`, so nothing is marked delivered that Grok would drop.

Claude Code has its own cross-session messages (`<cross-session-message>` over sockets in `/tmp/cc-socks/`). It has no public or documented way to send one, so Everett does not use it; hooks are the supported path. This is future work if Anthropic documents a sender.

## Events

Everett knows what each session is doing, not just what it was about.

- **Kinds.** `done`, `blocked`, `needs-input`, `info`. An agent reports one with `everett_event(kind, message)`, or anyone with `everett event blocked "waiting on Max prompt"`. Events are appended to `~/.everett/events.jsonl` and set the session's state in `~/.everett/state/<session-id>.json`, with `since` (when that state began). Messages are capped at 300 characters and secret-filtered.
- **Automatic.** The Stop hooks (Claude Code, Codex, Grok) read the turn's last assistant message (from the hook input, else the transcript) and classify it deterministically, with no LLM:
  - `blocked`: a closing line states "blocked", "stuck", "waiting on" / "waiting for", "can't proceed" / "cannot continue", or "unable to proceed". Negated ("not blocked", "no longer blocked", "unblocked") and questioned ("is it blocked?") mentions do not count.
  - `needs-input`: a closing line ends with a question mark, or asks the user to act ("need you to", "please confirm", "should I", "do you want", "would you like", "let me know if" …).
  - `done`: anything else, a clean finish.
  Only the last four prose lines count; code blocks, quotes, and tables are ignored. An automatic event that repeats the session's current state within 10 minutes, or with the same text, is dropped, so a chatty session does not spam.
- **Seeing them.** `everett ls` prefixes sessions that are blocked or need input (`⚠ blocked 32h: waiting on Max prompt · <card>`). `everett_ls` returns `state` and `state_kind` for every session. `everett events --since 24h` lists them.
- **Subscriptions.** `everett subscribe <session|project>` (or `everett_subscribe`) puts another session's or a whole project's events into your inbox, so they are injected at your next turn. Stored in `~/.everett/subscriptions.json`. A session never receives its own events.
- **The human.** `blocked` and `needs-input` also go to you. By default that is a macOS notification (`osascript`). Set `notify_command` to run your own command as well, for example one that sends Max a message so it reaches your phone. It runs under `sh -c` with the one-line summary as `$1` and `EVERETT_EVENT_KIND`, `EVERETT_EVENT_TEXT`, `EVERETT_EVENT_SESSION`, `EVERETT_EVENT_PROJECT`, `EVERETT_EVENT_REASON`, and `EVERETT_EVENT_JSON` in its environment. Notifiers run detached, so hooks never wait on them.
- **Escalation.** A session still `blocked` or `needs-input` after `escalate_minutes` (default 30) triggers one more notification ("blocked for 45 min: …"). The check runs at most once a minute from any session's hooks, and on demand with `everett events --check` (for example from cron).

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

- **Claude Code**: adds `SessionStart`, `Stop`, `UserPromptSubmit`, and `PostToolUse` entries to `~/.claude/settings.json`. The last two deliver [live messages](#live-delivery); `Stop` also records [events](#events).
- **Codex** (0.155+): adds the same four entries to `~/.codex/hooks.json`. Codex needs `hooks = true` in `~/.codex/config.toml` and asks you to trust new hooks the first time they run.
- **Grok CLI**: writes `~/.grok/hooks/everett.json` with a `Stop` hook for automatic cards and events, and a `PostToolUse` hook for live delivery. Grok ignores `SessionStart` output, so Grok sessions get no card instruction or shared core at start. Grok also runs the hooks in `~/.claude/settings.json`; Everett's Claude hooks do nothing there: they need `session_id` and `transcript_path`, and Grok's documented hook input uses camelCase `sessionId` with no transcript path. `--grok` is included by default when `~/.grok` exists.
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

**Jev first.** Call `everett_send` with just `text`; do not call `everett_ls` and hand-pick a target session. Without `to`, Everett routes the request itself (Jev when a key is configured, else the local matcher) before delivering it, and the response reports the decision -- router, chosen session, confidence -- so you can tell the user e.g. "jev picked \[claude\] ~/src/api". Pass `to` only when the user named a specific session or you are replying to one; it skips routing. On an ambiguous `ASK` decision nothing is sent -- the response carries `candidates` (close sessions) to disambiguate, then send again with `to`. `everett_ls` is for a quick overview of what sessions are doing, not for picking a send target.

| Tool | Arguments | Returns |
|---|---|---|
| `everett_ls` | `hours?`, `harness?` | Sessions with their cards (and `source`, such as `t3code`); your own session is marked `you`. Overview only -- not for picking a send target. |
| `everett_route` | `text`, `router?` | `SESSION` / `NEW` / `ASK`, confidence, router used, candidates. Never sends. |
| `everett_send` | `text`, `to?`, `spawn?`, `dir?`, `harness?`, `timeout?`, `session_id?`, `mode?`, `wait?`, `reply_to?` | Without `to`, routes then delivers, reporting the route decision (router, session, confidence; `candidates` on `ASK`, nothing sent). The reply of a resumed session, or the queued message id for a live one (plus its reply when `wait` is set). `reply_to` answers a message you received. |
| `everett_learn` | `fact`, `project?`, `scope?` | Queues a fact for the shared core (secret-filtered). |
| `everett_core` | `project?` | The shared core this session's project sees. |
| `everett_card` | `what`, `state`, `next`, `session_id?` | Writes the calling session's card. |
| `everett_whoami` | none | The caller's session id, harness, hop count, and how they were detected. |
| `everett_inbox` | `session_id?`, `peek?` | The Everett messages (requests, replies, events) waiting for you; marks them delivered. |
| `everett_event` | `kind`, `message`, `project?`, `session_id?` | Records done / blocked / needs-input / info for your session. |
| `everett_subscribe` | `target`, `unsubscribe?`, `session_id?` | Follows a session's or project's events in your inbox. |

Register it:

```bash
everett install-mcp            # print the registration for Claude Code, Codex, and OMP
everett install-mcp --apply    # back up, then register (idempotent)
```

- **Claude Code**: `claude mcp add --scope user everett -- <python> -m everett mcp`, or `--apply` adds `mcpServers.everett` to `~/.claude.json`.
- **Codex**: an `[mcp_servers.everett]` block in `~/.codex/config.toml`.
- **OMP**: `mcpServers.everett` in `~/.omp/agent/mcp.json`. OMP also imports Claude Code's servers.
- **Grok CLI**: `grok mcp add everett <python> -- -m everett mcp`, or `--apply` appends an `[mcp_servers.everett]` block to `~/.grok/config.toml`. Grok also imports Claude Code's servers by default. Included by default when `~/.grok` exists.

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
notify = "osascript"          # blocked/needs-input: osascript | command | both | none
notify_command = "…"          # e.g. a Max/Hermes command; gets the summary as $1 (sets notify default to both)
escalate_minutes = 30         # re-notify once when a session stays blocked this long (0 = never)
```

Environment overrides: `EVERETT_VAULT`, `EVERETT_VAULT_DIR`, `EVERETT_HARNESS`, `EVERETT_ROUTER`, `TYPESAFE_API_KEY`, `EVERETT_NOTIFY`, `EVERETT_NOTIFY_COMMAND`, `EVERETT_ESCALATE_MINUTES`, and `EVERETT_HOME` (the root used in place of `~`).

## Privacy

Everything stays on your machine. Everett reads the harness session files and writes only under `~/.everett/` (plus the vault note if you configure one, and the harness config files if you run `install-hooks --apply` or `install-mcp --apply`). The only network call Everett makes is to Jev, and only when a Jev key is configured. Use `--router local` to keep routing offline. `send`, `--spawn`, and `trunk merge --llm claude|codex` run your installed harness CLI, which talks to its own model provider as usual. The shared core is plain Markdown in `~/.everett/core/`. Notifications use macOS `osascript` locally; a `notify_command` you configure is yours to run, and it can send events wherever you point it.

Everett reads only the first and last 64 KB of each session file.

## Development

```bash
python3 -m unittest discover -s tests -v
bin/everett ls          # run from a checkout without installing
```

Tests run against a temporary HOME and never read or write your real session stores.

## License

MIT. See [LICENSE](LICENSE).
