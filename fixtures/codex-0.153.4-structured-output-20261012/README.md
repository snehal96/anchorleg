# codex 0.153.4: `outputSchema` on `turn/start` (2026-10-12)

`structured.jsonl`: one turn over `codex app-server` (stable API) with `outputSchema` set to
`{answer: integer, word: string}` (required, no other properties). Model `gpt-5.6-luna`, effort
`low`; one model call.

- The final `agentMessage` text is the JSON object itself (`{"answer":42,"word":"joyful"}`);
  there is no separate structured field.

Streaming deltas, rate-limit and hook messages were dropped; the rest went through
`scripts/scrub-capture.py`.
