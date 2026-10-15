# MCP elicitation in `claude -p` (Claude Code 2.1.300, 2026-10-15)

`server.py` is a minimal stdio MCP server whose tool `pick_color` sends `elicitation/create`
(a one-field form). Claude Haiku called it in `claude -p` with a plugin whose `hooks/hooks.json`
holds both a mod module and a classic `Elicitation` command hook.

- `hook-input.json`: what the hook got on stdin (it fires in `-p`; a hook in a plugin next to
  `modules` works).
- `server-log.ndjson` (the server's own log, one JSON message per line): the tool call and the client's answer to `elicitation/create`, i.e. the
  hook's `{"hookSpecificOutput":{"hookEventName":"Elicitation","action":"accept","content":{"color":"teal"}}}`
  reached the server unchanged.

Then the same through anchorleg (`anchorleg run --mcp-config … --allowed-tools mcp__colors__pick_color`):
the question showed in `anchorleg permission list`, was answered `teal`, and the server got it.
6 Haiku requests in all. Scrubbed with `scripts/scrub-capture.py`.
