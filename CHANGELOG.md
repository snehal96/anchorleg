# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Homebrew tap (in progress).

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
