#!/usr/bin/env bash
# Stand-in for `codex exec --json` in anchorleg's end-to-end tests. Spends no quota.
#
# The account is the basename of $CODEX_HOME. The n-th launch for account X prints
# $FAKE_DIR/X.n.jsonl if it exists, else $FAKE_DIR/X.jsonl. Every launch appends
# "<account> <args…>" to $FAKE_LOG. Like the real CLI, a thread lives in a rollout file under
# $CODEX_HOME/sessions/, and `exec resume <id>` fails when this login has no such file. Each
# launch appends $FAKE_DIR/X.rollout.jsonl (quota lines) to the thread's rollout, if it exists.
#
# `codex app-server` (anchorleg's live quota read) answers `account/rateLimits/read` with
# $FAKE_DIR/X.limits.json as the result, or an auth error when $FAKE_DIR/X.nologin exists, and
# appends X to $FAKE_DIR/probes.log. It isn't a launch.
set -uo pipefail

account="$(basename "$CODEX_HOME")"

if [[ "${1:-}" == "app-server" ]]; then
  echo "$account" >>"$FAKE_DIR/probes.log"
  while IFS= read -r line; do
    case "$line" in
      *'"id":1'*) echo '{"id":1,"result":{"userAgent":"fake-codex"}}' ;;
      *'"id":2'*)
        if [[ -f "$FAKE_DIR/$account.nologin" ]]; then
          echo '{"id":2,"error":{"code":-32600,"message":"codex account authentication required to read rate limits"}}'
        elif [[ -f "$FAKE_DIR/$account.limits.json" ]]; then
          printf '{"id":2,"result":%s}\n' "$(cat "$FAKE_DIR/$account.limits.json")"
        else
          echo '{"id":2,"result":{"rateLimits":null}}'
        fi
        exit 0 ;;
    esac
  done
  exit 0
fi
printf '%s %s\n' "$account" "$(printf '%s' "$*" | tr '\n' ' ')" >>"$FAKE_LOG"  # one line per launch
n="$(grep -c "^$account " "$FAKE_LOG")"

# Positionals after `--`: [thread id] prompt.
resume=""
seen_dashes=""
is_resume=""
for arg in "$@"; do
  if [[ -n "$seen_dashes" ]]; then
    [[ -n "$is_resume" && -z "$resume" ]] && resume="$arg"
    break
  fi
  [[ "$arg" == "resume" ]] && is_resume=1
  [[ "$arg" == "--" ]] && seen_dashes=1
done

sessions="$CODEX_HOME/sessions/2026/10/08"
if [[ -n "$resume" ]] && ! ls "$CODEX_HOME"/sessions/*/*/*/rollout-*-"$resume.jsonl" >/dev/null 2>&1; then
  echo "Error: thread/resume: thread/resume failed: no rollout found for thread id $resume (code -32600)" >&2
  exit 1
fi

script="$FAKE_DIR/$account.$n.jsonl"
[[ -f "$script" ]] || script="$FAKE_DIR/$account.jsonl"

sid="${resume:-$(grep -o '"thread_id":"[^"]*"' "$script" | head -1 | cut -d'"' -f4)}"
if [[ -n "$sid" ]]; then
  mkdir -p "$sessions"
  rollout="$(ls "$CODEX_HOME"/sessions/*/*/*/rollout-*-"$sid.jsonl" 2>/dev/null | head -1)"
  [[ -n "$rollout" ]] || rollout="$sessions/rollout-2026-10-08T00-00-00-$sid.jsonl"
  echo '{"type":"session_meta"}' >>"$rollout"
  [[ -f "$FAKE_DIR/$account.rollout.jsonl" ]] && cat "$FAKE_DIR/$account.rollout.jsonl" >>"$rollout"
fi

cat "$script"
grep -q '"turn.failed"' "$script" && exit 1
exit 0
