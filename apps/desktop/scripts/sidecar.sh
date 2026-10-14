#!/usr/bin/env bash
# Build the anchorleg CLI from this repo and put it where Tauri bundles it from:
# src-tauri/binaries/anchorleg-<target triple> (D31: the app ships its own CLI).
# ANCHORLEG_CLI_DIR builds it from another checkout instead, e.g. a `cli/<topic>` worktree whose
# changes aren't on this branch yet (D65).
set -uo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "${ANCHORLEG_CLI_DIR:-$here/../..}" && pwd) || exit 1
target=${TAURI_ENV_TARGET_TRIPLE:-$(rustc -vV | sed -n 's/^host: //p')}
profile=${ANCHORLEG_SIDECAR_PROFILE:-release}
flag=()
[ "$profile" = release ] && flag=(--release)
cargo build --manifest-path "$root/Cargo.toml" -p anchorleg --locked --target "$target" ${flag[@]+"${flag[@]}"} || exit 1
mkdir -p "$here/src-tauri/binaries"
cp "$root/target/$target/$profile/anchorleg" "$here/src-tauri/binaries/anchorleg-$target"
echo "sidecar: src-tauri/binaries/anchorleg-$target (from $root)"
