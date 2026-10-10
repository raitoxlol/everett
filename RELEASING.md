# Releasing Everett

The Rust binary (`everett-rs/`) is the primary install; the Python package is the
reference implementation and ships in the same tag. Release = a `vX.Y.Z` git tag;
CI builds everything else. Run these steps from `main` with a clean tree and an
authenticated GitHub CLI. The public repository is `raitoxlol/everett`; nothing
is published to PyPI.

## 1. Pick the version

The tag **must** match `everett-rs/Cargo.toml` (`version = "X.Y.Z"`); the release
job fails on a mismatch. Bump the version in `everett-rs/Cargo.toml`, refresh
`Cargo.lock` (`cargo check` does it), and update `CHANGELOG.md` (move the release
notes out of Unreleased). Commit as the version bump.

## 2. Verify before tagging

```bash
python3 -m unittest discover -s tests
cargo test --manifest-path everett-rs/Cargo.toml
cargo clippy --manifest-path everett-rs/Cargo.toml --all-targets -- -D warnings
```

Sanity-check the binary: `cargo run -p everett -- --version`, `everett doctor`.

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
- Builds release binaries for `x86_64-apple-darwin`, `aarch64-apple-darwin`,
  `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, packages each as
  `everett-<target>.tar.gz` + `.sha256`.
- Renders `dist/everett.rb` (Homebrew formula) via `scripts/render_formula.sh`
  and uploads everything to the `vX.Y.Z` GitHub release.

Watch it: `gh run watch --repo raitoxlol/everett`.

## 6. Homebrew tap

Copy the generated `everett.rb` from the release assets into the
`raitoxlol/homebrew-tap` repository (`Formula/everett.rb`) and commit. The tap
lags the tag by however long that step takes — `install.sh` does not.

## 7. Verify the install path

```bash
curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh
everett --version   # should print X.Y.Z
brew tap raitoxlol/tap && brew install everett   # once the tap is updated
```

`install.sh` verifies the `.sha256` checksum; `EVERETT_VERSION=vX.Y.Z` pins a
release, `EVERETT_INSTALL_DIR` overrides `~/.local/bin`.

## 8. Python reference package

The Python package builds as part of `ci.yml` on every push (wheel + sdist
artifacts, installed-journey proof via `scripts/verify_release.py`). It is the
reference implementation — it is not published to PyPI and needs no release
step of its own; it rides the same tag.
