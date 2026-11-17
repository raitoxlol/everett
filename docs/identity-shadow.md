# Account discovery and policy shadow candidate (#148)

This candidate is an offline fixture evaluator, not a production authorization path.
`src/identity_shadow.rs` is imported only by tests and the offline example.
Production binary and library builds do not import it. No endpoint, grant, queue, credential, registration store,
or inbox is changed. Inputs must be synthetic fixtures; do not export live secrets.

## Domains and trust

`AccountPrincipal { issuer, subject } -> ClientDevice -> SessionIdentity` separates
verified account ownership, a client/device binding, and the stable recipient ID.
An OAuth client name is not a conversation ID. A native record supplies its existing
host ID; a gateway registration supplies its existing server-issued logical ID.
`HostAttested` describes the source of a native adapter record, not a cryptographic
attestation verified by this evaluator. `OwnerRegistered` never claims the host
supplied a conversation identifier. Private identity handles are not input fields.

Neither native Session nor external Registration carries authenticated account
ownership. The evaluator therefore requires a fixture mapping to ClientDevice.
These caller-supplied mappings are NOT authentication. Any future live adapter must
construct them from verified account/device state and check revocation at request
time. Never accept these JSON mappings from a production caller as authority.
The `current` decision is supplied fixture evidence; this evaluator does not call
or replace the current gateway authorizer.

## Existing behavior and bounded improvement

At baseline ac017338fc700a88db95d4e50e6cfef7095c46e3, Gateway::bound derives
identity_owner from (account, client_id); identities::scope additionally includes
issuer/resource. Registered identities are listed within that connection scope.
Native discovery is a separate adapter path. The shadow projection assembles both
under a single explicit account mapping without changing either current path.

Discovery preserves full IDs and disambiguates duplicate names with full IDs.
Resolution accepts only exact IDs or explicit aliases. Duplicate IDs, aliases,
unknown client mappings and alias collisions reject the entire snapshot rather
than silently choosing a target. Foreign accounts/issuers and revoked clients or
sessions cannot receive proposed same-owner messaging permission. Discovery retains
revoked records as tombstones with their state, but policy denies them.

Registration and online reachability are separate. Native `running` and registration
timestamps do not establish remote pickup availability; projection starts Unknown.
Stale/offline reachability does not change an inbox-only messaging proposal.
Dispatch, pickup and reply are deliberately `not_observed`; the evaluator never
sends, wakes, reads inboxes, or infers delivery from a policy decision. Messaging
proposals do not grant wake, execute, transcript-read or shared-memory capabilities.

## Migration proposal, not an enabled migration

1. Build verified account/device ownership mappings outside untrusted tool arguments.
   Missing mappings remain in legacy behavior; do not infer ownership from titles,
   filesystem proximity, OAuth client names, or claimed IDs.
2. Project native and registered records preserving exact inbox IDs and history.
   Keep old client-scoped identity handles and fixed/native grants unchanged.
3. Review explicit alias mappings. Aliases resolve to the original inbox; they do
   not create inboxes, copy history, or dispatch a second time. Collisions block.
4. Evaluate current and proposed decisions on isolated fixtures. No shadow result
   overrides the current decision. A future approved cutover must independently
   address authority provenance, account/device revocation freshness and consent.
5. Only a separately approved implementation may wire this into live discovery or
   messaging.

#132 remains host installation/availability work. #100 diagnostics and #103 transport
observations remain separate. PR163 is already in the baseline and is unchanged.

## Run

`cargo test --locked --manifest-path everett-rs/Cargo.toml --test identity_shadow`

`cargo run --locked --manifest-path everett-rs/Cargo.toml --example identity_shadow < fixture.json`

The example accepts `{snapshot:{clients:[],sessions:[]},account:{issuer,subject},requests:[]}`.
See the integration fixture for complete records. Output includes discovery,
current/proposed decisions, and `shadow_only:true`. This is offline executable
proof only. Real ChatGPT conversation-ID availability, client registration,
client-to-client pickup/reply and live same-owner trust are not established here.

The example also accepts optional `native` and `registered` arrays of
`{client_id, record}`. Native records use the existing `Session` serialization;
registered records use `everett_identity list` row fields. The mapping and records
in this runner remain fixture input, not authenticated live data.

Held-out executable checks:
`python3 scripts/check-identity-shadow.py everett-rs/target/debug/examples/identity_shadow`

Use `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0`
on constrained development disks. The initial default build hit ENOSPC; the
reduced-debug build passed without removing any previous task artifacts.


## Verified ownership adapter follow-up

`gateway/ownership_shadow.rs` is compiled only under `cfg(test)`. It replaces
caller-supplied account mappings for its projections with the existing verification
and resolution functions. The original JSON example remains a synthetic fixture
runner; it is not an authenticated interface and has not gained any authority.

For logical chats, the adapter takes the gateway's trusted binding and verifier,
checks that their entire OAuth verification policy matches, calls
`OAuthVerifier::verify` on every projection, uses `Binding::for_oauth`, resolves the
private handle through `identities::resolve`, and checks the actual registration
through `identities::list`. Issuer, owner account and OAuth client come from these
verified results and trusted configuration, never a tool's account/client/session
arguments. Output contains the stable session ID and no handle or bearer token.
Missing account provenance, revoked credentials/handles, foreign clients, unavailable
registrations and mismatched verifier policies cannot produce a proven projection.

For native sessions, existing auth proves the device owner and a hook proves the
host-bound session ID. Neither proves historical session ownership. Consequently,
a legacy session without an explicit registration receipt returns unmapped even
when the current device is signed in. Fixture-only `NativeReceipt::register` models
an explicit registration operation, separately from discovery. It derives account
and device from `cloudflare_auth::load` plus `whoami`, and session ID from the
existing strict hook resolver `host::live_binding`; it never falls back to cwd or
session-file proximity. The Host argument is trusted process-inspector input,
supplied synthetically by the test harness, not a new model/tool parameter.

The receipt has private fields and no JSON deserializer. It is held only in fixture
memory; no live receipt store or migration writer is implemented. Projection with
that receipt rechecks the current credential and hook, including issuer, account,
device, harness and exact session ID. An account switch, changed host binding or
revoked credential fails instead of assigning old sessions to the last login.
Reconnection with the same verified binding preserves the inbox ID. A future
persisted receipt must have an operator-approved integrity and revocation design;
this candidate does not accept unsigned receipt JSON as authority.

The only production-code refactor extracts the existing OAuth binding construction
unchanged into `Binding::for_oauth` and makes the existing hook resolver visible
within the crate. No new runtime caller, endpoint, discovery action or registration
write is enabled. The current authorizer remains the sole authority. Projected
snapshots are observations at verification time, not reusable authorization grants;
subsequent revocation must be checked again, as the adapter does.

Verification uses isolated HOME/profile data and loopback fake auth servers with
synthetic credentials. It exercises the actual native verifier, OAuth verifier,
registration store and handle/hook resolvers for two logical chats plus two native
sessions, account/client/session forgery, account switches, different issuers,
revocation, reconnects, legacy-unmapped behavior, redaction and policy mismatches.
This is authentication/resolution integration proof, not live ChatGPT host proof.
Run `cargo test --locked --manifest-path everett-rs/Cargo.toml --lib gateway::ownership_shadow`.
The scored reproduction initially accepted a weaker verifier than the gateway's
owner policy; the corrected adapter rejects the mismatch. The existing library
suite guards unchanged behavior during that hill-climb.
