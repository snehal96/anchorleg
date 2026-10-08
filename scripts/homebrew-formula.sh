#!/usr/bin/env bash
# Print the Homebrew formula for a release (used by .github/workflows/release.yml).
#
#   scripts/homebrew-formula.sh <version> <dir with anchorleg-<target>.tar.gz.sha256 files>
#
# The formula installs the prebuilt binaries from the GitHub release: macOS (Apple silicon and
# Intel) and Linux (arm64 and x86_64).
set -uo pipefail

version="${1:?usage: scripts/homebrew-formula.sh <version> <sha256 dir>}"
dir="${2:?usage: scripts/homebrew-formula.sh <version> <sha256 dir>}"
version="${version#v}"
repo="https://github.com/snehal96/anchorleg"

sum() {
  local f="$dir/anchorleg-$1.tar.gz.sha256"
  [[ -f "$f" ]] || { echo "missing $f" >&2; exit 1; }
  cut -d' ' -f1 <"$f"
}
# Fail before printing anything if a target is missing.
for t in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
  [[ -f "$dir/anchorleg-$t.tar.gz.sha256" ]] || { echo "missing $dir/anchorleg-$t.tar.gz.sha256" >&2; exit 1; }
done
asset() {
  printf '      url "%s/releases/download/v%s/anchorleg-%s.tar.gz"\n      sha256 "%s"\n' \
    "$repo" "$version" "$1" "$(sum "$1")"
}

cat <<RUBY
class Anchorleg < Formula
  desc "Keeps headless coding agents working when an account hits its limit"
  homepage "$repo"
  version "$version"
  license any_of: ["MIT", "Apache-2.0"]

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
$(asset aarch64-unknown-linux-gnu)
    end
    on_intel do
$(asset x86_64-unknown-linux-gnu)
    end
  end

  def install
    bin.install "anchorleg"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/anchorleg --version")
  end
end
RUBY
