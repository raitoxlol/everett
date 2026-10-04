# Shared memory

## Sub-features

Global/project learning, pending counts, deterministic merge, core reads.

## How to get to it (user POV)

`everett learn "fact"`, `everett trunk merge --llm none`, and MCP learn/core.

## Driving it with the release helper

Queue one global and one project fact through CLI/MCP. Both must be pending, then present in the shared core after merging; pending count becomes zero. Observe `core/core.md` and `core/projects/verification.md` under the disposable Everett home.

## Gotchas

No model is called. The unit suite separately tests late facts, failed merges and concurrent merges. Facts remain pending until a merge; injection into a live harness is hook-dependent.
