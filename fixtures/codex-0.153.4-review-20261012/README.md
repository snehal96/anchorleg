# codex 0.153.4: `review/start` over `codex app-server` (2026-10-12)

`review.jsonl`: `probe.py` from `codex-0.153.4-subagent-20261012`, with the first `turn/start`
replaced by `review/start {threadId, target: {type: "custom", instructions}, delivery: "inline"}`.
A scratch git repo whose `calc.py` had an uncommitted change (`a + b` → `a - b`, plus a new `div`).
Model `gpt-5.6-luna`; about 6 model calls (the review ran `git diff` and read files).

- The answer to `review/start` is `{turn: {...}}`, like `turn/start`; the turn then streams as
  usual: `enteredReviewMode` item (the instructions), reasoning, `commandExecution`s, then an
  `exitedReviewMode` item whose `review` is the findings text, and the same text as the final
  `agentMessage`, then `turn/completed`.
- Findings look like `- [P1] <title> — <path>:<line>-<line>` with a paragraph each.
- One command's output (it read a skill file from the person's own Codex home) was replaced; machine-naming and hook messages were dropped.
