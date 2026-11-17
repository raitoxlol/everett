# MCP caller identity

Everett tools that act as "you" (`everett_card`, `everett_inbox`, `everett_event`,
`everett_subscribe`, replies and delivery through `everett_send`) need to know which session is
calling. Everett binds the caller from the host, never from tool arguments. Check the result with
`everett_whoami`: `session_id`, `harness`, `source`, and a `note` with the fix when unbound.

| Harness | How it binds | Setup |
| --- | --- | --- |
| Claude Code | `CLAUDE_CODE_SESSION_ID` in the MCP process env | `everett install-mcp --claude --apply` |
| Codex | per-call `_meta.threadId` (Unreleased) | `everett install-mcp --codex --apply` |
| Grok CLI 1.0.4+ | `GROK_SESSION_ID` | `everett install-mcp --grok --apply` |
| Pi | Everett's Pi extension registers the MCP server per session with `EVERETT_SESSION_ID` (Unreleased) | `everett install-hooks --pi --apply`; do not also list Everett in `~/.pi/agent/mcp.json` |
| OMP | `everett mcp --harness omp` + the OMP extension records the session the omp process holds (Unreleased) | `everett install-hooks --omp --apply` and `everett install-mcp --omp --repair --apply` |
| Devin CLI | `everett mcp --harness devin` + `SessionStart`/`UserPromptSubmit` hooks record the session (Unreleased) | `everett install-hooks --devin --apply` and `everett install-mcp --devin --repair --apply` |
| Hermes | `everett mcp --harness hermes`; binds only to the single interactive Hermes session created in the server's directory since the hermes process started (Unreleased) | add the entry printed by `everett install-mcp --hermes` to `~/.hermes/config.yaml`. Gateway mode cannot be bound |
| T3 Code | none | set `EVERETT_SESSION_ID` in the thread's MCP launch environment |
| dots / Grok Bot | `everett gateway` supplies an immutable binding | see [gateway.md](gateway.md) |

## How `--harness` binds

1. `EVERETT_SESSION_ID` or that harness's own variable, if set (ids inherited from an outer
   session of another harness are ignored).
2. The nearest harness process above the MCP server must be the named harness, otherwise the
   caller stays unbound.
3. The newest live record an Everett hook wrote for that process (session start, switch, resume,
   prompt). This follows `/new` and `/resume` in place.
4. Otherwise the one session of that harness created in the host's directory since it started,
   provided no other process of that harness in the same directory could own it.

Anything else (no host, two candidate sessions, a rival process) is unbound. Identity-requiring
tools then return a tool result with `isError: true` and a one-line fix, not a JSON-RPC error.

## `session_id` argument

Deprecated and removed from tool schemas. It is still accepted when it equals the bound session
(old clients keep working); a different value is a tool error, and it never establishes identity.


## Read-only identity diagnostics

Run `everett identity diagnose` for readable output, or add `--json` for status
fields. Native mode reads the existing credential and verifies it with the configured
issuer. It reports the account as verified separately from session ownership.
Historical native sessions remain `unmapped` because no supported ownership receipt
is available. Environment session IDs and nearby session files cannot establish
account ownership. Local session routing and existing grants remain unchanged.

To diagnose an existing logical chat registration, use:

```sh
everett identity diagnose --gateway-config /absolute/path/to/binding.json --credentials-stdin --json
```

Supply a JSON object containing only `access_token` and `identity_handle` through
stdin from the existing secure credential source, then close stdin. Input is bounded
to 4096 bytes. Do not put either value in command arguments or paste them into logs.
Run in the existing gateway environment containing `EVERETT_OAUTH_INTROSPECTION_SECRET`.
No new credentials, registrations or permissions are created by this command.

Gateway mode contacts the configured introspection endpoint, reuses its current
verification rules, and resolves the private handle in the verified account/client
scope. A successful result exposes the existing exact session ID with a stable label.
It omits account names, display names, bearer tokens, introspection secrets and private
handles. Unknown, foreign and revoked handles share `identity_unresolved`; missing or
conflicting registration metadata reports `registration_unavailable`. Missing account
provenance and fixed legacy bindings remain explicitly unmapped.

`availability` stays `unknown`. Dispatch, pickup and reply stay `not_observed`.
A mapped diagnostic is not a send grant or delivery receipt. The command does not
invoke the backend, register, send, wake, acknowledge messages, or write local state.
Auth verification can perform read-only network requests, like `everett whoami`.

Exit 0 means the diagnostic completed, including an explained unmapped result.
Exit 1 means verification/resolution failed or is unavailable. Exit 2 means malformed
input, unreadable configuration/credential, or missing verifier setup. Error reports
use fixed reason codes and do not echo input or remote error bodies.

CLI verification uses isolated profiles and loopback auth fixtures in
`gateway_native` (filter `identity_diagnostic`). Additional offline cases run with
`EVERETT_VERIFY_BINARY=/path/to/everett python3 scripts/test_identity_diagnostics.py`.
