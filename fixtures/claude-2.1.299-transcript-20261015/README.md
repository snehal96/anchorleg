# Claude Code 2.1.299: a session's own transcript (2026-10-15)

`transcript.jsonl`: the file Claude Code keeps for a session under
`<config dir>/projects/<folder>/<session id>.jsonl`, from a real `anchorleg run` on haiku
(`--add-dir`, `--instructions`; read a file in the extra folder, one model call plus the tool
result). `anchorleg sessions import` reads these files.

Only the `user`, `assistant`, `ai-title` and `custom-title` lines were kept (the file also has
attachments, queue operations and cost records, which list the person's plugins and settings);
then `scripts/scrub-capture.py`.
