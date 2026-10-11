#!/usr/bin/env python3
"""Stand-in for `codex app-server` in anchorleg's end-to-end tests. Spends no quota.

The account is the basename of $CODEX_HOME. Messages are JSON lines on stdin/stdout, as with the
real app server (fixtures/codex-0.153.4-app-server-*).

- `account/rateLimits/read` (anchorleg's live quota read): answers with $FAKE_DIR/X.limits.json as
  the result, or an auth error when $FAKE_DIR/X.nologin exists, appends X to
  $FAKE_DIR/probes.log, and exits. It isn't a launch.
- `thread/start` / `thread/resume`: the n-th launch for account X plays $FAKE_DIR/X.n.jsonl if it
  exists, else $FAKE_DIR/X.jsonl. Like the real CLI, a thread lives in a rollout file under
  $CODEX_HOME/sessions/, and resuming a thread this login has no rollout for fails with
  "no rollout found". Each launch appends $FAKE_DIR/X.rollout.jsonl (quota lines) to the rollout.
- `turn/start`: appends "<account> <args…> <thread request> model=… effort=… sandbox=…
  approval=… -- <text>" to $FAKE_LOG (one line per turn), then prints the script's messages.
  A request in the script (it has an "id") waits for anchorleg's answer, which is appended to
  $FAKE_DIR/answers.log; a "cancel" ends the turn as interrupted, like the real CLI. After an
  `account/rateLimits/updated` it waits a moment for a `turn/interrupt` (anchorleg's soft stop),
  which ends the turn as interrupted and is appended to $FAKE_DIR/interrupts.log. A later turn
  in the same launch plays $FAKE_DIR/X.followup.jsonl.
"""

import glob
import json
import os
import select
import sys

account = os.path.basename(os.environ["CODEX_HOME"].rstrip("/"))
fake = os.environ["FAKE_DIR"]
args = sys.argv[1:]
pre_args = args[: args.index("app-server")] if "app-server" in args else args


def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def read():
    line = sys.stdin.readline()
    return json.loads(line) if line.strip() else None


def rollout(tid):
    found = glob.glob(f"{os.environ['CODEX_HOME']}/sessions/*/*/*/rollout-*-{tid}.jsonl")
    return found[0] if found else None


thread = None  # (method, params, tid)
turns = 0
script_lines = []
while True:
    msg = read()
    if msg is None:
        sys.exit(0)
    method = msg.get("method")
    params = msg.get("params") or {}
    if method == "initialize":
        send({"id": msg["id"], "result": {"userAgent": "fake-codex"}})
    elif method == "account/rateLimits/read":
        with open(f"{fake}/probes.log", "a") as f:
            f.write(account + "\n")
        if os.path.exists(f"{fake}/{account}.nologin"):
            send({"id": msg["id"], "error": {"code": -32600, "message": "codex account authentication required to read rate limits"}})
        elif os.path.exists(f"{fake}/{account}.limits.json"):
            send({"id": msg["id"], "result": json.load(open(f"{fake}/{account}.limits.json"))})
        else:
            send({"id": msg["id"], "result": {"rateLimits": None}})
        sys.exit(0)
    elif method in ("thread/start", "thread/resume"):
        log = os.environ["FAKE_LOG"]
        n = 1 + sum(1 for l in open(log) if l.startswith(account + " ")) if os.path.exists(log) else 1
        script = f"{fake}/{account}.{n}.jsonl"
        if not os.path.exists(script):
            script = f"{fake}/{account}.jsonl"
        script_lines = [l for l in open(script).read().splitlines() if l.strip()]
        tid = params.get("threadId")
        if method == "thread/resume" and not rollout(tid):
            with open(log, "a") as f:
                f.write(f"{account} {' '.join(pre_args)} thread/resume {tid} (not found)\n".replace("  ", " "))
            send({"id": msg["id"], "error": {"code": -32600, "message": f"no rollout found for thread id {tid}"}})
            continue
        if not tid:
            for l in script_lines:
                tid = (json.loads(l).get("params") or {}).get("threadId")
                if tid:
                    break
        thread = (method, params, tid)
        path = rollout(tid)
        if not path:
            os.makedirs(f"{os.environ['CODEX_HOME']}/sessions/2026/10/08", exist_ok=True)
            path = f"{os.environ['CODEX_HOME']}/sessions/2026/10/08/rollout-2026-10-08T00-00-00-{tid}.jsonl"
        with open(path, "a") as f:
            f.write('{"type":"session_meta"}\n')
            if os.path.exists(f"{fake}/{account}.rollout.jsonl"):
                f.write(open(f"{fake}/{account}.rollout.jsonl").read())
        send({"id": msg["id"], "result": {"thread": {"id": tid, "path": path}}})
    elif method == "turn/start":
        turns += 1
        how, tp, tid = thread
        text = " ".join(i.get("text", "") for i in params.get("input", [])).replace("\n", " ")
        what = how + (f" {tid}" if how == "thread/resume" else "")
        line = (
            f"{account} {' '.join(pre_args)} {what} model={tp.get('model')} "
            f"effort={params.get('effort')} sandbox={tp.get('sandbox')} "
            f"approval={tp.get('approvalPolicy')} -- {text}"
        ).replace("  ", " ")
        with open(os.environ["FAKE_LOG"], "a") as f:
            f.write(line + "\n")
        send({"id": msg["id"], "result": {"turn": {"id": f"u{turns}", "status": "inProgress"}}})
        lines = script_lines
        if turns > 1:
            follow = f"{fake}/{account}.followup.jsonl"
            lines = open(follow).read().splitlines() if os.path.exists(follow) else []
        for l in lines:
            if not l.strip():
                continue
            out = json.loads(l)
            send(out)
            if out.get("method") == "account/rateLimits/updated":
                ready, _, _ = select.select([sys.stdin], [], [], 2.0)
                incoming = read() if ready else None
                if incoming and incoming.get("method") == "turn/interrupt":
                    with open(f"{fake}/interrupts.log", "a") as f:
                        f.write(json.dumps(incoming) + "\n")
                    send({"id": incoming["id"], "result": {}})
                    send({"method": "turn/completed", "params": {"threadId": tid, "turn": {"id": f"u{turns}", "status": "interrupted", "error": None}}})
                    break
            if "id" in out and "method" in out:
                answer = read()
                with open(f"{fake}/answers.log", "a") as f:
                    f.write(json.dumps(answer) + "\n")
                if ((answer or {}).get("result") or {}).get("decision") == "cancel":
                    send({"method": "turn/completed", "params": {"threadId": tid, "turn": {"id": f"u{turns}", "status": "interrupted", "error": None}}})
                    break
