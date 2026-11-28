# Approved T3 launch and verification

An agent can start a new T3 run only in the owner's fixed default project or an
explicitly approved project alias. `everett_start_status` then checks the exact
accepted run owned by that registered identity. It does not grant general T3
access or establish a correlated inbox reply.

This candidate was prepared against main `dbf1e12`. All runtime verification
below used synthetic fixtures. No live grant, launch or connector change was
performed. The examples describe a future owner-authorized flow.

## Agent-driven launch and status flow

Suppose the user asks an AI agent to start work in the approved `everett` project.
The agent first inspects this hosted connector's tools. It registers a private
identity once, retaining its registration key and returned handle privately.
After reconnecting it reuses the stored handle, rather than creating a new inbox.

The following is tool-call pseudocode. Secret variables and request state live
in the agent's private conversation state, never in source code or public logs.

```javascript
const identity = await everett_identity({
  action: "create",
  registration_key: privateRegistrationKey,
  name: "Project assistant",
  kind: "chat"
});
const info = await everett_whoami({
  identity_handle: identity.identity_handle
});
```

Before a start, the agent checks the expected gateway resource and build, requires
`capabilities.native_session_start === true`, and reads `native_start_scope`.
It matches the user's requested alias against `native_start_scope.projects` and
the connector's `everett_start.project` enum. If either is missing, it reports
the missing grant or stale schema and stops. A request for an unapproved Teresis
project must not fall back to Everett or another transport. Existing-thread
handoffs use `everett_directory` and `everett_send`, rather than a new start.

Once the user has authorized this start, the agent stores one fresh URL-safe
request ID of 32 to 128 characters and the exact arguments before dispatch:

```javascript
const request = {
  identity_handle: identity.identity_handle,
  request_id: durableRequestId,
  project: "everett",
  text: authorizedTaskText
};
// Persist request privately before making the call.
const receipt = await everett_start(request);
```

Omitting `project` uses the original fixed grant. Omitted model and effort use
owner defaults; explicit values must match an advertised authorized pair.
After a timeout or reconnect, the agent repeats the identical request with the
same handle and ID. Changing selection under a reserved ID is rejected. A
confirmed preflight rejection returns `dispatch_not_started`; after resolving
that issue the same ID and inputs can be retried. An uncertain dispatch keeps
its reservation and must not be retried under a fresh ID.

A successful MCP example has `dispatch_confirmed: true`, the exact `project_id`,
thread/run IDs, `configured_provider`, attested `effective_provider`, and matching
`requested`/`effective` model and effort. It still has `pickup_confirmed: false`,
`completion_confirmed: false`, `reply_confirmed: false`, and a null native session
ID. The agent reports acceptance, then calls the registered read-only status tool:

```javascript
const observed = await everett_start_status({
  identity_handle: identity.identity_handle,
  request_id: durableRequestId
});
```

The response separates the immutable `receipt` from current `reconciliation`.
The agent reports only the evidence returned:

| Evidence | What the agent can report |
| --- | --- |
| `configured_provider` in scope/receipt | Owner-configured provider instance |
| Confirmed receipt and attested `effective_provider`/`effective` | Accepted dispatch with attested launch selection |
| Exact run's executed model and valid start timestamp | T3-reported provider pickup |
| Exact run's terminal status | Completed, failed, cancelled, interrupted or rolled-back outcome |
| `reply_confirmed: false`, `native_session_id: null` | Reply and native session identity remain unconfirmed |

A queued run, missing metadata or a legacy CLI receipt establishes no pickup.
A thread card's model/status and completion alone establish no correlated reply.
Further status calls are read-only and must use the original ID and identity.

## Authorization, receipts and reconciliation

An optional `approved_projects` map fixes exact project/provider instance IDs
behind aliases. The reserved `default` alias and an omitted selector preserve
the existing fixed grant. Models, effort pairs and MCP endpoint remain shared
within the owner policy. No actual Teresis grant is included in this patch.
See [gateway configuration](gateway.md) for the configuration and schema contract.

The journal key remains issuer/resource/account/client/registered-identity and
request ID across all selections. The selected fixed policy participates in the
fingerprint. Empty alias maps are omitted from policy serialization to preserve
legacy fingerprints. Policy changes may reject old replay with a mismatch;
they cannot silently relaunch the reserved ID.

Whoami exposes scope only after a registered identity resolves behind the exact
OAuth account/client gate. `everett_start_status` accepts no thread, project,
prompt or provider argument. It opens the journal read-only and queries only
its accepted thread/run through loopback native MCP `t3_thread_read`. It checks
exact thread/project/run/provider and returns selected run metadata, discarding
transcript items and thread-card model/status. Status does not mutate receipts,
dispatch, add destinations, wake or control runs.

## Synthetic verification

The native gateway integration suite passed 55 tests. The plugin suite passed
10 tests. Strict all-target Clippy and `git diff --check` passed. Run from the
repository root with the existing Rust/Python toolchains:

```sh
cargo test --locked --manifest-path everett-rs/Cargo.toml --test gateway_native
python3 -m unittest discover -s plugins/everett/tests -v
cargo clippy --locked --manifest-path everett-rs/Cargo.toml --all-targets -- -D warnings
git diff --check
```

The native checks cover two synthetic approved project/provider pairs, unknown
alias/raw-ID/type rejection before dispatch, unchanged defaults, legacy receipt
replay, account/client/private-identity isolation, restart-safe replay, changed
selection rejection, attested selection, and legacy dispatch-only reporting.
Reconciliation covers queued, picked-up, completed, failed, missing run, wrong
project/provider, ignored card state, transcript suppression and an unchanged
journal byte image. Plugin checks cover alias and status schema refresh.

The initial build hit a temporary-filesystem disk quota. A task-specific writable
`TMPDIR` resolved it without changing dependencies, credentials or running services.
The full Rust suite and live hosted/provider acceptance were not run locally.
CI results must be assessed separately for the PR head.

## Owner rollout gates and limits

1. Review the draft PR and its exact-head checks before installation or deployment.
2. Separately approve exact Teresis project/provider configuration if desired.
   Preserve the existing fixed grant and journal. This source change creates no
   live grant and supplies no credential.
3. Refresh the existing connector schemas. Add optional string
   `everett_start.project` with the exact alias enum and read-only
   `everett_start_status` with required string `request_id` and `identity_handle`,
   rejecting additional properties. Preserve structured results and the existing
   directory tool. A plugin bundle update alone does not refresh the app catalog.
4. Only after separately approved deployment and catalog refresh, verify scope
   with the registered identity. A separately authorized bounded live test must
   verify dispatch, pickup, terminal outcome, correlated reply and replay safety.

Reconciliation checks at most 50 recent runs and cannot recover an uncertain
start without accepted IDs. Legacy-only grants remain dispatch-only. Native
session identity remains null and reply proof remains false because supported
status metadata establishes neither. No real provider session was started to
prepare this change. No live connector, grant, tunnel, release or deployment
change is part of this PR.
