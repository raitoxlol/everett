# Routing and cards

## Sub-features

Session listing, local routing, caller exclusion, card updates, older targets and exact titles.

## How to get to it (user POV)

`everett ls --json`, `everett route "Fix upload retries for storage client" --router local --json`, and the equivalent MCP tools.

## Driving it with the release helper

Two synthetic sessions have distinct topics. CLI and MCP must select `verify-worker`, while the caller is `verify-sender`. Cards must be written and listing must show both sessions.

An additional Codex transcript is 96 hours old. The default window excludes it; MCP routing with `hours=168` selects it, and direct sending resolves its exact plain title. Duplicate titles are rejected by the unit suite.

## Gotchas

Local routing uses text overlap. No Jev credentials are used. Source proof requires `--source`; artifact proof must omit it.
