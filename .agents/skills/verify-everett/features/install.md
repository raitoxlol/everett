# Install and MCP discovery

## Sub-features

Console/module versions, pre-session harness detection, onboarding, doctor, initialize, tools/list, caller identity.

## How to get to it (user POV)

Install Everett, run `everett onboard --yes`, then `everett doctor`. A client launches `python -m everett mcp` over stdio.

## Driving it with the release helper

Pass the installed `--python` and `--cli` without `--source`. Proof requires matching versions, a fresh registration, successful launcher probe, ten tools listed, and ten tools called.

## Gotchas

The executable stub detects Claude without starting a model. No session store is present during onboarding. This does not exercise client-specific enable/trust settings. Only `install-mcp --repair --apply` refreshes stale registrations; backups preserve the original.
