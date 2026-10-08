#!/usr/bin/env bash
# Stand-in for `agy -p --output-format stream-json` in anchorleg's end-to-end tests. Spends no quota.
#
# The account is the basename of $HOME (agy keeps its login under it). The n-th launch for
# account X prints $FAKE_DIR/X.n.jsonl if it exists, else $FAKE_DIR/X.jsonl. Every launch appends
# "<account> <args…>" to $FAKE_LOG. Like the real CLI, a conversation lives in
# $HOME/.gemini/antigravity-cli/conversations/<id>.db, and `--conversation <unknown id>` doesn't
# fail: it warns on stderr and starts a new conversation.
set -uo pipefail

account="$(basename "$HOME")"
printf '%s %s\n' "$account" "$(printf '%s' "$*" | tr '\n' ' ')" >>"$FAKE_LOG"  # one line per launch
n="$(grep -c "^$account " "$FAKE_LOG")"

conversation=""
prev=""
for arg in "$@"; do
  [[ "$prev" == "--conversation" ]] && conversation="$arg"
  prev="$arg"
done
store="$HOME/.gemini/antigravity-cli/conversations"
if [[ -n "$conversation" && ! -f "$store/$conversation.db" ]]; then
  echo "warning: conversation \"$conversation\" not found" >&2
fi

script="$FAKE_DIR/$account.$n.jsonl"
[[ -f "$script" ]] || script="$FAKE_DIR/$account.jsonl"
id="$(grep -o '"conversation_id":"[^"]*"' "$script" | head -1 | cut -d'"' -f4)"
if [[ -n "$id" ]]; then
  mkdir -p "$store"
  echo db >>"$store/$id.db"
fi

cat "$script"
grep -q '"status":"ERROR"' "$script" && exit 1
exit 0
