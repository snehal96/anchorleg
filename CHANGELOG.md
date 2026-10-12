# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Homebrew tap (in progress).

## [0.4.0] — 2026-10-12

### Added
- Talk to a session while it works: `anchorleg send <run> -- …` (Enter in `anchorleg ui`). Codex
  takes the message into the running turn; Claude Code reads it when its turn ends, in the same
  run. `--interrupt` (Ctrl+X) stops the agent at its next clean point and continues with the
  message.
- Questions from the agent reach you: Codex's `request_user_input` (anchorleg turns on Codex's
  experimental feature that offers it outside plan mode), and for Claude Code an
  `ask_user` tool added by anchorleg-mod (headless runs drop Claude's own question tool). Answer
  in `anchorleg ui` (number keys, your own words, `0` to skip) or with
  `anchorleg permission answer <id> <answer>…`. Secret answers are never passed or stored.
- `anchorleg ui`: the conversation shows tool output and the model's reasoning, and the call a
  working session is in, with Codex's command output live.
- `anchorleg ui`: `!cmd` runs a shell command in the task folder; its output goes with your next
  message.
- `anchorleg ui`: `/clear` (a new session in the same folder) and `/compact`, which now works for
  Codex too (`thread/compact/start`); the conversation notes the compaction.
- `anchorleg ui`: a working session lists its subagents (Claude Code's Agent calls, Codex's
  `multi_agent` subagents, Antigravity's `invoke_subagent`) with how many tool calls each has made
  and the one it's on. Their own steps no longer show as the session's.
- `anchorleg ui`: `@` completes the task folder's file paths; Ctrl+P / Ctrl+N (↑) bring back
  earlier messages.
- Questions wait until you answer them. `anchorleg run --answer-wait 10m` lets the agent go on
  alone after a while instead (tool-call approvals: 30m unless set).
- Fork a session (`anchorleg run --fork RUN`, `/fork` in the UI) and run a task in its own git
  worktree (`--worktree [NAME]`, `/worktree`).

- Permission mode per session: `anchorleg run --mode ask|edits`, `/mode` in the UI; follow-ups
  keep their session's mode.
- Undo a session: `anchorleg undo <run>` (U U in the UI) puts back every file it changed and
  removes files it created, working tree only, with a backup for `--redo`.
- See what a session changed: `anchorleg diff <run> [--stat]`, `v` in `anchorleg ui`. Every run
  saves the folder as it found it (`refs/anchorleg/run-<id>/start`, local only), so the diff shows
  the session's own changes. The agent's task list shows under the conversation, and its title
  shows how full the context is.
- Notifications: a desktop notification when a run waits for you (tool call, question, plan),
  finishes, fails, switches accounts or runs out of accounts. `anchorleg notify` turns them off or
  adds a command of your own (JSON on stdin), e.g. to reach your phone.
- Plan first: `anchorleg run --plan` (`/plan <task>` in `anchorleg ui`). The agent plans in Claude's
  or Codex's plan mode; you approve the plan or say what to change; then the same session
  carries it out.
- MCP in Codex runs: MCP tool calls are now asked as approvals (before, anchorleg rejected the
  request, so every MCP tool call failed), MCP servers' prompts (forms, sign-in links) reach you
  as questions, and Codex's requests for more sandbox access as `Permissions` approvals.
- Attach images and files: `--attach FILE` on `anchorleg run` and `anchorleg send`; in
  `anchorleg ui`, Ctrl+V pastes the clipboard's image and a dropped file is attached. Files are
  copied into the task folder's git-ignored `.anchorleg/attachments/`, so any account or CLI can
  read them; Codex also gets images as image input.

### Changed
- Follow-ups stay on the account their session ran on while it has room, instead of following
  the switch policy to another CLI, which could only continue from a handoff.
- Antigravity: `anchorleg send --interrupt` stops agy and continues its conversation with the
  message; plan runs keep the task's files unchanged (anchorleg's hook on accounts with their own
  `HOME`, agy's `--mode plan` on yours); `--mode ask` asks before edits on hook accounts.
- Antigravity's task list (the checklist file it keeps in its own folder) shows like the others,
  and goes into handoffs; its own notes (plans, task list) no longer count as files the task
  changed.
- `anchorleg ui`: a `/command` Codex can't run headless says so instead of reaching the model as
  text.

### Fixed
- Codex: a run ended as soon as a subagent of the task finished, losing the agent's answer.
  A subagent's thread is now told apart from the session's, and its tokens count toward the run.

## [0.3.0] — 2026-10-12

### Added
- Usage credits (Claude extra usage, Codex credits): anchorleg reads each account's credit state
  and, by default, stops a launch the moment it starts paying past the plan's limit and moves the
  task on. Per account, `anchorleg accounts set A --credits last-resort|allow` lets it pay when
  every account is out, or keep going on credits.
- `anchorleg usage [--since 24h|7d|30d|all] [--json]`: tokens and API-price cost per account and
  model, tokens per day, quota bars and credits.
- `anchorleg ui`: 24-hour usage in the top bar, `u` opens a usage panel, and accounts paying from
  credits are marked.
- `anchorleg status` shows each account's credits (JSON: `credits`, `credits_use`).
- A demo GIF in the README, recorded with vhs against stand-in CLIs (`scripts/demo/`).

### Fixed
- An older anchorleg no longer writes to a database a newer version has upgraded; it stops and
  says to update.

## [0.2.0] — 2026-10-11

### Added
- Switch policies: `drain` (priority order, the default), `balance` (least used first) and
  `save-weekly` (most weekly room first), set with `anchorleg policy` or `anchorleg run --policy`
  and shown in `anchorleg status` and `anchorleg ui` (`p` to change).
- `anchorleg run --prefer A` (A first while it has room) and `--account A` (only A).
- `anchorleg accounts set`: priority, `--reserve-7d` (keep part of the weekly window back),
  `--pinned`, `--enabled`.
- The new lane-switch icon, favicons and a share image for link previews.

## [0.1.0] — 2026-10-11, first public version

### Added
- `anchorleg run`: runs a task headless and switches accounts when one runs low or hits its limit.
- Claude Code: anchorleg-mod reports exact quota, pauses at a clean step at 90% (`--stop-at`)
  and forwards tool-call approvals to you.
- Codex: runs through `codex app-server`. Quota is read live before every launch and during
  the turn; commands and edits Codex asks about (network access, writes outside the task folder)
  wait for your answer; the turn pauses at `--stop-at` (default 90%) after a model call, before
  the next tool runs; usage-limit errors are recognised from Codex's own error codes. Threads
  move between logins.
- Antigravity (`agy`, under development): runs, resumes and switches on quota errors; edits
  allowed by default. An agy account with its own `HOME` asks you about shell commands and
  anything outside the task folder.
- Prebuilt binaries for macOS (arm64, x86_64) and Linux (x86_64, arm64), `.deb` packages, an
  install script with checksum checks, and `SHA256SUMS` on every release.
- Linux support: tokens use the Secret Service instead of the macOS Keychain.
- Handoff on every early stop: `.handoff/TASK.md` and a local git checkpoint under
  `refs/anchorleg/`; switching to another CLI starts fresh from the handoff.
- `anchorleg ui`: session manager with live conversations, replies, approvals, quota per account,
  per-provider model/effort and slash commands.
- `--json` output and stable exit codes for scripts; `anchorleg status`, `sessions`, `stop`,
  `settings`, `accounts` (with alias import).
