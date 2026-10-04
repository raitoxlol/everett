# Routing and cards

## Sub-features

Session listing, local routing, caller exclusion, card updates.

## How to get to it (user POV)

`everett ls --json`, `everett route "Fix upload retries for storage client" --router local --json`, and the equivalent MCP tools.

## Driving it with the release helper

Two synthetic sessions have distinct topics. CLI and MCP must select `verify-worker`, while the caller is `verify-sender`. Cards must be written and listing must show both sessions.

## Gotchas

Local routing uses text overlap. No Jev credentials are used. Source proof requires `--source`; artifact proof must omit it.
