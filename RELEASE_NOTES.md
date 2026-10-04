# Everett 1.3.0

Route requests, messages, and shared memory across your coding-agent sessions—from a CLI or MCP client.

This relaunch focuses on first-run setup, useful diagnosis, and reliable memory updates.
Everett is MIT licensed and supports Claude Code, Codex, OMP, Pi, Hermes Agent, and Grok CLI.

## Get started

Have Python 3.10+, pipx and at least one harness CLI installed on macOS or Linux:

```bash
pipx install git+https://github.com/raitoxlol/everett
everett onboard --yes --no-backfill
everett doctor
```

Restart configured harness clients after onboarding. Start a session and run `everett ls`.
No router key or model call is needed for setup. The package is not published on PyPI.

## What changed

- **First-run setup:** detect installed CLIs before any session exists; include Grok MCP; cancel safely on EOF; explain missing harnesses before waiting.
- **MCP diagnosis:** doctor checks actual stdio initialization and all 10 tools, then probes registered launchers. Stale or disabled registrations get exact repair commands with backups.
- **Memory reliability:** new facts stay pending while a merge is running, simultaneous mergers are refused, and failed merges retain queued facts. Use `everett trunk merge --llm none` for a local deterministic merge.
- **Protocol and logging:** validate advertised tool arguments, declare route caller identity, and keep argument values out of new MCP log entries.
- **Release checks:** isolated routing, two-way messages, session status and shared memory journeys; a macOS/Linux CI matrix; fresh wheel verification; a shorter quickstart and troubleshooting guide.

The server advertises 10 tools: `everett_ls`, `everett_route`, `everett_send`, `everett_inbox`,
`everett_event`, `everett_subscribe`, `everett_learn`, `everett_core`, `everett_card`, and `everett_whoami`.

## Upgrade

```bash
pipx upgrade everett-sessions
everett doctor
```

If doctor identifies a stale Claude registration:

```bash
everett install-mcp --claude --repair --apply
```

Use `--codex`, `--omp` or `--grok` for that client, then restart/enable its server. Old log entries
are preserved; only new entries omit argument values. No stored sessions or features are removed.
Python 3.10 gets the small `tomli` dependency; Python 3.11+ uses the standard library at runtime.

## Practical limits

Pi and Hermes need manual MCP configuration and inbox polling. Hook injection requires client
support and enabled/trusted hooks; a queued message is not a read receipt. T3 Code uses underlying
harness hooks for inbox delivery and cannot be resumed through Everett's CLI. Automatic launchd
scheduling and default desktop notifications require macOS; manual merging works on Linux.

Local routing needs no network. Jev routing and harness resumes/LLM merges use their configured
providers. Fresh-install verification uses synthetic stores and no paid model calls; it does not
claim live model-resume or GUI hook/trust verification for every harness.

## Verification

275 tests pass on macOS with Python 3.10 and 3.14. Fresh Python 3.10/3.14 wheel installs,
a source-distribution install, pipx, and a locally staged Homebrew recipe all pass the real CLI
and 10-tool stdio journey. The local source-package pipx install → onboarding → doctor took
10.0 seconds with Python, pipx and a detection-only harness stub already available.

The public Homebrew tap still points to 1.2.0 during preparation; it needs the published 1.3.0
tag/archive before updating. GitHub Actions has been added and will run after the prepared
commits are pushed. Live model resumes and GUI client trust prompts remain separate checks.
