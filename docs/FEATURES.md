# anchorleg features

Everything anchorleg does, with the command for each. The [README](../README.md) has the short
version; [CLI.md](CLI.md) has every option and the JSON contract.

## Features

- **Switches before you hit the wall.** anchorleg sees exact usage while the task runs (a small
  mod inside Claude Code; Codex reports it itself) and stops at a step boundary once any window
  reaches 90% (`--stop-at`). If no account has more room, the task carries on until the real
  limit, then switches.
- **Keeps the context.** The next account gets the session transcript, subagent transcripts and
  project memory, and resumes the session. When the context is large (over 100k tokens by
  default) it starts fresh from the handoff instead, so the next account doesn't pay for an
  uncached re-read.
- **A session manager in your terminal.** `anchorleg ui` lists every session with its live
  conversation (in Claude Code's style), takes new tasks and replies, and shows each account's
  quota. Keyboard first; the mouse works too.
- **Talk to it while it works.** Type a message to a working session (Enter in the UI, or
  `anchorleg send`): Claude Code and Codex take it into the turn that's running, at the next tool
  result. Ctrl+X (`--interrupt`) has it read now, stopping a running command if needed.
- **Send screenshots and files.** Paste an image (Ctrl+V) or drop a file into `anchorleg ui`, or
  pass `--attach`; both Claude Code and Codex read it, also after a switch.
- **See what it changed.** `v` in the UI opens a Changes panel with every file the session
  changed and its diff, as the CLI reported it, in any folder (or `anchorleg diff`). The Files
  tab (`f`, or **Files ▾** on the conversation) lists the files it created, docs and images
  included, and opens them. The agent's task list shows under the conversation as it works.
- **Tells you when it needs you.** A desktop notification when a session waits for an answer,
  finishes, fails or switches accounts; `anchorleg notify --command …` sends the same to your own
  script (a chat bot, a phone push).
- **Work toward a goal.** `--goal "cargo test passes"` (or `/goal` in the UI): the session keeps
  going, turn after turn, until the agent shows the condition holds, even when it moves to another
  account or another CLI on the way.
- **A second opinion.** `anchorleg review 12` (or `/review` in the UI): another account, or
  another CLI with `--cli codex`, reviews what session #12 changed, without touching files;
  `--fix` sends the findings back to #12 to fix.
- **A budget per task.** `--budget 2M` caps the tokens a task uses across every account and
  CLI; the agent stops at a clean point when it's spent.
- **Typed answers.** `--json-schema schema.json` makes the final answer follow your schema and
  returns it parsed in the JSON report, for scripts and task delegators.
- **Your existing sessions too.** `anchorleg sessions import` brings in the Claude Code and Codex
  sessions you started yourself, on every account, so you can see them in one place and continue
  any of them through anchorleg.
- **The same setup on every CLI.** `--add-dir`, `--instructions` and `--effort` go with the task
  to whichever account or CLI runs it; Claude Code's own `--agent`, `--allowed-tools`,
  `--mcp-config` and the like are passed through too.
- **Plan first.** `--plan` (or `/plan` in the UI): the agent plans in Claude's or Codex's plan
  mode, you approve, say what to change or edit the plan yourself (`e`), then the same session
  carries it out.
- **Subagents.** `anchorleg sessions agents 12`: every subagent the session started, working or
  done, with what it was asked, its steps and what it reported (Claude Code and Codex).
- **Side questions and rewind.** `anchorleg btw 12 -- "why this approach?"` (`/btw`): a read-only
  copy of the session answers while the session keeps working. `anchorleg rewind 14 -- "…"`
  (`/rewind`): back to just before run #14, conversation and files, and on from there; `--at`
  goes back to any message inside a run (the desktop app's "Edit and send again").
- **Named sessions.** `--name "auth fix"`, `anchorleg rename` or `/rename`; the CLI gets the name
  too.
- **The agent can ask you.** When Claude or Codex needs a decision, the question and its options
  show up in `anchorleg ui` (or `anchorleg permission list`) and the run waits for your answer.
- **You approve what the agent may do.** A background run can't show the CLI's permission
  prompt, so anchorleg asks you, with the diff for an edit: the session waits until you press
  **1** yes, **2** yes for this session, **3** no or **4** always, as a lasting rule
  (`anchorleg rules`, in Claude Code's syntax, e.g. `Bash(cargo test *)`, applied to every CLI).
  Nobody watching? `--mode auto` lets Claude Code's auto mode or Codex's auto-review decide
  instead, and shows what it decided.
- **No surprises from the CLI.** The conversation notes when the CLI answers with another model
  than you asked for, what your hooks blocked or said, and background commands that ended with
  a run.
- **One setup for every CLI.** `anchorleg sync mcp` gives every account the same MCP servers,
  through each CLI's own `mcp add`; a session that moves to another CLI takes the project's
  instructions for the old one (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`) along.
- **See what you're using.** `anchorleg usage` (and `u` in `anchorleg ui`) shows tokens and
  cost per account and model, tokens per day, quota bars and each account's usage credits.
  Cost is the API list price of the same tokens, so you can see what your subscriptions save.
- **Never spends money behind your back.** Usage credits (Claude extra usage, Codex credits)
  are off limits unless you allow them per account: as a last resort, or always.
- **Same model and effort on every account.** One setting per provider (`/model opus`,
  `/effort high`), so a task keeps the same strength when it moves. Your own slash commands and
  skills pass straight through.
- **Safe with your repo.** Checkpoints are local refs under `refs/anchorleg/`; anchorleg never touches
  your branch, index or stash, and never pushes.
- **Scriptable.** Every command has `--json` output and stable exit codes, so a task delegator, a
  CI job or another agent can run anchorleg as a subprocess.

## The session manager (`anchorleg ui`)

<p align="center">
  <img src="images/ui.svg" width="880" alt="anchorleg ui: sessions and agents with quota bars on the left, the live conversation and a reply box on the right">
</p>

| Key | What it does |
|---|---|
| `n` | New session: pick the folder, type the task, Enter |
| `/` or `m` | Type a message: a reply to the selected session, or a new task |
| `Enter` | Send |
| `1` `2` `3` | Answer a waiting tool call: yes · yes for this session · no |
| `s` | Stop the selected session |
| `v` | What the selected session changed (a diff) |
| `U` `U` | Undo the selected session's changes (`anchorleg undo --redo` brings them back) |
| `Tab` | Next pane (in the message box: complete a slash command) |
| `?` | All keys |

Sessions run as background processes, so they keep going when you close the UI.

## Commands, by example

```bash
anchorleg run --cwd ~/code/app -- "fix the failing auth tests"
anchorleg run --follow-up 12 -- "now add a test for the expired-token case"
anchorleg sessions                  # recent sessions
anchorleg status                    # quota per account, and who runs next
anchorleg usage                     # tokens and API-price cost per account and model, last 7 days
anchorleg send 12 -- "also update the docs"   # message a working session (--interrupt: now)
anchorleg run --attach error.png -- "fix this crash"   # attach images or files (also on send)
anchorleg run --plan -- "split the auth module"         # plan first; approve, then it runs
anchorleg run --goal "cargo test passes" -- "migrate to the new API"   # keep going until it holds
anchorleg review 12 --cli codex --fix                   # another CLI reviews #12; #12 fixes the findings
anchorleg run --budget 2M -- "port the CLI to clap 5"   # stop once the task has used 2M tokens
anchorleg run --json --json-schema out.json -- "list the flaky tests"   # typed result in the report
anchorleg run --worktree -- "try the new parser"        # in a git worktree of its own
anchorleg run --fork 12 -- "now try it with a macro"    # a new session from #12's history
anchorleg btw 12 -- "which database does this use?"     # a side question; #12 isn't disturbed
anchorleg rewind 14 -- "use a macro instead"            # back to before run #14, talk and files
anchorleg rules --add "Bash(cargo test *)"              # allowed in every session, on every CLI
anchorleg sync mcp --yes                                # the same MCP servers on every account
anchorleg stop 12                   # stop a session and its CLI
anchorleg diff 12 --stat            # what the session changed, as the CLIs reported it
anchorleg permission list           # tool calls waiting for you
anchorleg permission answer 3 yes   # yes | always | rule | no
anchorleg permission answer 4 2     # an agent's question: option number, label or your own words
```

## Supported CLIs in detail

| CLI | Status |
|---|---|
| Claude Code (`claude`) | Supported: switching, handoff, approvals, model and effort |
| Codex (`codex`, also bundled with the ChatGPT app) | Supported: live quota before and during every turn, switching, handoff, model and effort, approvals and the soft stop at `--stop-at`. Runs through `codex app-server` with a workspace-write sandbox: edits inside the task folder go ahead, and what Codex asks about (network, writes elsewhere) waits for you |
| Antigravity (`agy`) | **Under development**, not yet validated end to end: switching on quota errors, handoff, model and effort. Quota isn't visible. One account per `HOME`. An account with its own `HOME` gets approvals: edits inside the task folder go ahead, and shell commands and anything else wait for you (anchorleg adds an `anchorleg` hook to that `HOME`'s `.gemini/config/hooks.json`), and the agent can ask you questions through anchorleg's `ask_user` MCP tool (added to `.gemini/config/mcp_config.json`; not yet seen in a real run). On your own `HOME`, edits are allowed (`--mode accept-edits`) and shell commands need an allow rule in agy's settings |
| Kimi, Cursor (`cursor-agent`) | Planned |
