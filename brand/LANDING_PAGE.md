# Everett landing page copy outline

Draft wireframe and copy for review. Nothing is deployed. Grounded in Everett 2.4.0 (`everett-rs`, commit `d8ed972`) and `DESIGN.md` (`#f3eee5` paper, `#242825` carbon, `#b64f1a` oxide orange).

## Page structure

1. Top navigation bar
2. Hero section (headline, install tabs, live CLI and dashboard preview)
3. Supported harnesses strip
4. Problem section
5. Core capabilities
6. Cross-device messaging and optional mesh console
7. Delivery states reference
8. Account and security boundaries
9. Comparison table
10. Quickstart and footer

---

## 1. Top navigation bar

- Left: `[Horizon-Sun-Balloon Mark]` `Everett`
- Center links: `Features` · `Harnesses` · `Skill Hub` · `MCP` · `Security` · `Docs`
- Right: `GitHub` (text button) · `Install` (`#8e3811` primary button linking to `#quickstart`)

---

## 2. Hero section

### Eyebrow pill

`v2.4 · Native Rust binary for macOS and Linux · MIT`

### Primary headline

# One layer above all your coding-agent sessions.

### Headline A/B variants

- Variant B: `Keep your harnesses. Connect every session.`
- Variant C: `The dispatch desk for parallel coding agents.`
- Variant D: `Route requests, skills, and shared memory across every coding agent you run.`

### Subheadline

> Everett is a single native Rust binary that reads your existing Claude Code, Codex, OMP, Pi, Hermes, Grok, Devin, and T3 Code sessions read-only. Route requests, send messages with explicit delivery states, match `SKILL.md` playbooks, and share durable project memory through a CLI, 24 MCP tools, or a local dashboard.

### Primary and secondary buttons

- Primary (`#8e3811`): `Install Everett` (jumps to `#quickstart`)
- Secondary (bordered on `#fffaf1`): `Read the docs on GitHub`
- Text link: `Watch the 20s onboarding capture` (recapture `docs/assets/onboarding/onboarding-motion.gif` before launch so the welcome text matches 2.4.0)

### Tabbed install bar (`Shell` | `Homebrew` | `Cargo`)

```bash
# Shell: macOS and Linux, checksum-verified binary in ~/.local/bin
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh
everett onboard --yes --no-backfill
everett doctor
```

- Homebrew tab: `brew tap raitoxlol/tap && brew install everett`
- Cargo tab: `cargo install --git https://github.com/raitoxlol/everett everett`

Caption below the install bar:
`Local CLI commands, the local dashboard, and SSH peers need no account. Sign in once with everett login to call Everett MCP tools or use the optional Cloudflare relay.`

### Hero visual specification

Split two-column frame:

- **Left pane (carbon `#242825`):** Real `everett ls`, `everett route`, and `everett send` output.
  ```text
  $ everett ls
  ● claude  2m  ~/src/api        API: adding retry/backoff to upload client; next: integration test
  ○ codex  40m  ~/src/web        Web: dark-mode toggle in settings; state: styles done, wiring store
  ○ omp     3h  ~/src/infra      Infra: Terraform module for the staging bucket

  $ everett route "the upload retries should cap at 5"
  SESSION  (choice=s0, confidence=0.84, router=local)
    → [claude] ~/src/api — API: adding retry/backoff to upload client

  $ everett send "the upload retries should cap at 5"
  SENT  [claude] ~/src/api — API: adding retry/backoff to upload client
  Done. MAX_RETRIES is now 5 and the backoff test covers the cap.
  ```
- **Right pane (warm paper `#fffaf1`):** Compact `everett dashboard` manifest card showing the vertical activity rail, oxide-orange live dot, 50-word card body, harness tag, and copyable CLI buttons.

---

## 3. Supported harnesses strip

Eyebrow: `WORKS WITH THE HARNESSES YOU ALREADY RUN`

| Group | Tools | Scope |
| --- | --- | --- |
| Local CLIs (7) | Claude Code · Codex · OMP · Pi · Hermes Agent · Grok CLI · Devin CLI | Read-only discovery, headless resume and spawn, plus native hooks, extensions, or the Hermes plugin where supported |
| Desktop GUI overlay | T3 Code | Annotates underlying Claude, Codex, Grok, and Devin threads as `[t3code]`; exact thread IDs, inbox nudge via `t3 thread send`, `everett_control` |
| Cloud and external | OpenAI dots · Grok Bot · ChatGPT desktop (`Everett Local` and hosted plugin) | Registered external MCP inboxes, owner-bound Rust gateway, optional webhook nudges, and opt-in MCP Events |

Footnote: `Everett reads the session stores your tools already write. Hark and Muse compatibility is unverified.`

---

## 4. Problem section

### Section heading

## Parallel sessions drift when nothing sits above them.

### Three problem cards (`#fffaf1` on `#f3eee5`, 1px `#cbc1b3` border)

1. **Lost session state.** Six sessions are open across Claude Code, Codex, and Devin. You have to dig through tabs to find which worktree owns the retry cap or which agent has been blocked for 40 minutes.
2. **Repeated context and duplicated skills.** Every new session starts cold. You re-explain the same deploy rule 3 times a day while your `SKILL.md` playbooks sit scattered across `.claude`, `.codex`, `.agents`, and `.devin`.
3. **Wrapper lock-in and silent queues.** All-in-one agent GUIs force you to launch every agent inside their app. Broadcast agent chats spam every tab. Some tools blur an inbox write into a completed task.

Bridge line: `Named after Hugh Everett (many-worlds): every session branches from the same origin with its own history. Everett keeps every branch visible, reachable, and aligned.`

---

## 5. Core capabilities

### 1. Read-only discovery and 50-word session cards

**Scan every harness in one line.**
`everett ls` reads the JSONL transcripts and SQLite databases your harnesses already write. It reads bounded slices and opens every database read-only (`mode=ro`). Each session keeps a live card of 50 words or fewer (`what`, `state`, `next`), written by the agent at startup or generated deterministically by a zero-LLM Stop hook. Worktree sessions for T3 Code and Claude label by their repository name.

### 2. Local BM25 and optional Jev routing

**Send work to the matching branch. Ask when it is close.**
`everett route` and `everett send` score requests against each session's card, title, project name (including multi-word names like `web-mesh`), and recency. Both routers return `SESSION`, `NEW`, or `ASK` with addressable candidates. Local BM25 runs offline with zero API keys. Plug in an optional [Jev](https://typesafe.ai) key when running dozens of overlapping sessions.

```bash
everett send "cap upload retries at 5"
everett send --spawn "audit the billing SQL"
```

### 3. Direct pings, local receipts, and blocker events

**Immediate delivery when supported. Explicit receipts everywhere.**
Default `ping` runs idle local sessions headlessly, sends a verified native PTY nudge to an attached local Claude background session, nudges an attached OMP 18.12.2 extension watcher, or sends a fixed inbox nudge to an owner-local T3 thread. When an immediate transport is unavailable, `ping` returns an error without silently queueing. Use `--mode inbox` for hooks, MCP polling, cloud bots, or remote machines.

Check pickup and recover retained replies anytime with `everett status MESSAGE_ID`, `everett sent`, and `everett retry MESSAGE_ID`. Deterministic Stop hooks classify turns into `done`, `blocked`, and `needs-input`, alert you through macOS `osascript` or `notify_command`, and escalate blockers after 30 minutes.

### 4. Shared core memory with local search and expiry

**Learn a fact once. Every new session starts with it.**
Push durable facts with `everett learn`. A secret filter and entropy check block keys and tokens, facts cap at 500 characters, and `--ttl-seconds` stores expiring temporary memory. `everett trunk merge` distills queued facts into global and project core files of 300 words or fewer (`--llm claude`, `--llm codex`, or `--llm none`), archives history, and schedules nightly runs on macOS (`launchd`) or Linux (`systemd --user`).

SessionStart hooks inject up to 350 words of core in about 40 ms from local files. Search facts beyond the 300-word cap with `everett core search` (local SQLite BM25), or update facts immediately with `forget` and `correct`.

### 5. Cross-harness skill hub (`everett skills` and `everett_skill`)

**Hand each agent the one playbook it needs.**
Everett indexes `SKILL.md` files across `~/.claude/skills`, `~/.codex/skills`, `~/.agents/skills`, `~/.devin/skills`, project directories, and custom roots (`EVERETT_SKILL_DIRS`). Local zero-LLM matching (`everett skills --match` or `everett_skill`) returns the single matching skill, deduplicates copies across harnesses, and exposes every skill as an MCP prompt. Claude Code and Codex prompt hooks inject named skills automatically.

### 6. 24 native stdio MCP tools and owner-local control

**Let your agents coordinate over Rust MCP.**
Built on the official Rust MCP SDK (`rmcp`), `everett mcp` exposes 24 tools with host-bound caller identity, closed child stdin, audit logs that omit argument values, and owner-local `everett_control` to list, start, send, wake, and read T3 Code threads and supported local transcripts.

Tools: `everett_ls` · `everett_route` · `everett_send` · `everett_status` · `everett_sent` · `everett_retry` · `everett_inbox` · `everett_card` · `everett_skill` · `everett_event` · `everett_subscribe` · `everett_learn` · `everett_recall` · `everett_core` · `everett_whoami` · `everett_directory` · `everett_control` · `everett_agent_relay` · `everett_agent_send` · `everett_agent_status` · `everett_search` · `everett_peers` · `everett_broadcast` · `everett_tasks`

### 7. Local dispatch dashboard (`everett dashboard`)

**Inspect up to 80 local sessions in a browser with zero JavaScript runtime.**
`everett dashboard` serves a read-only HTML manifest straight from the Rust binary on `127.0.0.1`. Filter by harness, state, or search text, expand full cards inline, and copy safe CLI commands. It rejects foreign Host headers and write methods and sends zero session data off your machine.

---

## 6. Cross-device messaging

### Section heading

## Reach a second machine over SSH or the Cloudflare relay.

| SSH peers (account-free) | Cloudflare native relay (optional) |
| --- | --- |
| Pair machines over your existing SSH login with zero account, daemon, or hosted service. | Connect laptops, NUCs, and VMs behind NAT with `everett login` (GitHub device approval) and `everett relay`. |
| Pin host and device fingerprints once. The dedicated key runs restricted Everett RPC with no shell, PTY, or forwarding. | Payloads encrypt with `X25519 + HKDF-SHA256 + ChaCha20Poly1305` and sign with `Ed25519`. |
| Inbox-only delivery with explicit idempotency keys and `everett peer status` reply polling. | Hibernating WebSocket notifications make zero idle polling requests. Supports human-readable device names, opt-in exposed cards (`everett relay authors`), and an optional private read-only mesh console (`web-mesh/`) gated by expiring owner-approved read grants. |

---

## 7. Delivery states reference

### Section heading

## What each delivery state records.

Horizontal 4-stop manifest rail:

1. `queued`: written to the target inbox and retained for pickup.
2. `delivered`: authorized device ingress confirmed (relay) or inbox consumed.
3. `claimed`: target session read the inbox item.
4. `replied`: a reply linked with `reply_to` was enqueued or observed.

Footnote: `Task completion is separate from transport receipts. Run everett proof wake --to <session> to verify a local ping path with a random nonce canary.`

---

## 8. Account and security boundaries

### Section heading

## Exact boundaries before you install.

- **Offline CLI, verified MCP.** Local terminal commands (`ls`, `route`, `send`, `cards`, `skills`, `learn`, `core`, `dashboard`, `doctor`) and SSH peers work offline with zero account. Calling Everett MCP tools (`tools/call`) verifies 1 `everett login` per profile online.
- **Read-only on harness stores.** Transcript readers use bounded slices, SQLite opens with `mode=ro`, and `install-hooks --apply` and `install-mcp --apply` write timestamped backups before changing config files.
- **Secret filtering and redacted logs.** `everett learn` and `everett event` block keys and tokens. `~/.everett/mcp.log` records tool names, argument names, caller ID, status, and duration without logging argument values or reply bodies.
- **Documented threat model.** Everett refuses self-send, enforces host-bound caller identity, blocks remote-derived callers from `everett_control` and local wake, and documents relay metadata visibility in `docs/relay-threat-model.md`.

---

## 9. Comparison table

| Capability | Everett (Rust) | Agent wrapper GUIs | Broadcast agent chat | Terminal multiplexers |
| --- | --- | --- | --- | --- |
| Discovers existing sessions read-only | 7 CLIs + T3 + external inboxes | Wrapper sessions only | No | No |
| Routes to 1 matching session | Local BM25 + optional Jev | Manual tab switching | Broadcast | Manual |
| Cross-harness `SKILL.md` hub | Yes (`everett skills`) | Single app | No | No |
| Shared project memory across harnesses | Core + SQLite BM25 + TTL | Single app | Shared log | No |
| Native stdio MCP and owner control | 24 tools (`rmcp`) | Varies | Rarely | No |
| Single native binary, MIT open source | Yes (macOS + Linux) | Usually Electron | Python or Node | Yes |

---

## 10. Quickstart and footer (`#quickstart`)

### Section heading

## Install in under 2 minutes.

```bash
# 1. Install the checksum-verified Rust binary
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh

# 2. Detect harnesses, back up configs, and register hooks and MCP
everett onboard --yes --no-backfill

# 3. Verify stores, hooks, and live stdio MCP registration
everett doctor
```

Run bare `everett` in any terminal to launch the interactive TUI (`everett onboard`) with per-agent setup guides, exact file diffs, and optional account sign-in.

- Primary button (`#8e3811`): `Explore on GitHub` (`https://github.com/raitoxlol/everett`)
- Secondary button: `Read agent support and wake` (`docs/agent-support.md`)

### Footer columns

- **Brand:** `[Mark]` `everett` · MIT License · Named after Hugh Everett III.
- **Core:** Quickstart · Commands · Local Dashboard · Shared Core · Skill Hub · MCP
- **Harnesses:** Agent Support & Wake · T3 Code · OpenAI dots · Grok Bot · ChatGPT / Everett Local
- **Security:** Privacy · Account Setup · Relay Threat Model · Troubleshooting · Changelog
- **Project:** GitHub · Issues · Homebrew (`raitoxlol/tap`)

## Pre-publish checklist

- Recapture `docs/assets/onboarding/onboarding-motion.gif` and `onboarding-summary.png` on Everett 2.4.0 so the welcome and summary wording match the binary.
- Capture 1 light-mode `everett dashboard` screenshot (`#f3eee5` paper) and 1 20-second `ls` + `route` + `send` clip using synthetic sessions.
- Keep all capability statements aligned with `docs/agent-support.md` and `docs/v2.2-proof-bar.md`.
