# codex 0.153.4: a subagent (`multi_agent`) over `codex app-server` (2026-10-12)

`subagent.jsonl`: one turn driven by `probe.py` (as in `codex-0.153.4-app-server-turn-20261011`,
but it stops at the `turn/completed` of the session's own thread), model `gpt-5.6-luna`, effort
`low`. Prompt: "Spawn one subagent (spawn_agent) and ask it to list the files in this folder. Wait
for it, then reply with its answer only." About 5 model calls (3 parent, 2 subagent).

- `item/started`/`item/completed` of a `collabAgentToolCall` with `tool: "spawnAgent"`
  (`receiverThreadIds` holds the new thread once completed), then one with `tool: "wait"`, whose
  completed `agentsStates` carries the subagent's final `message`.
- The subagent's thread streams on the same connection, told apart only by `threadId`:
  `thread/status/changed`, `turn/started`, its items (`userMessage` with its prompt, reasoning,
  `commandExecution`, `agentMessage`), `thread/tokenUsage/updated` and **its own `turn/completed`**,
  before the parent's.
- No `subAgentActivity` items were sent.

```bash
MODEL=gpt-5.6-luna APPROVE=accept python3 probe.py subagent.jsonl "$PWD" turn "Spawn one subagent …"
```

Scrubbed with `scripts/scrub-capture.py` (paths, user name, the machine-naming and hook messages).
