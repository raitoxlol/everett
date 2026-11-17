# Account discovery and policy shadow candidate (#148)

This candidate is an offline fixture evaluator, not a production authorization path.
`src/identity_shadow.rs` is imported only by its tests and example. The binary and
library do not import it. No endpoint, grant, queue, credential, registration store,
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
   messaging. Original Astra review is required before this candidate merges.

#132 remains host installation/availability work. #100 diagnostics and #103 transport
observations remain separate. PR163 is already in the baseline and is unchanged.
RAI-67 OAuth work and mini-duck are outside scope.

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
