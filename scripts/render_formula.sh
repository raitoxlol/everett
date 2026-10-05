#!/bin/sh
# Print the Homebrew formula for the prebuilt binaries: render_formula.sh VERSION DIST_DIR
set -eu
version="$1"
dist="$2"
base="https://github.com/raitoxlol/everett/releases/download/v$version"

sha() { cut -d' ' -f1 "$dist/everett-$1.tar.gz.sha256"; }

asset() {
  printf '      url "%s/everett-%s.tar.gz"\n' "$base" "$1"
  printf '      sha256 "%s"\n' "$(sha "$1")"
}

cat <<RUBY
class Everett < Formula
  desc "Layer above every coding-agent session on your machine"
  homepage "https://github.com/raitoxlol/everett"
  version "$version"
  license "MIT"

  on_macos do
    on_arm do
$(asset aarch64-apple-darwin)
    end
    on_intel do
$(asset x86_64-apple-darwin)
    end
  end

  on_linux do
    on_arm do
$(asset aarch64-unknown-linux-musl)
    end
    on_intel do
$(asset x86_64-unknown-linux-musl)
    end
  end

  def install
    bin.install "everett"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/everett --version")
  end
end
RUBY
