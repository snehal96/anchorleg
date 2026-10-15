# Claude Code 2.1.300: subagent transcripts (2026-10-15)

What Claude Code keeps for each subagent a session starts, under
`<config dir>/projects/<folder>/<session id>/subagents/`: `agent-<id>.jsonl`, the subagent's own
transcript, and `agent-<id>.meta.json` (`description`, `agentType`, `toolUseId` of the parent's
Agent call, `requestShape`). From a real `anchorleg run` on haiku asking for two
general-purpose agents ("Count files": `ls`, "Read a.txt": read a file); Claude ran both in the
background. Each ran one tool and reported back with a `SubagentHandback` call.

Only the `user` and `assistant` lines were kept, and only the keys anchorleg reads (the files
also hold full copies of each model request, with the person's settings, tools and plugins);
then `scripts/scrub-capture.py`. `anchorleg sessions agents` reads these files.
