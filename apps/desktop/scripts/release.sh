#!/usr/bin/env bash
# Build the release bundles for this machine's OS into apps/desktop/release/, with SHA256SUMS.
#   macOS: one universal .dmg (Apple silicon and Intel, both native), the CLI inside universal too.
#   Linux: AppImage and .deb for the host architecture.
# Targets can be named: scripts/release.sh aarch64-apple-darwin. ANCHORLEG_CLI_DIR builds the
# bundled CLI from another checkout (see sidecar.sh).
set -uo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here" || exit 1
case "$(uname -s)" in
  Darwin) default=(universal-apple-darwin); bundles=app,dmg ;;
  Linux) default=("$(rustc -vV | sed -n 's/^host: //p')"); bundles=appimage,deb ;;
  *) echo "unsupported OS" >&2; exit 1 ;;
esac
targets=("$@")
[ ${#targets[@]} -eq 0 ] && targets=("${default[@]}")
out="$here/release"
rm -rf "$out" && mkdir -p "$out"
for t in "${targets[@]}"; do
  echo "== $t"
  # tauri runs `npm run sidecar` (beforeBuildCommand) with TAURI_ENV_TARGET_TRIPLE set to $t.
  npx tauri build --target "$t" --bundles "$bundles" || exit 1
  b="src-tauri/target/$t/release/bundle"
  for f in "$b"/dmg/*.dmg "$b"/appimage/*.AppImage "$b"/deb/*.deb; do
    [ -f "$f" ] && cp "$f" "$out/"
  done
done
cd "$out" && shasum -a 256 -- * > SHA256SUMS && cat SHA256SUMS
