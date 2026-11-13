# Everett structural refactor decision log

Requested workflow: [untangle-codebase](https://github.com/caldoai/skills/blob/main/skills/untangle-codebase/SKILL.md).
Base: `f0f23563f30df729305c02c869c2339750db2074`. The user selected both optional comment and test cleanup. No deployment, credential changes, new dependencies, or changes in other worktrees are authorized.

## Inventory and redesign

Each entry puts the intended owner above the observed coupling. All accepted entries are implemented; verification and repeat-review evidence follow below.

1. **Put generic private storage below transport domains.**
   `peer.rs:92–147` owns directory creation, atomic replacement and file locking used by inbox, receiver, relay, Cloudflare login, MCP and systemd scheduling. Move the existing primitives and migrate all callers. Preserve error codes/text, permissions, blocking locks and exact fsync ordering. **Implemented.**
2. **Give shared nudge command context a transport-neutral owner.**
   `claude_wake.rs:477–522` owns executable/profile/provider resolution and generated inbox/reply commands used by Claude, T3 and OMP. Move shared command context; keep each transport's prompt and validation with that transport. **Implemented.**
3. **Make delivery classification a shared typed decision.**
   `send.rs:679–795` and `cli.rs:651–692` repeat T3/OMP/attachment/headless transport selection. Use one classification owner for dispatch and previews; preserve preflight-before-enqueue, late transport validation, uncertain-dispatch retention and distinct receipt meanings. **Implemented.**
4. **Make external registration explicitly root-bound.**
   `gateway/identities.rs` reconstructs registration paths under captured home while `adapters/external.rs` writes through global home. Put lookup/register/remove and path rules behind one root-bound store. Preserve noclobber creation, retry repair, revocation durability and cleanup retries. No runtime home mismatch was reproduced. **Implemented.**
5. **Put T3 CLI mechanics below control and wake workflows.**
   `control.rs:23–52` depends on discovery/parser code in `t3_wake.rs`, while both invoke the CLI and interpret receipts. One adapter should own discovery/capture/JSON extraction. Preserve control exit-2 handling versus nudge exit-zero-only handling, timeout caps and error wording. **Implemented.**
6. **Give harness identity and shared capabilities one authority.**
   Native names, spawnable lists and scan/binary associations repeat across `main.rs`, `send.rs`, `registry.rs`, `host.rs` and setup. Centralize facts that are actually identical; keep process-detectable, external, setup, extension and preview-only capabilities distinct. **Implemented.**
7. **Resolve Jev credentials once, with their source.**
   `route.rs:29` and `onboard.rs:153` repeat precedence; `doctor.rs:356–358` independently obtains value and source through those domains. Put resolution in config and let consumers format its typed source. Decide the existing empty Hermes assignment discrepancy explicitly. **Implemented.**
8. **Put common harness subprocess protocol below messaging and memory.**
   `core.rs:21,685–727` uses messaging-owned environment/command helpers and repeats Codex output-file handling. Extract shared execution mechanics while preserving caller-specific purpose, timeout/errors, isolated merge cwd, runner injection, caller binding and secret checks. **Implemented.**
9. **Put Pi base-format parsing below the OMP extension.**
   `adapters/pi.rs:3` imports its base-format reader from `omp.rs:34–90`. Reverse that ownership; retain provider scan entrypoints and distinct roots/labels. **Implemented.**
10. **Put card coverage with the card domain.**
    `doctor.rs:342` depends on `cli::card_coverage` at `cli.rs:183`. Move the shared summary out of presentation without changing output/order/counting. **Implemented.**
11. **Put relay operation/result contracts below HTTP wiring.**
    `owner-relay.ts:3` imports deployment `Env` and `Operation` from `index.ts`; the env is only forwarded to `super`. Move operation/result vocabulary to the protocol owner and remove the needless deployment-specific generic. These are type-only edges, not a runtime cycle. **Implemented.**
12. **Return the typed relay result over internal DO RPC.**
    `owner-relay.ts:194–212` serializes a result which `index.ts:73` and `test/relay.worker.ts:44` immediately parse. Return the result directly; keep external HTTP JSON and error/status mapping unchanged. This internal RPC change requires coordinated Worker/DO code update on any future deployment. **Implemented.**
13. **Use one auth-route predicate.**
    `index.ts:39–40` repeats the unused exported `hosted-config.ts:65` predicate. Consume its authoritative definition. **Implemented.**
14. **Share bounded-stream mechanics, not timeout policy.**
    `auth.ts:26` and `hosted-auth.ts:31` duplicate byte caps, fatal UTF-8 decoding and reader cleanup. A transport helper can own these mechanics while preserving auth's five-second deadline and relay's existing no-deadline behavior. **Implemented.**
15. **Remove duplicated hop policy and independently maintained tool counts.**
    `send::MAX_HOPS` duplicates `inbox::MAX_HOPS` only for test callers. Plugin runtime smoke tests hardcode fourteen while verification docs still say thirteen. Use the actual policy/capability owner; retain the distinct wire ceiling of 64. **Implemented.**
16. **Repair the false-positive timezone test in the final cleanup pass.**
    `timefmt.rs:86` invokes a short name with `--exact`; the actual name is qualified. The original command executed zero tests and exited successfully. Keep timezone subprocess isolation, fix the exact name, and assert that the child test executed. **Implemented.**

## Boundaries to retain

- Remote lineage remains durable/monotonic and remote-derived callers stay inbox-only.
- T3 owns provider resume points. OMP watcher acceptance, Claude native prompt submission, provider replies and inbox consumption remain different observations.
- Claude version/process/session/cwd checks and scoped PTY cleanup remain transport requirements.
- Webhook retry/backoff and one-attempt native wake scheduling implement different external contracts.
- Shared SSH/Cloudflare alias locking prevents shadowing; do not split that ownership lock.
- Core's nonblocking merge lock and credential-specific stronger fsync guarantees differ from generic storage.
- Distinct harness formats and setup capabilities remain explicit; there is no universal supported-harness list.
- OAuth account scope, allowlists, PKCE, token revocation, destination grants and private chat handles remain unchanged.
- Relay namespace isolation, tombstones and post-transaction notification ordering remain intact.
- Shared Rust/TypeScript signing fixtures already check the wire contract; do not add a schema generator.
- Public CLI/MCP interfaces and intended behavior remain stable. This is structural work, not a new-feature or general bug-hunt pass.

## Candidates not yet promoted to implementation

The broad `cli::Args` bag couples command vocabularies. It needs a demonstrated workflow seam before changing every command signature; extracting an equally broad bag to another file would not fix it. Revisit after the confirmed domain dependencies above are removed.

Thin Pi/provider adapters, pagination wrappers, explicit binary overrides and portable-plugin opt-in have actual domain purposes. They are not surplus merely because they are short.

## Verification record

- Initial inventory: three read-only subagents covering Rust messaging, remaining Rust domains, and Cloudflare/plugins/scripts.
- Dependency inventory: `docs/refactor/structure_inventory.py` produces approximate local import references and cycles, explicitly including type-only/test references. It is an inspection aid, not proof of runtime cycles.
- Baseline and per-unit results will be recorded as checks finish.

## Follow-up audit

The read-only repeat audit found three worthwhile finishes: keep Codex output-file selection in execution as well as its read/cleanup; move the native process matcher beside harness facts and remove its forwarding wrapper; prove the memory caller reads Codex file output over conflicting stdout. All three are implemented. The catalog exposes an exhaustive native enum to adapters, so stable names do not import their scanners. Provider subprocess arguments, process-detectable subsets, setup stores, liveness windows and authorization remain domain-specific.

Comment cleanup removes implementation-restating section banners, stale OMP detection wording, a misplaced setter comment and redundant step descriptions. Retained comments explain external formats, security boundaries, durability and platform constraints. Test cleanup removes the plugin's independent tool-count assertion, derives the journey count from its required capability list, and repairs the timezone subprocess's false success. No security/provider regression was removed.

The broad CLI argument bag was reconsidered after extraction. Its remaining consumers are presentation workflows, so changing every command signature is not justified by a demonstrated independent-change seam in this pass.

The repeat Rust reviews report no remaining worthwhile findings after those finishes. The Cloudflare/integration repeat review likewise found no remaining structural or comment cleanup work; its tool-count cleanup findings were completed afterward.

Reproduce the import inventory with `python3 docs/refactor/structure_inventory.py . --output /tmp/everett-structure.json`. It observed peer references fall from 12 callers to 7 and send references from 5 to 3; test/type references count, so these are inspection counts rather than runtime coupling metrics.

Current verification: baseline Rust `cargo test` exited 0 with 597 top-level tests passed; the first refactored full run exited 0 with 599 top-level tests passed. The final memory regression adds one test. Cloudflare typecheck, 89 tests, owner/public dry-run bundles, plugin runtime tests (8), receiver acceptance automation tests (15), and strict Clippy passed. Final `cargo test` exited 0: 600 top-level tests passed, 0 failed, 0 ignored across 48 test executables/doc-test invocations. The repaired timezone case reports `test timefmt::tests::naive_iso_uses_local_time_like_python ... ok`. `cargo build --release --locked` exited 0. Against that release binary, the CLI/MCP release journey passed its one test, relay diagnostics passed five tests, and plugin installation/runtime verification passed eight tests.

Rust totals count the final summary of each Cargo test executable once. Several tests re-execute test binaries and print nested summaries; summing every printed summary would double-count those checks. No ignored tests were reported.

## Sol review ledger

Round 1 used `codex_pro`, `gpt-6.1-sol`, high reasoning effort. Verdict: **APPROVE**, no actionable findings; no fixes or declined findings. Usage was not returned. The review covered the base diff and all relevant untracked source. Sol independently reran Rust library tests and send/external/control/T3/core/release/onboarding/gateway suites, Cloudflare typecheck and tests, and diff whitespace checks. It did not independently rerun the full Rust suite, strict Clippy or packaged artifact checks. The primary ran those checks as recorded above. No live provider or hosted ChatGPT delivery was exercised.
