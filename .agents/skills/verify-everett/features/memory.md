# Shared memory

## Sub-features

Global/project learning, pending counts, deterministic merge, live core reads, durable BM25 search, temporary retrieval-only memory, exact forget/correct through local MCP.

## How to get to it (user POV)

`everett learn "fact"`, `everett trunk merge --llm none`, and MCP learn/core.

## Driving it with the release helper

Queue one global and one project fact through CLI/MCP. Both must be pending, then present in the shared core after merging; pending count becomes zero. Observe `core/core.md` and `core/projects/verification.md` under the disposable Everett home.

## Gotchas

No model is called. The unit suite separately tests late facts, failed merges and concurrent merges. Durable facts remain pending until a merge; injection into a live harness is hook-dependent. Temporary facts never enter merge or injected context.

`cargo test --locked --test mcp_stdio core_` keeps one MCP connection open across external CLI learn/merge, corrects and forgets active/pending facts, checks stored files after another merge, and rejects secret corrections. `cargo test --locked --test core` covers concurrent merge refusal and generated mirror updates. The gateway journey rejects mutation arguments. Historical backups are retained and are not merge inputs.

The stdio search journeys retain an early fact after the compact core evicts it, then correct/forget store-only facts and restart MCP. They assert source metadata, durable IDs, the stored rows, private permissions, scoping, limits, literal query terms, and secret exclusion. `failed_learning_does_not_publish_a_searchable_fact` checks rollback when pending-file persistence fails. `core_correction_can_be_retried_after_a_store_commit_failure` holds a real SQLite read transaction, proves a partial failed change, and finishes it with the same request after the lock is released. Legacy core-only and pending-only corrections separately survive store commit failure and core-file permission failure, then finish on retry. No history is imported into the new store.

`cargo test --locked --test mcp_stdio temporary_` exercises TTL receipts, literal search, merge exclusion, real SessionStart context, expiry through a controlled stored timestamp, expired correction/forget, duplicate refresh, explicit permanent promotion, and lifetime-collision rejection. Legacy active/pending permanent copies are checked before a temporary learning writes memory. Expiry filters retrieval; it does not erase retained rows or history.
