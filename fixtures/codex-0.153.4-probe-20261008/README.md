# Codex CLI probe (codex-cli 0.153.4, bundled in ChatGPT.app, 2026-10-08)

Binary: `/Applications/ChatGPT.app/Contents/Resources/codex`. Model `gpt-5.6-luna`, effort low,
`-s read-only`, tiny prompts. 2 model requests on a **free** plan account.

- `exec-start.jsonl`: `codex exec --json --skip-git-repo-check …` reading notes.txt. Events:
  `thread.started` (thread_id), `turn.started`, `item.started`/`item.completed`
  (`command_execution`, `agent_message`), `turn.completed` (usage). No quota in the stream.
- `exec-resume.jsonl`: `codex exec resume <thread_id> --json "…"` answered from the earlier turn.
- `exec-not-logged-in.jsonl`: an empty `CODEX_HOME`: `error` events "Reconnecting… 401
  Unauthorized", exit 1. One `CODEX_HOME` = one login.
- `rollout-token-count.jsonl`: from `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*-<thread_id>.jsonl`
  (trimmed to `session_meta` + `token_count`). `rate_limits.primary`/`secondary`:
  `used_percent`, `window_minutes` (43200 = 30 days on the free plan), `resets_at` (unix s),
  plus `plan_type`, `rate_limit_reached_type`.
