---
name: rerun-hardening-checks
description: Rerun the checks that stop Everett's repeated mistakes, and prove each one still fails on the historical bug. Use after touching test helpers, retry or delivery code, or private file writes, and before trusting a hardening check.
---
# Rerun hardening checks

Each check below was proved red on a real past mistake and green on `main`. A check that cannot be shown red is not trusted.

| Mistake (PR) | Prevents it | Rerun green | Rerun red (known-bad) |
| --- | --- | --- | --- |
| Test child inherits host `DEVIN_HOME`, lists real sessions (#230, #237, #241) | `common::fixture_command` clears the environment; `tests/fixture_isolation.rs` | `scripts/check-fixture-isolation.py` | `scripts/check-fixture-isolation.py --known-bad ebb13b2` |

## Rules

- Spawn every `everett` child from a test with `common::fixture_command(home)`. Add env on top of it; never start from `Command::new(CARGO_BIN_EXE_everett)`.
- The runner's red side must fail inside `ls_empty_home`. A build error or missing worktree is a failed proof, not a pass.
- Process-global `common::lib_home` is separate: it mutates this process's env and holds a lock.

## Limits

- Red runs rebuild the historical tree, so CI needs full history (`fetch-depth: 0`).
- The isolation check covers children spawned through `fixture_command`. Tests that build `Command` by hand are not caught until migrated.
