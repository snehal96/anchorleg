# Antigravity 1.3.3: plan mode and `ask_question` in `-p` (2026-10-12)

Plain `agy -p … --output-format stream-json --model gemini-3.8-flash-low` runs in scratch folders
(not through anchorleg), scrubbed with `scripts/scrub-capture.py`. 5 requests in total (one
probe with a workspace `.agents/hooks.json` isn't kept: the hook wasn't loaded).

- `plan-own-home.jsonl`: `--mode plan`, no other permission flags, "fix the bug in calc.py,
  planning only". It reads the file (`view_file` allowed), writes `plan.md` in its brain folder,
  gets a `system_message`, then tries `replace_file_content` on the task file, which headless
  mode refuses: the file was unchanged. The reply is the plan.
- `plan-command-denied.jsonl`: `--mode plan` and a task that made it run `ls` first: the command
  is denied (`denied_actions`), the reply is empty.
- `plan-skip-permissions.jsonl`: `--mode plan --dangerously-skip-permissions`: after the plan
  artifact it **carries the plan out** (edits the file, runs a test). So with permissions skipped,
  agy's plan mode doesn't keep files unchanged; anchorleg's hook does that instead.
- `question-skipped.jsonl`: asked to use `ask_question` first: the step shows as
  `step_type: "unknown"` (0.05 s, no tool info) and the reply says the question was skipped and a
  default was picked. In `-p` agy answers its own question tool with nothing.
- Streamed `text_delta` pieces were dropped (a path split across them escaped the scrubber); the
  full reply is in `result`.
