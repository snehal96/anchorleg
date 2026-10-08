# Antigravity CLI probe (agy 1.3.1, 2026-10-08)

Installed with the official `curl -fsSL https://antigravity.google/cli/install.sh | bash`
(binary in `~/.local/bin/agy`; it also appends a PATH line to every shell profile). It used the
Google account the Antigravity desktop app was already signed in with. Model
`gemini-3.8-flash-low`, tiny prompts; 2 runs.

- `print-start.jsonl`: `agy -p "<prompt>" --output-format stream-json --model M`. One JSON object
  per line keyed by `event`: `init` (`conversation_id`, `init.model`, `cwd`, `tools`,
  `permission_mode: "request-review"`), `step_update` (`step_index`, `state` ACTIVE/DONE,
  `step_type` user_input / agent_response / tool / system_message, `tool_name`, `tool_info`,
  `text_delta`, `usage`), `result` (`status` SUCCESS, `response`, `num_turns`, `usage`).
- `print-resume.jsonl`: the same with `--conversation <id>`: continued the conversation.

Not in the stream or the logs: quota numbers (the CLI's `quota_manager` refreshes them but
doesn't log them). Not found: an on-disk login file, so how to keep two Google accounts apart
is still open.

Failure shapes (2026-10-08, 1 more Flash request):

- `print-bad-model.jsonl`: `--model no-such-model`: one `result` with `status: "ERROR"` and
  `error` (model list trimmed), exit 1.
- `print-not-signed-in.jsonl`: an empty `HOME`: `result` ERROR "authentication failed or timed
  out", exit 1. agy also opens a browser sign-in page and asks for a code on stderr.
- `print-unknown-conversation.jsonl`: `--conversation <unknown id>`: **starts a new conversation**
  (new id) and answers; the only sign is `warning: conversation "<id>" not found` on stderr,
  exit 0. This run spent the 1 request.
