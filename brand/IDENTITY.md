# Everett brand identity

Draft for review. Grounded in Everett 2.4.0 (`everett-rs`, commit `d8ed972`), `README.md`, `CHANGELOG.md`, `DESIGN.md`, and `PRODUCT.md`.

## 1. Origin and story

Everett is named after physicist Hugh Everett III, who proposed the many-worlds interpretation of quantum mechanics.

Developers who use coding agents rarely run a single terminal. On a normal workday, one person might have 2 Claude Code sessions editing backend code, a Codex thread updating UI state, a Devin CLI or OMP run debugging a test, a T3 Code thread on another worktree, and a cloud agent like OpenAI dots or Grok Bot checking a routine. Some of those sessions may run on a laptop and others on a Linux workstation.

Each session branches from the same repo and builds its own history. Without a layer above them, the branches drift. You lose track of which terminal owns which fix. You repeat the same deploy rule to 4 agents. You copy `SKILL.md` playbooks across `.claude`, `.codex`, `.agents`, and `.devin`. You miss a session that has been waiting on input for 45 minutes.

Everett is a single native Rust binary (`everett-rs`) that sits above those sessions. It reads existing session stores without changing them, tracks each session with a short card, routes requests to the matching branch, sends messages with explicit delivery states, serves `SKILL.md` playbooks from one local hub, and shares durable project facts across harnesses.

## 2. Mission and vision

### Mission statement

Give developers and their coding agents one neutral, local-first layer to see, reach, equip, and share memory across parallel sessions, whatever harness started them.

### Vision

A multi-harness workflow where Claude Code, Codex, OMP, Pi, Hermes Agent, Grok CLI, Devin CLI, Muse Code, T3 Code, and registered cloud agents work like one organized desk. Each branch keeps its own history, durable facts and playbooks stay aligned, and every message reports its exact delivery state.

## 3. Tagline options

### Primary recommendation

> One layer above all your coding-agent sessions.

This line already appears in `everett-rs/Cargo.toml`. It states where Everett sits in the stack in 7 plain words.

### Functional subline

> Route requests, messages, skills, and shared memory across your coding-agent sessions with one native Rust binary.

### Other options

| Use case | Tagline | Notes |
| --- | --- | --- |
| Primary (recommended) | One layer above all your coding-agent sessions. | Matches `Cargo.toml`. Clear and direct. |
| Action-first | Route requests, messages, skills, and shared memory across every coding agent you run. | Literal. Fits directories and subheads. |
| Short editorial | Many sessions. One mind. Keep your harnesses. | Echoes the README section `Shared core: one mind, many sessions`. |
| Visual metaphor | The dispatch desk for parallel coding agents. | Matches `DESIGN.md` (`Everett dispatch desk`). |
| Direct switch | Keep your harnesses. Connect your sessions. | Good for hero A/B tests against wrapper GUIs. |
| Technical | Native Rust meta-orchestration for parallel coding agents. | Fits Show HN and `r/rust`. |

## 4. Audience

### Multi-harness terminal developer

- Runs 3 to 10 parallel coding-agent sessions daily on macOS or Linux.
- Mixes Claude Code, Codex, OMP, Devin CLI, Grok CLI, Pi, Hermes Agent, and T3 Code across git worktrees and terminal splits.
- Wants to find active sessions fast, stop repeating project facts, share one set of `SKILL.md` playbooks, and get alerted when an agent is blocked.
- Trusts tools that ship as one native binary, open session stores read-only, work offline for local CLI commands, and state their limits plainly.

### Multi-machine and agent-to-agent developer

- Splits work across a MacBook and a Linux box or VM, or connects cloud agents (OpenAI dots, Grok Bot, ChatGPT desktop via `Everett Local`).
- Wants safe cross-machine delivery without opening a shell over SSH, and wants agents to hand off bounded tasks over MCP with verified caller identity.
- Trusts pinned SSH fingerprints, end-to-end encryption (`X25519 + HKDF-SHA256 + ChaCha20Poly1305` and `Ed25519` signatures), and a published threat model.

## 5. Positioning against alternatives

Everett is a local-first CLI, 18-tool MCP server, loopback HTML dashboard, and cross-device relay for parallel coding agents.

| Category | Examples | How they work | How Everett differs |
| --- | --- | --- | --- |
| All-in-one agent GUIs | Agent desktops and custom launchers | You must start sessions inside their app for the app to track or message them. | Everett reads the stores your harnesses already write (`~/.claude`, `~/.codex`, `~/.omp`, `~/.pi`, `~/.grok`, Hermes, Devin, T3 Code) read-only. You keep your terminal and editor. |
| Broadcast agent chat | Shared message rooms and pub-sub buses | Messages broadcast to every agent, or every agent polls one noisy room. | Everett routes each request to one matching session (`SESSION`, `NEW`, or `ASK`) using local BM25 or optional Jev. On a tie, it lists candidates and sends nothing. |
| Multi-agent frameworks | LangGraph, CrewAI, AutoGen | You write Python or TypeScript agent graphs against raw LLM APIs. | Everett connects the interactive coding agents you already run through CLI commands, hooks, and 18 stdio MCP tools. |
| Terminal multiplexers | tmux, Zellij, Ghostty splits | They organize panes and processes, with no awareness of cards, routing, skills, or shared memory. | Everett adds live 50-word cards, blocker alerts, routing, skill matching, and shared memory on top of your existing terminal setup. |
| Hosted memory SaaS | Cloud vector and memory APIs | They require an account and copy context to a remote server for basic recall. | Everett stores `~/.everett/core/` on local disk with secret filtering, 300-word distilled core files, local SQLite BM25 search, and expiring temporary facts. |

## 6. Messaging pillars

### 1. Read-only discovery across existing harnesses

Everett works with the sessions you already have.

- Discovers 8 local CLIs: Claude Code, Codex, OMP, Pi, Hermes Agent, Grok CLI, Devin CLI, and Muse Code.
- Recognizes T3 Code threads over Claude, Codex, Grok, and Devin as `[t3code]` without listing them twice.
- Connects OpenAI dots and Grok Bot through registered external inboxes and the Rust gateway, plus ChatGPT desktop via `Everett Local` and the hosted plugin.
- Reads bounded transcript slices and opens SQLite databases read-only (`mode=ro`). It leaves harness session files untouched.
- Ships as one native Rust binary (`everett-rs`, Rust 1.89, LTO and stripped) for macOS arm64/x86_64 and static Linux musl x86_64/aarch64.
- Limit to keep exact: Hark compatibility is unverified.

### 2. Targeted routing with live session cards

Everett sends each request to the one session it belongs to.

- Local BM25 plus recency scoring runs offline with zero API keys.
- Optional Jev (`typesafe.ai`) routing handles dense fleets with the same `SESSION`, `NEW`, and `ASK` contract.
- Worktree sessions for T3 Code and Claude resolve to their repository name, and multi-word project names like `web-mesh` match in order.
- Each session keeps a card of 50 words or fewer (`what`, `state`, `next`), written by the agent at startup or built deterministically by a zero-LLM Stop hook.
- Ambiguous `ASK` results return addressable candidates and send nothing.

### 3. Explicit delivery states and blocker alerts

Everett separates dispatch, inbox queueing, pickup, and reply.

- Default `ping` resumes idle local sessions headlessly, sends a verified native PTY nudge to an attached local Claude background session, nudges an attached OMP 18.12.2 extension watcher, or sends a fixed inbox nudge to an owner-local T3 thread through `t3 thread send`.
- When an immediate transport is unavailable, `ping` returns an error without silently queueing.
- Explicit `--mode inbox` handles hook injection (`UserPromptSubmit`, `PostToolUse`, OMP steering), MCP polling, external cloud agents, and remote peers.
- Local receipt commands (`everett status MESSAGE_ID`, `everett sent`, `everett retry MESSAGE_ID`, and `everett inbox --reply-to ID`) check pickup and recover retained replies.
- Deterministic Stop hooks classify turns into `done`, `blocked`, and `needs-input`, notify via macOS `osascript` or `notify_command`, and escalate blockers after 30 minutes.
- Limit to keep exact: `queued` records an inbox write, `delivered` records device ingress or inbox consumption, `claimed` records pickup, and `replied` records a reply linked by `reply_to`. Task completion is separate.

### 4. Shared core memory with local search and expiry

Sessions keep independent working histories while sharing durable project knowledge.

- `everett learn` filters secrets by pattern and entropy, caps facts at 500 characters, and supports temporary facts with `--ttl-seconds` (1 to 31536000 seconds).
- `everett trunk merge` distills queued facts into global and per-project core files of 300 words or fewer using `--llm claude`, `--llm codex`, or deterministic `--llm none`. Previous versions archive under `~/.everett/core/history/`.
- SessionStart hooks inject up to 350 words of core in about 40 ms from local files.
- `everett trunk schedule` installs nightly merges via macOS `launchd` or Linux `systemd --user` timers.
- `everett core search` runs local SQLite BM25 over accepted learnings beyond the 300-word cap. `forget` and `correct` update active, pending, and searchable memory right away.

### 5. Cross-harness skill hub

Everett indexes `SKILL.md` playbooks across every harness on the machine and returns the single skill that fits a task.

- `everett skills`, the `everett_skill` MCP tool, and MCP prompts (`prompts/list`, `prompts/get`) read `~/.claude/skills`, `~/.codex/skills`, `~/.agents/skills`, `~/.config/devin/skills`, `~/.devin/skills`, project skill folders, and custom roots in `EVERETT_SKILL_DIRS` or `[skills] dirs`.
- Local lexical matching (`--match` or `action: "match"`) runs with zero LLM calls and returns 1 matching skill or null.
- Identical copies deduplicate with clear source precedence. Per-project use counts live in `~/.everett/skills/usage.json`.
- Claude Code and Codex prompt hooks inject explicitly named skills (such as `/prove`) or suggest 1 fitting user-owned skill per session.

### 6. Clear account and cross-device boundaries

Everett states exactly what runs offline and what uses a network service.

- Local CLI commands (`ls`, `route`, `send`, `cards`, `skills`, `learn`, `core`, `trunk`, `dashboard`, `onboard`, `doctor`) and SSH peers require zero Everett account.
- `everett dashboard` serves a read-only local HTML view on `127.0.0.1` with zero JavaScript runtime and rejects foreign Host headers and write methods.
- Calling Everett's 18 stdio MCP tools (`tools/call`, built on `rmcp`) requires 1 `everett login` per Everett profile. Each call verifies the Cloudflare credential online, enforces host-bound caller identity, and logs tool and argument names without logging values.
- SSH peers (`everett peer`) verify pinned host and device fingerprints and run restricted forced-command RPC with no shell, PTY, or forwarding.
- Optional Cloudflare native relay (`everett relay`) encrypts payloads with `X25519 + HKDF-SHA256 + ChaCha20Poly1305`, signs envelopes with `Ed25519`, uses hibernating WebSockets so quiet receivers make zero scheduled polling requests, supports human-readable device names and opt-in exposed cards (`everett relay authors`), and offers an optional private read-only mesh console (`web-mesh/`) gated by expiring owner-approved read grants.
- Limit to keep exact: cross-owner relay grants are unavailable on Cloudflare, and relay metadata limits live in `docs/relay-threat-model.md`.

## 7. Voice and tone guidelines

### Core traits

1. **Action first.** Start with the command, state, or answer. Keep sentences short (25 words or fewer).
2. **Literal and factual.** State what Everett runs, what it writes, and what it skips. Skip metaphors, idioms, and AI hype.
3. **Peer to peer.** Write as one developer talking to another. Show real CLI output, flags, and exit codes.
4. **Honest about boundaries.** Put limits next to features. State what each delivery state records and which commands need `everett login`.

### Tone by surface

| Surface | Style | Example |
| --- | --- | --- |
| CLI and doctor | Short, diagnostic, 1 next command. | `Stale Claude MCP launcher detected. Run: everett install-mcp --claude --repair --apply` |
| README and docs | Structured tables, copyable commands, explicit boundaries. | `Local CLI commands and SSH peers need no account. Calling Everett MCP tools requires everett login in the same profile.` |
| Landing page | Scannable benefits backed immediately by real terminal output. | `Eight harnesses. One native Rust binary. Zero wrapper lock-in.` |
| Launch and social | First-person builder voice with concrete workflows and clear limits. | `We built a native Rust layer above Claude Code, Codex, OMP, Devin, and T3 Code so parallel sessions can share routing, skills, and memory.` |

### Rewrites

| Avoid | Use |
| --- | --- |
| Unleash an autonomous 10x AI engineering swarm. | See, route, message, and share memory across parallel Claude Code, Codex, OMP, Devin, Grok, Pi, Hermes, and Muse Code sessions. |
| Instantly wakes and syncs every AI agent in real time. | Resumes idle local sessions headlessly, nudges verified local Claude, OMP, and T3 sessions, and queues explicit inbox messages everywhere else. |
| Completely offline zero-account MCP and cloud sync. | Local CLI, local dashboard, and SSH peers work offline with no account. Calling MCP tools and using the optional Cloudflare relay require `everett login`. |
| Infinite neural hive mind that never forgets. | Secret-filtered learnings distilled into 300-word global and project cores, backed by local SQLite BM25 search and expiring temporary facts. |

### Word choices

| Use | Avoid |
| --- | --- |
| harness, coding-agent session | AI employee, digital coworker, agentic workforce |
| route, send, ping, hand off | autonomously orchestrate, swarm, magically sync |
| session card, shared core, skill hub | hive mind, neural memory, AI brain |
| single native Rust binary | blazingly fast next-gen platform |
| read-only discovery | deep telemetry, surveillance |

## 8. Logo and visual direction

### Finalized Everett logo mark (`M6`, Many-Worlds Horizon Balloon)

Evolved from the `everett-v2.2.0-launch.mp4` horizon-sun-balloon mark through a 3-provider design council and finalized as `M6` (`brand/assets/everett-mark.svg`).

- **Geometry (`viewBox="0 0 128 128"`):** master circle `R=43` centered at `(64, 60)`. The upper sun dome ends at `y=54` with `r=1.8` bottom fillets. A `108x7` horizon bar (`y=62..69`, `r=3.5` caps) flows into a connected `S`-ribbon (`7`-unit middle bar at `y=77..84`, equal `8`-unit gaps with `r=4` U-turns, `8.5`-unit bridges) whose outer contour completes 90% of the top circle (`60°` tangent at `y=97.24`) and finishes in a soft balloon apex (`r=5.5`). Built from 2 closed analytical paths with zero strokes and zero polyline chains.
- **Monochrome primary:** `everett-mark.svg` (`currentColor`), `everett-mark-black.svg` (`#000000`), and `everett-mark-white.svg` (`#FFFFFF`).
- **Two-tone sun edition:** `everett-mark-sun-light.svg` (`#B64F1A` dome + `#141517` lower `S` on paper) and `everett-mark-sun-dark.svg` (`#FF5A1F` dome + `#EDE6DC` lower `S` on `#0E0C0B` ink).
- **Favicons and app icons:** `everett-favicon-dark.svg`, `everett-favicon-light.svg`, and `everett-favicon-sun.svg` (`128x128` tiles with `rx=28`).
- **Lockups and banners:** horizontal lockups (`everett-lockup-light.svg`, `everett-lockup-dark.svg`), `1120x260` GitHub README hero banners (`everett-hero-light.svg`, `everett-hero-dark.svg`), and a `1200x630` OpenGraph card (`everett-og-card.png`).
- **Companion marks in existing surfaces:** `everett onboard` (`everett-rs/src/tui.rs`) keeps its animated Braille sunset balloon in the terminal; `site/assets/favicon.svg` on `raitox.tech/everett` uses the 16x16 pixel tree mark. Use `M6` as the primary Everett product mark across README, app icons, lockups, and marketing.

### Wordmark

- Use sentence-case `Everett` in display lockups (matching `Everett 2.4`) and lowercase `everett` in CLI contexts.
- Set the wordmark in **Sora** weight `700` with `-0.035em` tracking (outlined as `<path>` geometry in `everett-lockup-*.svg` and `everett-hero-*.svg`).
- Lockup layout: `M6` horizon balloon mark beside or stacked above `Everett`, with optional version pill (`v2.4`) or monospace install strip (`brew tap raitoxlol/tap && brew install everett`).

### Color palette

Two linked palettes already exist across Everett's live surfaces:

#### Product and local dashboard palette (`DESIGN.md` + launch video)

| Token | Hex | Role |
| --- | --- | --- |
| `paper` | `#f3eee5` | Primary warm light canvas |
| `paper-raised` | `#fffaf1` | Elevated cards and manifest rows |
| `paper-muted` | `#e9e1d5` | Inset strips, table headers, secondary pills |
| `ink` | `#141517` / `#1d211f` | Primary text, dark bars in mark, dark pills |
| `ink-soft` | `#5d625d` | Secondary metadata, paths, and ages |
| `carbon` | `#242825` | Navigation rail and dark terminal frames |
| `orange` | `#b64f1a` | Brand signal accent (sun dome, active stop, highlight) |
| `orange-deep` | `#8e3811` | Primary CTA fill with `#fffaf1` text |
| `orange-wash` | `#f2d7c5` | Soft active tint on light surfaces |
| `green` | `#216b4c` | Detected harness or passing check |
| `red` | `#a1382f` | Attention, blocked state, or error |
| `focus` | `#145e75` | Keyboard focus ring |
| `line` | `#cbc1b3` | 1px dividers and card borders |

#### Dark web and `raitox.tech/everett` palette (`raitox.tech/DESIGN.md` + `site/assets/style.css`)

| Token | Hex | Role |
| --- | --- | --- |
| `site-ink` | `#0E0C0B` | Dark website and docs ground |
| `site-bone` | `#EDE6DC` | Headings, bold text, links, dark-mode mark bars |
| `site-sun` | `#FF5A1F` | Vivid orange accent on `raitox.tech` and `web-mesh` |
| `site-momo` | `#F596AA` | Section numbers, primary buttons, routed state on `raitox.tech` |
| `orange-bright` | `#d97236` | Dark-mode oxide orange for the horizon-sun-balloon dome |

#### Signal rule (from `DESIGN.md`)

Orange marks activity and primary actions. Red marks attention. Green marks a detected harness or passing check. Every status indicator pairs color with a text label or shape (circle for ordinary activity, rounded square for attention). Use no decorative gradients and no glass panels.

### Typography

1. **Brand display, wordmark, public site, and docs:**
   - Primary sans: **Sora** (`brand/assets/fonts/Sora-var.woff2`, SIL OFL 1.1; weight `400` to `700`, `-0.035em` display tracking). Designed by Jonathan Barnbrook and Julián Moncada; wide low-contrast geometric counters pair cleanly with `M6`.
   - Mono: **Geist Mono** (`site/assets/fonts/GeistMono-400.woff2`) for commands, session IDs, file paths, exit codes, and labels.
2. **Local dashboard (`everett dashboard`, zero remote fonts per `DESIGN.md`):**
   - Sans: `ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif` (Title `1.75rem / 700 / -0.035em`, Heading `1rem / 700 / -0.015em`, Body `0.88rem / 1.55`).
   - Mono: `ui-monospace, SFMono-Regular, Menlo, monospace` (`0.76rem / 1.45`).
3. **Motion and video endcards:**
   - Display: **Sora** (`600` to `700`, `-0.035em`) paired with **Geist Mono** for terminal pills.
