# Everett brand guidelines

Draft reference sheet for review. Nothing is published. Applies to CLI copy, the local dashboard, README, docs site, landing page, release notes, and social posts. Grounded in Everett 2.4.0 (`everett-rs`, commit `d8ed972`), `DESIGN.md`, and `PRODUCT.md`.

## Canonical lock (do not regenerate)

The visual identity below is locked. Future sessions, marketing assets, sites, and releases must reuse these exact files and tokens instead of drawing new marks or swapping display fonts:

- **Canonical mark:** `M6` Many-Worlds Horizon Balloon ([`brand/assets/everett-mark.svg`](assets/everett-mark.svg), plus static black, white, sun two-tone, favicon, lockup, and hero variants).
- **Canonical typography:** **Sora** ([`brand/assets/fonts/Sora-var.woff2`](assets/fonts/Sora-var.woff2), SIL OFL 1.1) for wordmarks, headings, and web copy, paired with **Geist Mono** (`site/assets/fonts/GeistMono-400.woff2`) for commands, paths, IDs, and labels.
- **Machine-readable manifest:** [`brand/brand.json`](brand.json).

## 1. Brand basics

- **Name:** `everett` in lowercase for logo lockups and CLI contexts; `Everett` in prose and headings. Named after physicist Hugh Everett III (many-worlds): every coding-agent session branches from the same origin with its own history.
- **Mission:** Give developers and their coding agents one neutral, local-first layer to see, reach, equip, and share memory across parallel sessions, whatever harness started them.
- **Primary tagline:** One layer above all your coding-agent sessions.
- **Functional subline:** Route requests, messages, skills, and shared memory across your coding-agent sessions with one native Rust binary.
- **Design direction:** Everett dispatch desk (`DESIGN.md`). Warm paper surfaces, a dark charcoal rail, 1px rules, and one oxide-orange signal accent. Real session data owns the screen.

## 2. Five accuracy rules

Check every piece of copy against these 5 facts before publishing:

1. **Supported harnesses:** 8 local CLIs (Claude Code, Codex, OMP, Pi, Hermes Agent, Grok CLI, Devin CLI, Muse Code), T3 Code overlay (`[t3code]` over Claude, Codex, Grok, Devin), OpenAI dots and Grok Bot (registered external inboxes), and ChatGPT desktop (`Everett Local` and hosted plugin). Hark and Muse VM agent compatibility is unverified.
2. **Routing:** Local BM25 plus recency scoring runs offline with zero keys. Jev (`typesafe.ai`) is optional. `ASK` returns candidates and sends nothing.
3. **Delivery receipts:** `queued` records an inbox write, `delivered` records device ingress or inbox consumption, `claimed` records pickup, and `replied` records a reply linked by `reply_to`. Task completion is separate. "Active" in `everett ls` and `everett dashboard` is a process and recency check (file changed in the last 10 minutes), separate from a live heartbeat.
4. **Accounts:** Local CLI commands (`ls`, `route`, `send`, `cards`, `skills`, `learn`, `core`, `dashboard`, `doctor`) and SSH peers need zero Everett account. Calling the 18 stdio MCP tools (`tools/call`) and using the optional Cloudflare E2EE relay require 1 `everett login` per profile.
5. **Verified proof only:** Publish zero invented customer logos, unmeasured speed claims, or cross-owner Cloudflare relay grants (unavailable). Link directly to `docs/agent-support.md` and `docs/relay-threat-model.md`.

## 3. Voice and writing rules

- Start with the action, command, or answer. Keep sentences short (25 words or fewer).
- Use plain, literal words. Skip metaphors, idioms, em dashes, and AI hype ("swarm", "10x", "hive mind", "seamless").
- State what Everett does directly. Avoid contrast framing ("X, not Y") and avoid "doesn't prove" disclaimers. State what each command ran and what it returned.
- Put caveats and limits beside the feature they apply to.
- Use sentence-case headings and digits for numbers (`8 harnesses`, `18 tools`, `50 words`).

## 4. Visual tokens

### Colors (`DESIGN.md` plus dark extension)

| Role | Light (dispatch desk) | Dark (terminal / GitHub dark) | Rule |
| --- | --- | --- | --- |
| Primary canvas | `paper` `#f3eee5` | `carbon-deep` `#171a18` | Warm paper or deep carbon ground |
| Elevated surface | `paper-raised` `#fffaf1` | `carbon-raised` `#242825` | Separate with 1px rules |
| Primary text | `ink` `#1d211f` | `paper-ink` `#f3eee5` | High contrast (AA/AAA) |
| Secondary text | `ink-soft` `#5d625d` | `paper-ink-soft` `#a9a69d` | Metadata, paths, ages |
| Hairline rule | `line` `#cbc1b3` | `carbon-line` `#3a403c` | 1px borders and table dividers |
| Brand signal | `orange` `#b64f1a` | `orange-bright` `#d97236` | Active stops, live dots, highlights |
| Primary action | `orange-deep` `#8e3811` | `orange-bright` `#d97236` | Primary button fill (`#fffaf1` text on light) |
| Soft tint | `orange-wash` `#f2d7c5` | none | Subtle light-mode active tint |
| Healthy / detected | `green` `#216b4c` | `#3aa174` (draft) | Detected harness or passing check |
| Attention / blocked | `red` `#a1382f` | `#d65a4f` (draft) | `⚠ blocked`, refusal, error |
| Keyboard focus | `focus` `#145e75` | `#f3eee5` | Visible outline on focus |

Signal rule: orange marks activity and primary actions, red marks attention, and green marks a detected harness or passing check. Always pair color with a text label or shape (circle for normal activity, rounded square for attention). Use no gradients, no second brand accent, and no glass panels.

### Typography

- **Brand display, wordmark, website, and docs:**
  - Primary sans: **Sora** (Jonathan Barnbrook and Julián Moncada, SIL OFL 1.1; self-hosted variable font in [`brand/assets/fonts/Sora-var.woff2`](assets/fonts/Sora-var.woff2), weight `400` to `700`, `-0.035em` display tracking).
  - Mono: **Geist Mono** (`site/assets/fonts/GeistMono-400.woff2`) for commands, IDs, paths, exit codes, and ages.
- **Local dashboard (`everett dashboard`, zero remote fonts):**
  - Sans: `ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif` (Title `1.75rem / 700 / -0.035em`, Heading `1rem / 700 / -0.015em`, Body `0.88rem / 1.55`).
  - Mono: `ui-monospace, SFMono-Regular, Menlo, monospace` (`0.76rem / 1.45`).

### Mark, shapes, and motion

- **Final mark (`M6`, Many-Worlds Horizon Balloon):** built on an `R=43` circle at `(64, 60)` on a `128x128` grid. A buoyant upper dome (`y=54`, `r=1.8` corners) sits over a `108x7` horizon bar (`y=62..69`, `r=3.5`) and a connected `S`-flow reflection (`7`-unit middle bar, `8`-unit equal gaps, `8.5`-unit bridges) wrapping `60°` around the circle (`y=97.24`) into a soft balloon apex (`r=5.5`). Pure analytical `A`/`Q`/`H`/`L` fills with zero strokes.
- **Wordmark:** `Everett` (display) or `everett` (CLI) in **Sora** weight `700`, `-0.035em` tracking (converted to vector `<path>` outlines in the standalone lockup and hero SVGs so GitHub renders the exact glyph shapes). Clear space around the lockup is at least the height of `e`.
- **Corner radii:** Dashboard uses `10px` surfaces, `7px` controls, `5px` tags. `raitox.tech` uses squircle tokens `--r-sm` (`6px`/`10px`), `--r-md` (`10px`/`16px`), `--r-lg` (`16px`/`26px`). App icon tiles use `rx=28` on `128x128`.
- **Motion:** 150 to 180 ms transitions. Honor `prefers-reduced-motion` on web and `EVERETT_NO_ANIM=1` and `NO_COLOR=1` in the terminal.

## 5. Package and asset index

### Brand and marketing drafts

| File | Contents |
| --- | --- |
| [`BRAND_GUIDELINES.md`](BRAND_GUIDELINES.md) | This master reference sheet. |
| [`IDENTITY.md`](IDENTITY.md) | Origin, mission, 6 taglines, audience, comparison, 6 messaging pillars, voice guide, and finalized `M6` logo specs. |
| [`LANDING_PAGE.md`](LANDING_PAGE.md) | Section-by-section landing page wireframe and copy outline. |
| [`LAUNCH_ANNOUNCEMENT.md`](LAUNCH_ANNOUNCEMENT.md) | Everett 2.4 launch post and Show HN draft. |
| [`README_HERO.md`](README_HERO.md) | Light/dark README hero block, 6 curated badges, SVG banner spec, and asset plan. |
| [`SOCIAL_POSTS.md`](SOCIAL_POSTS.md) | 5 post drafts for X, Bluesky/Mastodon, LinkedIn, Reddit, and a Skill Hub spotlight. |

### Finalized `M6` visual asset family (`brand/assets/`)

| File | Purpose |
| --- | --- |
| [`assets/brand-family-sheet.png`](assets/brand-family-sheet.png) | Complete visual family sheet (`brand-family-sheet.html` source). |
| [`assets/everett-mark.svg`](assets/everett-mark.svg) | Canonical `currentColor` mark (`128x128`, pure fills). |
| [`assets/everett-mark-black.svg`](assets/everett-mark-black.svg) / [`everett-mark-white.svg`](assets/everett-mark-white.svg) | Static `#000000` and `#FFFFFF` monochrome marks. |
| [`assets/everett-mark-sun-light.svg`](assets/everett-mark-sun-light.svg) / [`everett-mark-sun-dark.svg`](assets/everett-mark-sun-dark.svg) | Two-tone edition (`#B64F1A` / `#141517` light; `#FF5A1F` / `#EDE6DC` dark). |
| [`assets/everett-favicon-dark.svg`](assets/everett-favicon-dark.svg) / [`light`](assets/everett-favicon-light.svg) / [`sun`](assets/everett-favicon-sun.svg) | App icon and favicon tiles (`128x128`, `rx=28`). |
| [`assets/everett-lockup-light.svg`](assets/everett-lockup-light.svg) / [`everett-lockup-dark.svg`](assets/everett-lockup-dark.svg) | Horizontal `M6` + vector-outlined `Sora 700` `Everett` lockups (`325x112`). |
| [`assets/everett-hero-light.svg`](assets/everett-hero-light.svg) / [`everett-hero-dark.svg`](assets/everett-hero-dark.svg) | GitHub README theme-switching hero banners (`1120x260`, outlined `Sora 700` wordmark). |
| [`assets/everett-og-card.png`](assets/everett-og-card.png) | `1200x630` OpenGraph / social share card (`everett-og-card.html` source). |
| [`assets/fonts/Sora-var.woff2`](assets/fonts/Sora-var.woff2) / [`Sora-OFL.txt`](assets/fonts/Sora-OFL.txt) | Self-hosted `Sora` variable font (`100` to `800`) and SIL OFL 1.1 license. |
