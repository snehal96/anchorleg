# Probe: does `interrupt {send_now: true}` put a message into Claude's running turn (stream-json input)?
import json, subprocess, sys, threading, time, uuid
work, out = sys.argv[1], sys.argv[2]
p = subprocess.Popen([os_claude := sys.argv[3], "-p", "--input-format", "stream-json", "--output-format", "stream-json",
     "--verbose", "--model", "haiku", "--allowedTools", "Bash(ping:*)"],
    cwd=work, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
log = open(out, "w")
t0 = time.time()
def send(obj):
    line = json.dumps(obj)
    log.write(json.dumps({"_sent": obj, "_t": round(time.time()-t0, 2)}) + "\n"); log.flush()
    p.stdin.write(line + "\n"); p.stdin.flush()
def user(text):
    send({"type": "user", "uuid": str(uuid.uuid4()), "session_id": "", "parent_tool_use_id": None,
          "message": {"role": "user", "content": text}})
user("Run the shell command `ping -c 25 127.0.0.1` with the Bash tool (in the foreground). When it finishes, reply with just the word DONE.")
results = 0; sent = False
for line in p.stdout:
    log.write(line if line.endswith("\n") else line + "\n"); log.flush()
    try: ev = json.loads(line)
    except Exception: continue
    if not sent and ev.get("type") == "assistant" and any(c.get("type") == "tool_use" for c in ev["message"]["content"]):
        time.sleep(3)
        user("Also, in the same reply, include the word BANANA.")
        time.sleep(1)
        send({"type": "control_request", "request_id": "sn-1", "request": {"subtype": "interrupt", "send_now": True}})
        sent = True
    if ev.get("type") == "result":
        results += 1
        # Give a second result a chance (if the message became its own turn), then close.
        threading.Timer(25, lambda: p.stdin.close()).start()
p.wait()
log.write(json.dumps({"_exit": p.returncode, "_results": results, "_t": round(time.time()-t0, 2)}) + "\n")
