# Minimal MCP stdio server: one tool, `pick_project`, that asks the user via elicitation/create.
import json, sys

LOG = sys.argv[1] if len(sys.argv) > 1 else None

def log(x):
    if LOG:
        with open(LOG, "a") as f:
            f.write(json.dumps(x) + "\n")

def send(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()

def read():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    m = json.loads(line); log({"in": m}); return m

while True:
    m = read()
    method, mid = m.get("method"), m.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": m["params"].get("protocolVersion", "2025-06-18"),
              "capabilities": {"tools": {}}, "serverInfo": {"name": "picker", "version": "0.1"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": mid, "result": {"tools": [{"name": "pick_project",
              "description": "Ask the user which project to use. Returns the user's choice.",
              "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "tools/call":
        send({"jsonrpc": "2.0", "id": "e1", "method": "elicitation/create", "params": {
            "message": "Which project should the ticket go to?",
            "requestedSchema": {"type": "object", "properties": {
                "project": {"type": "string", "title": "Project", "oneOf": [
                    {"const": "ENG", "title": "Engineering"}, {"const": "OPS", "title": "Operations"}]}},
                "required": ["project"]}}})
        while True:
            r = read()
            if r.get("id") == "e1":
                break
        res = r.get("result") or {}
        text = f"action={res.get('action')} content={json.dumps(res.get('content'))}"
        send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": text}]}})
    elif mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "result": {}})
