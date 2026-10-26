# Shared memory

## Sub-features

Global/project learning, pending counts, deterministic merge, live core reads, exact forget/correct through local MCP.

## How to get to it (user POV)

`everett learn "fact"`, `everett trunk merge --llm none`, and MCP learn/core.

## Driving it with the release helper

Queue one global and one project fact through CLI/MCP. Both must be pending, then present in the shared core after merging; pending count becomes zero. Observe `core/core.md` and `core/projects/verification.md` under the disposable Everett home.

## Gotchas

No model is called. The unit suite separately tests late facts, failed merges and concurrent merges. Facts remain pending until a merge; injection into a live harness is hook-dependent.

`cargo test --locked --test mcp_stdio core_` keeps one MCP connection open across external CLI learn/merge, corrects and forgets active/pending facts, checks stored files after another merge, and rejects secret corrections. `cargo test --locked --test core` covers concurrent merge refusal and generated mirror updates. The gateway journey rejects mutation arguments. Historical backups are retained and are not merge inputs.
