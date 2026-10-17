#!/usr/bin/env bash
# Build the anchorleg CLI from this repo and put it where Tauri bundles it from:
# src-tauri/binaries/anchorleg-<target triple> (D31: the app ships its own CLI).
# For universal-apple-darwin it builds both Mac architectures, joins them with lipo, and keeps
# each one's copy as well (Tauri builds the app per architecture first).
# ANCHORLEG_CLI_DIR builds it from another checkout instead, e.g. a `cli/<topic>` worktree whose
# changes aren't on this branch yet (D65).
set -uo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "${ANCHORLEG_CLI_DIR:-$here/../..}" && pwd) || exit 1
target=${TAURI_ENV_TARGET_TRIPLE:-$(rustc -vV | sed -n 's/^host: //p')}
profile=${ANCHORLEG_SIDECAR_PROFILE:-release}
flag=()
[ "$profile" = release ] && flag=(--release)
build() {
  cargo build --manifest-path "$root/Cargo.toml" -p anchorleg --locked --target "$1" ${flag[@]+"${flag[@]}"}
}
mkdir -p "$here/src-tauri/binaries"
out="$here/src-tauri/binaries/anchorleg-$target"
if [ "$target" = universal-apple-darwin ]; then
  build aarch64-apple-darwin || exit 1
  build x86_64-apple-darwin || exit 1
  lipo -create -output "$out" \
    "$root/target/aarch64-apple-darwin/$profile/anchorleg" \
    "$root/target/x86_64-apple-darwin/$profile/anchorleg" || exit 1
  # Tauri compiles the app once per architecture and wants that architecture's copy too.
  for a in aarch64-apple-darwin x86_64-apple-darwin; do
    cp "$root/target/$a/$profile/anchorleg" "$here/src-tauri/binaries/anchorleg-$a" || exit 1
  done
else
  build "$target" || exit 1
  cp "$root/target/$target/$profile/anchorleg" "$out"
fi
echo "sidecar: src-tauri/binaries/anchorleg-$target (from $root)"
