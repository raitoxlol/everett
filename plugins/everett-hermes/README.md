# Everett for Hermes Agent

This Hermes-supported portable Agent Plugins v1 package connects Hermes to Everett's existing
local stdio MCP server and bundles a short skill. It uses the `plugin.json` and `mcp.json`
format; it does not install another Everett runtime.

## Requirements

- Everett is already installed, and `everett` is on the `PATH` visible to the Hermes process.
- Run `everett login` in the same local Everett profile before using the tools. The existing
  account check remains in force.
- This package has no ChatGPT or separate ChatGPT-plugin dependency.
- Use a Hermes Agent release with Agent Plugins v1 support. The package's plugin-manager
  installation, MCP discovery, and tool-call path were exercised against Hermes Agent commit
  `fe8b920cae09`.

## Install

For a local Everett Git checkout, use its absolute `file://` URL and the plugin subdirectory:

```sh
hermes plugins install "file:///absolute/path/to/everett#plugins/everett-hermes" --no-enable
hermes plugins enable everett-hermes
```

The local `file://` source must be a Git checkout containing committed package files. Repository-ref
installs use repository content fetched by Hermes; make sure the package files are included.
To install from the repository:

```sh
hermes plugins install raitoxlol/everett#plugins/everett-hermes --no-enable
hermes plugins enable everett-hermes
```

Start a new Hermes session (or restart the Hermes gateway) after enabling. Remove the package
with `hermes plugins remove everett-hermes`.

## Tools and identity

The MCP server is named `everett_hermes` and exposes Everett's MCP tool set. For a read-only check,
use `everett_ls` and `everett_whoami`; Hermes exposes them as
`mcp__everett_hermes__everett_ls` and `mcp__everett_hermes__everett_whoami` through its
`tool_search` / `tool_describe` / `tool_call` flow. `everett_ls` is an overview, not a way to
select a handoff target.

The plugin deliberately passes no session identity. Everett binds only identity it detects from
the actual host; a shared or gateway-managed Hermes process may not identify one unique caller, in
which case `everett_whoami` reports that limitation rather than fabricating a session ID.

Hermes `config.yaml` MCP entries with the same server name override the package's entry. This
package uses `everett_hermes` to avoid replacing an existing `everett` registration. If both
servers are configured under different names, both connect and expose duplicate Everett tools;
remove the old manual registration if you want only this plugin.

## Runtime smoke

The opt-in `tests/smoke.py` exercises the Hermes client with loopback-only model and account
fixtures. It runs `hermes pm install` in the isolated profile and requires a matching Hermes
`oneshot` session read from `state.db` in read-only mode.

```sh
source /path/to/hermes-agent/activate
python3 plugins/everett-hermes/tests/smoke.py --everett-binary /path/to/everett --hermes-executable /path/to/hermes
```
