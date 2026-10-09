# Everett

Route requests, messages, and shared memory across your coding-agent sessions—from a CLI or MCP client.

> **Demo GIF/video placeholder:** the launch clip is being planned.

**Works with:** Claude Code · Codex · OMP · Pi · Hermes Agent · Grok CLI · Devin CLI.
T3 Code threads are recognized through their underlying harness sessions.

## Quickstart

Have Python 3.10+, pipx, Git, and at least one harness CLI on your PATH. Everett supports macOS and Linux.

```bash
pipx install git+https://github.com/raitoxlol/everett
everett onboard --yes --no-backfill
everett doctor
```

Onboarding detects installed CLIs even before their first session. It backs up and registers
hooks and MCP for detected Claude Code, Codex, OMP, and Grok installations. Restart those clients,
then run `everett ls`. Doctor checks the registered launchers and lists your exact next steps.
Pi, Hermes, and Devin sessions can be listed/routed; they need manual MCP configuration and inbox polling.
No router key or model call is needed for this setup. Codex may ask you to enable/trust its hooks.

Python 3.11+ uses the standard library at runtime. Python 3.10 also installs the small `tomli`
parser. The package name is `everett-sessions`; install from GitHub, not an unpublished PyPI name.

### Other install options

```bash
brew tap raitoxlol/tap
brew install everett
```

The tap can lag GitHub: check `everett --version`, or use pipx for the latest source.
You can also use `uv tool install git+https://github.com/raitoxlol/everett`,
`pip install git+https://github.com/raitoxlol/everett` inside a venv, or `pipx install .` from a checkout.

### Setup options

`everett onboard` opens a seven-step wizard: welcome, detect, hooks, MCP, optional Jev/merge schedule,
backfill, and final confirmation. `q` or EOF cancels without applying defaults. Non-TTY terminals
use plain prompts. `--yes` explicitly applies defaults without prompting:

```bash
everett onboard --yes                       # apply every default, non-interactively
everett onboard --yes --span-days 7         # backfill cards for the last 7 days (default 3, 1-30)
everett onboard --yes --no-backfill         # skip card backfill
everett onboard --yes --no-mcp              # skip MCP registration
everett onboard --yes --jev-key-env MY_KEY  # save a Jev key from an env var (validated first)
everett onboard --yes --schedule-merge      # macOS: also schedule nightly `everett trunk merge`
```

**Smarter routing (optional).** Jev ([typesafe.ai](https://typesafe.ai)) picks the right session
when many are running; without it, routing uses local BM25 matching without network calls.
Harness resumes and LLM memory merges use their model providers. The onboarding step offers
"paste a key" or "skip" -- if a key is already found
(`TYPESAFE_API_KEY`, `~/.everett/config.toml`, or `~/.hermes/.env`), it shows "Jev key found ✓
(source)" and defaults to skip. A pasted key is validated with one test route call (5 s timeout);
on failure you can keep it anyway or skip. The key is masked while typing and never echoed or
logged, and is saved as `typesafe_api_key` in `~/.everett/config.toml` (created `chmod 600`,
preserving every other key). The same step has a "merge shared memory nightly" toggle -- see
[Nightly auto-merge](#nightly-auto-merge) below.

Everett reads the session stores the harnesses already write: `~/.claude/projects`,
`~/.codex/sessions`, `~/.omp/agent/sessions`, `~/.pi/agent/sessions`, `~/.grok/sessions`,
Hermes' `state.db` files, the Devin CLI's `sessions.db` (plus `transcripts/*.json`),
and T3 Code's `~/.t3/userdata/state.sqlite`. Every database is opened read-only.

## Demo

### Local dashboard (Rust binary)

```bash
cargo install --path everett-rs
everett dashboard
everett --hours 168 dashboard --port 7347 --no-open
```

The Mac-first dashboard opens automatically on macOS and prints its loopback URL
on other platforms. It reads the same provider stores and Everett cards as
`everett ls`, showing up to 80 recent non-automated sessions. Search by project,
card, or session ID; filter by harness or state; expand a row for card details
and a copyable inbox-send command. Refresh rescans local data.

It is read-only: copying a command does not execute it, deliver a message, or
wake an agent. “Active” is Everett's existing process/recency heuristic, not a
verified agent heartbeat. The server binds only to `127.0.0.1`, rejects foreign
Host headers and write methods, and sends no session data to a hosted service.
Stop it with Ctrl-C. This command is not in the Python CLI.

**Future direction:** an Everett account could connect CLI installations across
devices and expose their local activity through either a self-hosted or hosted
dashboard. Account auth, peer transport, and hosting (including a custom-domain
frontend on Vercel) are not implemented here. A Rust local-data service would
still be required; a static website cannot read another device's session stores.

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
| `everett dashboard [--port 7347] [--no-open]` | Native Rust binary only: read-only local browser dashboard with session cards, filters, activity state, and copyable CLI actions. |
| `everett route "<text>" [--router local\|jev] [--json]` | Picks the session a request continues: `SESSION`, `NEW`, or `ASK`, with a confidence and the resume command. It never sends. |
| `everett send "<text>" [--dry-run] [--timeout S] [--router …]` | Routes the request (jev when a key is configured, else local) and delivers it, printing `routed by jev → [claude] ~/src/api — … (0.87)` before sending. A session with a live harness process gets it in its inbox, injected at its next turn or tool call (see [Live delivery](#live-delivery)). An idle one is resumed headless after it goes quiet (up to 2 min), and the reply is printed. `NEW` and `ASK` send nothing (`ASK` prints its candidates; resend with `--to`). |
| `everett send … [--mode auto\|resume\|inbox] [--wait S]` | `--mode` forces headless resume or inbox delivery (default `auto`). `--wait S` waits up to S seconds for an inbox reply; without it the reply arrives in your inbox later. |
| `everett reply <message-id> "<text>"` | Answers an Everett message; the reply goes to the sender's inbox. |
| `everett inbox [--session S] [--peek] [--json]` | Shows the messages waiting for this session (or the human inbox when run outside a session) and marks them delivered. |
| `everett event done\|blocked\|needs-input\|info "<msg>" [--project P]` | Records what this session is doing (see [Events](#events)). |
| `everett events [--since 24h] [--session S] [--json] [--check]` | Recent events across sessions. `--check` also runs the blocked-too-long escalation. |
| `everett subscribe <session\|project> [--as S] [--remove]` | Delivers another session's (or a project's) events to your inbox. |
| `everett send --to <id-prefix\|name> "<text>"` | Skips routing and delivers to one session. The target is an exact id, a unique id prefix, an exact title (case-insensitive), the card's name (`Atlas: …` → `atlas`), or the project folder name. If more than one session matches, Everett lists them and sends nothing. |
| `everett send --spawn [--dir D] [--harness H] "<text>"` | When routing says `NEW` with confidence ≥ 0.6, starts a new headless session and prints its reply and new session id. The directory defaults to the folder of the best-matching session, else the current one. |
| `everett cards` | Card coverage per harness: agent-written, automatic, missing. |
| `everett onboard [--yes] [--span-days N] [--no-backfill] [--no-mcp]` | Friendly first-time setup TUI (welcome, detect, hooks, MCP, optional Jev key + nightly merge, optional card backfill, summary); `--yes` runs it non-interactively for scripts. |
| `everett install-hooks [--claude] [--codex] [--omp] [--grok] [--apply]` | Prints the hook registrations. `--apply` backs up the file, then merges idempotently. |
| `everett mcp` | Runs the stdio MCP server (harnesses start it; see below). |
| `everett install-mcp [--claude] [--codex] [--omp] [--grok] [--apply]` | Prints or registers the MCP server for each harness. |
| `everett doctor` | Installed CLIs, stores, hooks, live stdio/MCP registration checks, cards, router, and exact next commands. |
| `everett learn "<fact>" [--project P] [--scope global\|project]` | Pushes a fact to the shared core inbox. Secrets are rejected. |
| `everett trunk merge [--llm claude\|codex\|none] [--dry-run]` | Distills the inbox into the shared core. |
| `everett core [show\|edit-path\|history] [--project P]` | Shows the core a new session in this folder receives, the file to edit, or past merges. |
| `everett trunk [view] [--dry-run]` | Writes the session list as a Markdown note into your notes vault (optional). |
| `everett trunk schedule [--at HH:MM] [--remove] [--llm claude\|codex\|none] [--apply]` | Prints (default) or, with `--apply`, installs/removes a macOS LaunchAgent that runs `everett trunk merge` nightly (default 04:00, `--llm claude`). See [Nightly auto-merge](#nightly-auto-merge). |

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
| Devin CLI | `devin --resume <id> --print <text>` | `devin -p <text>` |
| T3 Code | Inbox via underlying harness hooks; no CLI resume | not supported |

The harness appends the request and the reply to that session's history. Hermes sessions that belong to a chat platform (Telegram, Discord, and so on) are listed and routable, but `send` refuses them, because a CLI resume would not reach that chat. Scripted Hermes runs (cron, oneshot, webhook) are hidden like other automated runs. Grok sessions are read from `~/.grok/sessions/<url-encoded cwd>/<id>/` (`summary.json` for id, folder, and title; `prompt_history.jsonl` for the typed requests). A Grok session counts as running while `~/.grok/active_sessions.json` names it with a live process id. Headless `grok -p` runs are hidden like other scripted runs. Devin CLI sessions are read from `sessions.db` under `$DEVIN_HOME`, else the platform data dir (`~/Library/Application Support/devin/cli` on macOS, `~/.local/share/devin/cli` on Linux): the `sessions` table gives id, folder, title, and activity; `message_nodes` gives the user asks when the CLI build has it, otherwise `transcripts/<id>.json` carries the listing. Hidden sessions are skipped. Devin has no Everett hooks, so delivery is a headless `devin --resume <id>` run.

T3 Code is a desktop GUI that runs Codex, Claude Code, and Grok underneath. Everett marks the
matching session with source `t3code` (`[t3code]` in `ls`) and uses the thread title when needed.
Automatic sends use the inbox; delivery requires the underlying harness's hooks. CLI resume is
refused because it would fork T3's own resume point. You can also continue the thread in T3 Code.

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
| Pi, Hermes, Devin | no hooks | no hooks | messages wait in the inbox; the agent reads them with `everett_inbox` |

Grok also runs the hooks in `~/.claude/settings.json`. Everett's Claude delivery hook recognizes Grok's camelCase input and leaves a Grok session's messages alone on `UserPromptSubmit`, so nothing is marked delivered that Grok would drop.

Claude Code has its own cross-session messages (`<cross-session-message>` over sockets in `/tmp/cc-socks/`). It has no public or documented way to send one, so Everett does not use it; hooks are the supported path. This is future work if Anthropic documents a sender.

## Events

Everett knows what each session is doing, not just what it was about.

- **Kinds.** `done`, `blocked`, `needs-input`, `info`. An agent reports one with `everett_event(kind, message)`, or anyone with `everett event blocked "waiting on staging key"`. Events are appended to `~/.everett/events.jsonl` and set the session's state in `~/.everett/state/<session-id>.json`, with `since` (when that state began). Messages are capped at 300 characters and secret-filtered.
- **Automatic.** The Stop hooks (Claude Code, Codex, Grok) read the turn's last assistant message (from the hook input, else the transcript) and classify it deterministically, with no LLM:
  - `blocked`: a closing line states "blocked", "stuck", "waiting on" / "waiting for", "can't proceed" / "cannot continue", or "unable to proceed". Negated ("not blocked", "no longer blocked", "unblocked") and questioned ("is it blocked?") mentions do not count.
  - `needs-input`: a closing line ends with a question mark, or asks the user to act ("need you to", "please confirm", "should I", "do you want", "would you like", "let me know if" …).
  - `done`: anything else, a clean finish.
  Only the last four prose lines count; code blocks, quotes, and tables are ignored. An automatic event that repeats the session's current state within 10 minutes, or with the same text, is dropped, so a chatty session does not spam.
- **Seeing them.** `everett ls` prefixes sessions that are blocked or need input (`⚠ blocked 32h: waiting on staging key · <card>`). `everett_ls` returns `state` and `state_kind` for every session. `everett events --since 24h` lists them.
- **Subscriptions.** `everett subscribe <session|project>` (or `everett_subscribe`) puts another session's or a whole project's events into your inbox, so they are injected at your next turn. Stored in `~/.everett/subscriptions.json`. A session never receives its own events.
- **The human.** `blocked` and `needs-input` also go to you. By default that is a macOS notification (`osascript`). Set `notify_command` to run your own command as well, for example one that pushes a message to your phone. It runs under `sh -c` with the one-line summary as `$1` and `EVERETT_EVENT_KIND`, `EVERETT_EVENT_TEXT`, `EVERETT_EVENT_SESSION`, `EVERETT_EVENT_PROJECT`, `EVERETT_EVENT_REASON`, and `EVERETT_EVENT_JSON` in its environment. Notifiers run detached, so hooks never wait on them.
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

1. **Push.** Any session (or you) runs `everett learn "Atlas deploys from main; staging is auto"`. The fact is checked by a secret filter (key and token patterns plus an entropy check), capped at 500 characters, and appended to `~/.everett/core/inbox.jsonl` with the time, session id, harness, folder, and project.
2. **Merge (slow path).** `everett trunk merge` distills the inbox into `~/.everett/core/core.md` (global) and `~/.everett/core/projects/<project>.md`, at most 300 words each. With `--llm claude` or `--llm codex`, a headless harness run is prompted to deduplicate and resolve contradictions; invalid or secret-bearing output changes nothing. `--llm none` appends and removes normalized duplicates with no LLM. Previous core files and the processed inbox are archived under `~/.everett/core/history/<time>/`. Facts learned during a merge stay pending for the next one; a second simultaneous merge is refused. A configured vault gets a generated `Core.md` mirror.
3. **Pull.** The SessionStart hooks (Claude Code, Codex, OMP) add the global core plus the current project's core (at most 350 words in total) to each new session's context, with one line telling the agent to run `everett learn` when it finds something other sessions should know. The hooks read local files only, take about 40 ms, and stay silent on any error.

The project is the enclosing git repository's folder name, else the folder name. Your home folder is not a project. Default merge engine: `merge_llm` in the config, else `claude`.

### Nightly auto-merge

`everett trunk schedule` installs a macOS LaunchAgent so the shared core merges itself every night instead of waiting on someone to run `everett trunk merge`:

```bash
everett trunk schedule                       # print what would be installed (default 04:00, --llm claude)
everett trunk schedule --apply               # write the LaunchAgent and launchctl bootstrap it
everett trunk schedule --at 02:30 --llm codex --apply
everett trunk schedule --remove --apply      # launchctl bootout it and delete the plist
```

`--apply` writes `~/Library/LaunchAgents/dev.everett.core-merge.plist` (label `dev.everett.core-merge`) and runs `launchctl bootstrap gui/<uid> <plist>`; without `--apply` it only prints the plist. The scheduled job runs `everett trunk merge --llm <x>` once a day and skips cleanly when the inbox is empty, same as running it by hand. Logs go to `~/.everett/logs/merge.log`. `everett doctor` reports whether it's currently scheduled. `everett onboard` offers the same thing as a "merge shared memory nightly" toggle next to the Jev step.

## For agents: MCP

Everett is mostly used by agents. `everett mcp` is a stdio MCP server (JSON-RPC 2.0, protocol versions 2025-06-18, 2025-03-26, and 2024-11-05) written with the standard library only, so every MCP-capable harness gets Everett as native tools:

**Jev first.** Call `everett_send` with just `text`; do not call `everett_ls` and hand-pick a target session. Without `to`, Everett routes the request itself (Jev when a key is configured, else the local matcher) before delivering it, and the response reports the decision -- router, chosen session, confidence -- so you can tell the user e.g. "jev picked \[claude\] ~/src/api". Pass `to` only when the user named a specific session or you are replying to one; it skips routing. On an ambiguous `ASK` decision nothing is sent -- the response carries `candidates` (close sessions) to disambiguate, then send again with `to`. `everett_ls` is for a quick overview of what sessions are doing, not for picking a send target.

| Tool | Arguments | Returns |
|---|---|---|
| `everett_ls` | `hours?`, `harness?` | Sessions with their cards (and `source`, such as `t3code`); your own session is marked `you`. Overview only -- not for picking a send target. |
| `everett_route` | `text`, `router?`, `session_id?`, `hours?` | `SESSION` / `NEW` / `ASK`, confidence, router used, candidates. Never sends; excludes the caller when known. |
| `everett_send` | `text`, `to?`, `hours?`, `spawn?`, `dir?`, `harness?`, `timeout?`, `session_id?`, `mode?`, `wait?`, `reply_to?` | Without `to`, routes then delivers, reporting the route decision (router, session, confidence; `candidates` on `ASK`, nothing sent). The reply of a resumed session, or the queued message id for a live one (plus its reply when `wait` is set). `reply_to` answers a message you received. |
| `everett_learn` | `fact`, `project?`, `scope?` | Queues a fact for the shared core (secret-filtered). |
| `everett_core` | `project?` | The shared core this session's project sees. |
| `everett_card` | `what`, `state`, `next`, `session_id?` | Writes the calling session's card. |
| `everett_whoami` | none | The caller's session id, harness, hop count, and how they were detected. |
| `everett_inbox` | `session_id?`, `peek?` | The Everett messages (requests, replies, events) waiting for you; marks them delivered. |
| `everett_event` | `kind`, `message`, `project?`, `session_id?` | Records done / blocked / needs-input / info for your session. |
| `everett_subscribe` | `target`, `unsubscribe?`, `session_id?` | Follows a session's or project's events in your inbox. |

Listing, routing and sending default to a 72-hour window. Pass the same larger `hours` value
to these MCP tools when continuing an older session. Resume/spawn children have closed stdin;
the MCP connection remains available for the client's protocol messages.

Register it:

```bash
everett install-mcp            # print the registration for Claude Code, Codex, OMP, and Grok
everett install-mcp --apply    # back up, then register (idempotent)
```

- **Claude Code**: `claude mcp add --scope user everett -- <python> -m everett mcp`, or `--apply` adds `mcpServers.everett` to `~/.claude.json`.
- **Codex**: an `[mcp_servers.everett]` block in `~/.codex/config.toml`.
- **OMP**: `mcpServers.everett` in `~/.omp/agent/mcp.json`. OMP also imports Claude Code's servers.
- **Grok CLI**: `grok mcp add everett <python> -- -m everett mcp`, or `--apply` appends an `[mcp_servers.everett]` block to `~/.grok/config.toml`. Grok also imports Claude Code's servers by default. Included by default when `~/.grok` exists.

The registration uses the Python interpreter and package path of the install you ran it from.
For another MCP client, configure a stdio server with that interpreter as `command` and
`["-m", "everett", "mcp"]` as `args`. `python -m everett mcp` is also a valid launcher when
that Python has Everett installed. Restart clients after changing their registration.

**Caller identity.** Everett reads the calling session from the environment the harness gives the server: `CLAUDE_CODE_SESSION_ID` (Claude Code), `CODEX_THREAD_ID` (Codex), `HERMES_SESSION_ID` (Hermes), `PI_SESSION_FILE` (Pi), or `GROK_SESSION_ID` (Grok documents it for hooks; for MCP servers it is unverified). `EVERETT_SESSION_ID` overrides all of them. OMP and the Devin CLI expose none. When nothing is detected, agents pass `session_id` to `everett_send` and `everett_card`. An MCP server starts once per session, so after a harness switches sessions in place (for example `/clear`), pass `session_id` explicitly.

**Safety.**
- The server never sends to the caller's own session.
- `EVERETT_HOPS` travels through every delivery, and a request that has already been forwarded 3 times is refused, so two agents cannot ping-pong.
- A new session starts only with `spawn: true`.
- `everett_learn` runs the secret filter.
- Calls are logged to `~/.everett/mcp.log` with tool name, argument names, caller id, result status and duration. Argument values and replies are not logged. Existing log entries from older versions are preserved.

Tool failures come back as `isError: true` with a message written for the agent. Malformed requests get standard JSON-RPC error codes.

## Configuration

Everything is optional. `~/.everett/config.toml`:

```toml
vault = "~/Notes"             # enables `everett trunk`
vault_dir = "Everett"         # folder inside the vault (default "Everett")
default_harness = "claude"    # NEW suggestions: claude | codex | omp | pi | hermes | grok
router = "local"              # local | jev
typesafe_api_key = "…"        # Jev key ([jev] api_key also works)
merge_llm = "claude"          # trunk merge engine: claude | codex | none
notify = "osascript"          # blocked/needs-input: osascript | command | both | none
notify_command = "…"          # e.g. a push-notification command; gets the summary as $1 (sets notify default to both)
escalate_minutes = 30         # re-notify once when a session stays blocked this long (0 = never)
```

Environment overrides: `EVERETT_VAULT`, `EVERETT_VAULT_DIR`, `EVERETT_HARNESS`, `EVERETT_ROUTER`, `TYPESAFE_API_KEY`, `EVERETT_NOTIFY`, `EVERETT_NOTIFY_COMMAND`, `EVERETT_ESCALATE_MINUTES`, and `EVERETT_HOME` (the root used in place of `~`).

## Privacy

Session indexing and state are local. Everett reads harness stores and writes its own state under
`~/.everett/`, plus configured vault notes and explicitly applied harness registrations. Headless
resumes let the harness append its own history. Local routing needs no network; optional Jev
sends routing context to its API. `send`, `--spawn`, and LLM merges call your installed harness,
which may contact its provider. `trunk merge --llm none` is local. Desktop notifications use macOS
`osascript`; a custom `notify_command` can send events wherever you configure it.

Transcript adapters generally inspect bounded slices; Grok prompt-history scans and automatic
card backfills may read more. SQLite stores are opened read-only. No harness session data is copied
to a cloud service for plain `ls`, local routing, or inbox delivery.

## Troubleshooting

| Symptom | Next step |
| --- | --- |
| `everett: command not found` | Run `pipx ensurepath`, open a new terminal, then `everett --version`. If needed, reinstall from the GitHub URL above. |
| `No module named everett` | Use the Python from Everett's venv; a pipx install is isolated from your system Python. `everett install-mcp --apply` records the correct interpreter. |
| MCP client shows zero tools | Run `everett doctor`. It must list **10 tools over stdio**. For a stale/disabled launcher, run `everett install-mcp --claude --repair --apply` (replace `--claude` with `--codex`, `--omp` or `--grok`), then restart/enable the server in the client. Fix malformed config before repair. |
| Session not found / no live reply | Start a harness session or run `everett --hours 168 ls` for older ones; send to its exact id or title. In MCP, pass `hours: 168` to listing and sending. Hook injection needs enabled/trusted hooks; Pi/Hermes poll `everett_inbox`. A queued message is not a read receipt. |
| Codex reports `Interrupted system call (os error 4)` | Delivery is unconfirmed. Check the target session before retrying; for an open session with Everett hooks, use `--mode inbox` (MCP: `mode: "inbox"`). Failed resumes are not retried automatically. |
| Shared fact missing / scheduling unavailable | Run `everett trunk merge --llm none`, then `everett core`. Automatic launchd scheduling and default desktop notifications require macOS; manual merging works on Linux. |

## Development

```bash
python3 -m unittest discover -s tests -v
bin/everett ls          # run from a checkout without installing
```

Tests run against a temporary HOME and never read or write your real session stores.

For a release, build a wheel and drive the installed CLI and every MCP tool:

```bash
python3 -m pip install . build
python3 -m build
python3 scripts/verify_release.py --source . --evidence .audit/source-journey.json
```

Use `--python /path/to/venv/bin/python --cli /path/to/venv/bin/everett` without `--source`
for fresh artifact proof. The helper records commands/protocol output and checks routing, messages,
replies, status subscriptions and global/project memory. See the [verification skill](.agents/skills/verify-everett/SKILL.md).
GitHub Actions runs the suite and fresh wheel journey on macOS/Linux and Python 3.10/3.14, plus Linux 3.12,
and `cargo test` for the Rust binary on macOS/Linux.

## Rust port (`everett-rs/`)

A single-binary Rust rewrite lives alongside the Python tree: one `everett` binary, no
Python interpreter needed, fast startup. The Python package remains the reference.

Release tags publish prebuilt binaries for macOS (arm64, x86_64) and Linux (static musl,
x86_64, aarch64), each with a `.sha256`, plus a rendered Homebrew formula (`everett.rb`).
Rerun either command to update:

```bash
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh   # ~/.local/bin
cargo install --git https://github.com/raitoxlol/everett everett                         # from source
```

`install.sh` verifies the checksum; `EVERETT_VERSION=vX.Y.Z` pins a release and
`EVERETT_INSTALL_DIR` changes the target directory. To build and test locally:

```bash
cd everett-rs
cargo build --release            # binary at target/release/everett
cargo test                       # fixture-store suite (temp HOME, never touches real stores)
```

Parity status (Rust vs Python):

| Area | Status |
|---|---|
| `ls`, `cards`, `route`, `send`, `event`, `events`, `subscribe`, `reply`, `inbox` | Same output shapes and exit codes |
| `trunk view` / `merge` / `schedule` (plist + launchctl) | Same; schedule is macOS-only as before |
| `learn`, `core` | Same; secret filter and file locking identical |
| `install-hooks`, `install-mcp`, `doctor`, `onboard --yes` | Same; hooks read `EVERETT_HOME` |
| `onboard` interactive | Same seven-step wizard as the Python curses TUI (ratatui); falls back to plain prompts off a TTY |
| `mcp` | Same 10 tools, same schemas, newline-delimited JSON-RPC 2.0 |
| Session stores | Read-only for all harnesses (claude/codex/omp/pi/hermes/grok/devin + t3code overlay); sqlite opened `mode=ro` |
| Send/resume argv | `claude --resume`, `codex exec resume`, `omp -r`, `pi --session`, `hermes chat --resume`, `grok --resume`, `devin --resume --print` |
| Python-only surface | `everett` console_script entry points and `python -m everett` |

Tests: `cargo test` runs the fixture suite — `ls`/`route`/`send --dry-run`/`cards`/`trunk`/`learn`/`event`/`inbox` journeys, an MCP `initialize`/`tools/list`/`tools/call` stdio round-trip, and pty-driven onboarding wizard runs (rendered-screen assertions via vt100), all against synthetic stores in a temp HOME.

## License

MIT. See [LICENSE](LICENSE).
