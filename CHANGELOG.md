# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Codex approvals: Codex now runs through `codex app-server`. Commands and edits Codex asks
  about (network access, writes outside the task folder) wait for your answer; quota is also
  updated during the turn, and usage-limit errors are recognised from Codex's own error codes.
- Codex soft stop: when Codex reports a window at `--stop-at` (default 90%), anchorleg stops the
  turn after the model call, before the next tool runs, writes the handoff and continues on a
  login with more room.
- Antigravity approvals (under development): an agy account with its own `HOME` asks you about shell commands and
  anything outside the task folder, in `anchorleg ui` or `anchorleg permission answer`.
- Release pipeline: prebuilt binaries for macOS and Linux, `.deb` packages, Homebrew tap and an
  install script (in progress).
- Linux support (in progress): builds and passes the test suite; tokens use the Secret Service.

## [0.1.0] — first public version

### Added
- `anchorleg run`: runs a task headless and switches accounts when one runs low or hits its limit.
- Claude Code: anchorleg-mod reports exact quota, pauses at a clean step at 90% (`--stop-at`)
  and forwards tool-call approvals to you.
- Codex: quota read live before every launch (`codex app-server`, no model request); sessions
  move between logins.
- Antigravity (`agy`, under development): runs, resumes and switches on quota errors; edits
  allowed by default.
- Handoff on every early stop: `.handoff/TASK.md` and a local git checkpoint under
  `refs/anchorleg/`; switching to another CLI starts fresh from the handoff.
- `anchorleg ui`: session manager with live conversations, replies, approvals, quota per account,
  per-provider model/effort and slash commands.
- `--json` output and stable exit codes for scripts; `anchorleg status`, `sessions`, `stop`,
  `settings`, `accounts` (with alias import).
