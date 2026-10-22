# Cloudflare native login and OAuth contract

The Cloudflare backend and native Rust adapter are implemented in Everett 2.0.0.
The CLI supports native device approval and encrypted relay through outbound polling.
Hosted enrollment accepts only the configured GitHub owner. Public-issuer cross-device
delivery and live ChatGPT OAuth remain unverified.

## Development endpoint

The development issuer is `https://everett-dev.raitox-everett-dev.workers.dev`, an exact
origin without a trailing slash. The Worker is `everett-dev` in the Raitox tech account.
The recorded 2026-10-21 checkpoint reported HTTPS health 200 with GitHub credentials
unconfigured. That checkpoint does not establish current auth readiness.
Fresh interactive onboarding defaults to `https://everett.raitox.tech`, not this development
issuer. Use explicit issuer configuration to test a development deployment.

## Native enrollment

| Request | Response / behavior |
| --- | --- |
| POST `/v2/auth/device`, JSON `{name}` | 200 `{device_code,user_code,verification_uri,verification_uri_complete,expires_in:600,interval:5}`. Pending creates no membership. |
| GET `/enroll?code=<user_code>` | Owner browser page. Sign in through `/auth/github?return_to=<encoded local enroll URL>`. |
| POST `/v2/auth/poll`, JSON `{device_code}` | 200 `{state,interval:5}` for pending; denied/cancelled/consumed/expired are terminal. Polling sooner than five seconds returns 429 `slow_down`. Approved delivery may proceed immediately. |
| Approved poll | 200 `{state:"approved",token,agent_id,owner,expires_at}`; Unix milliseconds. Delivered once. Persist atomically with mode 0600 before clearing pending state. Never print the token or device_code. Lost delivery requires new enrollment; revoke an orphan credential in the owner page. |
| POST `/v2/auth/cancel`, JSON `{device_code}` | Cancels a pending request. |
| POST `/v2/relay/whoami`, JSON `{}`, Authorization bearer | 200 `{result:{owner,deviceId,role:"member"}}`. Browser sessions and OAuth access tokens cannot substitute for the native credential. |
| POST `/v2/relay/register` | Existing v2 device signature/key contract. Registration is required for normal relay operations. |
| POST `/v2/relay/revokeAgent`, JSON `{deviceId:own_id}` | Native self-revoke requires registered keys under existing relay operation rules. Owner browser revoke also handles never-registered credentials. |

Native credentials expire after 30 days. No native renewal endpoint exists. Non-2xx JSON errors are `{error:code}`. Invalid credentials 401; owner/CSRF denials 403; validation 400; body cap 413; rate limiting 429; unconfigured 503; provider failure 502. A transient failure must not erase an otherwise valid stored credential. All Everett commands must continue honoring EVERETT_HOME. Device IDs are immutable server-issued IDs; caller names are display labels.

Browser enrollment distinguishes waiting, completed approval, denied, cancelled, expired and revoked credentials. Close guidance appears only for an authoritatively approved/consumed request whose credential remains active. It confirms browser approval; CLI polling and private persistence complete in the terminal. Native credentials never appear in HTML.

Browser mutations use exact Origin, owner session cookie and hidden CSRF value. POST `/enroll/decision` form `{csrf,user_code,decision:"approve"|"deny"}`; `/enroll/revoke` form `{csrf,agent_id}`; `/auth/logout` form `{csrf}`. These return 303. Native credentials never grant browser owner authority.

## OAuth for the existing HTTP gateway

Discovery is `/.well-known/oauth-authorization-server`. Authorization code + S256 PKCE only; scope `everett`; public client auth `none`. CIMD client URLs, redirect URIs and resource URLs must be explicitly allowlisted and match exactly. DCR is unsupported and no registration endpoint is advertised.

- GET `/oauth/authorize`: client_id, redirect_uri, resource, response_type=code, scope=everett, state, code_challenge, code_challenge_method=S256. Browser initiation cookie binds consent to this request.
- POST `/oauth/consent`: form csrf, request_id, decision, name; owner consent redirects to the exact callback with code/state/iss, or access_denied.
- POST `/oauth/token`: form grant_type=authorization_code, code, client_id, redirect_uri, code_verifier; resource/scope if supplied must match. Refresh uses grant_type=refresh_token, refresh_token, client_id and optional matching resource/scope.
- POST `/oauth/introspect`: form token plus private introspection bearer secret. Active result contains active, iss, aud, exp (seconds), scope, sub, everett_agent, everett_agent_name, client_id. Compatible with existing Rust gateway OAuth introspection contract.
- POST `/oauth/revoke`: client-bound token revocation. Owner credential revocation also invalidates introspection and refresh immediately.

Codes expire in five minutes; access tokens in 15 minutes; refresh families have a fixed 30-day lifetime. Refresh rotation is mandatory. Code or refresh replay revokes the family. Tokens are hashes at rest. Native relay credentials and OAuth gateway tokens are separate.

## Configuration and rollout boundary

Required native auth names: EVERETT_ISSUER, AUTH_GITHUB_ID, AUTH_GITHUB_SECRET, EVERETT_OWNER_GITHUB_ID, EVERETT_OWNER_GITHUB_LOGIN. OAuth adds EVERETT_OAUTH_CLIENT_IDS, EVERETT_OAUTH_REDIRECT_URIS, EVERETT_OAUTH_RESOURCES, EVERETT_OAUTH_INTROSPECTION_SECRET (32+ chars).

Set the GitHub app callback to `<EVERETT_ISSUER>/auth/callback/github` for the selected deployment. Existing Convex app/deployment and existing gateway remain unchanged. Do not copy credentials into source, reports, chat or vault. Configure secret values privately through Cloudflare secret input.

The native HTTP adapter and outbound polling are implemented. A physical Mac/NUC
send/read/reply, reconnect, and revocation journey passed with synthetic GitHub and a
locally forwarded production Worker. The [2.0.0 release notes](https://github.com/raitoxlol/everett/releases/tag/v2.0.0) record successful hosted
first-run GitHub approval. See the [native relay guide](../docs/cloudflare-native.md).

Remaining work includes public-issuer cross-device delivery acceptance, live ChatGPT OAuth,
an explicit re-enrollment/migration plan, cross-owner grants, and deployed capacity
measurement. Hosted enrollment still accepts only the configured GitHub owner. Allowing
other people to create accounts requires a separate authorization and account-isolation design.

## Remote-device standards assessment

Current /v2/auth/device and /v2/auth/poll are an Everett-specific protocol, not RFC8628. They already use public HTTPS and outbound polling, so a separate browser can approve without localhost callbacks. [DEVICE-FLOW-PLAN.md](DEVICE-FLOW-PLAN.md) records the additive standard grant proposal, exact gaps and live acceptance gates. That proposal is not implemented or advertised; existing endpoints and JSON fields remain stable.
