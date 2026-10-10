# Everett 1.5.0

Everett is now Rust only. The Python package is gone; the `everett` binary carries every command.

## Install or update

```bash
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh
everett install-hooks --apply && everett install-mcp --apply
everett doctor
```

`install.sh` installs or updates `everett` in `~/.local/bin` (override with `EVERETT_INSTALL_DIR`,
pin with `EVERETT_VERSION=v1.5.0`) and verifies the archive's SHA-256 before installing.
To build from source: `cargo install --git https://github.com/raitoxlol/everett everett`.

**Upgrading from pipx:** `install-hooks --apply` and `install-mcp --apply` back up your config and
point the old Python hooks, `-m everett mcp` launchers and the OMP extension at the native binary.
Then `pipx uninstall everett-sessions`. `pipx install` no longer works.

## What changed

- **Breaking:** the Python package, `pyproject.toml`, `bin/everett` and the Python tests are removed.
  Its test coverage is ported to `everett-rs/tests/`; CI runs the release journey against a release build.
- **New harnesses:** current T3 Code projections (inbox-only continuation, no fake hook pickup),
  Devin CLI MCP registration via `install-mcp --devin`, and OpenAI dots / Grok Bot as durable external
  sessions (`everett external add|list|remove`).
- **`everett gateway`:** owner-bound MCP gateway for dots and Grok Bot with a fixed identity, explicit
  destination grants, private stdio or bearer-authenticated loopback HTTP, and exact-ID-only delivery.
  No automatic wake-ups or cross-device transport.
- **`everett dashboard`:** read-only, loopback-only local HTML dashboard with recent sessions,
  Everett cards, filters and copyable CLI actions.
- **`everett login` / `whoami` / `logout`:** optional OIDC device-code sign-in (`docs/auth.md`).
- **Audit hardening:** stale hook/MCP detection and repair, per-session inbox locking and compaction,
  atomic config/subscription writes, MCP panic containment, full-mtime auto-card freshness, hop-limit
  enforcement for live sessions, and many adapter parity fixes. See `CHANGELOG.md` for the full list.

## Verification

GitHub Actions runs clippy (`-D warnings`), `cargo test` and the release journey (all 10 MCP tools over
isolated stores, no Python on `PATH`) on macOS and Linux before building the release targets.
Live dots/Grok connections, real T3 hook pickup and GUI client trust prompts remain owner checks.
