# Everett Cloudflare backend

**Status: native GitHub approval and encrypted cross-device relay are deployed and verified on the public issuer.** The native Rust adapter supports outbound polling. On 2026-10-24, a real NUC sender woke a dedicated idle Claude session on a Mac through `https://everett.raitox.tech` and received its correlated reply. The Mac reused its existing Claude login; Everett enrollment and agent-provider authentication are separate. Public multi-account enrollment is enabled. Two real GitHub accounts passed separate device-list and namespace checks; foreign pairing/revocation were denied, same-account Mac/NUC messaging passed, and revoking one test credential left its sibling and the original account active. Live ChatGPT OAuth and deployed capacity remain unverified.

The Worker HTTP adapter uses one SQLite-backed `OwnerRelay` Durable Object per owner namespace. Membership, credentials, permanent revocation/replay records, device keys, counters and ciphertext queues share one authoritative transaction boundary. No D1 permission mirror is needed for this slice.

## Local commands

Node v24.16.0 was used. From the repository root:

```sh
npm --prefix cloudflare ci --legacy-peer-deps --no-audit --no-fund
npm --prefix cloudflare run typecheck
npm --prefix cloudflare test
npm --prefix cloudflare run build
npm --prefix cloudflare run build:public
npm --prefix cloudflare run dev
```

`build` is a **local Wrangler dry run**. `build:public` dry-runs the separate `wrangler.public.jsonc` profile against the deployed `everett` Worker configuration, including its account, custom domain, public variables and SQLite Durable Object migrations. `dev` uses local Workers storage. The default `wrangler.jsonc` remains local and has no account ID, public route, API credential or production binding. The public profile has no secret values; private GitHub and OAuth introspection secrets must be managed through Cloudflare's encrypted secret settings. Wrangler metrics are disabled. The legacy-peer flag works around an npm 10 optional-peer resolver crash observed during installation; dependencies are pinned in the lockfile.

`GET /health` reports configuration presence only. Hosted auth validates all required settings and fails closed with 503 when incomplete. There is no user-header bypass or insecure auth mode. Anonymous device requests cannot create membership. No deploy script targets production.

## Auth integration interface

The public Worker accepts only `POST /v2/relay/<operation>` with JSON arguments. Responses are `{result:...}` or non-2xx `{error:code}`. The native client uses authenticated HTTP requests and outbound polling.

Hosted auth uses the private `AUTH_STATE` Durable Object. An optional alternative is an operator-controlled private `AUTHENTICATOR` service binding. It receives `POST https://everett-auth.internal/verify-agent` with only the Authorization header forwarded. The host is a binding-routing label, not public DNS. Redirects are not followed. Success must return:

```ts
{ issuer: string, subject: string, org: string | null,
  agentId: string, role: 'admin' | 'member', expiresAt: number }
// expiresAt: Unix milliseconds
```

That service must verify cryptographic credential validity, exact issuer, audience/resource, expiry, scopes and agent binding. Owner browser sessions must not become agent credentials. Agent/org/role values must come from server-controlled records, never user-editable metadata. Missing or failed verification denies requests. Never log bearer values. Every operation also checks authoritative active membership and credential rows in the DO.

After verified owner consent, a trusted enrollment Worker selects `OWNERS.idFromName(namespace(identity))` and calls binding-only `stub.enroll(identity, role, serverGeneratedEventId)`. Identity is `{issuer,subject,org}`. The result is `{ok:true,value:{owner,deviceId}}` or `{ok:false,error,status}`. The DO generates immutable device IDs. Bind the returned ID into issued credentials. Event retries cannot rebind identity/role or reactivate a revoked credential. New agent approval requires a new event/ID; revoked memberships remain terminal. Role changes are not implemented.

The namespace binding is privileged; only trusted relay/enrollment Workers may hold it. No public caller can select a principal or invoke enrollment. `PrincipalVerifier` injection exists for synthetic tests; the production default export never supplies an override. Browser login, approval and OAuth issuance/refresh are implemented in HostedAuth; the native Rust HTTP polling transport is implemented.

## Port and parity map

| Existing surface | Local slice | Remaining difference |
| --- | --- | --- |
| `relay:whoami` | `POST /v2/relay/whoami` | Same identity result; requires issued active credential. |
| `register`, `device`, `devices` | Same operation names/args | Own-owner only; strict device signatures, immutable keys and authoritative exact lookup. |
| `credentials`, `memberships` | Same names/args | Lists bounded to 256; stored role plus verified claim for admin. |
| `rotate` | Same name/args | New self-signature plus old-key authorization over old/new fingerprints. |
| `revokeAgent`, `revokeMember` | Same names/args | Permanent tombstones; fresh checks on every access. |
| `enqueue` | Same name/args | **Same-owner only, grantId:null**; cross-owner denied. |
| `pending` | Same result fields | 32 inspected rows/page, numeric-string cursor; HTTP snapshot, no reactive subscription. |
| `accept`, `observe`, `status` | Same names/args | Signed observations, exact sender/recipient checks, monotonic states. |
| `expire` | Bounded DO alarm | 64 ciphertext rows per alarm, rescheduled; alarm and queue writes transact together. |
| `provisionIdentity` | Internal `enroll` RPC | Generates immutable ID instead of accepting issuer-selected ID. |
| `grants:*` | Not implemented | All external routes fail closed. |
| Browser auth, native enrollment, OAuth | Implemented in HostedAuth | Native GitHub approval verified; live ChatGPT OAuth remains unverified. See CLI-CONTRACT.md. |

Canonical v2 device/envelope/observation arrays and fingerprints follow `PROTOCOL.md`. Strict RFC8032 Ed25519 verification uses `zip215:false`. The fixture is copied unchanged from the existing **public test-only** vector; deterministic seeds are test material. Tests decrypt its ciphertext plus a synthetic queued X25519/HKDF-SHA256/ChaCha20Poly1305 round trip. Server stores ciphertext and cannot verify plaintext/AEAD tags; client validation remains mandatory.

New envelopes require current endpoint fingerprints and permissions, valid signatures, absolute clock skew <=5 minutes, positive TTL <=24 hours, expiry after server time and signed hop/observed-parent lineage <=3. Message IDs and sender/idempotency pairs have permanent replay rows. Exact retries run authorization/key checks first and consume no rate/queue budget; changed signed bytes conflict. Attempt rows deliberately omit ciphertext and large crypto fields.

New-message limits: 120/minute per credential, 120/minute shared by issuer/subject, 600/minute per owner; fixed UTC-minute windows. Recipient capacity is 256 unexpired ciphertext rows; body cap is 64 KiB plus the AEAD tag. Counters and queue writes roll back together. Reads do not consume messages. Signed delivery deletes ciphertext; signed claim/reply requires delivery and never regresses state. Empty blocked pages can still have `isDone:false`; clients must follow cursors. Expired ciphertext is excluded immediately by server time; deletion depends on scheduled alarm execution. Downloaded plaintext cannot be recalled.

## Remaining acceptance and capabilities

1. Extend live acceptance to hostile browser request replay. Runtime tests cover claimed-request hiding and foreign browser revocation with mocked provider responses; live acceptance covers real enrollment, device-list isolation, foreign native pairing/revocation denial and independent revocation.
2. Prove live ChatGPT OAuth with the intended gateway and client. Native device approval is a separate flow.
3. Implement bilateral cross-account grants if cross-account messaging is needed. Current public-mode accounts can communicate only among their own devices.
4. Review any namespace migration explicitly. Owner strings are signed into envelopes; there is no automatic credential or data import between issuers.
5. Measure deployed CPU/storage and account capacity. Current anonymous request and credential quotas are bounded; passing local tests does not establish public scale.

## Original relay checkpoint

2026-10-21: `npm run typecheck` passed; **17 Workers/workerd tests passed**, using real local SQLite/RPC/alarms/object eviction; Wrangler dry-run passed, 617.93 KiB / 103.22 KiB gzip at this checkpoint. Tests cover original v2 bytes/signatures/decryption, identity binding, denial paths, actual encrypted queue round trip, monotonic signed observations, replay, TTL, rotation, revocation, blocked-page progress, account quotas/rollback, 256-row capacity and persistence across eviction.

The initial test run exposed unsupported `redirect:'error'` and expected RPC throws reported as unhandled errors. These were corrected to `manual` with explicit status checks and structured enrollment denials. Final run has no unhandled errors. At that original relay checkpoint no hosted deployment, browser OAuth, native connection or physical-device delivery was tested. See the update below for current provider evidence.

Official references checked: [SQLite transactions and alarms](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/), [current Workers runtime test plugin](https://developers.cloudflare.com/workers/testing/vitest-integration/write-your-first-test/), [SQLite DO migrations](https://developers.cloudflare.com/durable-objects/get-started/), [DO pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/).

## Hosted auth verification update

2026-10-21: final typecheck and **37 Workers runtime checks passed** (18 relay, 19 hosted auth). Independent review found anonymous body reads could hold the issuer state gate; the body is now bounded before entering that gate, with a responsiveness regression check. The regression verifies agent validation and owner revocation complete within one second while an anonymous body remains open; timeout and 413 limits pass. Simultaneous code exchanges and refresh replay tests also pass. Tests are synthetic provider evidence, not live browser proof. Development Worker upload and public HTTPS health are verified; auth is deliberately unconfigured until private credentials are supplied.

Workers test filenames end in `.worker.ts` and are included explicitly by their package configs. Native browser approval now displays explicit waiting/approved/denied/cancelled/expired/revoked guidance; close-page success requires authoritative approval plus active credential. Runtime tests confirm no native token is rendered. Live provider/browser proof remains pending.

## Native CLI acceptance fixture

`test/native-fixture.mjs` runs the actual production Worker, HostedAuth and SQLite OwnerRelay locally. It mocks only outbound GitHub token/identity responses, including expiring-token fields. There is no injected principal verifier, special enrollment HTTP route or deployed authentication bypass. Every other outbound request is denied. This is synthetic account evidence; it cannot prove real GitHub, a hosted gateway, physical-device delivery or OpenAI wakeups.

From the repository root, start a disposable loopback issuer:

```sh
node cloudflare/test/native-fixture.mjs serve 18790
```

Use `http://127.0.0.1:18790` as the explicit Cloudflare issuer with a separate `EVERETT_HOME` for each native installation. A test browser-launch stub can call the following helper with the verification URL received from the actual CLI:

```sh
node cloudflare/test/native-fixture.mjs approve 'http://127.0.0.1:18790/enroll?code=CODE_FROM_CLI'
node cloudflare/test/native-fixture.mjs deny 'http://127.0.0.1:18790/enroll?code=CODE_FROM_CLI'
node cloudflare/test/native-fixture.mjs revoke 'http://127.0.0.1:18790/' 'agent-ID_FROM_CLI'
```

The helper performs the local browser-bound GitHub state/callback and CSRF-protected owner decision. It does not visit GitHub or display a credential. Any process on this machine can approve these synthetic requests, so keep the fixture on loopback and use isolated homes. Ctrl-C disposes the runtime and temporary state; never use the fixture issuer or its credentials for a hosted deployment. Miniflare is resolved from pinned Wrangler dependencies; no additional install is needed.

Latest local verification: typecheck and **38 Workers checks pass** (18 relay +20 hosted auth). The integrated hosted-auth test exercises two actual approvals, independently signed device registrations, encrypted send/read/reply, storage eviction/reload, signed delivery/reply observations and revocation through the production HTTP entrypoint. Local fixture smoke also verified two approvals, native identity, denial and revocation with the other agent remaining active. Native Rust execution against this fixture remains the CLI owner's acceptance step.

## Gateway OAuth acceptance fixture

`test/oauth-fixture.mjs` adds an exact synthetic CIMD document to the local GitHub fixture. It exercises real server metadata, owner session/state/CSRF, authorization code/S256 consent, token exchange, introspection, refresh and revocation. It does not inject a caller identity or change deployed source. Start it with a fresh private directory, a free issuer port and the exact loopback gateway resource:

```sh
node cloudflare/test/oauth-fixture.mjs serve /absolute/private-fixture-directory 18839 http://127.0.0.1:18840/mcp
```

The directory must be owned mode0700. `fixture.json` and `introspection.secret` are created mode0600; values are suppressed. In another terminal:

```sh
node cloudflare/test/oauth-fixture.mjs issue /absolute/private-fixture-directory
node cloudflare/test/oauth-fixture.mjs refresh /absolute/private-fixture-directory
node cloudflare/test/oauth-fixture.mjs check /absolute/private-fixture-directory
node cloudflare/test/oauth-fixture.mjs revoke /absolute/private-fixture-directory
```

`issue` saves `tokens.json` and `access.token` mode0600 after verifying resource-bound introspection. `refresh` rotates and updates those private files. `revoke` revokes the OAuth family; `revoke-agent` instead exercises owner credential revocation. Both verify inactive introspection and HTTP400 on refresh. A second `issue` in the same directory is refused before creating an agent. Use a fresh fixture directory/runtime for a separate journey.

For the actual gateway binary, configure the fixture issuer and exact resource, load the matching `introspection.secret` into `EVERETT_OAUTH_INTROSPECTION_SECRET` inside a local process, unset `EVERETT_GATEWAY_TOKEN`, and use an isolated `EVERETT_HOME`. Read `access.token` directly inside the local MCP client process when setting its Authorization header. Never print/cat any of these files into agent context or logs. The fixture itself does not run the gateway or claim its tool behavior.

Actual HTTP smoke passed exchange/introspection/refresh, token-family and owner revocation, revoked refresh refusal and duplicate-issue refusal while another fixture agent stayed active. Public provider acceptance, actual gateway tool calls, physical devices, quotas and OpenAI remain separate proof. Native fixture commands remain backward compatible. Ctrl-C stops the local runtime; its private files remain for inspection or explicit cleanup.
