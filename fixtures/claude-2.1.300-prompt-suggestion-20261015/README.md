# Prompt suggestions in `claude -p` (Claude Code 2.1.300, 2026-10-15)

`claude -p --model haiku --output-format stream-json --verbose --prompt-suggestions=true
"Read calc.py and tell me in one sentence what's wrong. Don't change it."` in a folder whose
`calc.py` subtracts in `add`. After the turn's `result`, Claude prints one more line:
`{"type":"prompt_suggestion","suggestion":"fix the add function",…}`.

`--prompt-suggestions` takes a value (`true`), so write it `--prompt-suggestions=true`: a bare
flag swallows the prompt. 1 Haiku request (plus whatever the suggestion itself costs). The init
and SessionStart hook lines were dropped; scrubbed with `scripts/scrub-capture.py`.
