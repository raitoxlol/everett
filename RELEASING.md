# Publish Everett 1.3.0

These are maintainer commands to run after reviewing the prepared release. Preparation does
not push, tag or publish. Run from the release checkout with a clean working tree and authenticated
GitHub CLI. The public repository is `raitoxlol/everett`.

## 1. Verify and build

```bash
everett_repo="$(git rev-parse --show-toplevel)"
(
set -e
test "$(git -C "$everett_repo" branch --show-current)" = "everett/relaunch-1.3.0"
test -z "$(git -C "$everett_repo" status --porcelain)"
cd "$everett_repo/everett-rs"
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --locked
cargo build --release --locked
EVERETT_VERIFY_BINARY="$PWD/target/release/everett" cargo test --locked --test release_journey
)
```

## 2. Push the prepared branch and wait for CI

```bash
(
set -e
git -C "$everett_repo" push https://github.com/raitoxlol/everett.git HEAD:refs/heads/everett/relaunch-1.3.0
everett_run="$(gh run list --repo raitoxlol/everett --branch everett/relaunch-1.3.0 \
  --commit "$(git -C "$everett_repo" rev-parse HEAD)" --workflow ci.yml --limit 1 \
  --json databaseId --jq '.[0].databaseId')"
test -n "$everett_run" && test "$everett_run" != null
gh run watch "$everett_run" --repo raitoxlol/everett --exit-status
)
```

If GitHub has not created the run yet, repeat the run lookup when it appears. Proceed only
after all matrix jobs pass. If the main push below is rejected, inspect the new remote commits
before proceeding; use a normal fast-forward push.

## 3. Tag and publish

```bash
(
set -e
git -C "$everett_repo" push https://github.com/raitoxlol/everett.git HEAD:refs/heads/main
git -C "$everett_repo" tag -a v1.3.0 -m "Everett 1.3.0"
git -C "$everett_repo" push https://github.com/raitoxlol/everett.git refs/tags/v1.3.0
gh release create v1.3.0 --repo raitoxlol/everett --verify-tag --latest \
  --title "Everett 1.3.0" --notes-file "$everett_repo/RELEASE_NOTES.md"
)
```

## 4. Rust binaries and the binary Homebrew formula

Pushing the tag runs the `Rust binary` workflow (`.github/workflows/rust.yml`). It fails unless the
tag equals `v` + `everett-rs/Cargo.toml`'s version, so bump it first. After
`cargo test` passes on macOS and Linux, it builds the four targets and uploads
`everett-<target>.tar.gz`, `.sha256` and a rendered `everett.rb` to the release; it creates the
release from `RELEASE_NOTES.md` only if step 3 has not already. `install.sh` users get the new
binary on their next run.

Update the Homebrew tap with the rendered formula:

```bash
(
set -e
everett_tap="$(brew --repository raitoxlol/tap)"
gh release download vX.Y.Z --repo raitoxlol/everett --pattern everett.rb --dir "$everett_tap/Formula" --clobber
brew install raitoxlol/tap/everett && brew test raitoxlol/tap/everett
git -C "$everett_tap" commit -am "everett X.Y.Z (prebuilt binary)" && git -C "$everett_tap" push
)
```
