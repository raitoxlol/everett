# Everett 1.4.0

Everett now ships as a standalone Rust binary for macOS and Linux. No Python is needed to run it.

## Install or update

```bash
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh
everett doctor
```

`install.sh` installs or updates `everett` in `~/.local/bin` (override with `EVERETT_INSTALL_DIR`,
pin with `EVERETT_VERSION=v1.4.0`) and verifies the archive's SHA-256 before installing.
To build from source: `cargo install --git https://github.com/raitoxlol/everett everett`.

The Python package has been removed. After installing the binary, run `everett install-hooks --apply` and `everett install-mcp --apply` to replace Python registrations, then `pipx uninstall everett-sessions`.

## What changed

- **Rust binary releases:** `everett-<target>.tar.gz` + `.sha256` for macOS arm64/x86_64 and
  static Linux musl x86_64/aarch64, plus a rendered binary Homebrew formula (`everett.rb`).
- **Parity:** the Rust CLI and stdio MCP server expose the same commands and 10 tools as the
  Python implementation, including the ratatui onboarding wizard.
- **Devin CLI adapter:** `everett ls`, routing and `send` include Devin CLI sessions
  (`sessions.db` plus ATIF transcripts). `send` resumes with `devin --resume <id> --print`.
- **Linux:** the crate builds and tests on Linux. `doctor`'s MCP probe inherits the caller's
  environment, so it no longer writes `.everett/mcp.log` into the working directory.

## Verification

GitHub Actions runs `cargo test` on macOS and Linux before building the release targets.
The Rust end-to-end journey exercises all 10 MCP tools over isolated session stores.
Live model resumes and GUI client trust prompts remain separate checks.
