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
| `anchorleg accounts list [--json]` · `show <name>` · `add <name> …` · `rm <name>` · `set <name> [--priority N] [--reserve-7d F] [--pinned true\|false] [--enabled true\|false] [--credits never\|last-resort\|allow]` · `import-aliases [--yes] [--from-file F]` | works |
| `anchorleg policy [drain\|balance\|save-weekly]` | works |
| `anchorleg notify [--desktop on\|off] [--command CMD \| --no-command] [--test]` | works (macOS, Linux) |
| `anchorleg ui` | works: session manager, keyboard + mouse |
| `anchorleg sessions [--json] [--limit N]` | works |
| `anchorleg permission list [--json]` · `answer <id> yes\|always\|no` · `answer <id> <answer>… \| --skip` (questions) · `ask --json` (anchorleg-mod) | works (Claude, Codex; agy under development) |
| `anchorleg settings [<cli>] [--model M] [--effort E] [--args "…"] [--clear]` | works |
| `anchorleg stop <run>` | works |
| `anchorleg diff <run> [--stat]` · `anchorleg undo <run> [--yes \| --redo]` | works (git folders) |
| `anchorleg send <run> [--interrupt] [--attach FILE]… -- <message…>` | works (Claude, Codex, Antigravity) |
| `anchorleg status [--json]` | works |
| `anchorleg usage [--since 24h\|7d\|30d\|all] [--json]` | works |
| `anchorleg report --json` (stdin) | works |
| `anchorleg review <run> [--cli claude\|codex\|antigravity] [--account A] [--fix] [--json] [--no-wait] [-- <notes…>]` | works (Claude, Codex, Antigravity; git folders) |
| `anchorleg run [--follow-up RUN \| --fork RUN \| --review RUN [--cli C] [--fix]] [--worktree [NAME]] [--cwd DIR] [--plan] [--goal CONDITION [--goal-turns N]] [--mode ask\|edits] [--answer-wait DURATION] [--attach FILE]… [--json] [--no-wait] [--model M] [--stop-at F] [--fresh-above N] [--no-mod] [--policy P] [--prefer A \| --account A] -- <prompt…>` | works (Claude, Codex; Antigravity under development) |

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
  the CLI, which runs skills, custom commands and built-ins in `-p`; typing `/`
  lists what the session's CLI offers (Tab completes, ↑↓ choose).
  - `/clear` starts a new session in the selected session's folder (the old one stays in the list).
  - `/compact` summarises a finished session to free its context: Claude Code runs its own
    `/compact`, Codex gets `thread/compact/start`, Antigravity expands its own `/compact`. The
    conversation notes it (with the token counts Claude reports).
- **Shell commands**: `!cmd` runs `cmd` with `sh` in the task folder (up to 60 s) and shows its
  output in the conversation. Nothing goes to the agent yet: the output (up to 8 KB) is put in
  front of the next message you send in that folder, as Claude Code does with `!`.
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

Picks an account (see *Switch policies* below), runs
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

Codex runs get the same soft stop without a mod: Codex reports quota after every model call
(`account/rateLimits/updated`), and once a window reaches `--stop-at` anchorleg interrupts the
turn there, before the tools that call asked for run, then switches or stays the same way.
Antigravity reports no quota, so it switches on quota errors only.

### Switch policies

Every pick skips disabled and blocked accounts (rejected until a reset; 1 hour when the CLI gave
no reset time). An account at 95% or more in any window, or inside its weekly reserve
(`reserve_7d`: weekly use at or above `1 - reserve_7d`), is used only when no healthier account
is free, and then the least used of those wins; if everything is blocked, the run waits for the
earliest reset (or exits 75 with `--no-wait`). Among the healthy accounts:

- `--account A` limits every pick to A (no switching).
- `--prefer A`, else an account with `pinned = true`, goes first.
- Otherwise the policy (`--policy`, else `policy` in `config.toml`, set with `anchorleg policy`):
  `drain` takes the first by priority; `balance` the lowest fullest-window use; `save-weekly`
  the lowest weekly (`seven_day`) use, then the lowest fullest-window use. Ties keep priority
  order, and accounts with no quota data count as unused.

The same rules pick the account after a soft stop (with `--stop-at` as the threshold) and after a
limit hit. `anchorleg status` shows the next pick under the configured policy; the run's `start`
decision records the policy used.

```toml
policy = "balance"

[[account]]
name = "home"
reserve_7d = 0.2
pinned = false
credits = "last-resort"
```

### Usage credits

Claude's "extra usage" and Codex credits let an account keep working past its plan's limits,
billed to you. anchorleg reads their state from the CLI (Claude: `overageStatus`,
`isUsingOverage` in `rate_limit_event`; Codex: `credits` in its rate limits) and shows it in
`anchorleg status`, `anchorleg usage` and `anchorleg ui`. Each account's `credits` setting
decides whether anchorleg lets it spend them:

| `credits` | During a run | When picking an account |
|---|---|---|
| `never` (default) | the moment the CLI starts paying from credits, the launch is stopped (no grace period), the account is blocked for an hour and the task moves on | never picked for credits |
| `last-resort` | stopped like `never` | when every account is blocked, an account that reported credits available is picked (reason `credits`) instead of waiting, and that launch may pay |
| `allow` | keeps going on credits; a limit hit doesn't stop it unless the run then fails | as `last-resort` |

Tokens a launch spends while paying are marked in `anchorleg usage` (`credits_cost_usd`).
Not yet seen on a real account: the exact events Claude sends once extra usage kicks in, and
whether Codex spends credits by itself. Today's handling is built from the fields above.

## `anchorleg usage` → `UsageReport`

Tokens per account and model, recorded from every launch's result (Claude: `modelUsage`, with
its list-price `costUSD`; Codex: the turn's token usage; Antigravity: `result.usage`). The text
version adds a tokens-per-day sparkline and each account's quota and credits. `cost_usd` is what
the same tokens would cost at API list prices; on a subscription you don't pay it, and Codex and
Antigravity don't report it (`null`).

```json
{
  "schema": 1, "now": 1791442500, "since": 1790837700,
  "totals": [
    { "account": "claude-sm", "model": "claude-sonnet-5-5", "launches": 4,
      "input_tokens": 1200, "output_tokens": 8400, "cache_read_tokens": 412000,
      "cache_write_tokens": 36000, "cost_usd": 1.84, "credits_cost_usd": null }
  ],
  "by_day": [[1791417600, 457600, 1.84]]
}
```

`input_tokens` excludes cached input everywhere (Codex and Antigravity count it inside; anchorleg
splits it out). `by_day` is `[UTC day start, tokens, cost]`.

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

### Messages while it works

`anchorleg send <run> -- <message>` (or Enter in `anchorleg ui` on a working session) reaches the
agent without stopping the session:

- **Codex** takes it into the running turn (`turn/steer`), as typing in Codex does.
- **Claude Code** and **Antigravity** have no way to take a message mid-turn in `-p`, so it waits
  until the turn ends; then anchorleg continues the same session with it, in the same run.

`--interrupt` (Ctrl+X in the UI) reads it now: Codex's turn is interrupted; Claude stops before
its next model request (anchorleg-mod checks before each one, like the quota stop), never in the
middle of a tool call; Antigravity is stopped at once (it has no clean-point hook) and its
conversation resumed with the message. Either way the session then continues with your message. Messages not
delivered yet show in the conversation as `(queued)`. A session that isn't working takes a
normal reply (`anchorleg run --follow-up <run>`).

### What a session changed: `anchorleg diff`

Every run saves its folder as it found it, as a local ref `refs/anchorleg/run-<id>/start` (never
pushed; your branch, index and stash are untouched). `anchorleg diff <run>` shows what changed
since the session's first run started, new files included and your own earlier uncommitted
changes left out; `--stat` lists files and line counts. In `anchorleg ui`, **v** shows the same
for the selected session (↑↓ PgUp PgDn scroll, any other key closes). Sessions from before this
version have no start to compare with.

`anchorleg undo <run>` puts every file the session changed back the way the session found it and
removes the files it created (**U U** in the UI). Without `--yes` it only lists them. Only the
working tree changes: the branch, index and stash are left alone, and the folder is saved first
(`refs/anchorleg/run-<id>/before-undo`), so `anchorleg undo <run> --redo` brings the changes back.
A working session must be stopped first. The agent isn't told; mention it in your next message.

The agent's task list (Claude's TODO list, Codex's plan) shows under the conversation as it
works: ✔ done, ▸ in progress, ◻ to do. The conversation's title shows how full the context is
(`ctx 42k/200k (21%)`), from the last model request; above `--fresh-above` a switch starts the
next account fresh from the handoff.

### Notifications: `anchorleg notify`

Runs work in the background, so anchorleg tells you when one needs you or is over: a tool call,
question or plan waiting for an answer; a run done or failed; a switch to another account; every
account out. Desktop notifications are on by default (macOS Notification Center through
`osascript`, Linux `notify-send`); `anchorleg notify --desktop off` turns them off.

`--command CMD` also runs a command of yours for every notice, e.g. to reach your phone through a
chat bot or a push service. It runs with `sh -c`, gets the notice as JSON on stdin
(`{"event", "run_id", "title", "message"}`; events `waiting`, `done`, `failed`, `switched`,
`blocked`, `test`) and as `ANCHORLEG_EVENT`, `ANCHORLEG_RUN_ID`, `ANCHORLEG_TITLE`,
`ANCHORLEG_MESSAGE`. Notices name what is waiting, never an answer. `--test` sends one.

```bash
anchorleg notify --command 'curl -s -d "$ANCHORLEG_MESSAGE" https://ntfy.sh/my-anchorleg-topic'
```

On macOS the first notification may need permission: allow notifications for "Script Editor"
in System Settings → Notifications if none show up.

### Fork a session, or give a task its own worktree

`anchorleg run --fork <run> -- <message>` starts a **new** session from that session's history
(Claude `--resume … --fork-session`, Codex `thread/fork`); the original is left as it was, so you
can try another direction from the same point. In `anchorleg ui`: `/fork <message>` with the
session selected. Claude and Codex only.

`anchorleg run --worktree [NAME] -- <task>` runs the task in a git worktree of its own,
`<repo>/.anchorleg/worktrees/NAME` on a new local branch `anchorleg/NAME` (from `HEAD`; NAME
defaults to `task-<time>`), so two sessions in the same repository can't step on each other or on
you. Follow-ups run there too. Nothing is pushed; merge the branch when you like it, and remove it
with `git worktree remove`. In the UI: `/worktree <task>` as a new task.

Follow-ups and forks stay on the account their session ran on while it has room (unless
`--prefer`/`--account` say otherwise): on another CLI the session couldn't be resumed, only
continued from a handoff.

### What the agent is doing

The conversation shows each tool call with the first lines of its output (`⎿`), and the model's
reasoning dimmed (`✻`): Claude's thinking text when the CLI includes it, Codex's reasoning
summaries, and for Antigravity, which doesn't stream its thinking, how many tokens it thought for.
While a session works, the call it's in shows above **Working…** with how long it has been running;
Codex streams command output as it comes, Claude Code and Antigravity send it when the command ends.

Subagents the session started are listed under it, each with how many tool calls it has made and,
while it works, the call it's on: Claude Code's Agent calls (their steps come in the session's
stream), Codex's subagents (each runs in a thread of its own, on the same connection) and
Antigravity's `invoke_subagent` (anchorleg reads the subagent's own transcript, which Antigravity
keeps on disk; its steps never reach the session's stream). A subagent's steps aren't shown as the
session's; its answer is shown as the output of the call that started or waited for it.

### Typing in `anchorleg ui`

`@` followed by part of a name offers the task folder's files (as git sees them: tracked and
untracked, not ignored); **Tab** puts the path in. **Ctrl+P** / **Ctrl+N** (or **↑** in an empty
box with a session selected) bring back earlier messages: the session's own, or, for a new task,
the ones sent from this UI.

### Permission mode per session: `--mode`

`anchorleg run --mode edits` lets file edits go ahead in that session while commands and the rest
are still asked; `--mode ask` sends every question to you. The mode belongs to the session: its
follow-ups keep it unless they pass another one. It overrides the CLI settings'
`--permission-mode` for Claude; Codex already accepts edits inside the task folder, so both modes
are the same there. On an Antigravity account with its own `HOME`, `--mode ask` makes
anchorleg's hook ask before every change to the task's files; on your own `HOME` (no hook) agy
accepts edits either way. In `anchorleg ui`, `/mode edits` sets the selected session's mode (from its next
message) or, with no session selected, the mode new tasks start in; the conversation title shows it.

### Work toward a goal: `--goal`

`anchorleg run --goal "cargo test passes" -- "migrate the auth module to the new API"` keeps the
session working until the condition holds. With no task after `--`, the goal is the task. Every
turn is told the goal and ends its reply with one line:

- `GOAL: MET — <evidence>`: the run ends, done.
- `GOAL: NOT MET — <what is left>` (or no `GOAL:` line): anchorleg continues the same session
  with "The goal isn't met yet (<what is left>)", on whichever account it is on by then.
- `GOAL: IMPOSSIBLE — <why>`: the run ends, failed.

`--goal-turns N` (default 10) caps the turns; out of turns, the run fails with the goal `unmet`.
The goal is part of the session, not of one account: a switch to another account or another
CLI carries it (the handoff's **Done when** section, and the note on every turn), and a
follow-up of a session whose goal isn't met keeps working toward it. The JSON report has
`goal: {condition, status, turns, reason}`; `anchorleg sessions --json` has `goal` and
`goal_status`. In `anchorleg ui`, `/goal <condition>` starts a new task with that goal, or sets it
on the selected session (once it isn't working); the title shows `◎ goal <status>`.

Write the condition as something the agent can show in the conversation (a test run, a build's
exit code, a file count): the agent judges its own work from what it ran. Claude Code's own
`/goal` and Codex's thread goals loop inside one CLI, and Claude's verdicts don't show in `-p`
output, so anchorleg keeps the loop itself, the same for every CLI.

### Review a session's changes: `anchorleg review`

`anchorleg review 12` starts a new, read-only session that reviews what session #12 changed since
it started (its diff, capped at 60 KB, and the task it was given), preferably on another account
than #12's last one, so a second pair of eyes reads it. `--cli codex` (or `claude`,
`antigravity`) picks the reviewer's CLI; `--account A` picks the account. Text after `--` is
added to the instructions ("focus on error handling"). The reviewer lists findings as
`- [P0|P1|P2|P3] <title> — <path>:<line>` and ends with `VERDICT: approve` or
`VERDICT: changes requested`. `--fix` sends the findings back to #12 as a follow-up when the
review ends: the original session fixes the ones it agrees with and says why it skips the
others. `anchorleg run --review 12` is the same thing.

The review can't change files: Codex runs it with `review/start` in the read-only sandbox,
Claude Code in its default permission mode with Edit/Write/NotebookEdit refused and only
read-only git commands allowed, Antigravity with its edits refused. It needs a git folder and a
session that changed something, and refuses while #12 is working. In `anchorleg ui`,
`/review [fix] [notes]` reviews the selected session; the review's title shows `review of #12`.
`anchorleg sessions --json` has `review_of`.

### Plan first: `--plan`

`anchorleg run --plan -- <task>` (or `/plan <task>` as a new task in `anchorleg ui`) has the agent
plan before it changes anything:

- **Claude Code** runs in its plan mode (`--permission-mode plan`, replacing your own mode for
  those launches). `-p` doesn't offer Claude's ExitPlanMode tool, so the agent is told to end its
  reply with the plan.
- **Codex** runs in its plan collaboration mode with a read-only sandbox.
- **Antigravity**: on an account with its own `HOME`, anchorleg's hook refuses changes to the
  task's files while it plans (the agent reads the reason and plans on; agy's own `--mode plan`
  carries the plan out once permissions are skipped, so it isn't used there). On your own `HOME`
  it runs in agy's plan mode (`--mode plan`), which in `-p` reads and plans but has its edits
  refused.

When the plan is ready the run asks **Approve the plan above? Or type what to change.** (the plan
is the agent's last message in the conversation). **1** Approve: the same session carries it out
in your normal mode. **2** Keep planning, or your own words: the agent revises the plan and asks
again. The question waits for you (or `--answer-wait`); a skip, or no answer by then: the run
ends with the plan as its result, and
`anchorleg run --follow-up <run> -- "go ahead"` carries it out later. Planning survives an account
switch like any other step.

### Attachments: images and files

`--attach FILE` (repeatable) on `anchorleg run` and `anchorleg send` sends files with the message.
They're copied into the task folder, under `.anchorleg/attachments/` (that folder gets a
`.gitignore` of `*`, so they never reach git), and the message lists their paths. Every account and
CLI can then read them, also after a switch:

- **Claude Code** reads them with its Read tool, which shows images to the model.
- **Codex** also gets images directly with the first message (`localImage` input).

In `anchorleg ui`, **Ctrl+V** attaches the image on the clipboard (macOS: `osascript`; Linux:
`wl-paste` or `xclip`), and a file dragged onto the terminal (its path, quoted or with escaped
spaces) is attached instead of sent as text. The message box title shows `· N attached`.

```bash
anchorleg run --attach ~/Desktop/error.png -- "fix the crash in this screenshot"
anchorleg send 12 --attach build.log -- "the CI build fails like this"
```

### Questions from the agent

When the agent needs a decision only you can make, it asks, and the run waits for your answer
(30 minutes at most, like approvals):

- **Codex** asks with its own `request_user_input` (through `codex app-server`). Codex 0.153 offers
  that tool only in plan mode unless its experimental `default_mode_request_user_input` feature is
  on, so anchorleg turns that on for its launches (`-c features.…=true`, ignored by a Codex that
  doesn't know it). To keep it off, put `-c features.default_mode_request_user_input=false` in
  `anchorleg settings codex --args`.
- **Claude Code** drops its `AskUserQuestion` tool in `-p` runs, so anchorleg-mod adds an
  `ask_user` tool with the same input (the model sees it as `mcp__anchorleg-mod__ask_user`).
  Where the CLI does offer `AskUserQuestion`, that is answered the same way.

A question waits until you answer it, however long that takes (the run shows as waiting, and
you get a notification). If you'd rather the agent went on alone after a while, give the session
a limit: `anchorleg run --answer-wait 10m` (follow-ups keep it); with no answer by then the agent
decides itself (Codex gets no answers; Claude is told to decide). Tool-call approvals are refused
after 30 minutes without an answer unless `--answer-wait` says otherwise.

In `anchorleg ui` the conversation shows the question and its numbered options: press a number to
pick (several, then Enter, when more than one may be picked), type your own answer in the
message box, or press **0** to skip. In a terminal:

```bash
anchorleg permission list                 # shows each question and its options
anchorleg permission answer 7 2 "use src/" # one answer per question: option number, label or text
anchorleg permission answer 7 --skip       # the agent is told you didn't answer
```

anchorleg never passes **secrets**: answer a question marked secret with `-` and give the agent
the value another way (an environment variable, a file it can read). Otherwise it would sit in
anchorleg's database. Questions are stored as requests with `tool = "AskUserQuestion"`; the
JSON from `permission list` has `input.questions[]` (`id`, `header`, `question`, `options[]`
of `{label, description}`, `multi_select`, `secret`) and, once answered, `answers` (question
id → labels or text).

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

**Codex.** anchorleg runs Codex through `codex app-server` with a `workspace-write` sandbox
and `on-request` approvals (unless your `anchorleg settings codex --args` pick a sandbox or an
approval policy). Edits and commands inside the task folder go ahead; when Codex wants more
(network access, writing elsewhere) it asks, and the question shows up here as `Bash` or
`Edit`. If Codex offers no way to decline and continue, a "no" stops its turn and anchorleg
starts the next one with a note that you refused, so the agent carries on without it.

Codex also asks before every **MCP tool call**; that shows up as `mcp__<server>__<tool>` with
its arguments. A request for more sandbox access shows up as `Permissions  network · write
/path`; yes grants it for the current turn.

**MCP prompts (Codex).** When an MCP server asks for input (a form, or a link to open, e.g. to
sign in), it arrives as a question: one per form field, with its choices numbered (Yes / No for
a yes/no field), or "Done" / "Decline" for a link. Skipping declines it. Fields named like a
password, token or API key are treated as secrets: anchorleg doesn't pass them (answer `-`).
Claude Code's MCP prompts can't be answered from a mod yet, so in `-p` they are declined as
before.

**Antigravity (under development).** Headless agy can't ask either, and its hooks can refuse a call but not approve
one. So for an agy account with its own `HOME` (`--cmd "HOME=~/.agy-2 agy"`), anchorleg adds an
`anchorleg` entry to that `HOME`'s `.gemini/config/hooks.json`, lets agy run tools
(`--dangerously-skip-permissions`) and has the hook decide every call: reads and edits inside
the task folder go ahead; shell commands (shown as `Bash`), edits elsewhere, the browser, the web
and MCP tools wait for your answer, the same way as above. Outside anchorleg runs the hook leaves
the decision to agy. An agy account on your own `HOME` gets no hook: it runs with
`--mode accept-edits`, and shell commands need an allow rule in agy's settings. Your own
`--mode` or `--dangerously-skip-permissions` in `anchorleg settings antigravity --args` turns
the hook off.

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
  "policy": "drain",
  "next": { "action": "use", "account": "claude-two", "reason": "available" },
  "accounts": [
    {
      "name": "claude-sm", "vendor": "claude", "priority": 1,
      "blocked_until": 1791450000,
      "windows": [
        { "window": "five_hour", "status": "rejected", "used": 1.0,
          "resets_at": 1791450000, "source": "stream", "updated_at": 1791442000 }
      ],
      "credits": { "state": "off", "balance": null, "reason": "out_of_credits",
                   "updated_at": 1791442000 },
      "credits_use": "never"
    }
  ]
}
```

`next.action` is `use` (`reason`: `available`, `near_limit` or `credits`), `wait` (`until`,
`account`), or `no_accounts`. `credits` is `null` until the CLI has reported it; `state` is `off`,
`available`, `in_use` or `unlimited`. `used` is a fraction 0–1 and may be `null` (Claude often omits it).
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
