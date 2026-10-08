#!/usr/bin/env bash
# Install anchorleg from its GitHub release:
#
#   curl -fsSL https://raw.githubusercontent.com/snehal96/anchorleg/main/install.sh | bash
#
# Picks the build for this machine (macOS or Linux, arm64 or x86_64), checks its SHA-256 and
# puts `anchorleg` in ~/.local/bin. Settings, all optional:
#   ANCHORLEG_VERSION      a release tag such as v0.1.0 (default: the latest release)
#   ANCHORLEG_INSTALL_DIR  where the binary goes (default: ~/.local/bin)
#   ANCHORLEG_BASE_URL     where releases are downloaded from (default: GitHub)
set -uo pipefail

repo="https://github.com/snehal96/anchorleg"
dest="${ANCHORLEG_INSTALL_DIR:-$HOME/.local/bin}"
version="${ANCHORLEG_VERSION:-latest}"

fail() {
  echo "anchorleg install: $*" >&2
  exit 1
}

case "$(uname -s)" in
  Darwin) os=apple-darwin ;;
  Linux) os=unknown-linux-gnu ;;
  *) fail "no prebuilt binary for $(uname -s); build it with cargo (see the README)" ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) fail "no prebuilt binary for $(uname -m); build it with cargo (see the README)" ;;
esac
target="$arch-$os"

if [[ "$version" == latest ]]; then
  base="${ANCHORLEG_BASE_URL:-$repo/releases/latest/download}"
else
  base="${ANCHORLEG_BASE_URL:-$repo/releases/download/$version}"
fi
file="anchorleg-$target.tar.gz"

command -v curl >/dev/null || fail "needs curl"
tmp="$(mktemp -d)" || fail "can't make a temporary folder"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $file ($version)…"
curl -fsSL "$base/$file" -o "$tmp/$file" || fail "download failed: $base/$file"
curl -fsSL "$base/$file.sha256" -o "$tmp/$file.sha256" || fail "download failed: $base/$file.sha256"

want="$(cut -d' ' -f1 <"$tmp/$file.sha256")"
if command -v shasum >/dev/null; then
  got="$(shasum -a 256 "$tmp/$file" | cut -d' ' -f1)"
elif command -v sha256sum >/dev/null; then
  got="$(sha256sum "$tmp/$file" | cut -d' ' -f1)"
else
  fail "needs shasum or sha256sum to check the download"
fi
[[ -n "$want" && "$got" == "$want" ]] || fail "checksum mismatch for $file"

tar -xzf "$tmp/$file" -C "$tmp" || fail "couldn't unpack $file"
mkdir -p "$dest" || fail "can't create $dest"
install -m 755 "$tmp/anchorleg-$target/anchorleg" "$dest/anchorleg" || fail "can't write $dest/anchorleg"

echo "Installed $("$dest/anchorleg" --version) to $dest/anchorleg"
case ":$PATH:" in
  *":$dest:"*) ;;
  *)
    case "$(basename "${SHELL:-bash}")" in
      zsh) rc=~/.zshrc ;;
      bash) rc=~/.bashrc ;;
      *) rc=~/.profile ;;
    esac
    echo "Add $dest to your PATH: echo 'export PATH=\"$dest:\$PATH\"' >> $rc"
    ;;
esac
