# Private mesh console

A read-only Vercel-targeted console. Cloudflare remains the authority for consent,
account ownership, credential liveness, identities, queue counts and receipts.
The console is disabled until `ENABLED=true`; a read grant starts pending and
requires a separate owner decision in Everett account access. No deployment or
production account changes are part of this implementation.

## Initiate setup

Use Node 22 or newer. This package has no dependencies or install step.

```sh
node web-mesh/setup.mjs request \
  --credential-file "$HOME/.everett/cloudflare-auth.json" \
  --origin https://your-private-console.example \
  --output /absolute/private/directory/mesh-console.env
```

The CLI reads an owner-only native credential, authenticates to its issuer and
writes a new mode-0600 configuration without printing either bearer token. It
will not overwrite an existing file. An agent can invoke the same command; it
cannot approve its own request. The owner opens the printed approval URL, checks
the console origin, expiry and `mesh:read:v1` scope, then approves or denies.
Requests expire after 24 hours. Denied or revoked grants cannot be reapproved.
A revoked initiating native credential or membership also stops reads.

The generated file leaves `ENABLED=false`. After approval, set it to `true` in
this console's private server environment. Configure a private Vercel project
with root directory `web-mesh` and its checked-in `vercel.json`. No public env
prefixes are used. Deploying or changing Vercel/Cloudflare production settings
requires separate authorization. Vercel hosting has not been exercised by the
local checks; the adapter and Node handler have.

Each console instance serves exactly one configured owner. Configure:

| Server variable | Meaning |
| --- | --- |
| `ENABLED` | Must be exactly `true`. Defaults off. |
| `CONSOLE_ORIGIN` | Exact HTTPS console origin; loopback HTTP only for local work. |
| `CONSOLE_SECRET` | Generated 256-bit console login secret. Keep private. |
| `SESSION_KEY` | Independent generated 256-bit cookie signing key. |
| `MESH_ENDPOINT` | Exact Cloudflare issuer origin. |
| `MESH_TOKEN` | Narrow `emr_` read bearer, never a native/gateway credential. |
| `MESH_OWNER` | Pinned owner namespace from the setup response. |
| `MESH_MODE` | Optional `fixture` for explicitly labeled examples. Never automatic. |

Store the generated console access secret in the owner's password manager and
use it at the console's sign-in form. The browser receives only an opaque,
HttpOnly, Secure, SameSite=Strict, 15-minute signed session cookie. Source tokens
and the session signing key stay server-side. The console secret is a random
access secret, not a human-selected password. Rotate it and `SESSION_KEY`
together to invalidate all browser sessions. Sign out clears this browser's
cookie; an already copied cookie remains valid until its 15-minute expiry or
key rotation. Vercel deployment protection can provide an additional boundary,
but the server does not trust client-supplied identity headers.

## Revoke access

Open the issuer's `/enroll` account page, select the console in **Mesh read
grants**, and revoke it. Cloudflare checks the grant and native account liveness
on every snapshot. No source response is cached. An open visible console polls
every 15 seconds and clears all mesh data after a denied or failed read. Hidden
tabs clear their data immediately and reauthorize when visible. Data already
viewed cannot be recalled; no streaming or immediate browser push is claimed.

## Versioned boundary

`everett.mesh.v1` is independent of #103. The server adapter reconstructs an
allowlisted response, pins the owner and console origin, checks the version and
limits, and rejects unknown versions. It never forwards upstream JSON wholesale.
Browser input cannot select an owner, source URL, scope or relay operation.

- Device IDs are registered Everett identities. Last seen means the last
  successful server-observed relay request, not a connectivity guarantee.
- Session IDs/harnesses come from authenticated device exposure. Their exposure
  timestamp is server-observed; running processes are not independently attested.
  Session card text is excluded.
- Queue counts aggregate nonexpired queued attempts between the account's active
  devices. No message payload or attempt envelope is returned.
- The timeline records monotonic recipient observations only after the existing
  signature verifier accepts them. It records the signed device timestamp with
  the existing five-minute clock tolerance. Signed observations are evidence of
  what a device reported, not proof that a task completed. There are no invented
  historical receipts: recording begins with this Worker version. Up to 2,000
  observations are retained per owner and the newest 100 are shown.
- Self-reported status is a separate optional slot. The live projection reports
  it unavailable until a future status adapter is explicitly wired. The adapter
  supports only `idle`, `busy`, `blocked`, `offline`, a timestamp and the fixed
  confidence `self-reported`. It cannot relabel a status report as a signed
  receipt. Fixtures illustrate report age and unverified confidence.

Cloudflare isolates grants by verified GitHub account, then selects its immutable
owner namespace. The relay rechecks the stored credential/membership and filters
projection queries by subject. Grants contain a token hash, not the raw token;
there is no public HTTP path to the relay's internal projection RPC. Read tokens
are not accepted by the native relay verifier and cannot enqueue or revoke.
Consent mutations use the existing owner session, same-origin check and CSRF.

Receipt/last-seen metadata is recorded by the relay independently of whether a
console is enabled. This does not enable access; a pending/absent grant cannot
read it. No message body, transcript, session card, ciphertext, signature,
idempotency key, native credential or gateway credential enters the projection.

## Local checks

```sh
npm --prefix cloudflare run typecheck
npm --prefix cloudflare test
node scripts/mesh/score.mjs
npm --prefix web-mesh test
npm --prefix web-mesh run check
node web-mesh/test/local-e2e.mjs
```

The Worker suite covers default denial, explicit approval, CSRF, expiry, native
credential revocation, grant revocation using an existing reader, scoped-token
rejection by write APIs, two-account isolation, signed receipts, queue counts,
content exclusion and Durable Object eviction. Only GitHub's provider response
is synthetic. Worker storage, routing, enrollment, signature validation and
receipt persistence execute in the real local workerd runtime.

`local-e2e.mjs` runs the production Worker locally on 18869, invokes the setup CLI,
approves using a synthetic owner, enqueues an encrypted message, submits a real
signed recipient observation, then reads it through the actual Node console
handler on 18870. It revokes and checks that the same console session gets 403.
It never contacts production. Temporary native/read credentials are private and
removed on exit. `--hold` keeps this disposable local environment open for a
browser check. Its loopback-only control server on 18871 accepts `POST /revoke`
and `POST /stop`. Those control endpoints exist only in the test runner, not
in the application. Port conflicts fail instead of reusing another server.

For visual fixture work, set `ENABLED=true`, `MESH_MODE=fixture`, an exact
`CONSOLE_ORIGIN`, `CONSOLE_SECRET` and `SESSION_KEY`, then run
`npm --prefix web-mesh run dev`. No fixture is ever substituted for an upstream
failure. Both the API and the visible page label fixtures; they are not live
verification.

## Future controls and audit design

Write controls are deliberately absent from the browser API and Worker read
scope. A future change must add a new, separately approved capability; a read
grant cannot be upgraded in place. Proposed controls are pause routing, drain a
device queue and revoke a device. Each needs exact owner/device binding, bounded
expiry, fresh owner confirmation, idempotency, replay protection and enforcement
at the Cloudflare authority, including a final liveness check at execution.

Before enabling any control, add a durable append-only audit stream in the owner
namespace. Suggested `everett.mesh.audit.v1` fields are event ID, server timestamp,
verified actor ID, action, scope/grant ID, target device ID, decision, result,
request correlation ID and previous-event digest. Store no bodies or credentials.
Record approval, denial, revocation, attempted forbidden control and execution
outcome. Audit access needs its own retention/privacy policy and read scope.
The current receipt timeline is not an audit log, and current grant state is not
an immutable consent history. No control implementation or audit completeness
is claimed by this version.
