# Phase 0 capture — Claude Code 2.1.294, 2026-10-08

Real output from an early probe script with two config-dir accounts (`CC_DIR_1=~/.claude-sm`,
`CC_DIR_2=~/.claude-two`), model haiku. Results: `summary.txt`. Scanned for tokens and emails
before saving; paths, account names and installed plugins are scrubbed.

- `acct*-hello.jsonl`: one turn per account. `rate_limit_event` carries `unifiedWindows` with
  utilization even while "allowed".
- `resume-continue.jsonl`: `--resume` on the other account fails ("No conversation found").
- `resume-continue-copied.jsonl`: works after copying the session JSONL into that account's dir.
- `probe-mod.jsonl`: what the probe mod saw via `$.session.usage()` (not stream-json).
