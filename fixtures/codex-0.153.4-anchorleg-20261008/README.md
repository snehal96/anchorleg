# Codex through anchorleg (codex-cli 0.153.4, bundled in ChatGPT.app, 2026-10-08)

anchorleg run logs (`<store>/runs/run-<id>.jsonl`) from one real Codex account, `gpt-5.6-luna`,
effort low, in a scratch git repo. 2 model requests on a free plan account. Scrubbed with
`scripts/scrub-capture.py`.

- `run-write-file.jsonl`: `codex exec --json --skip-git-repo-check -c sandbox_mode="workspace-write"
  -m gpt-5.6-luna -c model_reasoning_effort=low -- "<prompt>"`. The agent wrote `answer.txt`
  (`file_change` item, `kind: "add"`), so anchorleg's workspace-write default applies.
- `run-follow-up.jsonl`: `anchorleg run --follow-up 1`, i.e. `codex exec resume --json … -- <thread>
  "<prompt>"`. Same thread, answered from the earlier turn.

The rollout file showed one 43200-minute window at 1% afterwards (`anchorleg status`: `30d 1%`).
