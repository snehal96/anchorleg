# Claude Code 2.1.296: `/compact` in a headless follow-up

`compact.jsonl` is an anchorleg run log (`anchorleg run --follow-up <id> -- /compact`) of a
session whose first turn was "Reply with just: hi", on Haiku. Claude runs `-p --resume <id>
/compact` as its built-in command: `system/status` `compacting`, then `system/compact_boundary`
with `compact_metadata` (`trigger: manual`, `pre_tokens`, `post_tokens`), the summary as a
string-content `user` message, `<local-command-stdout>Compacted </local-command-stdout>`, and a
`result` with `num_turns: 0`.

Made 2026-10-12 with two small requests (the first turn, and the compaction's summary), then
run through `scripts/scrub-capture.py`.
