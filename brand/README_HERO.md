# Everett README hero, badge, and visual asset suggestions

Draft suggestions for review. `README.md` has not been changed. Grounded in Everett 2.4.0 (`everett-rs/Cargo.toml`, `README.md`, `DESIGN.md`, `.github/workflows/rust.yml`).

## 1. Current README opening review

The current `README.md` opening is honest and thorough, and puts 3 dense caveat paragraphs ahead of the install command. Adding a compact hero block, 6 muted badges, and a 4-row compatibility table right under the quickstart keeps every boundary visible while letting first-time readers grasp the tool in 5 seconds.

## 2. Suggested README hero block

Uses GitHub `<picture>` theme switching so light mode sits on warm paper (`#f3eee5`) and dark mode sits on deep carbon (`#171a18`), paired with flat-square badges in Everett's palette.

```markdown
<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/brand/everett-hero-dark.svg" />
    <source media="(prefers-color-scheme: light)" srcset="docs/assets/brand/everett-hero-light.svg" />
    <img alt="Everett: one layer above all your coding-agent sessions" src="docs/assets/brand/everett-hero-light.svg" width="560" />
  </picture>
</p>

<h3 align="center">One layer above all your coding-agent sessions.</h3>

<p align="center">
  Route requests, messages, <code>SKILL.md</code> playbooks, and shared memory across parallel AI coding agents with one native Rust binary.
</p>

<p align="center">
  <a href="https://github.com/raitoxlol/everett/releases"><img alt="Release" src="https://img.shields.io/github/v/release/raitoxlol/everett?style=flat-square&color=b64f1a&labelColor=242825" /></a>
  <a href="https://github.com/raitoxlol/everett/actions/workflows/rust.yml"><img alt="Rust CI" src="https://img.shields.io/github/actions/workflow/status/raitoxlol/everett/rust.yml?style=flat-square&label=CI&labelColor=242825&color=216b4c" /></a>
  <img alt="Rust 1.89+" src="https://img.shields.io/badge/Rust-1.89%2B-b64f1a?style=flat-square&labelColor=242825" />
  <img alt="Platforms" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-5d625d?style=flat-square&labelColor=242825" />
  <img alt="MCP Tools" src="https://img.shields.io/badge/MCP-24%20tools-b64f1a?style=flat-square&labelColor=242825" />
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-216b4c?style=flat-square&labelColor=242825" /></a>
</p>

<p align="center">
  <a href="#quickstart">Quickstart</a> ·
  <a href="#demo">Demo and Dashboard</a> ·
  <a href="#for-agents-mcp">MCP (24 tools)</a> ·
  <a href="docs/agent-support.md">Agent Support</a> ·
  <a href="docs/relay-threat-model.md">Threat Model</a> ·
  <a href="CHANGELOG.md">Changelog</a>
</p>
```

Workflow note: `.github/workflows/rust.yml` runs `clippy` and `cargo test` on macOS and Linux. `.github/workflows/ci.yml` runs the release journey. Swap `rust.yml` for `ci.yml` if you prefer the release-journey badge.

## 3. Badge rules

Keep the shelf to 6 flat-square badges in Everett's exact colors:

| Badge | Purpose | Label BG | Value color |
| --- | --- | --- | --- |
| Release (`v2.4.0`) | Latest shipped binary | `#242825` | `#b64f1a` |
| CI (`rust.yml`) | Live macOS and Linux test status | `#242825` | `#216b4c` |
| Rust 1.89+ | Native binary signal | `#242825` | `#b64f1a` |
| Platform | Supported operating systems | `#242825` | `#5d625d` |
| MCP (24 tools) | Agent integration surface | `#242825` | `#b64f1a` |
| License (MIT) | Open-source license | `#242825` | `#216b4c` |

Skip vanity badges (stars, repo size, downloads, lines of code, social counters).

## 4. Hero SVG banners (ready in `brand/assets/`)

Both `1120x260` vector banners are generated under `brand/assets/` (copy to `docs/assets/brand/` when adopting in `README.md`):

- **`brand/assets/everett-hero-light.svg`:** `rx="14"` rectangle in `#F3EEE5` with a 2px `#CBC1B3` border, monochrome `#000000` `M6` mark, vector-outlined `Sora 700` `Everett` wordmark in `#141517`, `v2.4` badge, hairline divider, and monospace harness line `CLAUDE · CODEX · OMP · PI · HERMES · GROK · DEVIN · T3`.
- **`brand/assets/everett-hero-dark.svg`:** same layout on `#0E0C0B` (`site-ink`) with a `#2A2623` border, `#FFFFFF` `M6` mark, `#EDE6DC` outlined `Sora 700` wordmark, `#F596AA` (`momo`) `v2.4` badge, and `#9F988E` harness line.
- Flat fills only. Zero gradients, drop shadows, or embedded raster images.

## 5. Compact compatibility block (below quickstart)

```markdown
| Area | Current support and limits |
| --- | --- |
| Local CLIs (8) | Claude Code · Codex · OMP · Pi · Hermes Agent · Grok CLI · Devin CLI · Muse Code. Read-only discovery, headless resume and spawn, plus native hooks, extensions, or the Hermes plugin where supported. |
| GUI and cloud | T3 Code (`[t3code]` over Claude, Codex, Grok, Devin), OpenAI dots and Grok Bot (registered external inboxes and gateway), the Meta Muse VM agent (registered external inbox, `muse-agent`), ChatGPT desktop (`Everett Local` and hosted plugin). Hark compatibility is unverified. |
| Accounts | Local CLI, `everett dashboard`, and SSH peers need no account. Calling Everett MCP tools (`tools/call`) and optional Cloudflare E2EE relay require 1 `everett login` per profile. |
| Wake and receipts | Direct pings run on idle local sessions and verified local Claude, OMP 18.12.2, and owner-local T3 targets. Other paths use `--mode inbox`. See [agent support and wake](docs/agent-support.md) and [v2.2 proof-bar status](docs/v2.2-proof-bar.md). |
```

## 6. Visual asset plan

`README.md` lines 130 to 133 note that `docs/assets/onboarding/onboarding-motion.gif` and `onboarding-summary.png` predate the 2.4.0 welcome and account wording and need recapture. Capture 3 assets from synthetic sessions in a disposable `HOME`:

1. **`docs/assets/demo/dispatch-loop.gif` (18 to 22 seconds):** Run `everett ls`, `everett route "cap upload retries at 5"`, `everett send "cap upload retries at 5"`, and `everett skills --match "verify release"` on `#171a18` dark carbon with Menlo or JetBrains Mono, 12 to 15 fps, under 1.5 MB.
2. **`docs/assets/demo/dashboard-light.png` (1280x760 at 2x):** Capture `everett dashboard` on `#f3eee5` warm paper with 5 to 6 synthetic sessions across Claude, Codex, OMP, Devin, and `[t3code]`, with 1 row expanded showing its card and copyable CLI actions.
3. **Refreshed onboarding captures:** Run `sh scripts/demo-onboarding.sh` on the 2.4.0 binary to regenerate `docs/assets/onboarding/onboarding-motion.gif` and `onboarding-summary.png` with the current welcome text and `everett_ls` verification step.
