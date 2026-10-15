# Codex 0.153.4: a subagent's rollout (2026-10-15)

A subagent's own thread, as Codex keeps it under `$CODEX_HOME/sessions/YYYY/MM/DD/`: its
`session_meta` names the parent thread (`source.subagent.thread_spawn.parent_thread_id`) and its
nickname, then its items (the task it was given, a command, its answer) and `task_complete` with
the answer (`last_agent_message`). From a real `anchorleg run` on `gpt-5.6-luna` (effort low)
asking the agent to spawn one subagent that reads a file.

Only `session_meta` (the keys anchorleg reads) and the `task_started`, `item_completed` and
`task_complete` events were kept (the rest repeats the model's instructions); then
`scripts/scrub-capture.py`. `anchorleg sessions agents` reads these files.
