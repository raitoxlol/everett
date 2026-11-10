# Everett Local

A local stdio plugin for the ChatGPT desktop app (Codex mode) and Codex.
Uses the existing Everett account without a ChatGPT hosted OAuth connector.
Requires Python 3.9+ and Everett 2.2.1+. Remote delivery still uses the configured
Everett relay. Plugin version 0.3.1 makes the ChatGPT desktop layout the default.

## Install

From this directory, run with the absolute path to your current Everett binary:

```sh
python3 install.py --binary /absolute/path/to/everett
codex plugin marketplace add "$HOME/.local/share/everett-direct-plugin" --json
codex plugin add everett-direct@everett-direct-local --json
```

The installer checks the runtime before modifying the package and generates
machine-specific paths under `~/.local/share/everett-direct-plugin`. No credentials
are included. `EVERETT_BIN`, if set, must be an absolute executable path and pass
the same version check. An invalid override fails instead of choosing a stale binary.
The generated plugin has `.codex-plugin/plugin.json` and `.mcp.json`. Do not add
root `plugin.json`/`mcp.json` for ChatGPT: when they are present, ChatGPT desktop
ignores the plugin's local server. `--portable` writes them for other
agent-plugins clients; reinstalling without it removes them.
This directory is installer source. Run the installer before loading the
generated package; source alone has no configured MCP launch command.

### Use it in the ChatGPT desktop app

1. Quit and reopen ChatGPT after every install or reinstall; it caches plugins.
2. Switch to **Codex** mode and start a new chat. Regular ChatGPT mode cannot
   run local plugins.
3. Ask: "Use only the everett_local MCP server. Call everett_whoami, then
   everett_ls with hours=1, and show the full tool names."
4. The calls must be named `mcp__everett_local__everett_whoami` and
   `mcp__everett_local__everett_ls`, and `session_id` must not be empty. If they
   are named `mcp__everett__…`, another Everett MCP server answered instead; if
   no `everett_local` tools exist, recheck step 1 and that no root `plugin.json`
   is in `~/.codex/plugins/cache/everett-direct-local/everett-direct/<version>/`.

Verified 2026-11-10 with ChatGPT desktop (Codex mode, GPT-6.1 Sol) and Everett 2.2.1.

## Check startup

```sh
python3 "$HOME/.local/share/everett-direct-plugin/plugins/everett-direct/scripts/launch.py" \
  --config "$HOME/.local/share/everett-direct-plugin/runtime.json" --doctor
```

Doctor checks the runtime, not account authorization or relay reachability.
A startup error goes to stderr and exits nonzero, leaving stdout for MCP.
The plugin never creates a shared fake session identity, starts polling, opens
a browser, downloads binaries, or modifies existing connectors. It adds no hooks.
If the client already loads another Everett MCP server, disable that duplicate
in that client's plugin settings after checking which runtime it launches.

## Verification and future distribution

```sh
python3 -m unittest discover -s tests -v
EVERETT_VERIFY_BINARY=/absolute/path/to/everett python3 -m unittest discover -s tests -v
python3 tests/smoke.py --manifest "$HOME/.local/share/everett-direct-plugin/plugins/everett-direct/.mcp.json"
```

The smoke check initializes the actual installed MCP command, lists tools and
calls identity and local listing. It sends no messages and reads no inbox bodies.
The runtime test uses a disposable home and synthetic session, installs the
portable package, and calls the actual binary's identity and listing tools.
It does not establish ChatGPT app loading. Public desktop directory submission
requires approved-partner access, plus clean-machine/client installation checks
and an explicit publishing decision. See the official
[desktop-only guide](https://developers.openai.com/plugins/deploy/desktop-only/).
ChatGPT web/mobile needs a supported remote MCP connection. See
[ChatGPT setup and verification](https://github.com/raitoxlol/everett/blob/main/docs/chatgpt.md)
in the repository.
