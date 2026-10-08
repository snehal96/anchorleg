# AGENTS.md — anchorleg

Instructions for coding agents (Claude Code, Codex, …) working on this repo. People: see
[CONTRIBUTING.md](CONTRIBUTING.md), which says the same things at more length.

## What this project is

`anchorleg` keeps headless coding tasks running when an AI subscription hits its 5-hour or weekly
limit. It watches the quota on every account; when one runs low it stops the task at a clean
point, writes a handoff, and continues on another account or another CLI (Claude Code, Codex,
Antigravity).

- **`crates/anchorleg-core`**: supervisor, policy, account registry, quota store (SQLite), one
  adapter per vendor CLI (`src/adapters/`), handoff and checkpoints.
- **`crates/anchorleg`**: the `anchorleg` binary: CLI, TUI (`anchorleg ui`), JSON output.
- **`mod/`**: anchorleg-mod, a Claude Code mod (TypeScript) loaded into every Claude run. It
  reports exact quota, stops at step boundaries and forwards permission questions.
- **`fixtures/`**: real captured CLI output, replayed by the tests.

## Ground rules (non-negotiable)

- Only call the official binaries: `claude`, `codex`, `agy` (later `kimi`, `cursor-agent`). Never
  extract subscription OAuth tokens to call vendor APIs directly, and never build or use a proxy
  that turns a subscription into an API.
- The mod never changes credentials or bypasses limits inside a session. It reads usage and stops
  turns; the supervisor switches by relaunching the CLI.
- Tokens live in the system keychain (`keyring` crate). Never write a token to config, state,
  logs, fixtures or test output.
- anchorleg may create **local** checkpoint refs in the repo it works on. It never pushes.
- Anything that spends real quota uses the cheapest model and tiny prompts; say how many requests
  you made.

## Conventions

- Rust stable, 2024 edition. `./scripts/check.sh` (fmt, clippy `-D warnings`, tests, and the mod's
  tests when `claude` is installed) must pass before a change is done.
- Event parsing never crashes a run: unknown event types and fields are logged and skipped.
- New adapter behaviour is backed by a real capture in `fixtures/<cli>-<version>-<what>-<date>/`
  with a README saying how it was made. Run every capture through `scripts/scrub-capture.py`
  (it removes paths, user names, account names and installed plugins) and check the diff.
- Shell scripts: `bash`, `set -uo pipefail`, must pass `bash -n`.
- Mod changes: `claude plugin validate mod` and `claude plugin test mod`.
- Keep `README.md` and `docs/CLI.md` in step with user-visible changes.

## Verifying

- `cargo test` replays fixtures against fake CLIs (`crates/anchorleg/tests/fake/`), so it spends no
  quota.
- When behaviour depends on a real CLI, do one real run with a cheap model and say so. If
  something wasn't run, say it wasn't run.
