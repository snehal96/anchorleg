# Contributing to anchorleg

Thanks for helping. Bug reports, captured CLI output from new versions, and new adapters are all
welcome.

## Before you start

- For anything bigger than a small fix, open an issue first so we can agree on the approach.
- anchorleg only ever starts the official CLIs (`claude`, `codex`, `agy`) the way a person would.
  Changes that extract subscription tokens, call vendor APIs directly or proxy a subscription
  won't be accepted, whatever they enable.

## Setup

```bash
git clone https://github.com/snehal96/anchorleg && cd anchorleg
./scripts/check.sh
```

`check.sh` runs `cargo fmt --check`, `cargo clippy -- -D warnings` and the tests, plus the
Claude Code mod's own tests when `claude` is installed. It must pass before a PR is merged; CI
runs it on macOS and Linux.

## How the code is laid out

| Path | What's there |
|---|---|
| `crates/anchorleg-core/src/supervisor.rs` | picks an account, launches the CLI, switches |
| `crates/anchorleg-core/src/adapters/` | one module per CLI: turns its events into `AgentSignal`s |
| `crates/anchorleg-core/src/policy.rs` | which account to use next |
| `crates/anchorleg-core/src/handoff.rs`, `checkpoint.rs` | `.handoff/TASK.md` and git checkpoints |
| `crates/anchorleg/src/` | the CLI, `anchorleg ui`, JSON output |
| `mod/` | anchorleg-mod (TypeScript), loaded into Claude runs |
| `fixtures/` | real CLI output the tests replay |
| `crates/anchorleg/tests/fake/` | fake `claude`, `codex` and `agy` used by end-to-end tests |

## Tests and fixtures

Tests never spend quota: they replay captured output from `fixtures/` and run end-to-end
against the fake CLIs. When you rely on how a real CLI behaves:

1. Capture it with a cheap model and a tiny prompt.
2. Save it under `fixtures/<cli>-<version>-<what>-<yyyymmdd>/` with a short README: the exact
   command, the model, and how many requests it cost.
3. Scrub it: `scripts/scrub-capture.py fixtures/<folder>/*.jsonl` removes home paths, your user
   name, account names (list yours in the git-ignored `scripts/scrub-names.local`) and your
   installed plugins, skills and connectors. **Read the diff before committing.** Never commit
   tokens or emails.
4. Add a test that replays it.

## Adding a CLI

An adapter needs: a parser for the CLI's headless event stream, a way to tell one account from
another (a config folder or `HOME`), how to resume a session, and how to detect a limit (and,
ideally, read quota without spending a request). Look at `adapters/codex.rs` and
`adapters/agy.rs` for two different shapes, and add a fake CLI under `tests/fake/`.

## Pull requests

- Keep them focused; describe what changed and how you checked it (and any real runs).
- Update `README.md` / `docs/CLI.md` when behaviour users see changes, and add a line to
  `CHANGELOG.md` under *Unreleased*.
- By contributing you agree your work is dual-licensed under MIT or Apache-2.0, like the project.
