# Codex 0.153.4: a thread's rollout file (2026-10-15)

`rollout.jsonl`: the file Codex keeps for a thread under
`$CODEX_HOME/sessions/YYYY/MM/DD/rollout-…-<thread id>.jsonl`, from a real `anchorleg run` on
gpt-5.6-luna low (write a file in an extra writable folder, one turn).
`anchorleg sessions import` reads these files: the `event_msg` `item_completed` items
(`UserMessage`, `Reasoning`, `FileChange`, `AgentMessage`, …).

Kept: `session_meta` (only its id, cwd, version, source and timestamp; the rest is Codex's
system prompt and machine details) and the `task_started`, `item_completed` and `task_complete`
events; token counts, rate limits and world state were dropped. Then
`scripts/scrub-capture.py`.
