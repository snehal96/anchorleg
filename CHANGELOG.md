# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
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
- Antigravity (`agy`): runs, resumes and switches on quota errors; edits allowed by default.
- Handoff on every early stop: `.handoff/TASK.md` and a local git checkpoint under
  `refs/anchorleg/`; switching to another CLI starts fresh from the handoff.
- `anchorleg ui`: session manager with live conversations, replies, approvals, quota per account,
  per-provider model/effort and slash commands.
- `--json` output and stable exit codes for scripts; `anchorleg status`, `sessions`, `stop`,
  `settings`, `accounts` (with alias import).
