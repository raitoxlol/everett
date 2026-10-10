# Accounts and sign-in (cross-device identity)

Status: design for issue #4. Implemented surface is listed at the end; everything else is scope.

## Implemented (everett-rs)

- `everett login [--device NAME] [--open] [--force] [--json]` — RFC 8628 device flow:
  discovery via `{issuer}/.well-known/openid-configuration`, `authorization_pending` /
  `slow_down` (+5 s) / `access_denied` / `expired_token` handling, `expires_in` deadline,
  one `userinfo` call, atomic `~/.everett/auth.json` (0600). Idempotent unless `--force`.
- `everett whoami [--json]` — prints email/issuer/device; refreshes the access token when
  it is within 60 s of expiry and persists rotated refresh tokens; `invalid_grant` deletes
  the file and points at `everett login`.
- `everett logout [--json]` — RFC 7009 refresh-token revocation when the provider publishes
  `revocation_endpoint`, then deletes the file; still deletes when the provider is down.
- Config: `[auth] issuer` / `client_id` in `~/.everett/config.toml`, env
  `EVERETT_AUTH_ISSUER` / `EVERETT_AUTH_CLIENT_ID`; `AUTH_DEFAULT_*` constants are empty
  until the hosted tenant exists, so `login` exits 2 with configuration instructions.
- `everett doctor` prints `account: <email> via <issuer> (<device>)` or
  `account: not signed in (optional)`.
- Everything else in this document (device lists, `--browser`, transport of sessions or
  core) remains scope.

## What an Everett account is

An account is one identity shared by every device and every harness you use. Everett keeps the
rule from the README: agent sessions, inbox messages and shared-core memory stay on the machine
that produced them. The account layer carries **only** who you are and which devices are signed in.

Everett does not run its own user database. Identity comes from an OpenID Connect provider that
supports the OAuth 2.0 Device Authorization Grant (RFC 8628). The hosted default is an Everett
tenant on a managed provider; self-hosting means pointing the same CLI at any RFC 8628-capable
issuer (Auth0, Keycloak, Ory Hydra, Zitadel, Okta, …). The client code is provider-agnostic: it
reads `/.well-known/openid-configuration` and uses `device_authorization_endpoint`,
`token_endpoint`, `userinfo_endpoint` and `revocation_endpoint` from there.

## Sign-in flow (terminal, works over SSH)

```text
$ everett login
Open https://login.example.com/activate on any device and enter code  WDJB-MJHT
  (or open https://login.example.com/activate?user_code=WDJB-MJHT)
Waiting…
Signed in as alex@example.com on this device (mac-studio).
```

1. `POST device_authorization_endpoint` with `client_id`, `scope=openid email profile offline_access`.
2. Print `user_code` + `verification_uri` (and `verification_uri_complete` when present). Never
   auto-open a browser without `--open`; the terminal may be remote.
3. Poll `token_endpoint` with `grant_type=urn:ietf:params:oauth:grant-type:device_code` every
   `interval` seconds (default 5). Handle `authorization_pending` (keep polling), `slow_down`
   (interval += 5), `access_denied` / `expired_token` (stop, non-zero exit). Give up at
   `expires_in` (provider-supplied, typically 15 min).
4. Fetch `userinfo` once to get `sub`, `email`, `name`; store the token set.

`everett login` is idempotent: if a valid session exists it prints who you are and exits 0 unless
`--force`.

## Token storage on each device

File: `~/.everett/auth.json` (honours `EVERETT_HOME`), mode `0600`, written atomically
(tmp + rename), one JSON object:

```json
{
  "issuer": "https://login.example.com/",
  "client_id": "…",
  "sub": "auth0|abc123",
  "email": "alex@example.com",
  "name": "Alex",
  "device_name": "mac-studio",
  "access_token": "…",
  "refresh_token": "…",
  "expires_at": 1791600000,
  "scope": "openid email profile offline_access"
}
```

- `access_token` is short-lived; `refresh_token` is rotated by the provider on each refresh and
  the file is rewritten. A refresh failure with `invalid_grant` means the device was revoked: the
  file is removed and the user is told to run `everett login` again.
- `device_name` defaults to the hostname and can be set with `--device <name>`; it is sent to the
  provider as `device_name`-style metadata only when the provider accepts it, otherwise it is local.
- `auth.json` is never read by hooks, adapters, `ls`, `route`, `send`, inbox or MCP tools. Nothing
  in the current product requires being signed in; sign-in is the foundation for later
  cross-device features, not a gate on local use.
- Logs (`~/.everett/mcp.log`, hook output, `doctor`) print the email and issuer only, never a token.

## Revoking a lost device

- `everett logout` revokes the refresh token at `revocation_endpoint` (RFC 7009) if the provider
  publishes one, then deletes `auth.json`. If the network is down it still deletes the local file
  and says the remote revoke did not happen.
- A lost device is revoked from the provider's dashboard (Auth0: *User → Devices*, which lists
  refresh tokens by client/device). A device-list command in the CLI needs a provider management
  API, so it is scope for later, not part of this change.

## Configuration

`~/.everett/config.toml`:

```toml
[auth]
issuer = "https://login.example.com/"   # EVERETT_AUTH_ISSUER
client_id = "…"                         # EVERETT_AUTH_CLIENT_ID
```

Both default to the hosted Everett tenant baked into the binary (`AUTH_DEFAULT_ISSUER`,
`AUTH_DEFAULT_CLIENT_ID` in `everett-rs/src/auth.rs`). Until the hosted tenant exists those
defaults are empty and `everett login` explains how to configure an issuer.

## Commands

| Command | Behaviour | Exit |
|---|---|---|
| `everett login [--device NAME] [--open] [--force] [--json]` | Device-code sign-in, writes `auth.json` | 0 ok; 2 not configured; 5 timeout/denied; 6 provider error |
| `everett whoami [--json]` | Prints email, issuer, device name, token expiry; refreshes a stale access token first | 0 signed in; 1 not signed in |
| `everett logout [--json]` | Revokes the refresh token, deletes `auth.json` | 0 (also when nothing was stored) |

`everett doctor` gains one line: `account: alex@example.com via https://login.example.com/ (mac-studio)` or `account: not signed in (optional)`.

## Self-hosting

Any RFC 8628 issuer works. Minimal provider setup (what the hosted tenant also uses):

- Application type: *Native* / public client, **no client secret**.
- Grants: Device Code, Refresh Token (rotation on).
- Scopes: `openid email profile offline_access`.
- Point `issuer` + `client_id` at it; run `everett login`.

## Out of scope for this change

Transporting sessions, inbox messages or shared core between devices; a device list in the CLI;
a browser-callback login (`--browser`); a Python port of the client (the Rust binary is the
primary Everett since 1.4.0).

## Implementation notes (for everett-rs)

- New module `everett-rs/src/auth.rs`: `discover(issuer) -> Discovery`, `start_device_flow`,
  `poll_token`, `userinfo`, `refresh`, `revoke`, `load()/save()/clear()` for `auth.json`.
  HTTP via the existing `ureq` dependency; form-encoded bodies; 10 s per-request timeout.
- Everything network-facing takes the issuer from config so tests run against an in-process fake
  provider (`std::net::TcpListener` + hand-rolled HTTP, like the existing dashboard/gateway tests)
  that scripts `authorization_pending` → `slow_down` → success, plus `access_denied`,
  `expired_token`, refresh rotation and `invalid_grant` on refresh.
