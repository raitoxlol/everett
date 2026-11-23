---
name: rerun-hardening-checks
description: Rerun the checks that stop Everett's repeated mistakes, and prove each one still fails on the historical bug. Use after touching test helpers, retry or delivery code, or private file writes, and before trusting a hardening check.
---
# Rerun hardening checks

Each check below was proved red on a real past mistake and green on `main`. A check that cannot be shown red is not trusted.

| Mistake (PR) | Prevents it | Rerun green | Rerun red (known-bad) |
| --- | --- | --- | --- |
| Test child inherits host `DEVIN_HOME`, lists real sessions (#230, #237, #241) | `common::fixture_command` clears the environment; `tests/fixture_isolation.rs` | `scripts/check-fixture-isolation.py` | `scripts/check-fixture-isolation.py --known-bad ebb13b2` |
| T3 nudge sent twice on retry, in-flight window, or after an uncertain failure (#205) | `inbox/t3_dispatch/model.rs`: spec replayed against a real SQLite journal | `cargo test --lib t3_dispatch::model` | `scripts/check-dispatch-model.py` (mutates a copy back to each mistake; all must fail) |
| Dot event posted again after a refresh, a busy receipt write, or an uncommitted claim (#206) | `gateway_events_model.rs`: every status crossed with refresh and a writer-held database, then retries, against a real SQLite store | `cargo test --lib gateway_events::model` | `scripts/check-delivery-model.py` |

## Rules

- Spawn every `everett` child from a test with `common::fixture_command(home)`. Add env on top of it; never start from `Command::new(CARGO_BIN_EXE_everett)`.
- The runner's red side must fail inside `ls_empty_home`. A build error or missing worktree is a failed proof, not a pass.
- A mutant that survives, or fails to compile, is a failed proof. `check-dispatch-model.py` requires `test result: FAILED`.
- Process-global `common::lib_home` is separate: it mutates this process's env and holds a lock.

## Limits

- The model covers `dispatch_once` only, with a panic standing in for a process crash inside the dispatch window. It does not kill a real process, and it does not test the T3 CLI call itself.
- Dropping the `Some(None)` in-progress arm is not caught: the primary-key insert fails the same way. It is a redundant guard, not a gap.
- The delivery model retries by calling `deliver` directly: `leases()` also flushes receipts and hid a missing flush from the first draft of the model. Real polling that always goes through `leases()` makes that flush a second guard.
- Red runs rebuild the historical tree, so CI needs full history (`fetch-depth: 0`).
- The isolation check covers children spawned through `fixture_command`. Tests that build `Command` by hand are not caught until migrated.
