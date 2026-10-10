# Everett agent context

Read this before working in Everett so another agent can pick up the same state.

## Purpose and current state

- Everett is the layer above all harness sessions (Claude Code, Codex, OMP, Pi, Hermes, Grok CLI; T3 Code threads annotate the sessions they drive). It lists them, routes and delivers work between them, and gives them one shared core. Agents use it mainly through `everett mcp`.
- Session cards live under `~/.everett/cards/`. They summarize one session; they are not the project-wide source of truth.
- `.everett/WORKING.md` is the repo-wide current state. Dated notes in `.everett/handoffs/` preserve task-level transfers.

## Working together

1. Before changing files, read `.everett/WORKING.md` and the newest relevant handoff. Check `git status` and the current branch.
2. Add your task, owner/session, files, and status to the Active work table before parallel work. Keep file ownership distinct. If work overlaps, coordinate in the shared note before editing.
3. Update shared state at meaningful milestones. Keep it factual and concise; link evidence rather than copying long logs.
4. Before handing work off, add a dated note from `.everett/handoffs/TEMPLATE.md` with what changed, what remains, exact paths, checks run (or not run), and the next action. Update `.everett/WORKING.md` to point to it.
5. Preserve other agents' uncommitted edits. Do not commit, push, merge, or publish unless asked.

## Useful entry points

- `README.md` — user-facing commands and data sources.
- `everett-rs/src/main.rs`, `cli.rs` — command dispatch.
- `everett-rs/src/cards.rs` — per-session summaries shown by the router and listing.
- `everett-rs/src/route.rs` — local BM25 and Jev routing, resume commands.
- `everett-rs/src/config.rs` — `~/.everett/config.toml` and env overrides.
- `everett-rs/src/install.rs`, `doctor.rs` — hook/MCP install (migrates retired Python registrations) and health check.
- `everett-rs/src/trunk.rs`, `core.rs` — vault session view; shared core learn/merge/context.
- `everett-rs/src/mcp.rs`, `gateway.rs` — stdio MCP server; owner-bound gateway for dots/Grok Bot.
- `everett-rs/src/adapters/` — one module per harness (all read-only), plus the t3code overlay.
- `everett-rs/tests/` — integration tests; `tests/common` gives each test a temp HOME.

