# Everett for Hermes Agent

Native Hermes Agent plugin (`plugin.yaml` + Python) for [Everett](https://github.com/raitoxlol/everett).

It connects Hermes to your local Everett runtime with per-session caller identity across CLI, TUI, Desktop, and shared messaging gateways. The portable `plugin.json` + `mcp.json` pair is retained only for other hosts that support Agent Plugins v1 but cannot load Hermes's native plugin manifest; that compatibility path registers an MCP server, not the native Hermes features.

## What it adds

- **Per-session bound Everett tools** (`everett` toolset): discovers every tool from `everett mcp --harness hermes` and runs a bounded per-`(session_id, cwd)` stdio pool so shared gateways bind each conversation to its real Hermes session ID without fabricating identity.
- **Prompt-cache-safe shared core**: registers a bounded `everett` system prompt section frozen once per new session, injecting card guidance and Everett's compact shared core without mutating the cached prompt mid-conversation.
- **Automatic inbox pickup**: checks a session's Everett inbox with a zero-subprocess `stat` fast path, streams validated `O_NOFOLLOW` + `0600` files directly with per-record bounds, and surfaces only validated message-ID notices in `pre_llm_call`, `transform_tool_result` (preserving JSON object structure), and `pre_verify`. Call `everett_inbox` to read message contents; raw sender text never enters hook nudges.
- **Session lifecycle and fallback cards**: binds per-session liveness on `on_session_start` without clobbering sibling Hermes sessions in the same gateway process and keeps live wake active across idle gaps after `on_session_end`. On `/new`, `on_session_reset` tears down the old session and binds the replacement, carrying the route key when available; `on_session_delete` clears session tracking and MCP resources without writing a fallback card, and `on_session_finalize` releases liveness and closes cached MCP processes. An `<!-- everett:auto -->` fallback card is created only when no card exists.
- **Native `/everett` slash command**: `/everett status`, `/everett ls [hours]`, `/everett inbox`, `/everett card`, `/everett sent`, `/everett core`, and `/everett help` across CLI, TUI, Desktop, and gateway chats. With a valid bound session ID, `/everett card` and `/everett inbox` work from local session state without the `everett` executable; `/everett sent` requires it.
- **Opt-in live inbox wake**: when `live_wake: true` is enabled, a lightweight background watcher nudges active sessions via `ctx.inject_message` with validated local message IDs only (remote/relay traffic stays passive). Shared hosts need a session route key; the setting is off by default.
- **Host-isolation ready**: runs cleanly under both `plugins.isolation: in_process` and `plugins.isolation: host`.

## Requirements

- The native Hermes integration is Unreleased, after Everett v2.4.0; it is not included in v2.4.0.
- Everett 2.4+ installed with `everett` on the `PATH` visible to Hermes (or configured via `binary`).
- Run `everett login` once in the same local Everett profile to enable Everett MCP tools.
- Hermes Agent `>=0.20` with native plugin support.

## Install

From GitHub:

```sh
hermes plugins install raitoxlol/everett#plugins/everett-hermes --no-enable
hermes plugins enable everett-hermes
```

From a local Everett Git checkout:

```sh
hermes plugins install "file:///absolute/path/to/everett#plugins/everett-hermes" --no-enable
hermes plugins enable everett-hermes
```

Restart Hermes (or the Hermes gateway) after enabling. Remove it with `hermes plugins remove everett-hermes`.

If you previously added a manual `mcp_servers.everett` entry, remove it from the active profile's `config.yaml` so Hermes does not expose duplicate Everett tools. The default profile uses `~/.hermes/config.yaml`; `everett install-mcp --hermes` prints its manual fallback for that default path, so adapt it to the selected profile when using a named profile or custom `HERMES_HOME`. It does not edit configuration or support `--apply`. The manual entry has no session-ID override, so process detection can bind only one unambiguous interactive Hermes CLI session in the server's directory. An explicit `EVERETT_SESSION_ID` or `HERMES_SESSION_ID` binds that one session instead; it does not provide the native plugin's per-session gateway binding.

## Active Hermes profile and doctor

`everett doctor` inspects the selected profile's plugin files and `config.yaml`. It reports native plugin installation/enablement, recognizes an older portable package, and flags a manual `mcp_servers.everett` entry when it duplicates an enabled native plugin. It checks configuration state; it does not prove that a running Hermes client loaded the plugin or that a session call works.

Profile selection follows Hermes's home settings: a non-empty `HERMES_HOME` overrides the default root, with `~/` (or `~\` on Windows) expanded against OS `$HOME`; otherwise the root is `$HOME/.hermes` (not `EVERETT_HOME`). A valid non-`default` name in that root's `active_profile` selects `profiles/<name>` when that directory exists; otherwise the root/default profile is inspected. Edit settings and remove duplicate MCP entries in the selected profile's `config.yaml`.

## Portable and manual MCP alternatives

Use the portable `plugin.json`/`mcp.json` pair only with a non-Hermes host that accepts Agent
Plugins v1 metadata but not Hermes's native `plugin.yaml` manifest. It launches
`everett mcp --harness hermes` and provides MCP tools only; it does not register the Python plugin,
per-session native bridge, hooks, prompt context, skill, `/everett` command, automatic cards, or
live wake. The MCP server still needs `EVERETT_SESSION_ID`/`HERMES_SESSION_ID` in its environment
or a detectable Hermes process for session-bound calls; the portable manifest does not create
that identity for another host.

The manual `everett install-mcp --hermes` entry is a separate basic setup for Hermes itself, not
the portable Agent Plugins fallback. Use it only instead of the native plugin; the printed entry
does not add the native plugin's per-session bridge, hooks, or commands, and a static session ID
binds only that one session.

## Settings

Configure optional settings under `plugins.entries.everett-hermes.settings` in the active profile's `config.yaml` (the default profile uses `~/.hermes/config.yaml`):

```yaml
plugins:
  entries:
    everett-hermes:
      # Required by Hermes only if you want live_wake to trigger gateway chat turns:
      allow_gateway_injection: false
      settings:
        binary: ""              # Optional absolute path to `everett`
        inject_core: true       # Freeze Everett core + card note into new session prompts
        inbox_hooks: true       # Surface unread validated message IDs on turn/tool/verify hooks
        auto_cards: true        # Write fallback <!-- everett:auto --> cards on session end
        live_wake: false        # Opt-in background watcher using ctx.inject_message
        wake_poll_seconds: 5.0  # Poll interval for live_wake (1.0-60.0s)
        max_cached_clients: 16  # Max per-session MCP subprocesses kept warm (1-64)
```

## Security model

- **No fabricated identity**: native tool calls require a valid Hermes session ID and never fall back to process-based heuristics. Foreign harness env vars (`CLAUDE_SESSION_ID`, `CODEX_THREAD_ID`, etc.) are stripped from child processes.
- **Account gate preserved**: Everett's own `everett login` check stays enforced on every MCP call.
- **Untrusted inbox isolation**: inbox hooks and live wake traverse owned, non-group/world-writable ancestor directories and read only user-owned, private (`0600`) regular files via `O_NOFOLLOW` with per-record bounds, validate session and message IDs against strict ASCII patterns, exclude relay and named-peer records from live wake, and emit only constant-template notices containing IDs—never raw sender text.
- **No recursion**: when Everett spawns Hermes with `EVERETT_SEND` set, prompt injection, inbox hooks, auto-cards, and live wake stay silent.

## Verify

Validate the plugin with Hermes:

```sh
hermes plugins doctor plugins/everett-hermes --ci
hermes plugins validate plugins/everett-hermes
```

Run the unit and integration test suite:

```sh
python3 -m unittest discover -s plugins/everett-hermes/tests -v
```

Run the opt-in end-to-end smoke test against real `everett` and `hermes` executables (using isolated temp homes and loopback model/auth fixtures):

```sh
python3 plugins/everett-hermes/tests/smoke.py \
  --everett-binary /path/to/everett \
  --hermes-executable /path/to/hermes
```
