"""Drive one Codex turn through `codex app-server`, logging every message both ways.
Usage: probe.py <out.jsonl> <cwd> <mode> [prompt]
mode: models (list models, no request) | turn (one turn; approvals answered from APPROVE env: accept|decline)"""
import json, os, subprocess, sys, time
out, cwd, mode = sys.argv[1], sys.argv[2], sys.argv[3]
prompt = sys.argv[4] if len(sys.argv) > 4 else ""
codex = "/Applications/ChatGPT.app/Contents/Resources/codex"
p = subprocess.Popen([codex, "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, cwd=cwd)
log = open(out, "w")
def send(m):
    log.write(json.dumps({"dir": "out", "msg": m}) + "\n"); log.flush()
    p.stdin.write(json.dumps(m) + "\n"); p.stdin.flush()
def recv():
    line = p.stdout.readline()
    if not line: return None
    m = json.loads(line)
    log.write(json.dumps({"dir": "in", "msg": m}) + "\n"); log.flush()
    return m
def wait_id(i):
    while True:
        m = recv()
        if m is None: sys.exit("eof")
        if m.get("id") == i and "method" not in m: return m
send({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "anchorleg-probe", "version": "0"}}})
wait_id(1)
send({"method": "initialized"})
if mode == "models":
    send({"id": 2, "method": "model/list", "params": {}})
    r = wait_id(2)
    for m in r.get("result", {}).get("data", []): print(m.get("id") or m.get("model"), m.get("isDefault",""))
    p.kill(); sys.exit()
model = os.environ.get("MODEL")
if os.environ.get("RESUME"):
    send({"id": 2, "method": "thread/resume", "params": {"threadId": os.environ["RESUME"], "cwd": cwd, "approvalPolicy": "on-request", "sandbox": "workspace-write", "model": model, "excludeTurns": True}})
else:
    send({"id": 2, "method": "thread/start", "params": {"cwd": cwd, "approvalPolicy": "on-request", "sandbox": "workspace-write", "model": model}})
r = wait_id(2)
if "error" in r: print("ERROR", r["error"]); p.kill(); sys.exit()
tid = r["result"]["thread"]["id"]; print("thread", tid)
send({"id": 3, "method": "turn/start", "params": {"threadId": tid, "input": [{"type": "text", "text": prompt}], "effort": "low"}})
start = time.time()
while time.time() - start < 300:
    m = recv()
    if m is None: break
    meth = m.get("method")
    if meth and "id" in m:  # server request
        print("REQUEST", meth, json.dumps(m.get("params"))[:300])
        decision = os.environ.get("APPROVE", "accept")
        send({"id": m["id"], "result": {"decision": decision}})
    elif meth == "turn/completed" and m["params"].get("threadId") == tid:
        print("turn completed", json.dumps(m["params"])[:300]); break
    elif meth in ("item/completed", "error"):
        print(meth, json.dumps(m["params"])[:200])
p.kill()
