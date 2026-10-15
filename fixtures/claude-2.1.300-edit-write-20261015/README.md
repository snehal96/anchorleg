# Edit and Write results in `claude -p` (Claude Code 2.1.300, 2026-10-15)

`claude -p --model haiku --output-format stream-json --verbose --permission-mode acceptEdits
"Fix calc.py so add adds. Then create notes.md with one line: fixed add. Use the Edit and Write
tools."` in a folder whose `calc.py` subtracts. Each tool's `user` event carries
`tool_use_result`: for Edit `{filePath, oldString, newString, originalFile, structuredPatch:
[{oldStart, oldLines, newStart, newLines, lines}]}`, for a new file Write `{type: "create",
filePath, content, structuredPatch: []}`. anchorleg's Changes panel is built from these.

1 Haiku request. Init and SessionStart hook lines dropped; scrubbed with
`scripts/scrub-capture.py`.
