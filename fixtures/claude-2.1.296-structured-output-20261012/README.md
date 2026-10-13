# claude 2.1.296: `--json-schema` (2026-10-12)

`structured.jsonl`: `claude -p --model haiku --output-format stream-json --verbose --json-schema
'{"type":"object","properties":{"answer":{"type":"integer"},"word":{"type":"string"}},"required":["answer","word"],"additionalProperties":false}'
"What is 6*7? Also give one word for happy."`. 2 Haiku requests.

- Claude answers by calling a `StructuredOutput` tool with the object (Claude Code nudges it
  with a `[structured-output-enforce]` user message when it answers in text first).
- The `result` event carries `structured_output` (the object) and `result` (the same as JSON
  text).

Stream deltas and hook lines were dropped and the `init` line cut down to neutral fields; the
rest went through `scripts/scrub-capture.py`.
