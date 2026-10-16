#!/usr/bin/env bash
# Print one version's section of CHANGELOG.md, for the GitHub release's notes.
#   scripts/release-notes.sh v0.8.0 [CHANGELOG.md]
set -uo pipefail
version="${1#v}"
file="${2:-CHANGELOG.md}"
awk -v v="$version" '
  /^## \[/ { if (on) exit; on = index($0, "## [" v "]") == 1; next }
  on { print }
' "$file" | sed -e '/./,$!d'
