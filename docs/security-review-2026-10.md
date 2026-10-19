# Security review, October 2026

This review covers `main` at `435af2c` (v1.6.0) and open PRs #22–#25 at pinned heads. Every attack ran against synthetic identities, keys, and local stacks. No production deployment, real account, or real secret was used. No existing PR branch was modified.

| PR | Head |
| --- | --- |
| #22 local OIDC and Convex stack | `9ee4767` |
| #23 device discovery | `64328b8` |
| #24 device revocation | `d962ec0` |
| #25 owner-only hosted enrollment (read only) | `d8f6a14` |

## Findings

| ID | Severity | Where | Status |
| --- | --- | --- | --- |
| M1 | High | `main` | Fixed in this branch |
| M2 | Medium | `main` | Fixed in this branch |
| M3 | Low | `main` | Fixed in this branch |
| S1 | Low | `main` and PR #24 | Not fixed. Documented behavior |
| P22-1 | Low | PR #22, Convex runtime | Not fixed |
| P23-1 | Low | PR #23 | Not fixed |
| P23-2 | Informational | PR #23 | Not fixed |
| P25-1 | Low | PR #25 | Not fixed |

### M1: unbound stdio MCP accepted a model-supplied `session_id` (High)

**Prerequisite.** `everett mcp` starts without a host-detected caller (no `EVERETT_SESSION_ID` and no native harness variable). The gateway is not affected.

**Repro.** Seed `~/.everett/inbox/victim.jsonl`. On the unbound server, call `everett_inbox {"session_id":"victim","peek":true}`, then `everett_inbox {"session_id":"victim"}`, then `everett_card {"session_id":"victim", ...}`.

**Observed.** The caller read the victim's message, marked it delivered in `victim.done`, and wrote `cards/victim.md`. README and `docs/grok-bot.md` say a model-supplied `session_id` cannot establish identity.

**Fix.** `caller_session` in `everett-rs/src/mcp.rs` returns JSON-RPC `-32602` when `session_id` is supplied without a host binding. A matching `session_id` on a bound server still works.

**Regression tests.** `mcp_stdio::unbound_mcp_cannot_read_consume_or_overwrite_another_sessions_state`, plus updated `self_send_refused`, `unbound_mcp_cannot_deliver_or_reply_with_model_provenance`, and `audit::oversized_first_inbox_message_delivers_truncated` (now binds `EVERETT_SESSION_ID`).

**Behavior change.** A client that relied on passing `session_id` to an unbound server must now set `EVERETT_SESSION_ID` in the MCP launch environment, as the docs already require.

### M2: `grok-bot` and `openai-dot` messages were not labeled untrusted (Medium)

**Prerequisite.** A registered external harness sends or replies through bound `everett mcp` or `everett gateway`.

**Repro.** Register `ext-grok-bot` with `everett external add --harness grok-bot`. From that binding, reply to an owner handoff. Read the owner's inbox.

**Observed.** The owner received raw text with no `external`, `untrusted`, `trust`, or `instruction_source` field and no `[EXTERNAL / UNTRUSTED ...]` wrapper. Relay cross-org messages were already labeled. Only the logical external harnesses were missed.

**Fix.** `inbox::post` stamps the labels when `from_harness` is an external harness. `inbox::external` also derives provenance from `from_harness`, so messages queued before this fix are labeled on read. `persist_relay_context` keeps the labels when the recipient forwards derived content.

**Regression tests.** `external::previously_queued_external_messages_without_trust_flags_are_untrusted_on_read`, updated `external::stdio_mcp_queues_polls_and_replies_for_both_external_harnesses`, `gateway_native` stdio journey (recipient inbox and forward), and `relay_native` same-org and cross-org consumption through real `everett mcp` and `everett gateway` processes.

### M3: JSON-RPC ids `true`, `{}`, and `[]` were dispatched (Low)

**Repro.** Send `{"jsonrpc":"2.0","id":true,"method":"tools/call", ...}` to `everett mcp`.

**Observed.** The tool ran. JSON-RPC 2.0 allows only string, number, or null ids.

**Fix.** `mcp::handle` returns `-32600` with `id: null` and does not dispatch. The gateway already dropped these frames without dispatch, before and after the fix.

**Regression test.** `mcp_stdio::malformed_jsonrpc_ids_never_dispatch_tools`.

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

**Cause.** `relay:register` now requires the device ID to be a free display name, and `registry:named` returns revoked rows. `main` has no such coupling. Test: `discovery-attacks.test.ts`, case `name-squatting`.

### P23-2: registry names that the SSH CLI cannot pair (Informational, PR #23)

`registry:publish` accepts `.`, `..`, and names of 129–160 characters. `everett peer add` rejects them with exit 2 because `inbox::valid_id` allows 1–128 characters and excludes `.` and `..`. No traversal occurs.

### P25-1: anonymous callers can exhaust the enrollment budget (Low, PR #25)

**Repro.** Within one minute, call `hosted:begin` with fresh token hashes until `Enrollment rate limit` appears, then start a legitimate enrollment.

**Observed.** The legitimate enrollment failed until the next minute. `convex/hosted.ts` uses one global `hosted-enrollment` bucket of 30 per minute. Existing agents keep working. No owner, scope, revocation, issuer, or audience bypass was observed.

## Attacks that held

- **Auth.** Forged, tampered, expired, wrong-issuer, wrong-audience, and unsigned JWTs were rejected by real local JWKS verification. Client-supplied org, agent, and role fields were rejected or ignored. Non-owners could not approve, list, or revoke in PR #25. Revoked agent tokens, refresh, exchange, and re-registration were denied, including on a live WebSocket subscription.
- **Relay.** Cross-tenant row reads, deletes, and foreign acceptance failed. Absent or one-sided grants, inviter self-acceptance, and revoked grants blocked cross-org sends. Spoofed sender, modified signature or ciphertext, reused message IDs, and changed idempotency content were denied. Oversized (>64 KiB), malformed, and path-like envelopes were rejected. Per-grant and per-account rate limits held across two agents. Expired ciphertext and accepted ciphertext were deleted while replay tombstones remained.
- **Cross-device.** Three containers completed encrypted A → B → A (same org) and A → C → A (cross org, bilateral grant) through the PR #22 stack. The cross-org inbox was labeled untrusted.
- **SSH.** Two isolated Docker hosts with separate users, IPs, sshd, host keys, `HOME`, and `EVERETT_HOME` ran 19 checks on `main` and PR #24. Wrong host or device pins, forced-command escapes (`; id`, `$(...)`, newline, `peer serve --client attacker --setup`), unknown ops, `client` override fields, oversized frames, and pinned `known_hosts` replacement were all refused. Pairing, send, reply via `peer status`, and re-pairing worked. Only S1 failed.
- **MCP, Grok Bot, and dots.** Registration, queue-only delivery, refused resume and spawn, peek, take, and reply worked through `everett mcp` and `everett gateway` for both harnesses. A bound `session_id` spoof was rejected. The gateway exposed only `everett_whoami`, `everett_inbox`, `everett_card`, and `everett_send`, and rejected foreign destinations, foreign `reply_to`, ambiguous prefixes, and spawn. Registration IDs `../escaped`, `x/y`, `..`, `human`, `live`, `a.b`, `%2e%2e`, non-ASCII, and 101 characters were rejected without writing outside `~/.everett/external`.

## Test matrix

All runs used isolated `HOME` and `EVERETT_HOME` with `EVERETT_NOTIFY=none EVERETT_ROUTER=local CI=1`.

| Branch | `cargo test --locked` | `cargo clippy --all-targets -- -D warnings` | `npm test` | `npm run typecheck` | `release_journey` |
| --- | --- | --- | --- | --- | --- |
| `main` `435af2c` | pass | pass | pass, 34/34 | pass | pass |
| PR #22 `9ee4767` | pass | pass | pass, 35/35 | pass | pass |
| PR #23 `64328b8` | pass | pass | pass, 37/37 | pass | pass |
| PR #24 `d962ec0` | pass | pass | pass, 38/38 | pass | pass |
| PR #25 `d8f6a14` | pass | pass | pass, 46/46 | pass | pass |
| This branch | pass | pass | pass, 34/34 | pass | pass |

The MCP attack harness (74 checks) failed 10 checks on `main` and passed all 74 on this branch, with debug and release binaries.

## Not verified

- Real Grok Bot or ChatGPT dots cloud connections, Secure MCP Tunnel, and provider tool selection.
- Production Convex, real GitHub OAuth, and external OIDC issuers. JWT tests used a local issuer.
- The `doctor` → `relay:revokedDevices` → SSH prune path end to end against a live backend, and the 256-row revocation limit at scale.
- macOS hosts. All runs were Linux x86_64. Local Convex needed an Ubuntu 24.04 container because the host lacked GLIBC 2.38.
