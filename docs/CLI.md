# anchorleg CLI and JSON contract

anchorleg is a standalone CLI (D10). Other programs use it as a subprocess: run a command, read
one JSON object from stdout, check the exit code. Every JSON object has a `schema` field; a
change that breaks existing readers bumps it. Source of truth: `crates/anchorleg-core/src/output.rs`.

Logs go to stderr (`ANCHORLEG_LOG=debug` for more). stdout carries only the result.

## Files and environment

| What | Default | Override |
|---|---|---|
| Accounts | `~/.config/anchorleg/config.toml` | `ANCHORLEG_CONFIG` |
| Quota store + decision log | `~/Library/Application Support/anchorleg/anchorleg.db` | `ANCHORLEG_HOME` (dir) |
| Raw CLI output per run | `<store dir>/runs/run-<id>.jsonl` | `ANCHORLEG_HOME` |
| Tokens (only for `token_from = "keychain"`) | macOS Keychain, service `anchorleg` | — |

## Commands

| Command | Status |
|---|---|
| `anchorleg accounts list [--json]` · `show <name>` · `add <name> …` · `rm <name>` · `import-aliases [--yes] [--from-file F]` | works |
| `anchorleg ui` | works: session manager, keyboard + mouse |
| `anchorleg sessions [--json] [--limit N]` | works |
| `anchorleg permission list [--json]` · `answer <id> yes\|always\|no` · `ask --json` (anchorleg-mod) | works (Claude) |
| `anchorleg settings [<cli>] [--model M] [--effort E] [--args "…"] [--clear]` | works |
| `anchorleg stop <run>` | works |
| `anchorleg status [--json]` | works |
| `anchorleg report --json` (stdin) | works |
| `anchorleg run [--follow-up RUN] [--cwd DIR] [--json] [--no-wait] [--model M] [--stop-at F] [--fresh-above N] [--no-mod] -- <prompt…>` | works (Claude accounts) |

### Session manager: `anchorleg ui`

```
┌ Sessions ──────────┐┌ #12 · claude-sm · running · ~/repo ─────────┐
│ + New session      ││ you   fix the failing auth tests            │
│ ● #12 claude-sm …  ││   ⚙ Bash  cargo test auth                   │
│ ✓ #11 two …     ││ anchorleg claude-sm paused (quota at 91%) …     │
├ Agents ────────────┤│ agent All 14 auth tests pass now.           │
│ 1 claude-sm ██ 41% │├ Reply to #12 (Enter to send) ───────────────┤
│   Codex  not inst. ││ > …                                          │
└────────────────────┘└──────────────────────────────────────────────┘
 New (n)  Send (⏎)  Stop (s)  Help (?)  Quit (q)
```

- **Sessions**: every run, newest first (● working, ✓ done, ✗ failed, ■ stopped, ↳ a reply).
- **Conversation**: the selected session, live, in Claude Code's style: your messages on a grey
  band, then the agent's text and tool calls (`⏺`), and what anchorleg did (switches, pauses). A
  reply shows the whole session. A tool call waiting for approval shows its question here.
- **Message box**: with “+ New session” selected, a folder and a task start a new session;
  with a session selected, Enter sends a reply into it (`anchorleg run --follow-up`). A session
  that is still working can't take a reply until it finishes or is stopped.
- **Slash commands**: `/model` and `/effort` are anchorleg's (above). Any other `/command` is sent to
  the CLI, which runs skills, custom commands and built-ins like `/compact` in `-p`; typing `/`
  lists what the session's CLI offers (Tab completes, ↑↓ choose).
- **Agents**: accounts in the order they're used, with the fullest quota window; other agent
  CLIs shown as installed or not; Codex and Antigravity can be added as accounts, the others
  aren't usable yet.

Sessions run as background `anchorleg run` processes and keep going when the UI closes.
Keys: Tab moves between panes, `n` new, `/` type, Enter send, Esc leave the box, `s` stop,
`?` all keys, `q` quit; in Agents: Shift+↑↓ reorder, space on/off, `i` import, `o` settings of
that account's CLI, `d d` remove.
Everything is clickable, and the lists and conversation scroll with the wheel.

### Accounts

An account is the command you'd type to run that account (D9):

```bash
anchorleg accounts import-aliases            # preview what your shell aliases would add
anchorleg accounts import-aliases --yes      # write them
anchorleg accounts add work --cmd "CLAUDE_CONFIG_DIR=~/.claude-work ~/.local/bin/claude" --priority 1
anchorleg accounts add ci --token-stdin      # paste a `claude setup-token` token; stored in Keychain
anchorleg accounts add odd --shell "my-claude-alias"   # escape hatch: runs through $SHELL -ic
anchorleg accounts show work                 # the exact launch line anchorleg will use (tokens as ***)
```

`config.toml`:

```toml
[[account]]
name = "claude-sm"
vendor = "claude"          # claude | codex | kimi | cursor
priority = 1               # lower is tried first; default 100
bin = "~/.local/bin/claude"
env = { CLAUDE_CONFIG_DIR = "~/.claude-sm" }
# args = ["--model", "sonnet"]   # placed before anchorleg's own args
# token_from = "keychain"        # instead of env; Claude only
# shell = "claude-sm"            # instead of bin/args
# enabled = false                # keep it configured but never use it
```

Before launching, anchorleg removes inherited variables that would select another account
(Claude: `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`,
`CLAUDE_CONFIG_DIR`), then applies the account's own.

## `anchorleg run`

Picks an account (priority order, skipping blocked or ≥ 95% ones), runs
`<account launch> -p <prompt> --output-format stream-json --verbose [--model M]` in `--cwd`
with stdin closed, and records quota from the stream. On a limit hit it blocks that account
until its reset, copies the session (transcript, subagent transcripts, and newer project
`memory/` files) into the next account's config dir and resumes it with
"Continue from where you stopped." If the next account can't see the session, it starts once
more from a handoff prompt. Without `--json`, progress goes to stderr and the final message to stdout.

anchorleg-mod is loaded into every Claude launch (`--plugin-dir`, embedded in the binary; `--no-mod`
turns it off). It reports exact quota and, once any window reaches `--stop-at` (default 0.9),
stops the run before the next model request. anchorleg then switches to an account below that
threshold and resumes the session (switch rule `quota_stop`); if there's none, it continues on
the same account until the hard limit (`quota_stop_stay`).

### Handoff and checkpoints

Whenever a launch stops early (limit hit, quota pause, lost session), anchorleg writes
`<cwd>/.handoff/TASK.md`: the goal and later messages, the agent's task list, its last message,
files it wrote, edited and read, recent commands and `git status`. Any agent (or you) can finish
the task from it. Agents keep their decisions under `## Notes`; anchorleg keeps that section when it
rewrites the file. `.handoff/` is git-ignored.

In a git repo anchorleg also saves the working tree as a local commit `refs/anchorleg/run-<id>/<n>`
without touching your branch, index, stash or files. It never pushes.

```bash
git for-each-ref refs/anchorleg              # list checkpoints
git diff refs/anchorleg/run-12/1             # what changed since the switch
git checkout refs/anchorleg/run-12/1 -- .    # restore the files as they were at the switch
git update-ref -d refs/anchorleg/run-12/1    # delete one
```

At a switch, a session with more than `--fresh-above` tokens of context (default 100000) starts
fresh on the next account from the handoff instead of being resumed on a cold cache.

### Approving tool calls

A headless Claude run can't show its permission dialog, so by default it refuses anything that
needs one (writing files under the default mode, for example). anchorleg-mod passes those questions
to anchorleg instead, and the run waits for you:

- `anchorleg ui`: the session gets a `!`, the top bar says how many are waiting, and the
  conversation shows `Allow Write(src/a.rs)?` with **1** Yes, **2** Yes for this session (that
  tool, for the rest of the session, also after a switch), **3** No. Keys or clicks.
- Terminal: `anchorleg permission list`, then `anchorleg permission answer <id> yes|always|no`.

Only calls Claude itself would have asked about come here; nothing is allowed without an answer.
After 30 minutes with no answer, or if the run ends, the call is refused as before. The model
reads a "no" as a refusal and carries on. Per-CLI settings (below) can still pre-approve, e.g.
`--permission-mode acceptEdits`.

### Per-CLI settings: model, effort, arguments

One set per provider, shared by all its accounts, so a task keeps the same model and effort
when anchorleg moves it to another account (D20). They go in `[vendor.<cli>]` and reach only that
CLI:

```bash
anchorleg settings                                                  # list
anchorleg settings claude --model opus --effort high                # every Claude account
anchorleg settings claude --model default                           # back to the CLI's default
anchorleg settings claude --args "--permission-mode acceptEdits"    # Claude runs may edit files
anchorleg settings claude --clear
```

```toml
[vendor.claude]
model = "opus"
effort = "high"            # Claude: low, medium, high, xhigh, max
args = ["--permission-mode", "acceptEdits"]
```

`anchorleg run --model M` overrides the model for one run. In `anchorleg ui`, type `/model opus` or
`/effort high` in the message box (no argument shows the current value); it applies to the
selected session's provider from its next launch.

Extra `args` go before the account's own `args` (so an account can override them) and never reach
another CLI. Flags anchorleg sets itself (`-p`, `--output-format`, `--resume`, `--plugin-dir`, …)
are refused. In `anchorleg ui`: select an account in Agents, press `o` (or click Settings), edit,
Enter. Under Claude's default permission mode, `-p` runs can't write files; anchorleg sets no
default for you.

## `anchorleg run --json` → `RunReport`

```json
{
  "schema": 1,
  "run_id": 12,
  "outcome": "done",
  "result": "All tests pass.",
  "session_id": "4f6c…",
  "accounts_used": ["claude-sm", "claude-two"],
  "switches": [{ "from": "claude-sm", "to": "claude-two", "rule": "rejected", "at": 1791450000 }],
  "wait_until": null
}
```

| `outcome` | Exit code | Meaning |
|---|---|---|
| `done` | 0 | The task finished |
| `failed` | 1 | The CLI reported an error, or anchorleg hit one |
| — | 2 | Bad arguments |
| `all_blocked` | 75 | Every account blocked and `--no-wait`; see `wait_until` |
| `no_accounts` | 78 | No account configured for this task |
| `stopped` | 130 | `anchorleg stop` or Ctrl-C |

## `anchorleg status --json` → `StatusReport`

```json
{
  "schema": 1,
  "now": 1791442500,
  "next": { "action": "use", "account": "claude-two", "reason": "available" },
  "accounts": [
    {
      "name": "claude-sm", "vendor": "claude", "priority": 1,
      "blocked_until": 1791450000,
      "windows": [
        { "window": "five_hour", "status": "rejected", "used": 1.0,
          "resets_at": 1791450000, "source": "stream", "updated_at": 1791442000 }
      ]
    }
  ]
}
```

`next.action` is `use` (`reason`: `available` or `near_limit`), `wait` (`until`, `account`), or
`no_accounts`. `used` is a fraction 0–1 and may be `null` (Claude often omits it).
`source` is `stream` (CLI events), `mod` (exact, from anchorleg-mod) or `manual`.

## `anchorleg report --json` (stdin)

For anchorleg-mod (Phase 2), which converts its own numbers to this shape:

```json
{ "account": "claude-sm",
  "readings": [ { "window": "five_hour", "status": "warning", "used": 0.91, "resets_at": 1791450000 } ] }
```

`used` must be 0–1; `status`, `used`, `resets_at` are optional; unknown fields are rejected.
anchorleg-mod adds `"run_id"` and `"stop_reason"` to ask anchorleg to stop that run at this point.
Missing fields keep their last known value.
