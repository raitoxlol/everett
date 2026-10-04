---
name: verify-everett
description: Verify Everett releases, CLI setup, and all ten stdio MCP tools using isolated session stores. Use after routing, messaging, status, memory, or packaging changes.
---
# Verify Everett

## Launch

Install the checkout into a disposable venv: `python3 -m venv /tmp/everett-check-venv`, then `/tmp/everett-check-venv/bin/python -m pip install .`. Use a fresh path if that venv already exists. Python 3.10+ and macOS/Linux are supported. Only Python 3.10 needs the small conditional `tomli` dependency.

The public surface is the CLI and `python -m everett mcp`. No long-running background server is needed. The helper starts its own MCP process, waits for initialize/tools/list, and closes it after the journey.

## Doctor

Run the helper first when setup is uncertain. Its first steps run console/module version checks and `everett doctor` in a disposable home. Doctor must receive exactly ten tools over stdio. The helper then onboards a detection-only Claude stub before any session store exists and checks the registered launcher.

## Drive

From the repository root:

```sh
python3 scripts/verify_release.py --python /tmp/everett-check-venv/bin/python --cli /tmp/everett-check-venv/bin/everett --evidence .audit/installed-journey.json
```

For source changes, use `python3 scripts/verify_release.py --source . --evidence .audit/source-journey.json`.

Read [the feature map](features/README.md). The helper runs all four mapped journeys with the real CLI and JSON-RPC pipes, and checks every advertised tool. It uses no model credentials or paid calls. Synthetic Claude/Codex transcripts stand in for external storage. The Claude stub is detection-only and exits 88 if invoked. A Python Codex stub simulates resume/spawn and reads stdin; it must receive EOF while the MCP connection stays open.

## Evidence

JSON includes commands, exit codes, stdout/stderr, protocol requests/responses, tool names, duration, and cleanup confirmation. Check `status: passed`, ten distinct `tools_exercised`, `delivery_checks`, and `scratch_cleaned: true`. Assertions check cards, inbox consumption, replies, status, core files, older targets and child stdin. This is CLI/protocol evidence; GUI-client trust prompts and actual model resumes need separate checks.

## Cleanup

The helper closes or kills only its own MCP subprocess and removes its own temporary home. Evidence is written outside that home and survives cleanup, including failed runs. Do not delete or operate on real harness stores. Keep any venv until its artifact checks finish; never remove an existing path belonging to another task.

## Helpers

`scripts/verify_release.py` is an executable standard-library control helper. `--python` selects the installed interpreter, `--cli` checks the console script, and `--source` explicitly selects checkout code. Omit `--source` for packaged-install proof. Run the unit suite separately: `python -m unittest discover -s tests -v`.
