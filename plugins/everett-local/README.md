# Everett Local

A local stdio plugin for the ChatGPT desktop app (Codex mode) and Codex.
Uses the existing Everett account without a ChatGPT hosted OAuth connector.
Run `everett login` in the same Everett profile before calling tools. Native MCP
verifies the account online on every tool call, including `everett_whoami`.
Requires Python 3.9+ and Everett 2.2.1+. Remote delivery still uses the configured
Everett relay. Plugin version 0.3.2 makes the ChatGPT desktop layout the default
and adds `check.py`, a read-only install diagnostic.

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
   no `everett_local` tools exist, run `check.py` ([Check startup](#check-startup)),
   recheck step 1 and that no root `plugin.json` is in
   `~/.codex/plugins/cache/everett-direct-local/everett-direct/<version>/`.

Verified 2026-11-10 with ChatGPT desktop (Codex mode, GPT-6.1 Sol) and Everett 2.2.1.

## Check startup

```sh
python3 "$HOME/.local/share/everett-direct-plugin/plugins/everett-direct/check.py"
python3 "$HOME/.local/share/everett-direct-plugin/plugins/everett-direct/scripts/launch.py" \
  --config "$HOME/.local/share/everett-direct-plugin/runtime.json" --doctor
```

`check.py` verifies the installed layout without starting the MCP server: required
files, no portable root manifests, the `.codex-plugin` and `.mcp.json` entries,
the configured runtime (version check included), and marketplace registration.
Each failed check prints its remedy; it exits nonzero until every check passes.
It cannot prove the ChatGPT app loaded the server — only a real
`mcp__everett_local__*` call in a fresh Codex-mode chat does that.

Doctor checks the runtime, not account authorization or relay reachability.
After restarting the client, call `everett_ls`; a returned list, even empty,
verifies tool use. Discovery alone does not. If runtime configuration is invalid,
rerun `install.py --binary /absolute/path/to/everett` to repair it.
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

Run `everett login` in the local plugin's Everett profile before running the
manual smoke check. It initializes the installed MCP command, lists tools and
calls identity and local listing without sending messages or reading inbox bodies.
The automated runtime test installs the portable package in a disposable home
and verifies its tools with a synthetic session and a loopback account fixture.
It does not establish ChatGPT app loading. Public desktop directory submission
requires approved-partner access, plus clean-machine/client installation checks
and an explicit publishing decision. See the official
[desktop-only guide](https://developers.openai.com/plugins/deploy/desktop-only/).
ChatGPT web/mobile needs a supported remote MCP connection. See
[ChatGPT setup and verification](https://github.com/raitoxlol/everett/blob/main/docs/chatgpt.md)
in the repository.
