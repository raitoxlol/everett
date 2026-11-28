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
hosted clients with separate gateway or relay setup. This audit covers seven of the eight local clients; Muse Code is not in it yet.

Run `python3 plugins/everett-hermes/tests/smoke.py --upstream --everett-binary /path/to/everett --hermes-executable /path/to/hermes` to repeat the GitHub install and first tool calls. Omit `--upstream` to exercise the local checkout.

### Local inbox handoffs

The target in each exchange was a full session ID. The target consumed the request, replied
to its message ID, and the original sender checked the reply and pickup status. The
following results cover queue delivery with explicit pickup. They do not establish that
a dormant session can be woken or that a production model will independently choose the tools.

| Client | Discovery and caller | Skill read | Exact-target send, pickup and correlated reply/status |
| --- | --- | --- | --- |
| Claude Code | Native MCP registration and Claude caller ID | Native MCP | Both directions with a Codex thread. Claude print used a scripted loopback model and native tool grants. |
| Codex | Native app-server listed two persisted threads and identified each from its own request metadata | Native MCP | Thread-to-thread over app-server, including filtered reply pickup and sender status. No model required. |
| OMP | Native MCP routes and session-start hook binding | Native eval tool | Both directions with a Codex thread through OMP eval and a scripted loopback model. |
| Pi | Native extension and session-bound bridge | Native codemode tool | Both directions with a Codex thread through Pi codemode and a scripted loopback model. |
| Hermes Agent | Native plugin discovery and real oneshot row. Shared gateway MCP has no per-call session ID. | Native Hermes skill registry | Both directions with a real Codex thread using a native Hermes oneshot terminal and a scripted loopback model. The CLI read Hermes's per-turn session ID. See the bundled skill for the commands. |
| Grok CLI | Native MCP doctor. Caller identity tested with a fixture Grok session variable. | Fixture Grok skill | Both directions through stdio MCP and a fixture Devin peer. Native model turn requires Grok sign-in. |
| Devin CLI | Native MCP list. Onboarding installs its caller-binding hooks. Hook binding and store discovery tested with fixtures. | Fixture Devin skill | Both directions through stdio MCP and a fixture Grok peer. Native model turn requires Devin sign-in. |

To repeat the native exchanges, use two disposable sessions in one isolated home.
Materialize a Codex thread with a user message before listing it. In each client, read
the test skill, check the detected caller, and copy the peer's full ID from the list.
On the Hermes side, follow the terminal commands in the bundled `everett-hermes` skill.
For the MCP clients, send with `everett_send(to, text, mode="inbox")`, pick up with `everett_inbox` in
the target, then call `everett_send(reply_to, text)` there. On the sender, filter
`everett_inbox` by `reply_to` and call `everett_status(message_id)`. Check the
reply author, original ID, `picked_up: true` and `state: replied`. Reverse the roles.
The credential-independent Grok/Devin regression runs with
`cargo test --locked --test release_journey grok_and_devin_fixture_handoff` in `everett-rs`.
