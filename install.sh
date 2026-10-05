#!/bin/sh
# Install or update the prebuilt everett binary from GitHub releases.
#   curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh
# EVERETT_VERSION=v1.4.0 pins a release; EVERETT_INSTALL_DIR overrides ~/.local/bin.
set -eu

repo="raitoxlol/everett"
version="${EVERETT_VERSION:-latest}"
dir="${EVERETT_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
  *) echo "everett: no prebuilt binary for $(uname -s) $(uname -m); use: cargo install --git https://github.com/$repo everett" >&2; exit 1 ;;
esac

if [ "$version" = latest ]; then
  base="https://github.com/$repo/releases/latest/download"
else
  base="https://github.com/$repo/releases/download/$version"
fi
archive="everett-$target.tar.gz"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$base/$archive" -o "$tmp/$archive"
curl -fsSL "$base/$archive.sha256" -o "$tmp/$archive.sha256"
(cd "$tmp" && if command -v sha256sum >/dev/null 2>&1; then sha256sum -c "$archive.sha256"; else shasum -a 256 -c "$archive.sha256"; fi) >/dev/null

tar -xzf "$tmp/$archive" -C "$tmp"
mkdir -p "$dir"
install -m 755 "$tmp/everett-$target/everett" "$dir/everett"
echo "installed $("$dir/everett" --version) to $dir/everett"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "add $dir to your PATH" ;;
esac
