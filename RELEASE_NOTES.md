# Everett 2.2.0 — Cloudflare-only relay

Everett 2.2.0 removes the Convex relay backend. Cloudflare is the only hosted
relay; account-free local/offline messaging and explicit SSH pairing remain.

## Install or update

```bash
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh
everett doctor
```

The installer verifies archive checksums. Set `EVERETT_VERSION=v2.2.0` to pin
this version.

## Breaking changes

- Convex relay users must move to Cloudflare or use local/offline messaging or
  explicit SSH pairing. The Convex relay backend and its legacy OIDC,
  registry/address publication, SDK and CI are removed.
- Unsupported cross-owner grant CLI/MCP surfaces are removed; they are not
  carried forward as compatibility shims.

## What changed

- `ping` is now the CLI and MCP send default. Idle local sessions run headless;
  verified local Claude background sessions receive an immediate native inbox
  nudge. Unsupported immediate transports fail instead of silently queueing.
  `inbox`, `auto`, and `resume` remain explicit compatibility modes.
- Added opt-in authenticated author/session cards on Cloudflare (RAI-70), with
  `relay expose`, `relay authors`, and corresponding MCP actions. MCP
  `directory` continues to list device bundles.
- Added relay-bound delivery for registered external agents, including
  `grok-bot` and `openai-dot` (#97).
- Improved receiver resilience: failed capability-status writes no longer stop
  delivery (#109), and incomplete inbox rows or unreadable outgoing receipts
  are skipped rather than terminating the receiver (#110).
- Added hosted device names and advisory local renaming through
  `relay devices --names` and `relay rename NAME`.
- Added an owner-configured webhook nudge for matching `grok-bot` and
  `openai-dot` inbox messages. HTTP 200 indicates that the endpoint accepted a
  routine run; it does not establish message consumption.
- Validated native Claude ping requests remain durable when preflight or attach
  fails, and the local dashboard shows explicitly exposed peer cards with
  separate agent/session tags.

See [CHANGELOG.md](CHANGELOG.md) for the full list.

## Proof status

The maintainer waived the live proof gates for this release. Live same-org,
cross-org, and external relay-binding evidence is not recorded; this release
does not claim those flows were live-proven.
