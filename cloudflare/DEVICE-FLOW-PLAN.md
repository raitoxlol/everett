# Remote-device authorization: RFC 8628 assessment

Status: proposal, not implemented or advertised. Existing custom native login remains unchanged while the CLI adapter is being built. Hosted-client OAuth authorization code/PKCE remains a separate grant with separate token use. Source checked 2026-10-21 at Cloudflare review head5a375a6.

## Product boundary

A user explicitly starts sign-in on the NUC, SSH session or VM. That installation displays a public HTTPS verification URL and user code; the user approves it in a browser on another device. The terminal polls outward and privately saves its own newly issued credential. No inbound listener, localhost callback, copied browser cookie, shared machine token or copied credential is needed.

A public client ID identifies the CLI software. It does not identify or authenticate a particular installation. Each approval creates a separate immutable server-issued agent ID and independently revocable credential. A display name is an untrusted label. Device authorization does not prove hardware identity, VM persistence, harness compatibility or per-bot OS isolation.

## Verified current gaps

| Surface | Current custom protocol | Required standard path |
| --- | --- | --- |
| Start | JSON name at /v2/auth/device | Form-encoded public client_id and optional scope; no distributed client secret. |
| Binding | Request stores code/name/state | Bind client_id, granted scope, protocol kind and polling interval to the pending request. |
| Poll | JSON device_code at /v2/auth/poll | Token endpoint with the registered device-code grant URI, device_code and client_id. |
| Pending/errors | HTTP200 state; HTTP429 slow_down; fixed five seconds | Standard OAuth error responses; pending/slow_down continue, other errors stop; slow_down adds five seconds for every subsequent poll. |
| Success | token/state/agent_id/owner/expires_at | access_token, token_type=Bearer, expires_in and scope; agent/owner extensions may accompany these. |
| Discovery | Only code and refresh grants | Device authorization endpoint plus device-code grant metadata, only after implementation and verification. |
| Consent | Name, full user code, approve/deny | Also show exact limited requested permission and separate-install revocation consequences. |
| Generic OIDC client | Requires OpenID discovery, userinfo and openid/email/profile/offline_access | An OAuth device grant does not imply OIDC, userinfo or ID tokens; native provider adapter must consume OAuth metadata and appropriate scope. |

The current response already has device_code, user_code, verification_uri, verification_uri_complete, expiry and interval. HTTPS browser interaction and private per-agent revocation can be retained. Similar field names do not make the custom protocol compliant.

## Minimal additive contract for implementation

1. Add POST /oauth/device_authorization with UTF-8 form data: client_id=everett-cli, scope=everett:relay, optional bounded device_name extension. This proposed public client ID and scope are not live yet. A native-client allowlist is separate from the hosted CIMD URL allowlist. Empty parameters count as omitted, duplicate parameters fail, and unrecognized parameters are ignored as required by the device authorization request specification. Return the standard start fields with no secret displayed in the browser.
2. Add the device-code branch to POST /oauth/token: grant_type=urn:ietf:params:oauth:grant-type:device_code, client_id and device_code. Bind and validate all three against the stored request. Return HTTP400 authorization_pending, slow_down, access_denied or expired_token where appropriate; invalid/redeemed grants fail. Success returns the existing native relay credential under access_token, token_type=Bearer, expires_in, scope and documented everett_agent/everett_owner extensions. No refresh token in this minimal phase; renewal remains explicit re-enrollment. Cancellation can remain a documented Everett extension mapped to access_denied for subsequent standard polls.
3. Record a persistent per-request polling interval, increasing it by five seconds on slow_down. Clients wait before every poll, retain the increased interval, and use exponential backoff after a connection timeout. Terminal errors stop polling; a new request requires user intent. Never silently restart an expired request. Preserve an approved one-time token response atomically mode0600 on the originating installation before clearing its pending request. A lost successful response remains a documented re-enrollment/revoke-orphan case.
4. Make browser consent show the installation name, full code, limited account relay access and approval/revocation effects. Preserve browser owner session/CSRF/Origin checks and current authoritative completion states. Add protocol/client/scope fields through an explicit SQLite migration; existing requests default to legacy. Legacy custom poll must reject standard requests, preventing bypass of client binding or independent double redemption. Native grant handling must not depend on ChatGPT redirect/CIMD/introspection configuration; existing unconditional requireOAuth at /oauth/token must be split accordingly.
5. Advertise the device endpoint/grant only after runtime conformance tests pass and the CLI owner supports it. Additive rollout keeps the existing JSON endpoints stable until clients migrate. Native credentials remain accepted only by the relay verifier; gateway access/refresh families remain separate. No new issuer, no implicit device migration, no copied keys, and no reactivation of permanent tombstones.

The proposed permission is one fixed non-admin relay grant. Do not show selectable permissions that the server does not enforce. Fine-grained filesystem, shell or harness permission systems are outside this grant. The browser must not imply that approving a relay credential grants access to a user's files or executes a command.

## Acceptance evidence required

| Check | Current evidence |
| --- | --- |
| RFC protocol behavior | Missing implementation: client binding, form/grant/token response/errors/backoff/discovery, legacy-bypass and simultaneous redemption regression tests required. |
| Existing custom auth state machine | 37 Cloudflare Workers runtime checks passed before this assessment, with synthetic outbound providers. This is not RFC conformance or real account proof. |
| Remote browser approval | Not verified; new GitHub provider configuration and remote native adapter gate this. Approve a request actually started on NUC/SSH/VM from the Mac/phone browser. |
| Cross-device message and revoke | Not verified; remote native registration, encrypted send/read/reply and independent install revocation must pass with the Mac credential still active. |
| Grok Bot, Hark, Muse | Grok Bot command-line/shared-account capability does not prove per-bot isolation. A VM alone does not prove CLI install, durable storage or MCP support for Hark/Muse. Keep compatibility labels unverified until tested. |

Backend owner: cloudflare/** and provider configuration. CLI owner: native Rust transport/auth/UI and remote installation journey. Coordinator owns the unavoidable account interaction. No Rust, provider configuration or auth protocol behavior changed by this assessment.

## Primary sources

[RFC 8628 sections3.1–4](https://www.rfc-editor.org/rfc/rfc8628.html#section-3.1) define the wire format, polling and metadata; sections5–6 describe phishing/code/UX concerns. [RFC 6749 section5](https://www.rfc-editor.org/rfc/rfc6749.html#section-5) defines token responses/errors. [RFC 8414](https://www.rfc-editor.org/rfc/rfc8414.html) defines OAuth server metadata. Native apps with an appropriate local browser should also consider [RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html); device flow is particularly appropriate for headless or constrained remote installations.
