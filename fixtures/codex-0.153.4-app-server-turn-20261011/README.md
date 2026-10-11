# codex 0.153.4 — turns through `codex app-server`, with approvals (2026-10-11)

Real turns driven over `codex app-server` (stdio JSON-RPC) by `probe.py`. Each line is one
message, `{"dir": "out"|"in", "msg": …}`: `out` was sent to Codex, `in` came back. Model
`gpt-5.6-luna`, effort `low`, thread started with `approvalPolicy: "on-request"` and
`sandbox: "workspace-write"`. **2 model requests in total.**

| File | What happened |
|---|---|
| `turn-accept.jsonl` | New thread. The agent wrote `out.txt` inside the workspace without asking, then asked to run `curl` with network access (`item/commandExecution/requestApproval`, `availableDecisions: ["accept", {acceptWithExecpolicyAmendment…}, "cancel"]`, no `decline`). Answered `accept`; the command ran and the turn completed. |
| `resume-cancel.jsonl` | `thread/resume` of that thread, the same kind of request answered `cancel`: the command didn't run and `turn/completed` came with `status: "interrupted"` and no final message. |
| `resume-unknown.jsonl` | `thread/resume` of a thread id this login never saw: error `-32600 "no rollout found for thread id …"`. No request spent. |

Also seen: `account/rateLimits/updated` after every model call (same shape as
`account/rateLimits/read`), `thread/tokenUsage/updated` (`last.inputTokens` is the current
context), `turn/plan/updated` not used by this task.

Made with:

```bash
MODEL=gpt-5.6-luna APPROVE=accept python3 probe.py turn-accept.jsonl "$PWD" turn "Create a file out.txt containing hi. Then run this exact command, which needs network access: curl -sI https://example.com | head -1. Reply with its output only."
RESUME=<thread> MODEL=gpt-5.6-luna APPROVE=cancel python3 probe.py resume-cancel.jsonl "$PWD" turn "Run this exact command again: curl -sI https://example.org | head -1. Reply with its output only."
RESUME=01a12ad3-0000-7000-8000-000000000000 python3 probe.py resume-unknown.jsonl "$PWD" turn x
```

Scrubbed with `scripts/scrub-capture.py` (paths, user name; the machine-naming
`remoteControl/status/changed` and the person's own `hook/*` messages dropped).
