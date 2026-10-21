# Everett Cloudflare backend

**Status: isolated development backend deployed; live sign-in not configured.** Base PR #29 `bd70fbf`. Existing Convex, native transport, MCP tools and live OAuth are unchanged. See [CLI-CONTRACT.md](CLI-CONTRACT.md) for the implemented native enrollment and gateway OAuth contract. Development health: `https://everett-dev.raitox-everett-dev.workers.dev/health` returned HTTPS 200 on 2026-10-21.

The Worker HTTP adapter uses one SQLite-backed `OwnerRelay` Durable Object per owner namespace. Membership, credentials, permanent revocation/replay records, device keys, counters and ciphertext queues share one authoritative transaction boundary. No D1 permission mirror is needed for this slice.

## Local commands

Node v24.16.0 was used. From the repository root:

```sh
npm --prefix cloudflare ci --legacy-peer-deps --no-audit --no-fund
npm --prefix cloudflare run typecheck
npm --prefix cloudflare test
npm --prefix cloudflare run build
npm --prefix cloudflare run dev
```

`build` is a **local Wrangler dry run**. `dev` uses local Workers storage. The checked-in default has no account ID, public route, API credential or production binding. A separate development Worker was uploaded through the authenticated Cloudflare connector. Wrangler metrics are disabled. The legacy-peer flag works around an npm 10 optional-peer resolver crash observed during installation; dependencies are pinned in the lockfile.

`GET /health` reports configuration presence only. Hosted auth validates all required settings and fails closed with 503 when incomplete. There is no user-header bypass or insecure auth mode. Anonymous device requests cannot create membership. No deploy script targets production.

## Auth integration interface

The public Worker accepts only `POST /v2/relay/<operation>` with JSON arguments. Responses are `{result:...}` or non-2xx `{error:code}`. This is **not Convex's wire transport**.

Hosted auth uses the private `AUTH_STATE` Durable Object. An optional alternative is an operator-controlled private `AUTHENTICATOR` service binding. It receives `POST https://everett-auth.internal/verify-agent` with only the Authorization header forwarded. The host is a binding-routing label, not public DNS. Redirects are not followed. Success must return:

```ts
{ issuer: string, subject: string, org: string | null,
  agentId: string, role: 'admin' | 'member', expiresAt: number }
// expiresAt: Unix milliseconds
```

That service must verify cryptographic credential validity, exact issuer, audience/resource, expiry, scopes and agent binding. Owner browser sessions must not become agent credentials. Agent/org/role values must come from server-controlled records, never user-editable metadata. Missing or failed verification denies requests. Never log bearer values. Every operation also checks authoritative active membership and credential rows in the DO.

After verified owner consent, a trusted enrollment Worker selects `OWNERS.idFromName(namespace(identity))` and calls binding-only `stub.enroll(identity, role, serverGeneratedEventId)`. Identity is `{issuer,subject,org}`. The result is `{ok:true,value:{owner,deviceId}}` or `{ok:false,error,status}`. The DO generates immutable device IDs. Bind the returned ID into issued credentials. Event retries cannot rebind identity/role or reactivate a revoked credential. New agent approval requires a new event/ID; revoked memberships remain terminal. Role changes are not implemented.

The namespace binding is privileged; only trusted relay/enrollment Workers may hold it. No public caller can select a principal or invoke enrollment. `PrincipalVerifier` injection exists for synthetic tests; the production default export never supplies an override. Browser login, approval and OAuth issuance/refresh are implemented in HostedAuth; the native Rust transport remains separate work.

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
| Browser auth, native enrollment, OAuth | Implemented in HostedAuth | Real GitHub/ChatGPT acceptance remains; see CLI-CONTRACT.md. |
| Native Convex SDK/subscriptions | Unchanged | Rust HTTP/WebSocket adapter still needed; endpoint swap alone cannot work. |

Canonical v2 device/envelope/observation arrays and fingerprints follow `../convex/CONTRACT.md`. Strict RFC8032 Ed25519 verification uses `zip215:false`. The fixture is copied unchanged from the existing **public test-only** vector; deterministic seeds are test material. Tests decrypt its ciphertext plus a synthetic queued X25519/HKDF-SHA256/ChaCha20Poly1305 round trip. Server stores ciphertext and cannot verify plaintext/AEAD tags; client validation remains mandatory.

New envelopes require current endpoint fingerprints and permissions, valid signatures, absolute clock skew <=5 minutes, positive TTL <=24 hours, expiry after server time and signed hop/observed-parent lineage <=3. Message IDs and sender/idempotency pairs have permanent replay rows. Exact retries run authorization/key checks first and consume no rate/queue budget; changed signed bytes conflict. Attempt rows deliberately omit ciphertext and large crypto fields.

New-message limits: 120/minute per credential, 120/minute shared by issuer/subject, 600/minute per owner; fixed UTC-minute windows. Recipient capacity is 256 unexpired ciphertext rows; body cap is 64 KiB plus the AEAD tag. Counters and queue writes roll back together. Reads do not consume messages. Signed delivery deletes ciphertext; signed claim/reply requires delivery and never regresses state. Empty blocked pages can still have `isDone:false`; clients must follow cursors. Expired ciphertext is excluded immediately by server time; deletion depends on scheduled alarm execution. Downloaded plaintext cannot be recalled.

## Remaining work before cutover

1. Configure development secrets privately and prove the real GitHub owner approval and ChatGPT OAuth flow. Local runtime tests mock only outbound provider responses.
2. Add native HTTP/WebSocket transport. Prefer hibernatable WebSockets with payload-free invalidations and authorization rechecks; none exist here yet. Keep existing local MCP and direct SSH transport.
3. Port bilateral cross-owner grants, pinned bundles, reconfirmation, revocation and caps. Multiple owner DOs need an explicit consistency design; never trust unchecked permission mirrors.
4. Decide namespace and data migration. Owner strings are signed into bundles and envelope AAD. A new issuer changes them; migrate authoritatively or explicitly re-enroll/re-pair. Preserve permanent membership/credential/replay tombstones and pending state. This slice imports/deletes nothing.
5. Measure deployed CPU/storage limits and prove two physical machines send/read/reply, reconnect and revoke before replacing the service. SQLite DOs are Free-supported, but local correctness tests do not establish Free CPU fit or user capacity.

## Original relay checkpoint

2026-10-21: `npm run typecheck` passed; **17 Workers/workerd tests passed**, using real local SQLite/RPC/alarms/object eviction; Wrangler dry-run passed, 617.93 KiB / 103.22 KiB gzip at this checkpoint. Tests cover original v2 bytes/signatures/decryption, identity binding, denial paths, actual encrypted queue round trip, monotonic signed observations, replay, TTL, rotation, revocation, blocked-page progress, account quotas/rollback, 256-row capacity and persistence across eviction.

The initial test run exposed unsupported `redirect:'error'` and expected RPC throws reported as unhandled errors. These were corrected to `manual` with explicit status checks and structured enrollment denials. Final run has no unhandled errors. At that original relay checkpoint no hosted deployment, browser OAuth, native connection or physical-device delivery was tested. See the update below for current provider evidence.

Official references checked: [SQLite transactions and alarms](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/), [current Workers runtime test plugin](https://developers.cloudflare.com/workers/testing/vitest-integration/write-your-first-test/), [SQLite DO migrations](https://developers.cloudflare.com/durable-objects/get-started/), [DO pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/).

## Hosted auth verification update

2026-10-21: final typecheck and **37 Workers runtime checks passed** (18 relay, 19 hosted auth). Independent review found anonymous body reads could hold the issuer state gate; the body is now bounded before entering that gate, with a responsiveness regression check. The regression verifies agent validation and owner revocation complete within one second while an anonymous body remains open; timeout and 413 limits pass. Simultaneous code exchanges and refresh replay tests also pass. Tests are synthetic provider evidence, not live browser proof. Development Worker upload and public HTTPS health are verified; auth is deliberately unconfigured until private credentials are supplied.

The root Convex test runner uses Node; Workers test filenames end in `.worker.ts` and are included explicitly only by their package configs. Root typecheck and all46 Convex tests also pass after this isolation fix. Native browser approval now displays explicit waiting/approved/denied/cancelled/expired/revoked guidance; close-page success requires authoritative approval plus active credential. Runtime tests confirm no native token is rendered. Live provider/browser proof remains pending.
