# codex 0.153.4 — soft stop at `--stop-at` (2026-10-11)

A real `anchorleg run` on one Codex login (ChatGPT free plan, whose only window is 30 days and
was at 1%), model `gpt-5.6-luna`, effort `low`, in an empty git repo. **2 turns** (the stopped one
and the resumed one; a few model calls in all).

```bash
anchorleg run --json --stop-at 0.01 --cwd work -- "Create hello.txt containing hello, then reply with one short line."
```

`run.jsonl` is anchorleg's run log. After the first model call Codex pushed
`account/rateLimits/updated` (30d at 1% ≥ 0.01), anchorleg sent `turn/interrupt`, and the turn
ended `interrupted` right after its file change. anchorleg logged "paused at a clean point
(30d quota at 1%)", wrote `TASK.md` (here) and a checkpoint, found no login with more room, and
resumed the same thread on the same login with the stop off; Codex checked the file and finished.
Scrubbed with `scripts/scrub-capture.py`.
