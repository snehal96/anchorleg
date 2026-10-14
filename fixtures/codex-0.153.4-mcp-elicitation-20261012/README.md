# Codex 0.153.4: MCP tool-call approval and an MCP form (2026-10-12)

`requests.jsonl`: the `mcpToolCall` item and the two `mcpServer/elicitation/request` server
requests `codex app-server` sent during one real turn (gpt-5.6-luna, effort low, 1 turn).

How it was made: `elicit_server.py` (in this folder) was added as an MCP server with
`-c mcp_servers.picker.command=python3 -c 'mcp_servers.picker.args=["elicit_server.py"]'`, the
Codex binary was wrapped to `tee` its stdout, and anchorleg ran the prompt "Call the pick_project
tool from the picker MCP server once, then reply with only the exact text it returned." The lines
were picked out with `grep` and run through `scripts/scrub-capture.py`.

What it shows:

1. Codex asks before **every MCP tool call** with an elicitation whose `_meta.codex_approval_kind`
   is `mcp_tool_call` and whose `requestedSchema` has no fields. The tool name is only in the
   message (`run tool "pick_project"`); arguments are in `_meta.tool_params`. Answering
   `-32601` (what anchorleg did before) rejects the call: "user rejected MCP tool call".
2. The server's own `elicitation/create` arrives as a form elicitation with `_meta: null`.
   anchorleg answered `{"action":"accept","content":{"project":"OPS"}}` and the server got it.
