# Everett 2.4 launch announcement draft

Draft for review. Nothing is published. Grounded in Everett 2.4.0 (`everett-rs`, commit `d8ed972`). Includes a launch post and a Show HN submission.

## Part 1: Launch post

### Headline options

1. Recommended: Everett 2.4: one native Rust layer above all your coding-agent sessions
2. Problem-led: Keep your harnesses, connect your sessions with Everett 2.4
3. Feature-led: Everett 2.4: routing, skill matching, and shared memory across 7 coding agents

### Body

Most developers who use AI coding agents run several sessions at once. On a normal workday, you might have Claude Code refactoring a service in one worktree, Codex wiring a UI view in another, Devin CLI or OMP hunting a failing test, a T3 Code thread open on a side branch, and OpenAI dots or Grok Bot running a routine. Some sessions may live on your laptop and others on a Linux workstation.

Each session branches from the same repo and builds its own history. We named this tool Everett after physicist Hugh Everett III, who proposed the many-worlds interpretation of quantum mechanics.

Running many sessions creates 4 daily problems:
- You lose track of which terminal owns which task.
- You miss a session that has been blocked on your input for 45 minutes.
- You repeat the same deploy rules to every fresh session.
- Your `SKILL.md` playbooks get duplicated across `.claude`, `.codex`, `.agents`, and `.devin`, or dump a huge catalog into every prompt.

All-in-one agent GUIs ask you to launch every agent inside their app. Broadcast agent chatrooms spam every session. We wanted a neutral layer above the tools you already run.

Everett 2.4 is a single, MIT-licensed native Rust binary for macOS and Linux.

### What Everett 2.4 does

#### 1. Read-only discovery across 8 local CLIs, T3 Code, and cloud inboxes

`everett ls` reads the session stores your harnesses already write: Claude Code, Codex, OMP, Pi, Hermes Agent, Grok CLI, Devin CLI, and Muse Code. It reads bounded JSONL slices and opens SQLite databases read-only (`mode=ro`). It leaves harness transcripts untouched.

T3 Code threads (including T3-managed Claude, Codex, Grok, and Devin sessions) appear annotated as `[t3code]` so nothing is listed twice. Registered external inboxes connect OpenAI dots and Grok Bot, and `Everett Local` and the hosted plugin connect ChatGPT desktop.

```text
$ everett ls
● claude  2m  ~/src/api        API: adding retry/backoff to upload client; next: integration test
○ codex  40m  ~/src/web        Web: dark-mode toggle in settings; state: styles done, wiring store
○ omp     3h  ~/src/infra      Infra: Terraform module for the staging bucket
```

Each session keeps a card of 50 words or fewer (`what`, `state`, `next`), written by the agent at startup or generated deterministically by a zero-LLM Stop hook.

#### 2. Targeted routing

`everett route` and `everett send` score your request against session cards, titles, worktree-resolved repository names, and recency. Both return `SESSION`, `NEW`, or `ASK` with addressable candidates on a tie.

```text
$ everett send "the upload retries should cap at 5"
SENT  [claude] ~/src/api — API: adding retry/backoff to upload client
Done. MAX_RETRIES is now 5 and the backoff test covers the cap.
```

Local BM25 routing works offline with zero API keys. Add an optional [Jev](https://typesafe.ai) key if you run dozens of overlapping sessions.

#### 3. Direct pings, local receipts, and blocker events

Default `ping` runs idle local sessions headlessly, sends a verified native PTY nudge to an attached local Claude background session, nudges an attached OMP 18.12.2 extension watcher, or sends a fixed inbox nudge to an owner-local T3 thread through `t3 thread send`. When an immediate transport is unavailable, `ping` returns an error without silently queueing.

Explicit `--mode inbox` handles hook injection (`UserPromptSubmit`, `PostToolUse`, OMP steering), MCP polling, cloud bots, and remote peers. New in 2.4: `everett status MESSAGE_ID`, `everett sent`, and `everett retry MESSAGE_ID` check pickup and recover retained replies even after an inbox read.

Deterministic Stop hooks classify turns into `done`, `blocked`, and `needs-input`, notify you via macOS `osascript` or a custom `notify_command`, and escalate blockers after 30 minutes.

#### 4. Shared core memory with local search and expiry

Any session (or you) can store a durable fact with `everett learn`. A secret filter and entropy check block keys and tokens before anything hits disk. `everett trunk merge` distills queued facts into global and per-project core files of 300 words or fewer (using `--llm claude`, `--llm codex`, or deterministic `--llm none`), injects up to 350 words into every fresh session in about 40 ms, archives history, and schedules nightly runs via macOS `launchd` or Linux `systemd --user`.

`everett core search` runs local SQLite BM25 search over accepted learnings beyond the 300-word cap. `--ttl-seconds` stores temporary facts that expire from search, and `forget` and `correct` update facts right away.

#### 5. New in 2.4: cross-harness skill hub

`everett skills`, the `everett_skill` MCP tool, and MCP prompts (`prompts/list`, `prompts/get`) index `SKILL.md` playbooks across `~/.claude/skills`, `~/.codex/skills`, `~/.agents/skills`, `~/.config/devin/skills`, `~/.devin/skills`, project folders, and custom roots in `EVERETT_SKILL_DIRS`. Local lexical matching (`--match`) runs with zero LLM calls and returns the 1 skill that fits a task. Claude Code and Codex prompt hooks inject named skills automatically or suggest 1 fitting user-owned skill per session.

#### 6. 18 native MCP tools, local dashboard, and cross-device peers

- **18 stdio MCP tools (`rmcp`):** includes `everett_skill`, `everett_status`, `everett_sent`, `everett_retry`, and owner-local `everett_control` (for T3 Code threads and transcript reads), with host-bound caller identity and redacted audit logs.
- **Local dashboard (`everett dashboard`):** read-only HTML view on `127.0.0.1` with zero JavaScript runtime, showing up to 80 recent sessions and copyable CLI commands.
- **Cross-device peers:** pair machines account-free over SSH (`everett peer`, pinned fingerprints, restricted forced-command RPC), or opt into the Cloudflare native relay (`everett relay`) with `X25519 + HKDF-SHA256 + ChaCha20Poly1305` encryption, `Ed25519` signatures, hibernating WebSockets, and an optional private read-only mesh console (`web-mesh/`) gated by expiring owner-approved read grants.

### Why we moved to Rust

Everett started in Python. Moving to one native Rust binary (`everett-rs`, retiring the Python package in 1.5.0) removed Python venv and PATH drift inside hooks and MCP launchers. One binary now bundles SQLite (`rusqlite`), the `ratatui` onboarding TUI, the official `rmcp` SDK, the loopback HTML dashboard, and static Linux musl and macOS arm64/x86_64 builds with checksum verification.

### Boundaries to know before installing

- Local CLI commands (`ls`, `route`, `send`, `skills`, `learn`, `core`, `dashboard`, `doctor`) and SSH peers need zero Everett account. Calling Everett MCP tools (`tools/call`) requires 1 `everett login` per profile for online Cloudflare verification.
- Transport receipts are explicit: `queued` records an inbox write, `delivered` records device ingress or consumption, `claimed` records pickup, and `replied` records a reply linked by `reply_to`. Task completion is separate.
- Hark compatibility is unverified. Cross-owner Cloudflare relay grants are unavailable. Relay metadata visibility is documented in `docs/relay-threat-model.md`.

### Install

```bash
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh
everett onboard --yes --no-backfill
everett doctor
```

Homebrew (`brew tap raitoxlol/tap && brew install everett`) and Cargo (`cargo install --git https://github.com/raitoxlol/everett everett`) also work. Run bare `everett` in a terminal for the interactive setup TUI.

Everett is MIT-licensed at https://github.com/raitoxlol/everett.

---

## Part 2: Show HN submission draft

### Title options (80 characters or fewer)

1. Recommended: `Show HN: Everett - One native Rust layer above your parallel coding agents` (74 chars)
2. Alternative: `Show HN: Everett - Routing, skills, and shared memory for coding agents` (71 chars)
3. Alternative: `Show HN: Everett - Native Rust CLI and MCP layer for parallel AI sessions` (73 chars)

### Author comment

Hi HN. I built Everett because I run parallel sessions across Claude Code, Codex, OMP, Devin CLI, and T3 Code on a Mac and a Linux box. I kept losing track of which session owned which task, repeating the same repo rules to fresh agents, and duplicating `SKILL.md` playbooks across harness folders.

Broadcast agent chats flooded every session, and I wanted to keep my terminal and harnesses out of a wrapper GUI. Everett is a single native Rust binary (MIT) that reads existing session stores read-only (`~/.claude/projects`, `~/.codex/sessions`, `~/.omp/agent/sessions`, `~/.pi`, `~/.grok`, Hermes and Devin SQLite stores, and T3 Code state).

What 2.4.0 includes:
- Read-only discovery (`everett ls`) and a loopback-only read-only HTML dashboard (`everett dashboard`), backed by 50-word live session cards (agent-written or zero-LLM deterministic Stop-hook fallbacks).
- Offline local BM25 plus recency routing (or optional Jev) returning `SESSION`, `NEW`, or `ASK` with candidates.
- Direct `ping` delivery for idle local sessions and verified local Claude, OMP 18.12.2, and owner-local T3 targets, plus explicit `--mode inbox` delivery with `everett status`, `everett sent`, and `everett retry` receipt tracking.
- Shared core memory: secret-filtered `everett learn` facts distill into 300-word global and project cores injected at SessionStart, with local SQLite BM25 search and `--ttl-seconds` temporary expiry.
- Cross-harness skill hub (`everett skills` and `everett_skill`): indexes `SKILL.md` files across harnesses and matches the 1 fitting playbook locally with zero LLM calls.
- Cross-device peers: account-free SSH peers with pinned fingerprints and restricted forced-command RPC, or optional Cloudflare E2EE relay (`X25519 + ChaCha20Poly1305`, `Ed25519`, hibernating WebSockets).

Two limits up front:
1. Local CLI commands, `everett dashboard`, and SSH peers need no account. Calling the 18 stdio MCP tools (`tools/call`) requires 1 `everett login` per profile.
2. Native wake varies by harness. Exact per-harness discovery, hook pickup, and wake support are documented in `docs/agent-support.md`, and relay metadata visibility is in `docs/relay-threat-model.md`.

Repo: https://github.com/raitoxlol/everett. Feedback on multi-harness workflows, hook reliability, and cross-harness skill matching is welcome.
