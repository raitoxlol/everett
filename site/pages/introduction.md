Everett is a neutral layer over the coding-agent sessions you already run. It reads the
session stores your harnesses write, summarizes each session with a short card, routes a new
request to the session it continues, delivers messages between sessions, and gives every
session the same shared memory. You drive it from the `everett` CLI; agents drive it through
`everett mcp`.

It is one native binary for macOS and Linux. Local CLI sessions, routing, inboxes, hooks,
MCP registration, and SSH peers need no account. Everett MCP tool calls require
`everett login` in the same profile and verify the account online on every call.

## Works with

| Kind | Agents |
| --- | --- |
| Local harnesses | Claude Code · Codex · OMP · Pi · Hermes Agent · Grok CLI · Devin CLI |
| Overlay | T3 Code threads, recognized through the harness session each thread drives |
| Cloud agents | OpenAI dots and Grok Bot, through registered external MCP inboxes |
| Unverified | Hark and Muse |

A supported integration does not mean every agent can be woken. See
[Agent support and wake](/agent-support/) for the per-agent table.

## What it does

- **Discover.** `everett ls` lists recent sessions from every harness, one line each, by reading
  their existing stores read-only. Nothing needs to be launched through Everett first.
- **Summarize.** Each session gets a [card](/session-cards/) of 50 words or fewer: what it is
  about, its state, and the next step. Agents write their own; hooks write a fallback.
- **Route.** `everett route "<text>"` picks the session a request continues and answers
  `SESSION`, `NEW`, or `ASK` with a confidence. See [Routing](/routing/).
- **Deliver.** `everett send` routes and pings: idle sessions run headless, verified Claude
  background sessions get a native inbox nudge, and replies land in the sender's inbox. See
  [Sending and live delivery](/live-delivery/).
- **Report.** Sessions record `done`, `blocked`, `needs-input`, or `info` [events](/events/);
  blocked sessions notify you.
- **Remember.** `everett learn` feeds a [shared core](/shared-core/) that every new session
  receives at start and can search mid-session; `everett recall` searches every learned fact.
- **Reach other devices.** Inbox-only [SSH peers](/cross-device/) or the end-to-end encrypted
  [Cloudflare relay](/cloudflare-relay/) connect sessions on other computers.

## How the pieces fit

```text
 harness stores (read-only)          Everett                         you / agents
 ───────────────────────────    ─────────────────────────    ───────────────────────────
 ~/.claude/projects        ─┐   discover → cards → route     everett ls / route / send
 ~/.codex/sessions          │   inbox  ~/.everett/inbox/      everett mcp  (everett_* tools)
 ~/.omp, ~/.pi, ~/.grok     ├─▶ events ~/.everett/events.jsonl everett dashboard (loopback)
 Hermes / Devin databases   │   core   ~/.everett/core/
 T3 Code statev2.sqlite    ─┘   peers  SSH · Cloudflare relay  hooks inject messages + core
```

Everything Everett writes lives under `~/.everett/` (or `EVERETT_HOME`), except the shared
`memory.db` that every learned fact is also saved to (see [Shared core](/shared-core/)) and the
`everett_tasks` board at `<project>/.everett/tasks.json`. Harness stores are
opened read-only; a headless resume lets the harness append to its own history.

## Where to go next

| If you are… | Start with |
| --- | --- |
| Installing for the first time | [Quickstart](/quickstart/), then [Onboarding wizard](/onboarding/) |
| Connecting two agents end to end | [Connect Claude Code and Codex](/claude-code-codex/) |
| Learning the vocabulary | [Core concepts](/concepts/) |
| An AI agent with Everett tools | [Agent operating guide](/agent-guide/) and [MCP tools reference](/mcp/) |
| Looking up a command | [CLI reference](/cli/) |
| Connecting a second computer | [Cloudflare relay](/cloudflare-relay/) or [SSH peers](/cross-device/) |
| Connecting ChatGPT, a dot, or Grok Bot | [Integrations](/chatgpt/) |
| Debugging | [Troubleshooting](/troubleshooting/) |

## Reading these docs as an agent

Every page is also published as plain Markdown: append `.md` to the page path (for example
`/quickstart.md`) or use **View Markdown** in the page header. [`/llms.txt`](/llms.txt) lists
every page with a one-line summary and a *read when* hint, and [`/llms-full.txt`](/llms-full.txt)
concatenates the whole site into one file.

The pages are generated from `README.md` and `docs/` in the
[Everett repository](https://github.com/raitoxlol/everett); the repository is the source of truth.
