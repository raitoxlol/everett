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
- `everett/cli.py` — command dispatch.
- `everett/cards.py` — per-session summaries shown by the router and listing.
- `everett/route.py` — local BM25 and Jev routing, resume commands.
- `everett/config.py` — `~/.everett/config.toml` and env overrides.
- `everett/install.py`, `everett/doctor.py` — hook install and health check.
- `everett/trunk.py` — generated vault session view.
- `everett/core.py` — shared core: learn, merge, context injected by SessionStart hooks.
- `everett/mcp.py` — stdio MCP server (tools for agents).
- `everett/adapters/` — one module per harness: claude, codex, omp, pi, hermes, grok, plus the t3code overlay (all read-only).
- `tests/` — tests; every module imports `sandbox` first so HOME is a temp dir.

