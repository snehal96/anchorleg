# Phase 3 handoff dry run (claude 2.1.294, 2026-10-08)

`relay run --model haiku --permission-mode acceptEdits --stop-at 0.5 --fresh-above 1` in a
fresh git repo holding `notes.txt` ("secret word: KIWI-1"). Task: write draft.txt, read
notes.txt, write the secret word to answer.txt.

- claude-sm (7-day window at 67%) did step 1 and 2, then relay-mod paused it.
- relay wrote `TASK.md` (this folder) and checkpoint `refs/relay/run-4/1` (draft.txt).
- `--fresh-above 1` forced a fresh session: claude-two got only the handoff prompt, no
  `--resume`, and finished: answer.txt = KIWI-1.

`run-sm-pause-two-fresh.jsonl` is relay's run log (CLI stream plus relay's own lines).
Same result 3 of 3 (runs 4–6); 15 Haiku requests for the three.
