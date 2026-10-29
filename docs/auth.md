# Accounts, agents, and relay authorization

Local indexing, routing, inboxes, hooks, MCP, and explicit SSH peers need no sign-in.
Cloudflare is the only hosted relay. See [two-device setup](cloudflare-native.md)
and the [relay threat model](relay-threat-model.md).

## Set up local agents without an account

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
installs the Pi extension, which registers the MCP server per session.
`everett install-hooks --devin --apply` adds Devin's session-binding hooks. For Hermes,
add the `mcp_servers` block printed by `everett install-mcp --hermes` to
`~/.hermes/config.yaml` yourself.
Restart the configured agents and run `everett ls`. An empty list means no sessions were
found in the look-back window. In an MCP client, use `everett_whoami` to check caller identity.
A successful registration does not prove that a running agent has loaded the tools.

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
automatically opens agent selection. No separate login command or issuer setup is needed.
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


## Private storage and revocation

Protect the local OS account: it holds the native bearer credential and unencrypted
device keys. Never paste credentials in chat, logs or commands. Local sign-out does
not revoke previously delivered plaintext or remote credentials. Revoke a device
through the owner page or `everett devices revoke DEVICE-ID`; use
`everett peer revoke SSH-DEVICE-ID` separately for SSH authorizations.
Cloudflare does not publish SSH addresses or propagate SSH revocations.
