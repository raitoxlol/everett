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
- `brand/BRAND_GUIDELINES.md`, `brand/brand.json`, `brand/assets/` — locked Everett brand identity.

## Brand identity (locked)

Reuse the canonical assets under `brand/assets/`. Do not invent or regenerate alternate Everett logos, wordmarks, or display fonts:

- **Logo mark:** `M6` Many-Worlds Horizon Balloon (`brand/assets/everett-mark.svg`, `currentColor`, `128x128`, `R=43` at `(64,60)`, `60°` wrap, `108x7` horizon, `8u` gaps, 2 closed analytical paths, zero strokes). Static black/white (`everett-mark-black.svg`, `everett-mark-white.svg`), sun two-tone (`everett-mark-sun-light.svg`, `everett-mark-sun-dark.svg`), favicons (`everett-favicon-*.svg`), lockups (`everett-lockup-*.svg`), and README banners (`everett-hero-*.svg`) all derive from `M6`.
- **Typography:** **Sora** (SIL OFL 1.1, self-hosted at `brand/assets/fonts/Sora-var.woff2`, weight `700` at `-0.035em` for wordmarks and display headlines) paired with **Geist Mono** (`site/assets/fonts/GeistMono-400.woff2`) for CLI commands, IDs, and labels. Local `everett dashboard` keeps zero remote system fonts per `DESIGN.md`.
- **Colors:** monochrome `#000000` / `#FFFFFF`, warm paper `#F3EEE5` + oxide `#B64F1A` (`DESIGN.md`), and dark ink `#0E0C0B` + bone `#EDE6DC` + sun `#FF5A1F` + momo `#F596AA` (`raitox.tech/everett`).
- **Source of truth:** `brand/brand.json` and `brand/BRAND_GUIDELINES.md`.

