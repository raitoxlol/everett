# Defensive security review — 2026-10-19

## Scope and limits

Reviewed main v1.6.0 and the pinned revisions below. Existing PR branches were
read-only; fixes in this PR are based on main. No merges, production deployments,
real accounts, real credentials, or live model-provider connections were used.

This is a **bounded defensive review, not a completed penetration test**. Existing
negative security tests and normal synthetic integration journeys were executed.
No new exploit scripts, credential-theft attempts, authentication-bypass workflows,
or forced-command escape reproductions were built. An untested attack is not a pass.

## Immutable branch matrix

Commands: `cargo test --locked` and
`cargo clippy --all-targets -- -D warnings` in `everett-rs/`; `npm ci`,
`npm test`, and `npm run typecheck` at the repository root. The npm package is
at the root, not in `convex/`. Five separate-VM workers checked detached revisions.
All install/check exit codes were zero after correcting an npm-config isolation
error in the validation harness; that initial install failure was not a code defect.

| Revision | SHA | Cargo tests | Clippy | npm tests | Typecheck |
| --- | --- | --- | --- | --- | --- |
| main | `435af2c3750ae85a7eea5e0d233f830cb755e3b7` | PASS | PASS | PASS (34) | PASS |
| [#22](https://github.com/raitoxlol/everett/pull/22) | `9ee47679eef0f0401081b3a8470370e2f06f7f6e` | PASS | PASS | PASS (35) | PASS |
| [#23](https://github.com/raitoxlol/everett/pull/23) | `64328b8a84ccf8972119790415633e94e1776cfa` | PASS | PASS | PASS (37) | PASS |
| [#24](https://github.com/raitoxlol/everett/pull/24) | `d962ec09232e9d85b2a01463d88aa0c519cea7c5` | PASS | PASS | PASS (38) | PASS |
| [#25](https://github.com/raitoxlol/everett/pull/25) | `d8f6a14e73ac7c3a81b70ed713daa335419d16c3` | PASS | PASS | PASS (46) | PASS |

PR #22's six local dev-login regression tests also passed. These results describe
these SHAs, not future pushes to those PRs. The main checkout used Ubuntu 22.04,
Rust/Cargo 1.97.1 and Node 24.19.0; the local native backend ran in Ubuntu 24.04.

## Confirmed findings fixed here

### F1 — P2 / moderate: cloud-origin local messages lose external provenance

**Status: fixed.** `grok-bot` and `openai-dot` sends/replies used the correct bound
sender but local inbox messages did not receive the structured external/untrusted
markers. A local MCP consumer therefore saw ordinary text. Simply tagging messages
was insufficient: durable provenance retention filtered out messages without relay
lineage, so a later local forward could lose the classification again.

Evidence is a normal synthetic report sent by each external harness to a fixture
Codex session, peeked/taken over stdio MCP, then summarized into a Claude inbox.
The added `cloud_origin_provenance_survives_mcp_consumption_and_local_forwarding`
test failed on main with `trust: null` instead of `external-untrusted` and passes
with this change. No malicious prompt or model execution was needed or tested.

`inbox::post` now derives cloud-origin flags from harness provenance; consuming
external data also retains monotonic durable context even without remote lineage.
MCP output renders the existing warning, and local forwarding preserves taint.
The native gateway journey checks both direct sends and replies for both harnesses.
The existing stdio round-trip assertions now require labeled cloud replies.

Recheck:

```sh
cd everett-rs
cargo test --locked --test external
cargo test --locked --test gateway_native native_stdio_journey
```

These labels are a data boundary, not proof an LLM will ignore embedded instructions.
The owner controls MCP process identity; environment labels are not provider auth.

### F2 — P3 / low: concurrent MCP card writes share a temporary filename

**Status: fixed.** `tool_card` wrote `.SESSION.md.tmp` before renaming it. Separate
MCP workers updating the same card could rename/remove one another's temporary file,
causing a legitimate update to return `No such file or directory`.

`concurrent_card_updates_are_atomic_and_do_not_share_temporary_files` starts eight
independent stdio servers with one synthetic bound session and synchronizes 16
rounds of ordinary card updates. It failed on unchanged main with that error.
It now checks all 128 updates succeed, the final card is a complete last-round
body, and no temporary files remain. The test is concurrency-sensitive; one
successful pre-fix run would not establish the absence of the race.

The fix reuses the existing `peer::save` atomic writer (unique temporary files,
mode 0600, sync then persist). Last complete write still wins; this does not add
versioned card history or optimistic concurrency.

```sh
cd everett-rs
cargo test --locked --test mcp_stdio concurrent_card_updates
```

## Integration evidence

### Real SSH, not a mocked ssh executable

Independent Ubuntu 24.04 containers A and B used separate network namespaces,
filesystems, users' homes, freshly generated sshd host keys and Everett device keys.
The binary was installed at the same stable absolute path on each. Only synthetic
bootstrap keys were supplied; no host SSH agent or user key directory was mounted.

Observed: pairing with independently read host/device fingerprints, A-to-B inbox
delivery over real sshd, recipient take and reply, sender's correlated `replied`
status, and receiver revocation rejecting a subsequent pinned RPC with exit 7.
The first script run misparsed the successful human-readable `reply` output as JSON;
the corrected run completed. Reusing a test conversation also hit the intended hop
limit; the successful fresh-conversation run used distinct session IDs.

This does not establish forced-command escape resistance or resistance to a local
account owner modifying their own authorized keys. Existing synthetic `peer_native`
tests cover argument/pinning/key-file boundaries, not a new exploit demonstration.

### PR #22 local issuer and encrypted relay

The documented `up.sh` and `smoke.sh` ran on the pinned PR #22 checkout inside an
Ubuntu 24.04 container using Node 24 and the built native CLI. The native backend,
OIDC issuer and TLS proxy were local only; no hosted deployment was selected.
Docker Hub initially returned 429; the Ubuntu image was pulled through Google's
public mirror, without changing system libraries or disabling certificate checks.

Smoke run `2026-10-19T11-57-50-370Z`: **PASS, zero failed checks**. It observed
native device login and refresh, independently checked backend identity/claims and
credential permissions, registered distinct A/B devices, pinned public bundles,
verified queued ciphertext without plaintext before recipient startup, and completed
encrypted A→B pickup→reply→A inbox with signed `replied` status. It also exercised
the supplied TLS and invalid-signature diagnostics. This smoke uses separate homes
on one host; it is not itself a multi-host network test.

Additional three-container relay/grant validation and final fixed-branch suite
results will be recorded below before this review is finalized.

### Grok Bot and OpenAI dots

The documented native registration/MCP/gateway paths are covered by `external`,
`mcp_stdio` and `gateway_native` integration tests, including actual subprocesses
and loopback HTTP rather than live providers: explicit registration for each
harness, bound `whoami`, peek/take/reply, exact destinations, inbox-only delivery,
foreign-session argument rejection, gateway tool allowlist and HTTP restrictions.
F1 corrects the missing untrusted-label behavior. Jev, OMP and Pi adapters are not
changed by this PR.

No Grok Team Bot, ChatGPT Secure MCP Tunnel, OpenAI OAuth, provider polling, or
provider wake-up was connected. The docs correctly describe logical identities,
not provider authentication or automatic wake-up. Cloud-side compatibility remains
unverified; passing a local MCP client is not evidence of a live cloud connection.

## Existing regression coverage vs requested attack matrix

| Area | Executed defensive evidence | Still unverified |
| --- | --- | --- |
| Auth | Convex handler tests; PR #25 owner/session/approval/tombstone/token-binding tests; local PR #22 login/refresh and JWT diagnostics | Live hosted owner enrollment; active forged/expired/replayed JWT attack campaign; stolen real agent tokens |
| Relay authorization | Existing backend org/device/grant/routing/replay/quota/TTL/delete tests; native encryption/provenance tests; local encrypted smoke | Production access; stress/rate-limit bypass campaign; exhaustive malformed-envelope fuzzing |
| Prompt/data provenance | Existing cross-org relay/MCP tests plus F1 local external→MCP→forward regression | Actual LLM resistance to prompt injection; arbitrary tool-result transformations outside Everett |
| SSH | Existing peer fixtures plus real sshd functional pairing/send/reply/revoke | Forced-command escapes, key/pin bypass exploitation, revocation propagation from PR #24 on real SSH hosts |
| MCP/gateway | Existing malformed JSON-RPC, identity, traversal and gateway restriction tests; real stdio/HTTP subprocess journeys | Full protocol fuzzing, live internet-facing gateway or provider tunnels |

In-memory Convex handler tests do **not** prove JWT signature verification by a
live authentication service. PR #22's smoke adds a real local JWT boundary, but
does not turn PR #25's handler tests into a deployed owner-auth test.

## PR-specific review observations — not confirmed vulnerabilities

These source-review hypotheses have no new regression/reproduction here and are
**not fixed**. They are listed separately from the two confirmed main bugs, without
changing any existing PR branch or treating worker speculation as a finding.

- **#22, low / test robustness:** `scripts/dev-login/smoke.mjs` inspects only the
  first pending page for its ciphertext diagnostic; it may miss rows behind a
  cursor. The `run()` helper in `stack.mjs` sends SIGTERM at 120 seconds but has no
  independent forced-kill deadline if that child ignores termination. The separate
  shutdown routine does have a forced-kill path. Neither was shown to fail here.
- **#23, moderate / recovery hypothesis:** peer removal reaches the old pinned
  endpoint before deleting local trust; offline/changed-key recovery needs the
  documented manual receiver-first procedure. Authentication probing also precedes
  remote device fingerprint comparison. No bypass or credential disclosure was
  demonstrated; do not confuse this with the later authorization/install check.
- **#24, moderate / lifecycle hypotheses:** membership revocation and device
  revocation may not produce the same SSH cleanup set (`convex/relay.ts`,
  `everett-rs/src/doctor.rs`). A backend revocation committed before fallible local
  cleanup may also make an authenticated retry unavailable
  (`everett-rs/src/relay/mod.rs`, `cli.rs`, `convex/authorization.ts`). These need
  focused lifecycle/failure-injection tests on that PR before acceptance.
- **#25:** the bounded read-only review and existing tests identified no confirmed
  new defect. This is not a claim of exploit-proof owner enrollment or tokens.

## Remaining release checks

This document records bounded evidence, not security certification. Before a hosted
release: finish the outstanding real-host PR #24 revocation lifecycle checks and
exercise owner enrollment in an isolated PR #25 deployment; separately validate
Grok/ChatGPT integrations with owner-authorized test provider accounts.
