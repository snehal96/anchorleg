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

# With FAKE_HOOK set and agy's permissions skipped, the first tool call goes through the
# PreToolUse hook in $HOME/.gemini/config/hooks.json, like the real CLI; its answer is saved to
# $FAKE_DIR/hook.out and a "deny" ends the turn with the reason.
if [[ -n "${FAKE_HOOK:-}" && " $* " == *" --dangerously-skip-permissions "* ]]; then
  hooks="$HOME/.gemini/config/hooks.json"
  command="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["anchorleg"]["PreToolUse"][0]["hooks"][0]["command"])' "$hooks")"
  request="{\"toolCall\":{\"name\":\"run_command\",\"args\":{\"CommandLine\":\"$FAKE_HOOK\",\"Cwd\":\"$PWD\"}},\"workspacePaths\":[\"$PWD\"],\"conversationId\":\"fake\"}"
  answer="$(cd "$(dirname "$hooks")" && printf '%s' "$request" | sh -c "$command")"
  printf '%s\n' "$answer" >"$FAKE_DIR/hook.out"
  if [[ "$answer" == *'"deny"'* ]]; then
    printf '%s\n' '{"event":"init","conversation_id":"conv-d","init":{"model":"m"}}'
    printf '%s\n' '{"event":"result","result":{"conversation_id":"conv-d","status":"SUCCESS","response":"The command was refused.\n"}}'
    exit 0
  fi
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
