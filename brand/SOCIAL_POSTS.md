# Everett social and community post drafts

Drafts for review. Nothing has been posted. Grounded in Everett 2.4.0 (`everett-rs`, commit `d8ed972`).

## Draft 1: X launch thread (5 posts)

**1/5**
If you run parallel coding agents across Claude Code, Codex, OMP, Devin CLI, and T3 Code, you can keep your terminal and connect your sessions from above.

Everett 2.4 is a single native Rust binary (MIT) that discovers, routes, equips, and shares memory across all of them.
[Attach: 20s terminal GIF of `everett ls`, `everett route`, and `everett send`]

**2/5**
Everett opens the transcript and SQLite stores your harnesses already write in read-only mode. Your session files stay untouched.

Each session gets a 50-word live card (`what`, `state`, `next`). `everett send "cap upload retries at 5"` routes with offline local BM25 (or optional Jev). On a tie, it lists candidates and sends nothing.

**3/5**
Default `ping` runs idle local sessions headlessly or nudges verified local Claude, OMP 18.12.2, and owner-local T3 targets. When an immediate transport is unavailable, it returns an error without silently queueing.

For async handoffs, `--mode inbox` pairs with `everett status`, `everett sent`, and `everett retry` to track `queued`, `delivered`, `claimed`, and `replied` states.

**4/5**
New in 2.4: stop repeating repo rules and copying `SKILL.md` files across `.claude`, `.codex`, `.agents`, and `.devin`.

- `everett learn` builds secret-filtered 300-word global and project cores, backed by local SQLite BM25 search and `--ttl-seconds` expiry.
- `everett skills --match` and `everett_skill` return the 1 playbook that fits a task with zero LLM calls.

**5/5**
- Local CLI, loopback `everett dashboard`, and pinned SSH peers need zero account.
- One `everett login` unlocks 18 native Rust stdio MCP tools (`rmcp`) and optional Cloudflare E2EE relay (`X25519 + ChaCha20Poly1305`, `Ed25519`, hibernating WebSockets).

Install on macOS or Linux:
```
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh
everett onboard --yes --no-backfill
```
MIT: https://github.com/raitoxlol/everett

---

## Draft 2: Single-post announcement (X, Bluesky, Mastodon)

Everett 2.4 is out. One native Rust binary (MIT) above your parallel coding-agent sessions: Claude Code, Codex, OMP, Pi, Hermes, Grok CLI, Devin CLI, and T3 Code.

Read-only discovery, 50-word live cards, offline BM25 routing, cross-harness `SKILL.md` matching, secret-filtered shared core memory, local HTML dashboard, 18 MCP tools, and SSH or E2EE Cloudflare relay.

https://github.com/raitoxlol/everett
[Attach: light-mode `everett dashboard` screenshot]

---

## Draft 3: LinkedIn engineering post

Senior engineers rarely stick to one coding agent anymore. A normal afternoon mixes Claude Code in one worktree, Codex in another, Devin CLI or OMP on a background test failure, a T3 Code thread on desktop, and sometimes a Linux workstation over SSH.

We built Everett, named after Hugh Everett III (many-worlds), because every session branches from the same origin with its own history.

Everett 2.4 ships as a single native Rust binary (MIT) built around 4 rules:

1. Read existing session stores read-only. You keep your terminal, editor, and preferred harnesses.
2. Route each request to 1 matching session using offline local BM25 (or optional Jev). On a tie, Everett lists candidates and sends nothing.
3. Share durable context across harnesses. Secret-filtered `everett learn` facts distill into 300-word global and project cores injected at startup, backed by local SQLite BM25 search and temporary TTL expiry. New in 2.4, `everett skills` and `everett_skill` index `SKILL.md` playbooks across harnesses and match the 1 skill a task needs.
4. State boundaries plainly. Local CLI commands, the loopback-only HTML dashboard (`everett dashboard`), and pinned SSH peers require zero account. Calling the 18 native Rust MCP tools or using the optional Cloudflare E2EE relay requires 1 `everett login` per profile, with relay metadata limits documented in `docs/relay-threat-model.md`.

Open source (MIT) for macOS and Linux: https://github.com/raitoxlol/everett
[Attach: split image of terminal `everett ls` and `everett dashboard`]

---

## Draft 4: Reddit technical post (`r/rust` or `r/ClaudeAI`)

**Title (`r/rust`):** Everett 2.4: one native Rust binary to discover, route, and share memory across parallel coding-agent sessions (MIT)
**Title (`r/ClaudeAI`):** Everett 2.4 (MIT, Rust): see, route, share memory and `SKILL.md` playbooks across parallel Claude Code, Codex, Devin, OMP, and T3 sessions

I built Everett because running 5 to 8 parallel sessions across Claude Code, Codex, OMP, Devin CLI, Grok CLI, Pi, Hermes, and T3 Code meant constant tab hunting and repeating the same repo rules to every fresh agent.

We started in Python and retired the Python package in 1.5.0 for a single native Rust binary (`everett-rs`, Rust 1.89, LTO and stripped, static Linux musl and macOS arm64/x86_64).

What is inside 2.4.0:
- Read-only store adapters: bounded JSONL reads and bundled `rusqlite` opened with `mode=ro`. Harness transcripts stay untouched.
- Live 50-word session cards and deterministic Stop hooks that classify `done`, `blocked`, and `needs-input` with macOS `osascript` or custom `notify_command` alerts.
- Offline local BM25 plus recency routing (with worktree-aware repo names) or optional Jev routing returning `SESSION`, `NEW`, or `ASK`.
- Cross-harness skill hub (`everett skills` and `everett_skill`): indexes `SKILL.md` files across `.claude`, `.codex`, `.agents`, `.devin`, and project folders, matching the 1 fitting skill locally with zero LLM calls.
- Shared core memory: secret-filtered `everett learn` facts distilled into 300-word global and project cores, local SQLite BM25 search (`everett core search`), `--ttl-seconds` expiry, and immediate `forget` and `correct`.
- 18 stdio MCP tools (`rmcp`) with host-bound caller identity, a loopback-only read-only HTML dashboard (`everett dashboard`, zero JS runtime), account-free pinned SSH peers with forced-command RPC, and optional Cloudflare E2EE relay (`x25519-dalek`, `chacha20poly1305`, `ed25519-dalek`, hibernating WebSockets via `tokio-tungstenite`).

Two upfront limits:
- Local CLI commands, `everett dashboard`, and SSH peers need no account. Calling Everett MCP tools (`tools/call`) requires 1 `everett login` per profile.
- Direct wake paths exist for verified local Claude, OMP 18.12.2, and owner-local T3 targets. Other harnesses use headless resume or explicit `--mode inbox`. See `docs/agent-support.md`.

Repo (MIT): https://github.com/raitoxlol/everett. Happy to discuss the Rust architecture, `rmcp` setup, or hook contracts.

---

## Draft 5: Feature spotlight (Skill Hub and Shared Core)

Running several coding agents creates two steady sources of drag: duplicating `SKILL.md` playbooks across `.claude`, `.codex`, `.agents`, and `.devin`, and re-teaching every fresh session the same deploy rules.

Everett 2.4 solves both locally:

1. **Skill Hub (`everett skills` and `everett_skill`)**
   Indexes every `SKILL.md` across your harnesses and repo ancestors, deduplicates copies, and matches locally with zero LLM calls to return the 1 playbook a task needs. Claude Code and Codex prompt hooks inject named skills automatically.

2. **Shared Core (`everett learn` and `everett core`)**
   Save a secret-filtered fact once (durable or expiring with `--ttl-seconds`). Nightly `everett trunk merge` distills learnings into 300-word global and project cores injected at startup in about 40 ms, while `everett core search` runs local SQLite BM25 over facts beyond the 300-word cap.

Every session keeps its own working history while durable project facts and playbooks stay in sync.

https://github.com/raitoxlol/everett
[Attach: code card showing `everett skills --match` and `everett core search`]
