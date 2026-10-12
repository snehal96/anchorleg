#!/usr/bin/env bash
# Builds a throwaway anchorleg setup for the demo recording (demo.tape): two accounts backed by
# the test suite's fake CLIs, so recording spends no quota and shows no real account.
#
#   scripts/demo/setup.sh DIR   then   source DIR/env.sh
set -uo pipefail

dir="${1:?usage: setup.sh DIR}"
repo="$(cd "$(dirname "$0")/../.." && pwd)"
fakes="$repo/crates/anchorleg/tests/fake"
rm -rf "$dir"
mkdir -p "$dir"/{cfg/claude-pro,cfg/codex-plus,fake,home,bin,app/src/auth}

# The fakes print their whole script at once; these wrappers pace it like a real agent.
for cli in claude codex; do
  case "$cli" in
    claude) fake="$fakes/fake-claude.sh" ;;
    codex) fake="$fakes/fake-codex.py" ;;
  esac
  cat >"$dir/bin/$cli" <<WRAP
#!/usr/bin/env bash
"$fake" "\$@" | while IFS= read -r line; do printf '%s\n' "\$line"; sleep "\${DEMO_PACE:-0.7}"; done
WRAP
  chmod +x "$dir/bin/$cli"
done

cat >"$dir/config.toml" <<TOML
[[account]]
name = "claude-pro"
vendor = "claude"
priority = 1
bin = "$dir/bin/claude"
env = { CLAUDE_CONFIG_DIR = "$dir/cfg/claude-pro" }

[[account]]
name = "codex-plus"
vendor = "codex"
priority = 2
bin = "$dir/bin/codex"
env = { CODEX_HOME = "$dir/cfg/codex-plus" }
TOML

far=1791900000
# Claude works through the task, then anchorleg-mod reports 92% and asks for a stop.
cat >"$dir/fake/claude-pro.jsonl" <<JSON
{"type":"system","subtype":"init","session_id":"7f3c9a1e","model":"claude-sonnet-5-5"}
{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":$far,"rateLimitType":"five_hour","overageStatus":"rejected","overageDisabledReason":"out_of_credits","isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0.92,"resetsAt":$far},"seven_day":{"utilization":0.41,"resetsAt":$far}}}}
{"type":"assistant","session_id":"7f3c9a1e","message":{"content":[{"type":"tool_use","id":"t0","name":"TodoWrite","input":{"todos":[{"content":"Move login and session tests to Vitest","status":"in_progress","activeForm":"Moving tests"},{"content":"Move token and refresh tests","status":"pending","activeForm":"Moving tests"},{"content":"Remove the Jest config","status":"pending","activeForm":"Removing Jest"}]}}]}}
{"type":"assistant","session_id":"7f3c9a1e","message":{"content":[{"type":"text","text":"Moving the auth tests from Jest to Vitest, one file at a time."},{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"src/auth/login.test.ts"}}]}}
{"type":"assistant","session_id":"7f3c9a1e","message":{"content":[{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"src/auth/session.test.ts"}}]}}
{"type":"assistant","session_id":"7f3c9a1e","message":{"content":[{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"npx vitest run src/auth"}}]}}
{"type":"result","subtype":"success","is_error":false,"result":"","session_id":"7f3c9a1e","total_cost_usd":2.41,"modelUsage":{"claude-sonnet-5-5":{"inputTokens":3100,"outputTokens":11800,"cacheReadInputTokens":684000,"cacheCreationInputTokens":52000,"costUSD":2.41}}}
JSON

cat >"$dir/fake/codex-plus.limits.json" <<JSON
{"rateLimits":{"limitId":"codex","primary":{"usedPercent":12,"windowDurationMins":300,"resetsAt":$far},"secondary":{"usedPercent":30,"windowDurationMins":10080,"resetsAt":$far},"planType":"plus","rateLimitReachedType":null,"credits":{"hasCredits":false,"unlimited":false,"balance":null}}}
JSON
cat >"$dir/fake/codex-plus.jsonl" <<'JSON'
{"method":"turn/started","params":{"threadId":"019a2b","turn":{"id":"u1","status":"inProgress"}}}
{"method":"item/completed","params":{"threadId":"019a2b","item":{"type":"commandExecution","id":"c1","command":"/bin/zsh -lc 'npx vitest run src/auth'","aggregatedOutput":"","exitCode":0,"status":"completed"}}}
{"method":"thread/tokenUsage/updated","params":{"threadId":"019a2b","tokenUsage":{"last":{"inputTokens":48000,"cachedInputTokens":31000,"outputTokens":2900}}}}
{"method":"item/completed","params":{"threadId":"019a2b","item":{"type":"agentMessage","id":"m1","text":"Done: 14 auth tests run on Vitest, all passing. Jest config removed."}}}
{"method":"turn/completed","params":{"threadId":"019a2b","turn":{"id":"u1","status":"completed","error":null}}}
JSON

(cd "$dir/app" && git init -q && echo 'test("login", () => {})' >src/auth/login.test.ts \
  && git add -A && git -c user.name=demo -c user.email=demo@example.com commit -qm init)

# Quota as anchorleg-mod last reported it, so `anchorleg status` has something to show.
printf '{"account":"claude-pro","readings":[{"window":"five_hour","used":0.86,"resets_at":%s},{"window":"seven_day","used":0.41,"resets_at":%s}]}' "$far" "$far" \
  | ANCHORLEG_CONFIG="$dir/config.toml" ANCHORLEG_HOME="$dir/home" "$repo/target/release/anchorleg" report --json

cat >"$dir/env.sh" <<ENV
export ANCHORLEG_CONFIG="$dir/config.toml" ANCHORLEG_HOME="$dir/home"
export FAKE_DIR="$dir/fake" FAKE_LOG="$dir/fake.log" FAKE_MOD_STOP=claude-pro
export PATH="$repo/target/release:\$PATH" PS1='\$ '
cd "$dir/app"
ENV
