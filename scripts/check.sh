#!/usr/bin/env bash
# Everything a change must pass before it's called done (see AGENTS.md → Conventions).
set -uo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all --check &&
  cargo clippy --workspace --all-targets -- -D warnings &&
  cargo test --workspace || exit 1

# anchorleg-mod (mod/): validated and tested by Claude Code itself, when it's installed.
CLAUDE_BIN="${ANCHORLEG_CLAUDE_BIN:-$(command -v claude || true)}"
if [[ -n "$CLAUDE_BIN" ]]; then
  "$CLAUDE_BIN" plugin validate mod >/dev/null || { echo "anchorleg-mod: validate failed"; exit 1; }
  "$CLAUDE_BIN" plugin test mod || exit 1
else
  echo "anchorleg-mod: skipped (claude not on PATH; set ANCHORLEG_CLAUDE_BIN)"
fi
