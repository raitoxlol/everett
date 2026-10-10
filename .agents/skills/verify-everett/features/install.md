# Install and MCP discovery

## Sub-features

Version, pre-session harness detection, onboarding, doctor, initialize, tools/list, caller identity.

## How to get to it (user POV)

Install Everett (`install.sh`, Homebrew or cargo), run `everett onboard --yes`, then `everett doctor`. A client launches `everett mcp` over stdio.

## Driving it with the release journey

Set `EVERETT_VERIFY_BINARY` to the packaged binary. Proof requires a version, a fresh registration, a successful launcher probe, ten tools listed, and ten tools called.

## Gotchas

The executable stub detects Claude without starting a model. No session store is present during onboarding. This does not exercise client-specific enable/trust settings. `install-mcp --repair --apply` refreshes stale registrations; launchers left by the retired Python package are replaced by `--apply`. Backups preserve the original.
