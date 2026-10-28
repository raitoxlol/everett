# Wake Claude from an Everett inbox

Load this plugin to check for unread Everett messages when Claude tries to stop.
Claude receives a fixed nudge containing validated message IDs and commands to read
and reply. The optional channel can also nudge an interactive session while idle.

This directory lives under `plugins/` because it is a distributable Claude Code
integration. It needs no changes to Everett's Rust binary, user settings, existing
hook installer, or MCP registration. Python's standard library matches the repo's
existing script-test tooling. There are no dependencies to install or build.

## Load the Stop hook for one launch

Requirements are macOS or Linux, Python 3.9 or newer, and `everett` on `PATH` with
`inbox --session --peek --json` support. The live check used Claude Code 2.1.312.

```sh
export EVERETT_CLAUDE_PLUGIN_DIR=/absolute/path/to/everett/plugins/everett-claude
claude --plugin-dir "$EVERETT_CLAUDE_PLUGIN_DIR"
```

`--plugin-dir` loads this plugin for that launch. It does not install anything
into `~/.claude`. It also works with `claude -p`. The manifest and
`hooks/hooks.json` use the standard plugin layout and `${CLAUDE_PLUGIN_ROOT}`.

The hook takes its session ID from the hook's JSON stdin, not an inherited session
environment variable. It securely opens that session's inbox and optional `.done`
file through directory descriptors with `O_NOFOLLOW`, validates the opened objects
with `fstat`, and copies at most 1 MiB per file into a private temporary home. It
calls `everett inbox --session <sid> --peek --json` against that snapshot, so Everett
still handles delivery markers and expiry. The plugin accepts only records whose
`to` equals the current session. Discovery never writes to the original inbox or
provenance state. The snapshot is removed after the peek.

The suggested inbox-read command pipes Everett's JSON into a constant Python
wrapper with `trust: "untrusted-agent-data"` and a warning. Message strings stay
JSON data inside that wrapper. Replies use the existing Everett MCP tool
`everett_send(reply_to=..., text=..., session_id=...)` after `everett_whoami`
confirms the hook session. This API resolves the original only in the host-bound
session's inbox and rejects conflicting session arguments. It requires a separately
configured `everett mcp` connection with the correct host identity. When that API
is unavailable or bound to another session, report replies unavailable. The plugin
does not suggest the CLI's globally searching `everett reply` command.

The Stop hook returns nothing when `stop_hook_active` is true. It also returns
nothing on missing tools, invalid input, unsafe file permissions, timeouts, or
other errors. It limits each nudge to five distinct message IDs. After one
continuation the Stop loop guard permits stopping; remaining unread messages
need another turn or an explicitly enabled channel.

The CLI subprocess has a 600 ms deadline and a 1 MiB output limit. The hook has
an 850 ms execution deadline after interpreter startup and a two-second host
timeout. Live measurements are recorded in packet F. A missing `python3` is
also silent because the hook command redirects errors and exits successfully.

Existing `everett install-hooks` registrations still provide session binding,
card updates, and prompt/tool-time inbox pickup. `everett install-mcp` still
registers `everett mcp`. This plugin adds Stop-time continuation and does not
replace either installer. Existing inbox hooks can consume a message before
the Stop check, in which case there is nothing left to nudge.

## Enable the optional channel for one launch

The channel is a research preview. It is off by default. `channel.mcp.json`
is deliberately not named `.mcp.json`, so loading the Stop plugin starts no
channel process. To test the channel, load its config explicitly and bind the
server and Claude to the same new session ID.

```sh
export EVERETT_CLAUDE_PLUGIN_DIR=/absolute/path/to/everett/plugins/everett-claude
export EVERETT_SESSION_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
export EVERETT_CLAUDE_CHANNEL=1
claude --session-id "$EVERETT_SESSION_ID" \
  --plugin-dir "$EVERETT_CLAUDE_PLUGIN_DIR" \
  --mcp-config "$EVERETT_CLAUDE_PLUGIN_DIR/channel.mcp.json" \
  --dangerously-load-development-channels server:everett-wake
```

Confirm the development-channel warning yourself on every launch. Merely loading
the MCP server does not enable channel delivery. This config uses the directly
loaded `server:everett-wake` form. No marketplace is required. The marketplace
`plugin:everett@<marketplace>` form has not been verified for this package.

Use this channel only interactively. Packet D observed the development flag
being ignored under `-p` and documents the same limitation for SDK sessions on
the tested runtime. **Documentation drift:** the public channel docs fetched on
2026-10-28 now describe noninteractive channels. That newer behavior was not
verified here and is not a supported promise of this plugin. The Stop hook is
the tested headless path.

Anthropic authentication is required. Provider and organization policy can block
channels; Team and Enterprise administrators must enable them. Custom channels
need the development flag unless the applicable allowlist admits the plugin.
The flag cannot be enabled inside an already running session.

The server polls `everett inbox --peek --json` at 500 ms intervals after MCP
initialization. It coalesces up to five new IDs into one notification per poll.
A set suppresses duplicate IDs for the process lifetime, including after a
message disappears and reappears. At 10,000 distinct IDs it stops notifying
until restarted. Restarting can nudge unread messages again. Nothing marks a
message read or acknowledges a notification. Stdout is nonblocking, with a 1 MiB
outgoing queue and a one-second drain deadline. Closing stdin ends the watcher
without waiting for stdout to drain. There is no daemon, socket, or network listener.

## Safety reference

This checklist corresponds to all nine items in packet D's safety section.

| Packet D item | Handling |
| --- | --- |
| 1. Reading exposes sender text | The nudge labels all inbox results untrusted. Its read command wraps `--json` output inside a JSON object with a constant trust label and warning. Message text, sender names, and cards remain data. This does not guarantee a model will resist instructions embedded in data. |
| 2. Channel content and meta become model context | Content uses only the fixed template, validated session ID, and Everett message IDs. Local IDs are `m` plus 1 to 127 ASCII alphanumeric characters. Relay IDs are `relay-` plus exactly 64 lowercase hex digits. Invalid IDs, including option-looking strings, are skipped without scrubbing. `meta` is always `{}`. No sender-derived fields enter either. |
| 3. MCP instructions become model context | `instructions` is a constant shared by the Stop hook and channel. |
| 4. Stop reason becomes model context | It uses the same fixed template and validated identifiers. No sender text, name, or card enters `reason`. Session IDs match Everett's safe ASCII alphabet, exclude `.` and `..`, and are quoted in commands. |
| 5. Permission relay can authorize tools | The server declares only `claude/channel`, exposes no tools, and implements no permission request or verdict path. |
| 6. Inbox writer shares local trust | Both entrypoints traverse `.everett` and `inbox` using pinned directory descriptors. Opened directories must be user-owned and private. Opened inbox and optional `.done` files must be user-owned regular files with mode `0600`. `O_NOFOLLOW` rejects symlinks at open time, and `fstat` checks the actual opened object. The CLI reads a bounded private copy rather than reopening the source paths. No TCP or Unix listener accepts arbitrary nudges. The OS user remains the trust boundary; these checks do not authenticate another process running as that user. |
| 7. Silent drop and no acknowledgement | The inbox remains the source of truth. Neither entrypoint consumes messages. A lost wake has no delivery guarantee; use an explicit inbox read or a subsequent Stop. |
| 8. Queued events batch into a turn | The nudge lists every ID in its bounded batch and gives a separate read/reply command for each. The channel remembers each notified ID. |
| 9. Permission prompts can stall wake work | The plugin changes no permission mode. Reads, the JSON wrapper, and replies follow normal permissions. The live acceptance run explicitly used bypass mode in an isolated test project, matching packet D. |

## Run the checks

The automated tests use Python `unittest`, as existing repository scripts do.
They create private fixture inboxes, invoke the actual scripts, and exercise the
channel through JSON-RPC stdin/stdout. Protocol/error tests use a fake CLI.
Regression tests also use the installed Everett binary for inbox and MCP reply
semantics. Those tests skip when Everett is missing; they never build it. No model
call is needed.

```sh
python3 -m unittest discover -s "$EVERETT_CLAUDE_PLUGIN_DIR/tests" -v
claude plugin validate "$EVERETT_CLAUDE_PLUGIN_DIR"
```

The opt-in live check calls a real Claude Haiku model with a $0.25 budget cap.
Give it a new evidence directory inside an isolated worktree. It seeds an
isolated session store, sends through the real Everett CLI, checks that the
loaded plugin blocks Stop, verifies Claude's wrapped inbox read, and checks
that no messages remain unread. It records the full command and transcript.

```sh
python3 "$EVERETT_CLAUDE_PLUGIN_DIR/tests/live_check.py" \
  /absolute/path/to/isolated-worktree/.packet-f-evidence
```

The live check sets a temporary `EVERETT_HOME`, excludes user settings and
external MCP servers, and disables Claude session persistence. It checks the
real `~/.everett` for files named for its unique test session and removes only
those if present. Evidence and isolated fixture stores stay in the chosen
directory for review. No plugin is installed in the real Claude configuration.

See the [plugin format](https://code.claude.com/docs/en/plugins),
[manifest reference](https://code.claude.com/docs/en/plugins/manifest-reference),
[channels](https://code.claude.com/docs/en/channels), and
[channel protocol](https://code.claude.com/docs/en/channels-reference).
