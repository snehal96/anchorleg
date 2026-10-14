#!/usr/bin/env bash
# Stand-in for `claude -p` in anchorleg's end-to-end tests. Spends no quota.
#
# The account is the basename of $CLAUDE_CONFIG_DIR. The n-th launch for account X prints
# $FAKE_DIR/X.n.jsonl if it exists, else $FAKE_DIR/X.jsonl. Every launch appends
# "<account> <args…> [stop_at=…]" to $FAKE_LOG. Like the real CLI, `--resume <id>` fails unless the session
# file is in this account's own config dir; a fresh session writes that file (unless
# $FAKE_NO_SESSION_FILE is set, which makes every resume fail).
set -uo pipefail

# `mcp …` (anchorleg sync mcp): logged as "<account> mcp …" to $FAKE_DIR/mcp.log, nothing else.
if [[ "${1:-}" == mcp ]]; then
  printf '%s %s\n' "$(basename "$CLAUDE_CONFIG_DIR")" "$*" >>"$FAKE_DIR/mcp.log"
  exit 0
fi

# `auth login` / `auth status --json` (anchorleg accounts login): signs in by writing a marker in
# $CLAUDE_CONFIG_DIR, unless $FAKE_LOGIN_FAILS is set.
if [[ "${1:-}" == auth ]]; then
  case "${2:-}" in
    login)
      [[ -n "${FAKE_LOGIN_FAILS:-}" ]] && { echo "login cancelled" >&2; exit 1; }
      echo "Opening the browser to sign in…"
      touch "$CLAUDE_CONFIG_DIR/.fake-signed-in"
      exit 0 ;;
    status)
      if [[ -f "$CLAUDE_CONFIG_DIR/.fake-signed-in" ]]; then echo '{"loggedIn": true}'; else echo '{"loggedIn": false}'; fi
      exit 0 ;;
  esac
fi

account="$(basename "$CLAUDE_CONFIG_DIR")"

# `-p … /usage` (anchorleg reading quota): prints $FAKE_DIR/X.usage.json, or fails like a CLI
# that can't tell. Logged to $FAKE_DIR/usage.log, not $FAKE_LOG: it isn't a launch.
if [[ "${*: -1}" == /usage ]]; then
  echo "$account" >>"$FAKE_DIR/usage.log"
  [[ -f "$FAKE_DIR/$account.usage.json" ]] || exit 1
  cat "$FAKE_DIR/$account.usage.json"
  exit 0
fi

printf '%s %s [stop_at=%s]%s\n' "$account" "$(printf '%s' "$*" | tr '\n' ' ')" "${ANCHORLEG_STOP_AT:-}" \
  "${ANCHORLEG_PERMISSION_MODE:+ [permission_mode=$ANCHORLEG_PERMISSION_MODE]}${ANCHORLEG_BUDGET_LEFT:+ [budget_left=$ANCHORLEG_BUDGET_LEFT]}" \
  >>"$FAKE_LOG"  # one line per launch
n="$(grep -c "^$account " "$FAKE_LOG")"

resume=""
prev=""
for arg in "$@"; do
  [[ "$prev" == "--resume" ]] && resume="$arg"
  prev="$arg"
done

if [[ -n "$resume" ]] && ! ls "$CLAUDE_CONFIG_DIR"/projects/*/"$resume.jsonl" >/dev/null 2>&1; then
  printf '{"type":"result","subtype":"error_during_execution","is_error":true,"result":null,"errors":["No conversation found with session ID: %s"]}\n' "$resume"
  exit 1
fi

# Simulate anchorleg-mod: with the mod loaded and its stop on, an account named in $FAKE_MOD_STOP
# asks anchorleg to stop this run, as the mod does at a step boundary.
if [[ " $* " == *" --plugin-dir "* && " ${FAKE_MOD_STOP:-} " == *" $account "* && "${ANCHORLEG_STOP_AT:-0}" != "0" ]]; then
  printf '{"account":"%s","readings":[{"window":"five_hour","used":0.92}],"run_id":%s,"stop_reason":"quota at 92%%"}' \
    "$account" "$ANCHORLEG_RUN_ID" | "$ANCHORLEG_BIN" report --json || exit 3
fi

# Simulate anchorleg-mod asking the person about a tool call; the answer goes to $FAKE_DIR/ask.out.
if [[ -n "${FAKE_ASK:-}" && " $* " == *" --plugin-dir "* ]]; then
  printf '{"run_id":%s,"tool":"Write","input":{"file_path":"a.txt"},"reason":"not granted"}' "$ANCHORLEG_RUN_ID" \
    | "$ANCHORLEG_BIN" permission ask --json --wait 20 >"$FAKE_DIR/ask.out" || exit 4
fi

script="$FAKE_DIR/$account.$n.jsonl"
[[ -f "$script" ]] || script="$FAKE_DIR/$account.jsonl"

sid="$(grep -o '"session_id":"[^"]*"' "$script" | head -1 | cut -d'"' -f4)"
if [[ -z "$resume" && -n "$sid" && -z "${FAKE_NO_SESSION_FILE:-}" ]]; then
  mkdir -p "$CLAUDE_CONFIG_DIR/projects/-fake"
  echo '{}' >"$CLAUDE_CONFIG_DIR/projects/-fake/$sid.jsonl"
  mkdir -p "$CLAUDE_CONFIG_DIR/projects/-fake/memory"
  echo "notes from $account" >"$CLAUDE_CONFIG_DIR/projects/-fake/memory/notes.md"
fi

if [[ -n "${FAKE_SLEEP:-}" ]]; then
  head -1 "$script"   # the init line, then hang like a long task
  sleep "$FAKE_SLEEP" || exit 143   # stopped: never go on to print a result
fi
cat "$script"
grep -q '"is_error":true' "$script" && exit 1
exit 0
