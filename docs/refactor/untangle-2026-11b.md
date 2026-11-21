# Everett structural refactor decision log · November 2026, second pass

Base: `d8ed9726ddc1d9b02bdee32bd3f11a7d808af4fc` (GitHub main, Everett 2.4.0). The [first pass](untangle-2026-11.md) is already implemented. This pass uses [untangle-codebase](https://github.com/caldoai/skills/blob/main/skills/untangle-codebase/SKILL.md) with both optional additions: Comments (Remove comments that only restate the code.) and Tests (Remove tests that wouldn't catch a real bug, and fix the ones that fail without one.).

## Inventory and redesign

Paths starting with `src/` refer to `everett-rs/src/`. Inventory is against the base revision. Decisions describe the owners of changes and the behavior they must preserve.

1. **Give account sign-in and onboarding their own input types.** Keep flag-provider validation at the command boundary. Pass explicit sign-in options to device approval and explicit wizard options to onboarding. Keep handoff flag conflicts with the flag parser and preserve diagnostic wording and ordering.
   - Needless link: `src/cloudflare_auth.rs` takes every unrelated `cli::Args` field from callers and builds that bag from onboarding. JSON and revoke options seep into credential acquisition's interface.
   - Needless link: `src/onboard.rs`, `src/tui.rs` and `src/onboard_handoff.rs` inspect `cli::Args`. A new wizard flag must be checked independently against handoff's exclusion list. Preserve explicit vs default span days and the precedence of the preview-without-token message.
   - Surplus: `cmd_mcp`, `cmd_doctor` and `cmd_hook` forward directly to their real owners. The MCP launch harness is set through a global well before the only read in server startup. Preserve launch-harness fallback and existing identity validation.
2. **Have installers expose their hook catalog and own default selection.** Use their ordered settings-hook catalog for onboarding and request default hooks through the installer. Keep OMP and Pi extensions and the supported MCP/onboarding sets explicit where they differ.
   - Hidden link: `src/onboard.rs` repeats the settings-hook harness list in `src/install.rs`; handoff dissects installer tuples to simulate its default behavior.
   - Surplus: `src/cli.rs` tests each installer harness flag twice. Explicit flags determine the selection while presence determines the defaults. Default Hermes MCP remains opt-in.
3. **Keep auto-card and coverage decisions with cards.** Use the actual transcript adapter to determine fallback eligibility and put explicit regeneration beside its owner. Give coverage named counters and one construction path. Preserve quiet fallback, card freshness and edit protections. Keep hook refresh, backfill exclusivity and explicit regeneration as distinct write policies.
   - Hidden link: `src/cli.rs`, `src/onboard.rs`, and `src/hooks_common.rs` keep three lists for the same fallback formats.
   - Needless link: doctor and CLI know the positional `cards::Coverage` array including a permanently zero slot. `cards::apply` takes an unused clock argument.
4. **Put read-only session facts and transcript selection below their consumers.** Move the project matching key to the session domain, active T3 alias resolution to the read-only T3 adapter, and transcript file/tail reading to adapters.
   - Needless links: registry imports routing for a lowercase project key; control depends on wake to look up a thread and contains harness storage-specific reading code.
   - Hidden link: Grok discovery and control each know the chat-history file name. Preserve best-effort projections, strict dispatch binding, JSONL streaming, text bounds, Devin's unsupported SQLite result, and Hermes' SQLite reader.
   - Surplus: `adapters::t3code::annotate` has been replaced by overlay. A string wrapper for the control parser only serves its unit tests.
5. **Give launched-session history one owner.** Keep the existing append format and headless listing exemption.
   - Hidden link: `src/send.rs` appends `spawned.jsonl` with an `id` field that registry decodes independently.
6. **Keep T3 delivery metadata and its journal with the durable inbox.** Type the one binding stored when queueing and loaded on retry. Move journal mechanics below both mailbox compaction and dispatch, preserving the schema, reservation behavior, final preflight recheck, SQLite deadlines, lock order and fsync on retirement.
   - Hidden link: send hand-builds `t3_ping` and retry independently knows the field names.
   - Needless link: compaction calls the T3 wake workflow to prune journal rows while wake calls the inbox. Keep one-dispatch handling distinct from remote-start receipts.
7. **Make inbox selection and reveal explicit.** Share filters and the provenance/claim boundary across CLI and MCP and keep their respective response and hop policies. Derive public legacy and evidence-backed state from typed local observations. Persist relay provenance whenever a status actually reveals a reply.
   - Hidden links: CLI and MCP independently filter exact IDs and reply IDs, expose rows, and retain seen/claimed state; both disassemble reply JSON to remember provenance.
   - Special cases with independent requirements: preserve MCP's response bound before claim, exact pickup, relay-hop propagation, truthful pickup evidence, and the oversized reply recovery path.
8. **Define remote-start receipt diagnostics and MCP gateway capability names once.** Give replay and dispatch the same closed diagnostic vocabulary. Retain serialized names, fail-closed cache validation, legacy receipt upgrade, preflight, grant binding and the policy fingerprint. Share capability keys between native initialization and gateway compatibility negotiation.
   - Hidden links: `gateway/native_start.rs` separately enumerates cached diagnostic strings emitted by its transports; `gateway.rs` and `mcp.rs` duplicate compatibility keys.
9. **Keep Mesh policy beside the code enforcing it.** Centralize Worker route classification and each projection bound with its one-row lookahead. Use one active-grant condition and transition vocabulary. Share exact-origin and read-grant validation within the Node console's deployment root.
   - Hidden links: two Worker dispatchers repeat the Mesh path prefix; numeric limits are baked into query and presentation. Hosted grant states and loopback/token rules repeat across consumers.
   - Retain owner authorization, redirect restrictions, same account and active-credential filters, projection allowlists, separately deployed Worker/Node boundaries and local-only opt-ins.
10. **Use one docs-build traversal for fenced text and keep render state document-local.** Keep source-anchor and rendered-heading contracts distinct.
    - Hidden link: `site/build.mjs` duplicates fencing rules for headings and link rewriting. TOC code depends on module-wide renderer state set just before use.
    - Surplus: `allPagesFlat` only aliases `pages`. `MANIFEST.in` refers to the retired setuptools package and has no packaging consumer.
11. **Remove comments whose content follows directly from the implementation.** Keep comments that explain provider behavior, persisted formats, evidence, security, lock ordering and platform constraints.
12. **Run existing behavior checks at the real boundaries.** Do this after implementation. Include the Mesh Worker suite in the normal test command and give it the same public-auth fixture config. Make score scripts prove tests ran without freezing the count. In plugin archive tests assert required entries independently of the manifest builder, and make the T3 legacy fixture verify through `ls --json` while retaining the DB read-only check.
    - Hidden link and surplus: Mesh has a duplicate Vitest config, assertion-free copied helpers, and an independently hardcoded count. Plugin provenance may pass empty; its expected runtime list repeats the implementation its test should protect.
    - Special case: gateway event expiry test starts a short real-time deadline before the worker reaches the test gate. Make the readiness and deadline ordering explicit while retaining an expiry that actually occurs under lock.

13. **Share skill selection and search normalization below the callers.** Keep named and suggested selection explicit. Supply bounded skill bodies directly to hooks, and use typed lookup results for MCP usage tracking. Preserve native-skill suppression and named-only usage tracking.
    - Hidden and needless links: hook and tool flows repeat selection, hooks unpack MCP JSON just to read a body, and skill matching imports token normalization from routing.
14. **Place harness presence and UI state with their owners.** Share doctor/onboarding presence detection and derive the dashboard class and filter from the same status. Retain the installers' different default selection rules.
    - Needless link: onboarding imports doctor for detection. Dashboard repeats the branch order for attention and liveness.
15. **Tie acceptance manifest ownership to the run.** Keep creation exclusive and owner-only and retain inode, fsync, replacement and cleanup ordering.
    - Hidden link: the acceptance runner and its tests share a module-global path-to-inode map.
16. **Reuse identical checks across repeated delivery decisions.** Share the ping input check, the receiver rule checked both before and after prepare, and the captured-home inbox path. Remove a one-call pipe trait and pass-through formatting and card-exposure wrappers.
    - Retain receiver policy and trust rechecks on both sides of preparation and all transport-specific evidence. The proof workflow still owns its deadline and observation of a running resume child.

## Evidence to retain

- A normal notify retry is distinct from redispatch after an unconfirmed provider result. Never relax identity, account, exact target, remote lineage or prompt-promotion checks.
- Raw responses, legacy names and return codes are compatibility surfaces. Keep transport-specific evidence distinct from delivery or task completion.
- Some delegates process asynchronously and some files are created by external tools. Distinct process, platform, file format, feature availability and lock behavior remains deliberate.
- The requested Python discovery command on this base collects zero tests. Python test suites for plugins and operational scripts live in their own directories.

## Repeat-audit fixes

- Grok session rows carry a directory for activity tracking. Reuse the transcript path resolver when regenerating or backfilling cards so both read `chat_history.jsonl`. Hook events that already supply a file keep using it.
- The docs search index must recognize the same contours as headings and links. Use parsed Markdown heading tokens for both rendered anchors and search sections. Track fence character and length for source scans and link rewriting.
- Keep presence detection in installer code because config-only Devin detection depends on its install path. Reuse one dashboard status for filtering and CSS. Use a single isolated command builder for fixtures with and without stdin.
- Keep native-start journal decoding confined to the legacy string representation. Serde's enum representation may also accept tagged JSON objects, so validate the string shape explicitly.

## Verification

- Base confirmed with GitHub's `refs/heads/main` at `d8ed972` and crate version 2.4.0. The checkout is detached in the assigned worktree.
- `python3 -m unittest discover -s tests -v` exits 5 with 0 collected tests on both baseline and final. Current Python verification runs in `scripts/` and the plugin directories. Operational scripts passed 51 tests with the fixture Everett binary. Hosted, Hermes, and Local plugin suites passed 9, 8, and 11 tests.
- `npm run typecheck` and `npm test` in `cloudflare/` passed. The default test run now includes 9 Mesh cases for 101 total. Both Mesh score scripts reported `unmet=0`.
- Web Mesh passed 12 Node tests and syntax checks. The loopback journey exercised the Worker, console setup CLI and console handler through approval, receipt display, revocation and grant renewal. The Pi hook bridge passed 12 Node tests.
- The strict docs build generated 53 pages. Only the search index differs from the base. Tests against the renderer's production code verified that nested fences stay out and adjacent Setext/ATX duplicates get their own anchors.
- The baseline Rust run found a test counting the developer's own installed skills because its cwd escaped its temporary home. The fixture now runs inside that home. The first refactored full run had a gateway inbox HTTP timeout while other checks were running. The same case passed alone against both binaries and passed the next full run's gateway suite.
- Final `cargo test --locked` exited 0 with 778 tests passed and 0 failed across 62 executables and doc-test invocations. The count uses one summary per executable and includes the custom peer suite. The real CLI and all advertised stdio MCP tools passed the release journey.
- `cargo clippy --locked --all-targets -- -D warnings` and `git diff --check` passed. The final native-start journal regression failed against the pre-fix refactored binary and passed after constraining the decoder to strings.

The draft PR is based on `0f7eded`. Upstream already isolates the skills fixture working directory, so the PR keeps that behavior in the shared command builder and preserves the explicit working directory used by project-memory tests.

### Hill-climb checks

The local `hill-climb` scorer measured 10 targeted dependency edges on the base and 0 after the changes. The behavior guard compared 27 CLI and MCP probes against the baseline binary three times, with 0 mismatches and 0 spread. It scored 27 mismatches for an intentionally broken executable. The version probe checked the source version and revision and allowed the expected dirty build provenance difference. The packaged climb initializer writes the linked repository's Git exclude file, so its commit/reset workflow was outside the worktree boundary. The read-only score and output are in `.audit/untangle/`.

### Review ledger

Fresh read-only Codex reviewers covered Rust delivery/gateway, Rust runtime/setup, and non-Rust integrations. The first round found a cached receipt decoding regression, a Grok card path mismatch, and search anchor drift. Test diffs and compatibility checks accompany those fixes above. The runtime recheck cleared Grok, installer presence, dashboard filtering, and fixture environment findings. A second docs check found mismatched fence lengths and mixed Setext/ATX headings, now exercised against the renderer itself. The final recheck approved both the receipt shape and docs changes. It verified the index's H2/H3 anchor order against the 52 generated docs pages. A final read-only delivery pass approved T3 persistence, inbox claims, MCP compatibility and relay wake authorization.


### Remaining requirements

- Harness-specific parsers, setup capability subsets, root-bound authorization, verified inbox hops and distinct native, relay and hook receipts remain explicit. The prior pass already addressed general delivery classification. The proof operation retains its own dispatch deadline and the handle it uses to observe an outstanding resume process.
- The gateway's writer expiry test keeps a real SQLite transaction. The verification expiry case uses a controlled clock so it exercises the post-verification check without depending on scheduler speed.
- The earlier decision log and dependency locks are unchanged. Shipping tools, external identifiers, and production services were outside the authorized work.
