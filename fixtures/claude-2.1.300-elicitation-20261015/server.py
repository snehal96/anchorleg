#!/usr/bin/env python3
"""Minimal stdio MCP server: tool `pick_color` asks the user for a color via elicitation/create."""
import json, sys

LOG = sys.argv[1]

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n"); sys.stdout.flush()

def log(x):
    with open(LOG, "a") as f: f.write(json.dumps(x) + "\n")

pending = None
for line in sys.stdin:
    msg = json.loads(line); log({"in": msg})
    m = msg.get("method")
    if m == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {
            "protocolVersion": msg["params"]["protocolVersion"],
            "capabilities": {"tools": {}}, "serverInfo": {"name": "colors", "version": "1"}}})
    elif m == "tools/list":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"tools": [{
            "name": "pick_color", "description": "Ask the user for their favourite color.",
            "inputSchema": {"type": "object", "properties": {}}}]}})
    elif m == "tools/call":
        pending = msg["id"]
        send({"jsonrpc": "2.0", "id": "e1", "method": "elicitation/create", "params": {
            "message": "Which color do you like?",
            "requestedSchema": {"type": "object", "properties": {
                "color": {"type": "string", "title": "Color"}}, "required": ["color"]}}})
    elif "id" in msg and m is None and msg["id"] == "e1":
        send({"jsonrpc": "2.0", "id": pending, "result": {"content": [
            {"type": "text", "text": "elicitation answer: " + json.dumps(msg.get("result", msg.get("error")))}]}})
    elif "id" in msg and m is not None:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {}})
