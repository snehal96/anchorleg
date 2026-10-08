#!/usr/bin/env python3
"""Anonymise captured CLI output before it goes into fixtures/ (the repo is public).

    scripts/scrub-capture.py fixtures/<folder>/*.jsonl

Rewrites the files in place:
- home folders, temp folders and session scratch paths -> neutral ones (/Users/me, /tmp/...)
- your username (anywhere) and account names in RENAME -> neutral names
- init events: drops the installed plugins, MCP connectors, skills and personal slash commands
  (keeps Claude's built-in commands, which tests use); commands_changed events: keeps only
  built-in commands
- drops SessionStart hook output lines (they echo the person's own hooks)

List your own account names in scripts/scrub-names.local first. Review the diff before committing.
"""

import getpass
import json
import re
import sys

HOME = re.compile(r"/Users/[A-Za-z0-9._-]+")
HOME_DASHED = re.compile(r"-Users-[A-Za-z0-9._]+(?=-)")
PATHS = [
    # Claude Code session scratchpads: /private/tmp/claude-502/<project>/<session-uuid>/scratchpad
    (re.compile(r"(/private)?/tmp/claude-\d+/[^\"/\s]+/[0-9a-f-]{36}/scratchpad"), "/tmp/scratch"),
    (re.compile(r"-private-tmp-claude-\d+-[^\"/\s]*?-[0-9a-f]{8}-[0-9a-f-]{27}-scratchpad"), "-tmp-scratch"),
    # macOS per-user temp dirs
    (re.compile(r"(/private)?/var/folders/[a-z0-9_]+/[A-Za-z0-9_]+/T"), "/tmp"),
    (re.compile(r"-private-var-folders-[a-z0-9_]+-[A-Za-z0-9_]+-T"), "-tmp"),
]
USER = re.compile(r"\b" + re.escape(getpass.getuser()) + r"\b")
# Your own account names -> neutral ones, one "old new" pair per line in
# scripts/scrub-names.local (git-ignored, so the real names never reach the repo).
RENAME: dict[str, str] = {}
try:
    with open(__file__.rsplit("/", 1)[0] + "/scrub-names.local", encoding="utf-8") as _f:
        for _line in _f:
            if _line.strip() and not _line.startswith("#"):
                _old, _new = _line.split()
                RENAME[_old] = _new
except FileNotFoundError:
    pass
# Claude's own commands; anything else in an init list is the person's.
BUILTIN = {
    "compact", "context", "clear", "config", "cost", "effort", "model", "mcp", "init",
    "review", "security-review", "usage", "code-review", "simplify", "debug", "verify",
    "batch", "loop", "schedule", "agents", "output-style", "fast", "rename", "recap",
    "doctor", "color", "focus", "reload-plugins", "reload-skills",
}
PERSONAL_LISTS = ("mcp_servers", "plugins")
FILTERED_LISTS = ("slash_commands", "skills", "terminal_slash_commands")


def text(s: str) -> str:
    for pattern, repl in PATHS:
        s = pattern.sub(repl, s)
    s = HOME.sub("/Users/me", s)
    s = USER.sub("me", s)  # e.g. the owner column of `ls -l` in a tool result
    s = HOME_DASHED.sub("-Users-me", s)
    for old, new in RENAME.items():
        s = s.replace(old, new)
    return s


def scrub_event(v: dict) -> dict | None:
    if v.get("type") == "system" and str(v.get("subtype", "")).startswith("hook_"):
        return None
    if v.get("type") == "system" and v.get("subtype") == "init":
        for key in PERSONAL_LISTS:
            if key in v:
                keep = [p for p in v[key] if isinstance(p, dict) and str(p.get("name", "")).startswith("anchorleg")]
                v[key] = keep
        # MCP tools are named after the person's connectors (mcp__<server>__<tool>).
        if isinstance(v.get("tools"), list):
            v["tools"] = [t for t in v["tools"] if not str(t).startswith("mcp__")]
        for key in FILTERED_LISTS:
            if key in v:
                v[key] = [c for c in v[key] if c in BUILTIN]
    # The full command list (with descriptions) names every skill, plugin and connector the
    # person has installed: keep only Claude Code's own.
    if v.get("type") == "system" and v.get("subtype") == "commands_changed":
        v["commands"] = [c for c in v.get("commands", []) if isinstance(c, dict) and c.get("builtin")]
    return v


def scrub_file(path: str) -> None:
    with open(path, encoding="utf-8") as f:
        lines = f.read().splitlines()
    out = []
    for line in lines:
        try:
            v = json.loads(line)
        except ValueError:
            out.append(text(line))
            continue
        if isinstance(v, dict):
            v = scrub_event(v)
            if v is None:
                continue
        out.append(text(json.dumps(v, ensure_ascii=False, separators=(",", ":"))))
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(out) + ("\n" if out else ""))


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    for p in sys.argv[1:]:
        scrub_file(p)
        print(f"scrubbed {p}")
