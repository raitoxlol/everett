# Accounts, agents, and relay authorization

Local CLI indexing, routing, inboxes, hooks, MCP registration, and explicit SSH peers need no sign-in.
All native stdio MCP tool calls require `everett login` in the same Everett profile
and verify the saved Cloudflare account online on each call.
Cloudflare is the only hosted relay. See [two-device setup](cloudflare-native.md)
and the [relay threat model](relay-threat-model.md).

## Register local agents without an account

Install a supported harness CLI, then run:

```sh
everett onboard --yes --no-backfill
everett doctor
```

This applies default hooks and MCP registrations for detected agents, with backups.
It skips browser approval and older session cards. To inspect registration examples, run
`everett install-hooks` and `everett install-mcp` without `--apply`. These commands do not
change files, but can show agents that onboarding has not detected.

Onboarding does not cover every listed harness. `everett install-hooks --pi --apply`
installs the Pi extension, which exposes Everett tools through Pi's `registerTool` API
and binds the MCP bridge to each session.
`everett install-hooks --devin --apply` adds Devin's session-binding hooks. For Hermes,
add the `mcp_servers` block printed by `everett install-mcp --hermes` to
`~/.hermes/config.yaml` yourself.
Restart the configured agents and run `everett ls`. An empty list means no sessions were
found in the look-back window. Before using MCP tools, run `everett login` in this profile.
Restart each MCP client and ask it to call `everett_ls`; a returned list, even empty,
verifies that the tool works. Then use `everett_whoami` to check caller identity.
A successful registration does not prove that a running agent has loaded the tools.
Doctor checks configuration and the launcher, not whether the client loaded tools.
If a tool reports that an account is required, run `everett login` and retry.

The wizard welcomes you with "Local setup needs no Everett account. Sign in to use
Everett MCP tools." Choose "Choose setup path", then "Choose how to connect".
Without a configured issuer, Enter or `l` continues locally; local registration does
not sign you in or authorize tool calls.

Initialization, `ping`, and `tools/list` remain available without account verification.
No native tool is exempt, including `everett_whoami` and relay `diagnose` through MCP.
Terminal `everett relay diagnose` stays offline. Hosted gateway requests use their
separate owner/grant authorization through the scoped worker; `EVERETT_GATEWAY_*`
variables do not bypass verification for ordinary native `tools/call` requests.

## Hosted Cloudflare native sign-in

The hosted relay at `https://relay.raitox.tech` accepts GitHub accounts into isolated relay
namespaces. Account identity stays `https://everett.raitox.tech` so existing owner keys,
tokens and queued messages keep working if the network hostname changes. Devices communicate
only within their account. Local setup and SSH peers remain account-free;
[custom deployments](../cloudflare/CLI-CONTRACT.md#configuration-and-rollout-boundary)
can restrict enrollment to an owner.

Cloudflare enrollment uses its own native API and credential, separate from cloud-agent OAuth grants. On a new installation, run `everett` normally: choose sign-in to open
`https://relay.raitox.tech/enroll` with this device's approval code. Continue with GitHub,
compare and approve the named device, and return to the terminal; verified approval
automatically opens agent selection. Successful wizard sign-in needs no additional login;
if you chose local setup or skipped sign-in with `--yes`, run `everett login` before using MCP tools.
Existing explicit account configuration is preserved, and authenticated normal launches
skip automatic onboarding. Use `everett onboard` to repeat setup. Cancelled approval can
be retried or followed by explicit local setup; headless bare runs print help.

The [2.1.0 release notes](https://github.com/raitoxlol/everett/releases/tag/v2.1.0)
record real public delivery and second-account isolation/revocation acceptance.
Production uses `EVERETT_ENROLLMENT=public`; self-hosted deployments can retain `owner`
(see [the rollout contract](../cloudflare/CLI-CONTRACT.md#configuration-and-rollout-boundary)).
For a custom deployment or an explicit repair:

```sh
everett login --provider cloudflare --issuer https://YOUR-EVERETT-ISSUER --base-url https://YOUR-RELAY --device laptop --open
everett whoami --provider cloudflare --json
```

The browser compares the full code and device name and approves the request. The CLI
polls, atomically saves the one-time native credential, then asks the server to verify
its immutable agent ID and owner. A pending request creates no membership. `--device`
is a display label. Approval never signs into Claude, Codex or another agent's provider.

| Action | Command / result |
| --- | --- |
| Start without waiting | `everett login --provider cloudflare --issuer https://YOUR-EVERETT-ISSUER --base-url https://YOUR-RELAY --no-wait` prints a public approval link/code. |
| Resume | Run `everett login --provider cloudflare` again; pending issuer and request are retained privately. |
| Cancel | Ctrl-C during waiting, or `everett login --provider cloudflare --cancel`. Approval racing cancellation is never claimed as cancelled. |
| Retry | Denied, expired and cancelled requests clear pending state; rerun login. `--force` requests a new approval. |
| Verify / sign out | `everett whoami --provider cloudflare` checks the server. `everett logout --provider cloudflare` removes the local credential only. |

For a configured installation, `[auth] provider = "cloudflare"` (environment
`EVERETT_AUTH_PROVIDER`) and `issuer` (`EVERETT_AUTH_ISSUER`) select identity.
`base_url` (`EVERETT_AUTH_BASE_URL`) selects the relay origin; if it is unset, a
configured `issuer` is still used as the connect URL. Fresh installs default to
`https://relay.raitox.tech` for connect and `https://everett.raitox.tech` for identity.
GitHub sign-in always starts and finishes on the identity issuer, including when
the browser opened the relay host, so the GitHub OAuth app keeps that single callback.
Peer pins accept every origin in `relay_aliases` (`EVERETT_AUTH_RELAY_ALIASES`) as the
same relay. When the setting is unset, the public issuer and `https://relay.raitox.tech`
are that list. An explicit list replaces it. Origins outside the list still have to match exactly.
Cloudflare is the default and only supported provider.
Successful native onboarding saves `provider = "cloudflare"`.
`cloudflare-auth.json` and `cloudflare-pending.json` live beneath the `.everett` directory
rooted by `EVERETT_HOME`/`HOME`, written atomically with mode 0600 in a private directory.
Neither native tokens nor device polling secrets appear in command output. Failed
verification, transient HTTP errors and failed revocation preserve saved credentials.
A lost one-time delivery needs a new enrollment; revoke its orphan on the owner page.
Native credentials expire after 30 days; there is no refresh endpoint.

`logout --provider cloudflare --revoke-device` requests server revocation and removes
the local credential only after confirmation. This requires registered relay keys;
revoke a never-registered credential through the owner approval page. Local sign-out
and remote revocation are distinct. The selected Cloudflare provider now supports
encrypted native relay operations and outbound inbox polling. Its private relay state
lives under `.everett/relay-cloudflare`. Retired keys and pins are never loaded. See
[connect two computers through Cloudflare](cloudflare-native.md) for exact commands and
verification boundaries. Dots/Grok client OAuth has its own consent/grant;
Hark/Muse compatibility remains unverified.


## Native MCP sign-in

Run `everett login` once with the same Everett profile (`EVERETT_HOME` or `HOME`)
used by your harnesses. They share the saved credential. Tool calls verify it
with the configured relay each time, including in already-running `everett mcp`
processes. Initialization and tool discovery work while signed out. Tool execution
requires a current credential and a successful online check. Signing out locally,
expiry, or server revocation takes effect on the next tool call.

The harness provides the conversation binding for its inbox. The account and device
reported by `everett_whoami` describe the sign-in, and each conversation retains its
own session ID. Hosted gateway clients authenticate through the gateway's configured
transport. See [gateway authentication](gateway.md).


## Private storage and revocation

Protect the local OS account: it holds the native bearer credential and unencrypted
device keys. Never paste credentials in chat, logs or commands. Local sign-out does
not revoke previously delivered plaintext or remote credentials. Revoke a device
through the owner page or `everett devices revoke DEVICE-ID`; use
`everett peer revoke SSH-DEVICE-ID` separately for SSH authorizations.
Cloudflare does not publish SSH addresses or propagate SSH revocations.
