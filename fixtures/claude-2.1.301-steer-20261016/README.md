# Claude Code 2.1.301: messages to a running turn (`--input-format stream-json`)

Made 2026-10-16 with `probe.py` (in this folder) on Haiku, about 9 small requests in all, then run
through `scripts/scrub-capture.py`. Command:

    claude -p --input-format stream-json --output-format stream-json --verbose \
      --model haiku --allowedTools 'Bash(ping:*)'

The prompt goes in as the first stdin line, `{"type":"user","uuid":…,"session_id":"",
"parent_tool_use_id":null,"message":{"role":"user","content":…}}`. Lines starting `{"_sent"`
are what the probe wrote to stdin (with the time); `{"_exit"` is the end. Each user message
gets `command_lifecycle` events (`command_uuid` = the message's `uuid`, `state` `queued` →
`started` → `completed`).

- `steer.jsonl`: a plain message sent while the first of two `ping -c 8` calls ran. It went
  `queued`, then `started` right after that tool result, and the model answered it in the same
  turn: one `result`, "DONE BANANA". Claude folds a waiting message into the running turn at the
  next tool result, as the terminal does.
- `send-now.jsonl`: a message, then `{"type":"control_request","request_id":"sn-1","request":
  {"subtype":"interrupt","send_now":true}}`, during `ping -c 25`. The answer was
  `{"still_queued":[<the message>],"send_now":"stopped"}`: the CLI ended the tool ("The user
  doesn't want to proceed…", "[Request interrupted by user for tool use]"), the turn's `result`
  was `error_during_execution`, and the message ran as the next turn.
- `send-now-too-late.jsonl`: the same, but the tool was refused at once (a hook of the person's
  blocked `sleep`), so the turn had already taken the message in: `send_now: "nothing_waiting"`,
  and the message ran as its own turn after the first `result`.

The process stays up between turns until stdin is closed. Init lists `capabilities` including
`interrupt_send_now_v1` and `msg_lifecycle_v1`.
