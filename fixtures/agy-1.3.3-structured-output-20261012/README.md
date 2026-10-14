# agy 1.3.3: `--json-schema` (2026-10-12)

`structured.jsonl`: `agy --output-format stream-json --json-schema schema.json -p "What is 6*7?
Also give one word for happy."` with the schema `{answer: integer, word: string}`, required, no
other properties. One request (agy's default model).

- `-p` must come last or take the prompt directly: `agy -p --output-format …` takes
  `--output-format` as the prompt.
- The `result` event carries `structured_output` (the object, exactly the schema's fields) and
  `json_schema`; its `response` text has extra fields (`toolAction`, `toolSummary`), so the
  object is read from `structured_output`.
