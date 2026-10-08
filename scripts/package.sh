#!/usr/bin/env bash
# Package a release build for one target into dist/ (used by .github/workflows/release.yml).
#
#   scripts/package.sh <target>          e.g. aarch64-apple-darwin, x86_64-unknown-linux-gnu
#
# Expects `cargo build --release --locked --target <target> -p anchorleg` to have run. Writes:
#   dist/anchorleg-<target>.tar.gz          anchorleg-<target>/{anchorleg, README.md, LICENSE-*}
#   dist/anchorleg-<target>.tar.gz.sha256
#   dist/anchorleg_<version>_<arch>.deb     Linux only, with cargo-deb installed
set -uo pipefail
cd "$(dirname "$0")/.."

target="${1:?usage: scripts/package.sh <target>}"
bin="target/$target/release/anchorleg"
[[ -x "$bin" ]] || { echo "no $bin; build it first" >&2; exit 1; }

name="anchorleg-$target"
stage="$(mktemp -d)"
mkdir -p dist "$stage/$name"
cp "$bin" README.md LICENSE-MIT LICENSE-APACHE "$stage/$name/" || exit 1
tar -C "$stage" -czf "dist/$name.tar.gz" "$name" || exit 1
rm -rf "$stage"
(cd dist && shasum -a 256 "$name.tar.gz" >"$name.tar.gz.sha256") || exit 1
echo "dist/$name.tar.gz"

if [[ "$target" == *-linux-* ]] && command -v cargo-deb >/dev/null; then
  deb="$(cargo deb -p anchorleg --target "$target" --no-build --no-strip --output dist 2>&1 | tail -1)" || {
    echo "cargo deb failed: $deb" >&2
    exit 1
  }
  echo "$deb"
fi
