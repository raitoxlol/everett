# Security review, October 2026

This review covers `main` at `435af2c3750ae85a7eea5e0d233f830cb755e3b7` (v1.6.0) and open PRs #22–#25 at pinned heads. All runtime work used synthetic identities, keys, and local stacks. No production deployment, real account, or real secret was used. PRs #22–#25 remained read-only. Nothing was merged.

PR #26 is the consolidated findings/fixes PR. It includes PR #27's cloud-provenance forwarding and concurrent-card-write regressions, without merging either PR. This document combines recovered evidence from the [original review](https://app.devin.ai/sessions/994b48918ee54e9181177ceb9a36ef64) and the [independent defensive validation](https://app.devin.ai/sessions/db4f9ff95ab44c339115f2e2c4de8728). The [continuation](https://app.devin.ai/sessions/fb75e8e412bd43bf8f68031c042d74e0) reviewed both branches and re-ran the combined fix branch. It did not repeat the original attack campaign. [Final verification](https://app.devin.ai/sessions/3d3ef9a8fd4740fca852926e463e23de) repeated the pinned branch matrix and the combined fixed-branch checks described below.

This is bounded defensive evidence, not security certification or an exhaustive penetration test. A successful test does not establish that an untested attack is impossible. In-memory Convex tests inject authenticated identities; only the explicitly identified local-backend runs exercised actual JWT signature verification.

| PR | Head |
| --- | --- |
| #22 local OIDC and Convex stack | `9ee47679eef0f0401081b3a8470370e2f06f7f6e` |
| #23 device discovery | `64328b8a84ccf8972119790415633e94e1776cfa` |
| #24 device revocation | `d962ec09232e9d85b2a01463d88aa0c519cea7c5` |
| #25 owner-only hosted enrollment (read only) | `d8f6a14e73ac7c3a81b70ed713daa335419d16c3` |

## Findings

| ID | Severity | Where | Status |
| --- | --- | --- | --- |
| M1 | High | `main` | Fixed in this branch |
| M2 | Medium | `main` | Fixed in this branch |
| M3 | Low | `main` | Fixed in this branch |
| M4 | Low | `main` | Fixed in this branch, consolidated from #27 |
| S1 | Low | `main` and PR #24 | Not fixed. Documented behavior |
| P22-1 | Low | PR #22, Convex runtime | Not fixed |
| P23-1 | Low | PR #23 | Not fixed |
| P23-2 | Informational | PR #23 | Not fixed |
| P25-1 | Low | PR #25 | Not fixed |

### M1: unbound stdio MCP accepted a model-supplied `session_id` (High)

**Prerequisite.** `everett mcp` starts without a host-detected caller (no `EVERETT_SESSION_ID` and no native harness variable). The gateway is not affected.

The impact is access between logical sessions under the same local OS account, not an OS privilege escalation. The local account and MCP launcher remain trusted.

**Repro.** Seed `~/.everett/inbox/victim.jsonl`. On the unbound server, call `everett_inbox {"session_id":"victim","peek":true}`, then `everett_inbox {"session_id":"victim"}`, then `everett_card {"session_id":"victim", ...}`.

**Observed.** The caller read the victim's message, marked it delivered in `victim.done`, and wrote `cards/victim.md`. README and `docs/grok-bot.md` say a model-supplied `session_id` cannot establish identity.

**Fix.** `caller_session` in `everett-rs/src/mcp.rs` returns JSON-RPC `-32602` when `session_id` is supplied without a host binding. A matching `session_id` on a bound server still works. Codex's host-written per-call `_meta.threadId` is accepted as its binding because Codex overwrites that value at the MCP dispatch boundary.

**Regression tests.** `mcp_stdio::unbound_mcp_cannot_read_consume_or_overwrite_another_sessions_state`, `mcp_stdio::codex_call_metadata_binds_identity_without_process_environment`, plus updated `self_send_refused`, `unbound_mcp_cannot_deliver_or_reply_with_model_provenance`, and `audit::oversized_first_inbox_message_delivers_truncated` (now binds `EVERETT_SESSION_ID`).

**Behavior change.** Claude Code, Grok CLI 1.0.4+, Codex, and `everett gateway` bind automatically. Pi, Devin, OMP, Hermes, T3 Code, and other clients without supported host metadata must set `EVERETT_SESSION_ID` per session in the MCP launch environment. A client can no longer establish identity by passing a tool argument.

Published host contracts were checked before keeping the manual-binding list. [Pi's session variables](https://pi.dev/docs/latest/environment-variables) are injected into shell tools, not MCP servers. Devin puts `session_id` in [lifecycle-hook payloads](https://docs.devin.ai/cli/extensibility/hooks/lifecycle-hooks), while its [MCP configuration](https://docs.devin.ai/cli/extensibility/mcp/configuration) offers static `env` values and no per-call session field. OMP's [MCP configuration](https://omp.sh/docs/mcp) likewise expands process/static configuration; its session ID is available to Everett's OMP extension through `sessionManager`, not on MCP calls. No automatic Pi, Devin, or OMP binding is inferred from their shell/hook APIs.

### M2: `grok-bot` and `openai-dot` messages were not labeled untrusted (Medium)

**Prerequisite.** A registered external harness sends or replies through bound `everett mcp` or `everett gateway`.

**Repro.** Register `ext-grok-bot` with `everett external add --harness grok-bot`. From that binding, reply to an owner handoff. Read the owner's inbox.

**Observed.** The owner received raw text with no `external`, `untrusted`, `trust`, or `instruction_source` field and no `[EXTERNAL / UNTRUSTED ...]` wrapper. Relay cross-org messages were already labeled. Only the logical external harnesses were missed.

**Fix.** `inbox::post` stamps the labels when `from_harness` is an external harness. `inbox::external` also derives provenance from `from_harness`, so messages queued before this fix are labeled on read. A reply derives provenance from the exact original message. Merely peeking at or consuming a logical Grok Bot/OpenAI dot message does not write durable relay context or taint unrelated later resume/spawn work; durable session context remains limited to messages with relay lineage.

**Regression tests.** `external::previously_queued_external_messages_without_trust_flags_are_untrusted_on_read`, `external::cloud_origin_provenance_survives_reply_without_tainting_unrelated_forward`, updated `external::stdio_mcp_queues_polls_and_replies_for_both_external_harnesses`, the `gateway_native` stdio journey, and `relay_native` same-org and cross-org consumption through real `everett mcp` and `everett gateway` processes.

### M3: JSON-RPC ids `true`, `{}`, and `[]` were dispatched (Low)

**Repro.** Send `{"jsonrpc":"2.0","id":true,"method":"tools/call", ...}` to `everett mcp`.

**Observed.** The tool ran. JSON-RPC 2.0 allows only string, number, or null ids.

**Fix.** `mcp::handle` returns `-32600` with `id: null` and does not dispatch. The gateway already dropped these frames without dispatch, before and after the fix.

**Regression test.** `mcp_stdio::malformed_jsonrpc_ids_never_dispatch_tools`.

### M4: concurrent MCP card updates shared one temporary filename (Low)

**Repro.** Run `cargo test --locked --test mcp_stdio concurrent_card_updates` in `everett-rs/`. The regression starts eight independent stdio MCP processes bound to the same synthetic session and synchronizes 16 rounds of ordinary `everett_card` updates.

**Observed.** The old `.SESSION.md.tmp` path let one process rename another process's temporary file. A legitimate update failed with `No such file or directory`. This was reproduced on unchanged main in the independent review and during final verification. One successful baseline run does not disprove this concurrency-sensitive race.

**Fix.** `tool_card` uses the existing `peer::save` atomic writer: unique temporary file, private permissions, sync, then persist. Last complete write wins. No optimistic concurrency or card history is promised.

**Regression test.** `mcp_stdio::concurrent_card_updates_are_atomic_and_do_not_share_temporary_files` checks all 128 updates succeed, the final card is a complete last-round body, and no temporary files remain. The test and fix from #27 are retained in #26.

### S1: a held-open SSH channel can submit one frame after revocation (Low, not fixed)

**Prerequisite.** The attacker holds the paired client key and opens the forced-command channel before the receiver runs `everett peer revoke`.

**Repro.** On two Docker hosts (`alice@A`, `bob@B`), A opens `ssh bob@B` with the paired key and sends nothing. B runs `everett peer revoke <client>`; the `authorized_keys` tag is gone. A then writes one `send` frame within the 20-second `peer serve` timeout.

**Observed.** B queues the message with `peer_client` set to the revoked ID. A new login with the same key fails with `Permission denied (publickey)`. `main` and PR #24 behave the same.

**Status.** Not fixed. `docs/cross-device.md` says revocation takes effect on new SSH connections. Re-checking `authorized(client)` after `read_frame()` in `peer::serve` would shrink the window but not close it.

### P22-1: correctly signed JWTs with a future `nbf` are accepted (Low, PR #22)

**Repro.** In the PR #22 local stack, sign an RS256 ID token with the local issuer key, valid `iss`, `aud`, `exp`, and claims for a provisioned agent, and `nbf` one hour, one day, or one year ahead. Call `relay:whoami` and `relay:devices`.

**Observed.** The real local Convex backend returned the identity and devices. The native client rejects the same token (`everett-rs/src/relay/mod.rs` checks `nbf`). Forged, tampered, expired, wrong-issuer, wrong-audience, and `alg=none` tokens were rejected.

**Impact.** The attacker needs a token the issuer actually signed. No tenant boundary is crossed. `convex/authorization.ts` is identical on `main`, so `main` may share the behavior, but a `main` backend was not run. PR #25's local runtime showed the same result; Convex Auth tokens there do not carry `nbf`.

### P23-1: a member's display name can block a provisioned agent's first registration (Low, PR #23)

**Repro.** An ordinary member calls `registry:publish` for its own device with `name` set to a provisioned but unregistered victim agent ID. The victim's valid `relay:register` fails with `Device name is already in use in this account`. Revoking the member does not clear it.

**Cause.** `relay:register` now requires the device ID to be a free display name, and `registry:named` returns revoked rows. `main` has no such coupling. The [PR #23 reviewer](https://app.devin.ai/sessions/3594eb1eda3745468f0d08c8566a8f05) reproduced this with a separate fixture test, not a test committed to main. The existing PR branch was not patched.

### P23-2: registry names that the SSH CLI cannot pair (Informational, PR #23)

`registry:publish` accepts `.`, `..`, and names of 129–160 characters. `everett peer add` rejects them with exit 2 because `inbox::valid_id` allows 1–128 characters and excludes `.` and `..`. No traversal occurs.

### P25-1: anonymous callers can exhaust the enrollment budget (Low, PR #25)

**Repro.** Within one minute, call `hosted:begin` with fresh token hashes until `Enrollment rate limit` appears, then start a legitimate enrollment.

**Observed.** The legitimate enrollment failed until the next minute. `convex/hosted.ts` uses one global `hosted-enrollment` bucket of 30 per minute. Existing agents keep working. No owner, scope, revocation, issuer, or audience bypass was observed.

The [PR #25 local-backend reviewer](https://app.devin.ai/sessions/055c708d72f54c0cbcefcd89c4189bcf) confirmed the availability failure on a within-minute repeat; an initial run crossed a minute boundary and did not reproduce it. This is separate from the intended per-agent relay budgets. No regression for the PR-only hosted API is added to main, where that API does not exist.

## Isolated cross-device evidence

### SSH hosts

Two Ubuntu 24.04 containers had separate network namespaces, filesystems, users (`alice` and `bob`), sshd processes, host keys, Everett keys, `HOME`, and `EVERETT_HOME`. No owner SSH agent or real key directory was mounted. Pairing used independently read host/device fingerprints and a stable absolute binary path.

The [SSH reviewer](https://app.devin.ai/sessions/1b27133bdca34fa2bf44b8dbec05065a) exercised pairing, A→B inbox delivery, take/reply, sender status polling, receiver revocation, rejection on a fresh SSH connection, and explicit re-pairing on main and #24. Eighteen of nineteen security/journey checks passed on each binary; the nineteenth exposed S1. A separate [integration run](https://app.devin.ai/sessions/db4f9ff95ab44c339115f2e2c4de8728) also completed normal two-host pairing/send/reply/revoke.

These are real OpenSSH connections, not the fake-SSH fixture used by some Rust tests. They do not establish the complete registry→doctor→authorized_keys revocation path.

### PR #22 encrypted relay and cross-org grant

The documented local dev-login stack and smoke ran inside Ubuntu 24.04, with Node 24, the native CLI, local OIDC issuer, Convex backend, and TLS proxy. The host's older GLIBC could not run the backend directly. No cloud deployment was selected. The documented smoke passed with zero failed checks; its separate homes on one host alone are not a multi-host test.

Both the [original relay reviewer](https://app.devin.ai/sessions/54b28c7a44344060be6555954292bdb0) and the [independent integration run](https://app.devin.ai/sessions/db4f9ff95ab44c339115f2e2c4de8728) then used three isolated client containers plus a local service container. Each client had its own network namespace, home, and keys. Synthetic A/B identities shared an org; C belonged to another org. Pinned SSH tunnels made the loopback-only stack reachable without disabling relay TLS/CA verification.

Observed through the native CLI:

- Device-code login, server-derived identity, registration, and public-bundle pinning on each client.
- Encrypted A→B→A delivery, recipient take/reply, and signed `replied` status. Pending backend rows contained ciphertext without the plaintext marker before recipient startup.
- A→C grant invitation, bilateral acceptance, encrypted A→C→A delivery, and external/untrusted labels on the cross-org inboxes.
- Grant revocation. Device B revocation denied subsequent identity queries and re-registration, while A's positive control continued to work. Tombstones were not removed for retries.

The original relay reviewer also checked denied post-grant-revocation sends directly at the local backend. The independent integration run did not separately repeat post-revocation native cross-org delivery. Neither run proves production behavior or PR #24's SSH cleanup propagation.

### Grok Bot and OpenAI dots

The local portions of `docs/grok-bot.md`, `docs/openai-dots.md`, and `docs/gateway.md` were exercised with explicit `grok-bot` and `openai-dot` registrations. Real `everett mcp` and `everett gateway` subprocess clients covered bound identity, queue-only delivery, inbox peek/take/reply, and the gateway's exact-destination and inbox-only restrictions. Native HTTP tests cover the authenticated loopback transport, Host/Origin checks, frame bounds, and tool restrictions. Shared core is optional; with it disabled the gateway exposes four tools.

M1 and M2 were discrepancies with the documented identity/data boundaries on main and are fixed here. The docs correctly distinguish a logical owner binding from vendor authentication, automatic cloud wake-up, and provider consumption. No live Grok Bot or existing OpenAI dot was connected. The Secure MCP Tunnel, provider permissions, credential UI, and tool selection remain owner-side verification, not inferred successes from local MCP tests. No claim is made that an LLM obeys the untrusted warning.

## Recovered negative-test observations

The original review reported the following observations from synthetic fixtures and local services. These are bounded checks, not proof of exhaustive attack resistance. Consolidation reran the committed suites, not the separate one-off attack harnesses or live stacks.

- **Auth.** Forged, tampered, expired, wrong-issuer, wrong-audience, and unsigned JWTs were rejected by real local JWKS verification. Client-supplied org, agent, and role fields were rejected or ignored. Non-owners could not approve, list, or revoke in PR #25. Revoked agent tokens, refresh, exchange, and re-registration were denied, including on a live WebSocket subscription. An otherwise valid bearer JWT remains reusable until expiry or revocation; envelope replay protection is separate and does not make JWTs single-use.
- **Relay.** Cross-tenant row reads, deletes, and foreign acceptance failed. Absent or one-sided grants, inviter self-acceptance, and revoked grants blocked cross-org sends. Spoofed sender, modified signature or ciphertext, reused message IDs, and changed idempotency content were denied. Oversized (>64 KiB), malformed, and path-like envelopes were rejected. Per-grant and per-account rate limits held across two agents. Expired ciphertext and accepted ciphertext were deleted while replay tombstones remained.
- **Cross-device.** Three containers completed encrypted A → B → A (same org) and A → C → A (cross org, bilateral grant) through the PR #22 stack. The cross-org inbox was labeled untrusted.
- **SSH.** Two isolated Docker hosts with separate users, IPs, sshd, host keys, `HOME`, and `EVERETT_HOME` ran 19 checks on `main` and PR #24. Wrong host or device pins, forced-command escapes (`; id`, `$(...)`, newline, `peer serve --client attacker --setup`), unknown ops, `client` override fields, oversized frames, and pinned `known_hosts` replacement were all refused. Pairing, send, reply via `peer status`, and re-pairing worked. Only S1 failed.
- **MCP, Grok Bot, and dots.** Registration, queue-only delivery, refused resume and spawn, peek, take, and reply worked through `everett mcp` and `everett gateway` for both harnesses. A bound `session_id` spoof was rejected. The gateway exposed only `everett_whoami`, `everett_inbox`, `everett_card`, and `everett_send`, and rejected foreign destinations, foreign `reply_to`, ambiguous prefixes, and spawn. Registration IDs `../escaped`, `x/y`, `..`, `human`, `live`, `a.b`, `%2e%2e`, non-ASCII, and 101 characters were rejected without writing outside `~/.everett/external`.

## Test matrix

Native test subprocess fixtures isolate `HOME` and `EVERETT_HOME`; consolidation also sets `EVERETT_NOTIFY=none EVERETT_ROUTER=local CI=1`. The npm package is at the repository root, not in `convex/`; `npm ci`, `npm test`, and `npm run typecheck` ran there. Cargo commands ran in `everett-rs/`. There is no remaining Python runtime test suite.

The five pinned revisions passed in both earlier runs. Original branch evidence: [main](https://app.devin.ai/sessions/f0101c822ec04f9fb8e18c3f9283331a), [#22](https://app.devin.ai/sessions/54b28c7a44344060be6555954292bdb0), [#23](https://app.devin.ai/sessions/3594eb1eda3745468f0d08c8566a8f05), [#24](https://app.devin.ai/sessions/1b27133bdca34fa2bf44b8dbec05065a), [#25](https://app.devin.ai/sessions/055c708d72f54c0cbcefcd89c4189bcf). The [independent five-VM validation](https://app.devin.ai/sessions/db4f9ff95ab44c339115f2e2c4de8728) reconfirmed Cargo, clippy, npm, and TypeScript at the same SHAs. These results do not cover future pushes to those PRs. Final verification repeated Cargo tests, Clippy, root `npm ci`, npm tests and typecheck at all five pinned revisions; every command exited zero. Rust runs used a separate target directory per revision. An initial shared-target run produced stale-library missing-symbol errors across worktrees; those harness artifacts are not branch defects and are superseded by the isolated runs.

| Branch | `cargo test --locked` | `cargo clippy --all-targets -- -D warnings` | `npm test` | `npm run typecheck` | `release_journey` |
| --- | --- | --- | --- | --- | --- |
| `main` `435af2c` | pass | pass | pass, 34/34 | pass | pass |
| PR #22 `9ee4767` | pass | pass | pass, 35/35 | pass | pass |
| PR #23 `64328b8` | pass | pass | pass, 37/37 | pass | pass |
| PR #24 `d962ec0` | pass | pass | pass, 38/38 | pass | pass |
| PR #25 `d8f6a14` | pass | pass | pass, 46/46 | pass | pass |
| This branch | pass | pass | pass, 34/34 | pass | pass |

PR #22's separate `node --test scripts/dev-login/regression.mjs` suite also passed all six checks during final verification with `EVERETT_VERIFY_BINARY` pointing to its pinned native binary.

The pinned-branch `release_journey` column is the journey included in each full Cargo test run. On the combined fixes, `cargo build --release --locked` also passed, followed by `EVERETT_VERIFY_BINARY=<absolute-release-binary> cargo test --locked --test release_journey`: all 14 advertised MCP tools and the CLI journey passed against the optimized binary. Full fixed-branch Cargo tests, Clippy, root npm tests (34/34), typecheck and `git diff --check` passed as well.

`cargo fmt --all -- --check` fails on existing repository formatting, beginning with `src/adapters/claude.rs:15`, on both unchanged main and this branch. No repository-wide formatting change was made. Clippy with warnings denied and `git diff --check` are separate checks.

The original MCP harness (74 checks) failed 10 checks on `main` and passed all 74 on the original M1–M3 candidate, with debug and release binaries. That separate harness was not rerun after consolidation. Fixed-main regressions are committed here in `external`, `mcp_stdio`, `gateway_native`, and `relay_native`; they run without credentials or model calls. The packaged release journey verifies all 14 currently advertised MCP tools. Jev's adapter/router behavior, OMP, and Pi retain their existing synthetic coverage; no live Jev API was called.

## PR-specific observations, not confirmed findings

These remain review hypotheses, not additional confirmed vulnerabilities or suite failures. No other PR branch was changed.

- **#22:** the smoke ciphertext diagnostic reads only the first pending page; the startup child timeout sends SIGTERM without an independent forced-kill deadline. Failure with a paginated backlog or an uncooperative child was not reproduced.
- **#23:** removing a peer contacts its old pinned endpoint before local trust deletion. Offline/changed-key recovery needs the documented manual procedure. No key bypass or disclosure was demonstrated.
- **#24:** credential-only revocation with no registered device row, membership-vs-device cleanup, and retry after backend revocation succeeds but local cleanup fails need lifecycle tests. The 256-row cleanup bound and `revokedAt` migration on existing rows were not exercised live.
- **#25:** locally signed synthetic owner/non-owner JWTs and synthetic provider-account/session rows exercised enrollment/token lifecycle, not a real GitHub OAuth browser journey. The independent defensive run did not repeat that deployed-local auth fixture; its mocked-identity tests alone must not be described as JWT verification.

## Not verified

- Real Grok Bot or ChatGPT dots cloud connections, Secure MCP Tunnel, and provider tool selection.
- Production Convex, real GitHub OAuth, and external OIDC issuers. JWT tests used a local issuer.
- The `doctor` → `relay:revokedDevices` → SSH prune path end to end against a live backend, the 256-row revocation limit at scale, and the lifecycle/migration hypotheses above.
- Full protocol fuzzing, stress/rate-limit-bypass coverage, malicious local account owners rewriting their own keys, and actual model resistance to prompt injection. Labels delimit data, not model behavior.
- Interactive macOS cross-device hosts. Local runtime runs were Linux x86_64. Linux/macOS CI is a separate automated regression check, not macOS sshd or provider integration evidence.
