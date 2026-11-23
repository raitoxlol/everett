These are the terms every other page uses. Each section links to the page with the full contract.

## Harness and session

A **harness** is a coding-agent CLI or app: Claude Code, Codex, OMP, Pi, Hermes Agent, Grok CLI,
or Devin CLI. A **session** is one conversation that harness already recorded in its own store.
Everett discovers sessions; it does not launch or own them. Session IDs are the harness's own IDs.

**T3 Code** is not a separate harness. It is an overlay: Everett marks the Claude, Codex, or Grok
session a T3 thread drives with source `t3code` and can address the thread through the `t3` CLI.
See [T3 Code](/t3code/).

**External agents** (OpenAI dots, Grok Bot) have no local session store. They are registered
explicitly with `everett external add` and reach their inbox through the
[MCP gateway](/gateway/).

`everett ls` marks a session `●` (running) when its ID appears in a live process or its file
changed in the last 10 minutes. This is a recency heuristic, not a verified heartbeat.

## Card

A **card** is a note of at most 50 words at `~/.everett/cards/<session-id>.md`: what the session
is about, its state, and the next step. Agent-written cards are preferred; when a session has
none, the Stop hook writes an automatic card from the transcript, marked
`<!-- everett:auto -->`. Routing and `ls` prefer cards over raw transcript text. A card summarizes
one session; it is not project-wide truth. See [Session cards and hooks](/session-cards/).
Refresh existing AUTO-marked cards for Claude Code, Codex, and Grok CLI with
`everett cards --regenerate-auto`; add `--dry-run` to preview changes or `--hours N` to set the
look-back window. Agent-written and missing cards are left alone.

## Route

**Routing** picks which existing session a request continues. It returns one of:

| Decision | Meaning | What `send` does |
| --- | --- | --- |
| `SESSION` | One session clearly matches | Delivers to it |
| `NEW` | No session matches | Nothing, unless `--spawn` |
| `ASK` | Several sessions are close | Nothing; returns `candidates` |

The **local** router uses BM25 over cards, titles, requests, and project names, weighted by recency,
with no network. T3 and Claude worktree sessions use their repository name, not the
worktree folder's name; other working folders use their own name. Home directories
and Codex app chat folders are not projects. Project-name boosts match consecutive
words in order, ignoring case and punctuation (`web-mesh` matches `web mesh`, not
`mesh web`). The **Jev** router is used when a Typesafe key is configured. See [Routing](/routing/).

## Send, ping, and inbox

`everett send` (MCP: `everett_send`) routes, then delivers. Delivery **modes**:

| Mode | Behavior |
| --- | --- |
| `ping` (default) | Immediate: an idle local session runs headless and returns its reply; a verified attached Claude background session gets a native inbox nudge; a live OMP extension accepts it. Unsupported paths **fail** rather than queue. |
| `inbox` | Explicit queue for hooks, MCP polling, T3 Code, external agents, and remote devices. |
| `auto`, `resume` | Compatibility behavior, only when selected explicitly. |

The **inbox** is `~/.everett/inbox/<session-id>.jsonl`. **Hooks** inject up to five queued
messages into the receiving session at its next prompt or tool call; harnesses without injection
hooks read with `everett_inbox`. Replies use `reply_to` and land in the sender's inbox.

Receipts are evidence, not task completion: `queued` (written), `delivered`/`picked_up` (the
inbox was read), `replied` (a correlated reply exists). See [Delivery policy](/message-delivery/)
and [Status JSON](/dashboard-status/).

## Wake

**Wake** means starting a turn in an agent that is open but idle. Only some paths can do it:
Claude Code's verified native nudge and explicit `send --to <uuid> --wake`, OMP's live extension
watcher, an opt-in receiver policy after a relay import, and owner webhook nudges for dots and
Grok Bot. Everything else waits for a hook or polling. See [Agent support and wake](/agent-support/).

## Event

An **event** records what a session is doing: `done`, `blocked`, `needs-input`, or `info`. Stop
hooks classify the last assistant message automatically; agents can report one with
`everett_event`. `blocked` and `needs-input` notify you and escalate after
`escalate_minutes`. Other sessions can `subscribe` to a session's or project's events. See [Events](/events/).

## Shared core

The **shared core** is the memory every session shares. `everett learn` queues a secret-filtered
fact; `everett trunk merge` distills queued facts into a global core and per-project cores of at
most 300 words each; SessionStart hooks inject them into new sessions. Facts are also searchable
beyond the cap, can be forgotten or corrected, and can be temporary with a TTL. Every learned fact is
also saved to a Teresis `memory.db` that `everett recall` searches, for the project plus global by
default. See [Shared core](/shared-core/).

## Project

The **project** is the enclosing git repository's folder name, else the folder name. Your home
folder is not a project. Project scope applies to cards, events, subscriptions, and memory.

## Hops

Every delivery carries `EVERETT_HOPS`. With `hop_limit = "on"`, sends past three hops are refused
(exit 7). It is off by default. A session can never send to itself. See [Hop limit](/hop-limit/).

## Devices, peers, and the relay

A **peer** is another computer. Two transports exist, both inbox-only:

- **Local CLI setup and SSH peers** need no account: pair SSH peers once with confirmed host and device fingerprints. See [SSH peers](/cross-device/).
- The **Cloudflare relay** uses a signed-in account; devices on the same owner account trust each
  other's signed key bundles automatically. Messages are end-to-end encrypted (X25519, HKDF-SHA256,
  ChaCha20-Poly1305, Ed25519). See [Cloudflare relay](/cloudflare-relay/) and the [threat model](/relay-threat-model/).

Cross-account sends use private addressed inboxes and explicit grants scoped to one sender
account/device and one recipient inbox; see the [cross-account relay guide](/agent-relay/#cross-account-addressed-inboxes).

**Native MCP tools** require `everett login` in the client's Everett profile and online
verification on each call, even for local work. Registration and discovery alone do
not authorize tool calls.

Remote sends require `--mode inbox`. Remote provenance never permits a local wake or resume
(except an owner's opt-in receiver policy).

## Gateway

The **gateway** (`everett gateway`) is an owner-bound MCP server for cloud agents. It runs with
fixed access grants over private stdio, loopback HTTP, or OAuth-protected HTTPS, and exposes a
restricted tool set. See [MCP gateway](/gateway/).

## Files Everett writes

| Path | Contents |
| --- | --- |
| `~/.everett/config.toml` | [Configuration](/configuration/) |
| `~/.everett/cards/` | Session cards |
| `~/.everett/inbox/` | Per-session inboxes and `.done` receipts |
| `~/.everett/events.jsonl`, `state/` | Events and current session state |
| `~/.everett/subscriptions.json` | Event subscriptions |
| `~/.everett/core/` | Shared core, pending inbox, search store, history |
| `~/.local/share/teresis/memory.db` | Every learned fact, shared with Teresis clients (path set by `EVERETT_MEMORY_DB`; see [Shared core](/shared-core/)) |
| `~/.everett/spawned.jsonl` | Sessions Everett started headless |
| `~/.everett/mcp.log` | MCP call log (names and status, never argument values) |
| `~/.everett/cloudflare-auth.json` | Device credential (mode 0600) |
| `~/.everett/relay-cloudflare/` | Relay keys and pins |
| `<project>/.everett/tasks.json`, `tasks.lock` | Shared task board from `everett_tasks`, in the project's Git root |

Set `EVERETT_HOME` to use a different root; memory.db then moves to
`$EVERETT_HOME/.local/share/teresis/memory.db` unless `EVERETT_MEMORY_DB` is set.
