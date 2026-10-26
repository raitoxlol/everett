# Accounts, agents, and relay authorization

Local indexing, routing, inboxes, hooks, MCP, and SSH peers need no Everett sign-in.
Cloudflare is the active hosted transport. Convex is retired; the OIDC sections
below document retained compatibility code. That code needs an OIDC provider
**and deployment-side enrollment**;
`everett login` alone does not grant relay access. Fresh interactive onboarding uses
Everett's public native approval site; OIDC remains separately configured. See [deployment and journeys](cross-device.md)
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

The service at `https://everett.raitox.tech` accepts GitHub accounts into isolated relay
namespaces. Devices communicate only within their account. Local setup and SSH peers
remain account-free; [custom deployments](../cloudflare/CLI-CONTRACT.md#configuration-and-rollout-boundary)
can restrict enrollment to an owner.

Cloudflare enrollment uses its own native API and credential, separate from OIDC ID
and refresh tokens. On a new installation, run `everett` normally: choose sign-in to open
`https://everett.raitox.tech/enroll` with this device's approval code. Continue with GitHub,
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
everett login --provider cloudflare --issuer https://YOUR-EVERETT-ISSUER --device laptop --open
everett whoami --provider cloudflare --json
```

The browser compares the full code and device name and approves the request. The CLI
polls, atomically saves the one-time native credential, then asks the server to verify
its immutable agent ID and owner. A pending request creates no membership. `--device`
is a display label. Approval never signs into Claude, Codex or another agent's provider.

| Action | Command / result |
| --- | --- |
| Start without waiting | `everett login --provider cloudflare --issuer https://YOUR-EVERETT-ISSUER --no-wait` prints a public approval link/code. |
| Resume | Run `everett login --provider cloudflare` again; pending issuer and request are retained privately. |
| Cancel | Ctrl-C during waiting, or `everett login --provider cloudflare --cancel`. Approval racing cancellation is never claimed as cancelled. |
| Retry | Denied, expired and cancelled requests clear pending state; rerun login. `--force` requests a new approval. |
| Verify / sign out | `everett whoami --provider cloudflare` checks the server. `everett logout --provider cloudflare` removes the local credential only. |

For a configured installation, `[auth] provider = "cloudflare"` (environment
`EVERETT_AUTH_PROVIDER`) and `issuer` (`EVERETT_AUTH_ISSUER`) select this path. Omit the
provider setting for legacy explicit login commands, or use `provider = "oidc"`, to
retain existing OIDC behavior. Successful native onboarding saves `provider = "cloudflare"`.
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
is isolated under `.everett/relay-cloudflare`; existing OIDC/Convex keys and pins stay
under `.everett/relay`. Peers are pinned explicitly for each provider. See
[connect two computers through Cloudflare](cloudflare-native.md) for exact commands and
verification boundaries. Dots/Grok client OAuth has its own consent/grant;
Hark/Muse compatibility remains unverified.

## Existing OIDC sign-in

```sh
export EVERETT_AUTH_ISSUER='https://login.example.com/realms/everett'
export EVERETT_AUTH_CLIENT_ID='everett-relay'
everett login --device machine-a --force
# Approve the printed device code in a browser as this device's provisioned identity.
everett whoami --json
```

`--device` is a **local display name**, not an agent credential selector. It is not sent
as a trusted enrollment claim. `--open` explicitly opens the verification URL;
`--open=true` does the same. Omit it or use `--open=false` to only print the URL.
Discovery uses `{issuer}/.well-known/openid-configuration`;
RFC 8628 device authorization requests `openid email profile offline_access` without a
client secret. The CLI handles `authorization_pending`, `slow_down` (+5 seconds), denial
and expiration, then obtains user information. Login is idempotent unless `--force`.

| Command | Behavior |
| --- | --- |
| `everett login [--device NAME] [--open] [--force] [--json]` | Device-code login; writes private local credentials. |
| `everett whoami [--json]` | Local provider identity/subject/device name; refreshes a stale access token. Not a relay authorization check. |
| `everett logout [--json]` | Attempts RFC 7009 refresh-token revocation if supported, then deletes local credentials even if the provider is unavailable. Not backend agent revocation. |
| `everett relay whoami` | Server-verified owner namespace, agent/device ID and effective stored/JWT role. Requires operator provisioning, but not key registration. |
| `everett relay register` | Registers this already provisioned agent's self-signed public key bundle, not a new credential or org membership. |
| `everett relay devices` | Lists active registered agents in this namespace (maximum 256); alias: `everett relay credentials`. Not a provider enrollment/device-management API. |

Persist non-secret configuration in `~/.everett/config.toml`, or use the environment:

```toml
[auth]
issuer = "https://login.example.com/realms/everett" # EVERETT_AUTH_ISSUER
client_id = "everett-relay"                         # EVERETT_AUTH_CLIENT_ID
[relay]
url = "https://YOUR-DEPLOYMENT.convex.cloud"          # EVERETT_CONVEX_URL
```

Use the issuer's exact `iss` value, including its path/trailing slash. Provider defaults
are empty. A public/native client must support device-code login, refresh tokens and
signed **ID tokens at login and refresh**. An arbitrary OAuth access token is not a
substitute. Convex uses the configured issuer/discovery/JWKS to verify the ID token's
signature, issuer, audience and expiry. Rust performs local consistency checks and
requires a successful server `relay:whoami` before creating/reusing relay private keys.
See the official [Convex OIDC guide](https://docs.convex.dev/auth/advanced/custom-auth)
and [authentication in functions](https://docs.convex.dev/auth/functions-auth).

## Signed claims plus authoritative enrollment

Every agent/device needs its **own** JWT/refresh credential, with issuer-controlled,
non-client-editable top-level claims. Do not use user-writable profile metadata,
client-supplied org/role fields, a shared owner bearer token, or `--device` for authority.

| Claim | Contract |
| --- | --- |
| `everett_agent` | Required, stable, issuer-global unique credential ID matching `deviceId`; 1–160 characters from `[A-Za-z0-9_.-]`. Exactly one agent, never recycled after revocation. |
| `everett_org` | Optional, nonempty string up to 160 characters; authoritative organization membership. **Omit** for account fallback; do not send JSON null/empty string. |
| `everett_role` | `admin` for administration, **and** the active stored membership must independently have role `admin`. `member` cannot administer grants or other agents. |
| `iss`, `sub`, `aud`, `exp` | Standard issuer identity, public client audience and expiration; retain normal OIDC token fields. |

An org namespace is compact JSON `[issuer,"org",everett_org]`. Without the org claim,
it is `[issuer,"account",subject]`. The markers prevent account/org collisions.
Agents in the same org can chat after mutual local fingerprint pinning, even with
different subjects/harnesses/devices. Account fallback requires the **same issuer and
subject**, with different issuer-bound agent credentials on each device. A generic
single-user ID token reused on every machine is unsafe and unsupported.

An independently authorized deployment operator verifies the provider identity and
provisions its exact issuer, subject, org (or null), agent ID and role using internal
`relay:provisionIdentity`; exact commands are in [cross-device deployment](cross-device.md#provision-authoritative-identities).
No JWT-callable public provision/enroll API exists. Deployment keys never belong on
agent machines. Provisioning a role changes that subject's membership role across its
credentials in that namespace; it does not override the JWT role requirement.

**Every public backend function** checks a verified JWT, active authoritative membership
and the caller's active credential. Registered operations also check current bundles;
routing checks both endpoints and, across namespaces, the bilateral named-agent grant.
A relay admin JWT can manage its org but cannot register, read pending, send, or observe
as some other agent. Separate backend revocation is mandatory: merely disabling a user
or refresh token at the issuer does not invalidate an already issued JWT immediately.

## Practical Keycloak device-code setup

This recipe uses one **dedicated Keycloak user per agent/device** in one trusted realm.
It is intentionally simple: three distinct subjects can share org A/A/B while retaining
separate credentials. It is not a same-subject, per-login enrollment service. A production
issuer can instead bind different agents to one human subject via trusted server-side
session/enrollment logic; Everett does not implement that integration.

Use a supported Keycloak installation with persistent storage and HTTPS, reachable by
both machines **and** Convex. Do not expose `start-dev`, use localhost-only discovery
for cloud Convex, or give agents realm-administration credentials. Follow Keycloak's
[production guide](https://www.keycloak.org/server/configuration-production) and
[server administration guide](https://www.keycloak.org/docs/latest/server_admin/index.html).
In the Admin Console:

1. Create realm `everett`. Keep self-registration off for this controlled trial. Use the
   exact issuer from `https://login.example.com/realms/everett/.well-known/openid-configuration`.
   The discovery document must publish device authorization, token, userinfo and JWKS
   endpoints; their TLS certificates must be valid for the backend and client.
2. **Clients → Create client**: OpenID Connect, client ID `everett-relay`.
   **Client authentication OFF** (public/native), **OAuth 2.0 Device Authorization Grant ON**.
   Leave Standard/Implicit Flow, Direct Access Grants, service accounts and token exchange
   off for this trial. Everett uses neither client secrets nor browser callbacks/redirects.
   Keep ID-token signing RS256. Do not require DPoP: this CLI does not implement it.
3. Ensure `profile` and `email` client scopes are available, and the optional
   `offline_access` scope/role is granted to these users. Enable **Revoke Refresh Token**
   with a suitably restrictive reuse setting in realm token settings. Use short token
   lifetimes and protect each user's browser approval with MFA. Check that a refresh
   returns a fresh signed ID token with the same agent/org/role claims; stop rollout if not.
4. **Realm settings → User profile**: create single-valued managed attributes
   `everett_agent`, `everett_org`, `everett_role`, enabled always. For each attribute set
   **Who can edit: Admin only** and **Who can view: Admin only** (not User). Disable
   unmanaged attributes in realm settings. Audit account-console/API and federation write
   permissions too: no end user or agent may choose these values. Use no user-supplied
   group attributes for these claims.
5. **Clients → everett-relay → Client scopes → dedicated scope → Mappers →
   Configure a new mapper → User Attribute**: create one mapper for each attribute.
   Set User Attribute and Token Claim Name to the matching `everett_*` name, Claim JSON
   Type `String`, multivalued/aggregate off, **Add to ID token ON**. Put these mappers in
   the dedicated/default scope so they apply at device login and refresh. Do not use a
   client-wide hardcoded `everett_agent` mapper: that would identify every user as one agent.
6. **Users → Add user**: create `alice-a`, `alice-b`, `bob-c` with separate credentials,
   email/profile details and these admin-set attributes:

   | User | `everett_agent` | `everett_org` | `everett_role` |
   | --- | --- | --- | --- |
   | `alice-a` (machine A) | `machine-a` | `org-a` | `admin` |
   | `alice-b` (machine B) | `machine-b` | `org-a` | `member` |
   | `bob-c` (machine C) | `machine-c` | `org-b` | `admin` |

   `admin` here is an Everett org role, **not** a Keycloak realm-admin role. Assign the
   `offline_access` realm role if not already available. Record each user's immutable
   Keycloak **ID/UUID** from the Admin Console: this is `sub`, not the username/email.
   Provision matching backend rows before `relay register`. On each device, approve its
   device code as its own listed user; use a separate browser profile/sign out first to
   avoid accidentally approving as the previously logged-in identity.
7. Verify `everett whoami --json` has the expected subject (do not print/paste raw tokens),
   then `everett relay whoami` matches the expected namespace/device/role. Register on
   each machine and compare the printed public bundles/fingerprints out of band. A wrong
   subject or a user-editable claim is a configuration failure, not something to bypass.

Basic Keycloak user attributes yield one agent per subject. They cannot safely give two
machines different agents under one subject by changing `--device` or editing the local
credential file. For **account fallback**, use a provider's verified per-agent enrollment
mechanism that preserves one subject, omit `everett_org`, and provision `org:null` for
each distinct agent. Without that mechanism, use the org recipe above. No native
issuer-enrollment wizard is included.

## Private storage and revocation

SSH and relay keys remain independent, and existing SSH identities are preserved, not
migrated. `everett relay register` (and the listener) binds the device's SSH UUID from
`everett peer identity` (`~/.everett/peer/device_id`) and its SSH public key to the relay
device, signed by that SSH key; the binding is set once and unique per owner. Registry
revocation then prunes that SSH authorization even when the IDs differ. A device that has
not registered with an SSH identity is pruned only where its relay ID equals its SSH tag.
Doctor displays both local IDs and their match status while signed in.

`everett devices revoke DEVICE_ID` permanently revokes a same-owner credential as self
or as an active registered admin. It records the registered device's revocation timestamp
and removes matching local peer configuration and pins. Each remaining signed-in peer's
next `everett doctor` fetch prunes matching SSH authorizations. Offline peers retain SSH
access until that fetch; registry revocation is not immediate SSH enforcement.
`everett logout --revoke-device` attempts self-revocation before provider/local logout and
still logs out if the registry is unreachable. Ordinary `logout` does not revoke the device.

`~/.everett/auth.json` (or the home rooted by `EVERETT_HOME`) contains access, ID and
refresh tokens plus issuer/client ID/subject/profile/device-name/expiry/scope. It is
atomic, mode 0600; rotated refresh tokens are persisted. `invalid_grant` removes it
and requests a new login. An empty/expired ID token stops relay operation, never falls
back to opaque access tokens. Relay Ed25519 and X25519 private keys are independent
random keys, separate from SSH identity, stored in private files under
`~/.everett/relay/`. Do not copy that directory or `auth.json` between devices. Hooks/MCP
never return tokens, and deployment/admin keys must stay only in the operator secret store.

From a **different, active registered org admin** (or the affected agent itself):

```sh
everett relay revoke-agent machine-b   # alias: relay revoke-device machine-b
# Whole membership removal: use the exact provider subject/UUID, not username.
everett relay revoke-member B-SUBJECT-UUID
```

Then revoke that identity's provider sessions/refresh and offline tokens in Keycloak
(or the issuer's management console), remove peer pins where appropriate, and handle
OS/device compromise. `peer remove ALIAS` only removes local relay trust; `logout` only
handles local/provider tokens. Neither substitutes for backend revocation.

The backend retains **permanent** revoked-agent and revoked-membership tombstones;
a still-valid JWT cannot operate or re-register, and internal provisioning cannot revive
those identities. Query dependency invalidation denies already subscribed queries as
well; the listener reauthorizes staged ingress before exposing it. This is server-side
denial on revocation commit, not an instantaneous recall of plaintext already downloaded
or written locally. No real OIDC/WebSocket revocation journey is claimed as verified here.
A revoked agent needs a **new agent ID**; a revoked membership needs a **new issuer
subject**. Preserve tombstones across restore/migration. Lost signing keys require new
provisioning/pairing; normal rotation requires the old signing key and explicit re-pins
([rotation steps](cross-device.md#rotation-and-revocation)).

Transcript/session synchronization, shared-core synchronization, browser-callback login,
provider-management APIs and a hosted enrollment UI remain outside this relay.
