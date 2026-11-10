# Releasing Everett

Everett is one Rust binary (`everett-rs/`). Release = a `vX.Y.Z` git tag; CI
builds everything else. Run these steps from `main` with a clean tree and an
authenticated GitHub CLI. The public repository is `raitoxlol/everett`; nothing
is published to PyPI.

Prepare the candidate and evidence locally before requesting authorization to
publish. A tag push starts public release automation. Treat a Worker deployment,
receiver installation/restart, Homebrew push, and announcement as separate
external actions. Follow [the rollout procedure](docs/cloudflare-rollout.md)
for service inventory, canary checks, and rollback.

## 1. Pick the version

The tag **must** match `everett-rs/Cargo.toml` (`version = "X.Y.Z"`); the release
job fails on a mismatch. Bump the version in `everett-rs/Cargo.toml`, refresh
`Cargo.lock` (`cargo check` does it), and update `CHANGELOG.md` (move the release
notes out of Unreleased). Commit as the version bump.

## 2. Verify before tagging

```bash
cd everett-rs
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
EVERETT_VERIFY_BINARY="$PWD/target/release/everett" cargo test --locked --test release_journey
```

Run this on both macOS and a healthy Linux runner. A passing macOS run does not
clear the Linux gate. Keep logs with the exact source revision and binary hash.
From the repository root, also run:

```sh
npm --prefix cloudflare ci
npm --prefix cloudflare run typecheck
npm --prefix cloudflare test
npm --prefix cloudflare run build:public
python3 -m unittest discover -s scripts -p 'test_*.py' -v
python3 -m unittest discover -s plugins/everett-claude/tests -p 'test_*.py' -v
node --test everett-rs/tests/omp_session_start.test.mjs
EVERETT_VERIFY_BINARY="$PWD/everett-rs/target/release/everett" python3 -m unittest discover -s plugins/everett-local/tests -v
python3 scripts/package_local_plugin.py --output dist
```

Sanity-check the optimized binary with `--version` and `relay diagnose` in a
disposable home. The release journey exercises onboarding and doctor in an
isolated home, plus all 13 stdio MCP tools. It simulates harness resumes; actual
provider pickup and a correlated reply require separate live acceptance.

## 3. Write RELEASE_NOTES.md

`RELEASE_NOTES.md` at the repo root becomes the GitHub release body
(`gh release create --notes-file`). Summarize user-facing changes; point at the
CHANGELOG for the full list.

## 4. Tag and push

```bash
git tag -a "v$(sed -n 's/^version = \"\(.*\)\"/\1/p' everett-rs/Cargo.toml | head -1)" -m "Everett vX.Y.Z"
git push origin main --follow-tags
```

## 5. What CI then does (`.github/workflows/rust.yml`)

- `cargo clippy --all-targets -- -D warnings` and `cargo test` on Linux + macOS.
- Waits for the reusable Cloudflare workflow's tests, typecheck, and public
  dry-run bundle before building tagged artifacts. A successful dry run does
  not deploy the Worker.
- Builds release binaries for `x86_64-apple-darwin`, `aarch64-apple-darwin`,
  `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, packages each as
  `everett-<target>.tar.gz` + `.sha256`.
- Tests each optimized binary through the CLI/MCP release journey on a matching
  runner architecture. Other architectures are cross-built and checksum verified;
  they require a matching host for execution evidence.
- Renders `dist/everett.rb` (Homebrew formula) via `scripts/render_formula.sh`
  and packages the separate `everett-local-0.3.0.zip` installer plus checksum.
  Uploads everything to the `vX.Y.Z` GitHub release (creating the release
  from `RELEASE_NOTES.md` if step 3 has not already).

Check all tagged workflows and required reviews before calling the release
verified. Re-download published archives, verify their checksums, and run the
downloaded native binaries through the release journey on macOS and Linux.

## 6. Homebrew tap

Update the tap with the rendered formula:

```bash
(
set -e
everett_tap="$(brew --repository raitoxlol/tap)"
gh release download vX.Y.Z --repo raitoxlol/everett --pattern everett.rb --dir "$everett_tap/Formula" --clobber
brew install raitoxlol/tap/everett && brew test raitoxlol/tap/everett
git -C "$everett_tap" commit -am "everett X.Y.Z (prebuilt binary)" && git -C "$everett_tap" push
)
```

The tap lags the tag by however long that step takes — `install.sh` does not.

## 7. Verify the install path

```bash
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh
everett --version   # should print X.Y.Z
brew tap raitoxlol/tap && brew install everett   # once the tap is updated
```

`install.sh` verifies the `.sha256` checksum; `EVERETT_VERSION=vX.Y.Z` pins a
release, `EVERETT_INSTALL_DIR` overrides `~/.local/bin`.
