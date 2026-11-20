# Install and MCP discovery

## Sub-features

Version, pre-session harness detection, onboarding, doctor, initialize, tools/list, caller identity.

## How to get to it (user POV)

Install Everett (`install.sh`, Homebrew or cargo), run `everett onboard --yes`, then `everett doctor`. A client launches `everett mcp` over stdio.

## Driving it with the release journey

Set `EVERETT_VERIFY_BINARY` to the packaged binary. Proof requires a version, a fresh registration, a successful launcher probe, every advertised tool listed and called.

## Gotchas

The executable stub detects Claude without starting a model. No session store is present during onboarding. This does not exercise client-specific enable/trust settings. `install-mcp --repair --apply` refreshes stale registrations; launchers left by the retired Python package are replaced by `--apply`. Backups preserve the original.

## Local client verification

These checks used disposable Linux profiles. Successful tool calls used a loopback Everett account
verifier. Where a model was needed, a loopback model selected the tool.

| Client tested | Observed in the client | Skill evidence |
| --- | --- | --- |
| Claude Code 2.1.329 | MCP health check connected. A print session called `everett_whoami` and returned its Claude session ID. | Read a fixture skill through `everett_skill`. |
| Codex 0.171.0 | App-server called `everett_whoami` with its thread ID and `everett_ls` returned an empty list. | App-server called `everett_skill` and listed installed Codex skills. |
| OMP 19.0.1 | Its MCP routes included Everett. Calling `mcp__everett_whoami` through OMP's eval tool returned the live hook binding. | Read a fixture skill through `everett_skill`. |
| Pi 1.5.2 | Codemode called `mcp__everett__everett_whoami` and returned the extension-bound session ID. The older 0.73.1 SDK also executed the registered tool. | Read a fixture skill through `everett_skill`. |
| Hermes Agent 0.23.0 | Plugin manager installed the GitHub subdirectory. Chat exercised search, describe and call. Signed-out access returned login guidance. With fixture credentials the session list included a real oneshot database row. The oneshot caller was unbound. | Hermes opened `everett-hermes:everett-hermes` through its skill registry. |
| Grok CLI 1.0.89 | MCP doctor completed the handshake and found 18 tools. Headless execution required Grok sign-in. | Grok skill lookup covered by fixture. |
| Devin CLI 3000.14.30821 | MCP list showed Everett with `--harness devin`. Headless execution required Devin login. Host binding is covered by fixtures. | Devin skill lookup covered by fixture. |

T3 Code annotates sessions driven by local harnesses. OpenAI dots, Grok Bot and Devin Bot are
hosted clients with separate gateway or relay setup. This audit covers the seven local clients.

Run `python3 plugins/everett-hermes/tests/smoke.py --upstream --everett-binary /path/to/everett --hermes-executable /path/to/hermes` to repeat the GitHub install and first tool calls. Omit `--upstream` to exercise the local checkout.
