# Publish Everett 1.3.0

These are maintainer commands to run after reviewing the prepared release. Preparation does
not push, tag or publish. Run from the release checkout with a clean working tree and authenticated
GitHub CLI. The public repository is `raitoxlol/everett`; the package is not published on PyPI.

## 1. Verify and build

```bash
everett_repo="$(git rev-parse --show-toplevel)"
(
set -e
test "$(git -C "$everett_repo" branch --show-current)" = "everett/relaunch-1.3.0"
test -z "$(git -C "$everett_repo" status --porcelain)"
everett_build="$(mktemp -d "${TMPDIR:-/tmp}/everett-release.XXXXXX")"
python3 -m venv "$everett_build/venv"
"$everett_build/venv/bin/python" -m pip install "$everett_repo" build
"$everett_build/venv/bin/python" -m unittest discover -s "$everett_repo/tests" -v
"$everett_build/venv/bin/python" -m build "$everett_repo" --outdir "$everett_repo/dist"
)
```

Use the [verification helper](scripts/verify_release.py) against a fresh installed wheel,
with `--python` and `--cli` pointing into that venv and no `--source`. Preparation retains
local JSON evidence for wheel, sdist, pipx and staged Homebrew journeys under `.audit/`.

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
  --title "Everett 1.3.0" --notes-file "$everett_repo/RELEASE_NOTES.md" \
  "$everett_repo/dist/everett_sessions-1.3.0-py3-none-any.whl" \
  "$everett_repo/dist/everett_sessions-1.3.0.tar.gz"
)
```

## 4. Update the Homebrew tap

The archive URL exists only after the tag is published. This updates the existing recipe's
URL and checksum; Python 3.14 needs no runtime resources. Homebrew may upgrade installed
dependencies when testing the new formula.

```bash
(
set -e
brew tap raitoxlol/tap
everett_tap="$(brew --repository raitoxlol/tap)"
test -z "$(git -C "$everett_tap" status --porcelain)"
everett_archive="$(mktemp -t everett-1.3.0)"
curl -fL https://github.com/raitoxlol/everett/archive/refs/tags/v1.3.0.tar.gz -o "$everett_archive"
python3 - "$everett_tap/Formula/everett.rb" "$everett_archive" <<'PY'
import hashlib, re, sys
from pathlib import Path
formula, archive = map(Path, sys.argv[1:])
text = formula.read_text()
assert 'archive/refs/tags/v1.2.0.tar.gz' in text, 'Review the current tap before updating'
text = text.replace('archive/refs/tags/v1.2.0.tar.gz', 'archive/refs/tags/v1.3.0.tar.gz')
digest = hashlib.sha256(archive.read_bytes()).hexdigest()
text, count = re.subn(r'sha256 "[0-9a-f]{64}"', f'sha256 "{digest}"', text, count=1)
assert count == 1, 'No source checksum found'
formula.write_text(text)
PY
brew install --build-from-source raitoxlol/tap/everett
brew test raitoxlol/tap/everett
git -C "$everett_tap" diff -- Formula/everett.rb
git -C "$everett_tap" add Formula/everett.rb
git -C "$everett_tap" commit -m "everett 1.3.0"
git -C "$everett_tap" push
)
```

## 5. Rust binaries and the binary Homebrew formula

Pushing the tag runs the `Rust binary` workflow (`.github/workflows/rust.yml`). It fails unless the
tag equals `v` + `everett-rs/Cargo.toml`'s version, so bump that with `pyproject.toml`. After
`cargo test` passes on macOS and Linux, it builds the four targets and uploads
`everett-<target>.tar.gz`, `.sha256` and a rendered `everett.rb` to the release; it creates the
release from `RELEASE_NOTES.md` only if step 3 has not already. `install.sh` users get the new
binary on their next run.

To move Homebrew users onto the binary, replace the tap's Python formula with the rendered one:

```bash
(
set -e
everett_tap="$(brew --repository raitoxlol/tap)"
gh release download vX.Y.Z --repo raitoxlol/everett --pattern everett.rb --dir "$everett_tap/Formula" --clobber
brew install raitoxlol/tap/everett && brew test raitoxlol/tap/everett
git -C "$everett_tap" commit -am "everett X.Y.Z (prebuilt binary)" && git -C "$everett_tap" push
)
```
