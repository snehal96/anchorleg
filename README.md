<h1 align="center"><img src="docs/images/logo.svg" alt="anchorleg" width="400"></h1>

<p align="center">
  <b>Keep your coding agents working when you hit your limit, without paying for extra usage.</b><br>
  A session manager for Claude Code and Codex (Antigravity coming) that pauses a task at a clean point
  when an account runs low and continues it on your other subscription or account.
</p>

<p align="center">
  <a href="https://github.com/snehal96/anchorleg/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/snehal96/anchorleg/actions/workflows/ci.yml/badge.svg"></a>
  <a href="#license"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue"></a>
  <img alt="Platform: macOS | Linux" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey">
  <a href="https://github.com/snehal96/anchorleg/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/snehal96/anchorleg?filter=v*"></a>
  <img alt="Status: early" src="https://img.shields.io/badge/status-early-orange">
</p>

---

Long agent runs die at the worst moment: the 5-hour window fills up at 92% of the work, or the
weekly limit lands mid-refactor. Then you either pay for extra usage, metered, which adds up fast
on long runs, or you copy the context by hand to another tool or account and start over.

**Who it's for**

- **One Claude subscription and a big extra-usage bill.** Add a ChatGPT plan next to it. When
  Claude runs low, anchorleg continues the task on Codex from a handoff, and it never spends usage
  credits unless you allow it. A flat monthly plan instead of open-ended overage, and `anchorleg
  usage` shows what each account costs you.
- **Several accounts already** (work and personal, Claude and ChatGPT). anchorleg watches all
  of them and moves the task to whichever has room.

`anchorleg` does that for you. It starts the official CLI (`claude`, `codex` or `agy`) headless,
watches every account's quota, and when one gets close to its limit it:

1. **pauses** the task before the next model request, never in the middle of an edit,
2. **hands off**: writes `.handoff/TASK.md` and a local git checkpoint of your working tree,
3. **continues** the same session on your next account, with its transcript and project memory.

<p align="center">
  <img src="docs/images/demo.gif" width="880" alt="Terminal demo: anchorleg status shows claude-pro at 86% and codex-plus at 12%; anchorleg run pauses the task on claude-pro at 92%, writes .handoff/TASK.md, switches to codex-plus and finishes; then the handoff file and anchorleg usage are shown">
  <br><sub>Recorded with <a href="scripts/demo/demo.tape">vhs</a> against stand-in CLIs, so no quota was spent.</sub>
</p>

<p align="center">
  <img src="docs/images/handoff.svg" width="880" alt="Pause at a clean step when an account reaches 92%, hand off with .handoff/TASK.md and a local checkpoint, continue the same session on the next account with room">
</p>

## Contents

- [Quick start](#quick-start)
- [Desktop app](#desktop-app)
- [What it does](#what-it-does)
- [Everyday commands](#everyday-commands)
- [Install](#install)
- [Accounts](#accounts)
- [Which account goes first](#which-account-goes-first)
- [Ground rules](#ground-rules)
- [Supported CLIs](#supported-clis)
- [FAQ](#faq) · [Troubleshooting](#troubleshooting) · [Changelog](CHANGELOG.md)

## Quick start

You need macOS or Linux, and Claude Code or Codex signed in on each account you want to use.

**1. Install**

```bash
curl -fsSL https://raw.githubusercontent.com/snehal96/anchorleg/main/install.sh | bash
```

**2. Add your accounts**

```bash
anchorleg accounts login work                  # a Claude account, signed in through your browser
anchorleg accounts login gpt --cli codex       # a ChatGPT account for Codex
anchorleg accounts import-aliases --yes        # or: import the `claude-*` aliases you already have
```

**3. Give it a task**

```bash
anchorleg ui                                               # the session manager (n: new task)
anchorleg run --cwd ~/code/app -- "fix the failing auth tests"   # or straight from the shell
```

That's it. When an account gets close to its limit, the task pauses at a clean step and carries
on with the next account that has room. You'll see it happen in the conversation.

## Desktop app

A desktop window for the same sessions: start tasks, watch them, answer the agent, see quota and
what changed. It includes the CLI, so installing the app is the only step. Downloads (macOS
universal `.dmg`, Linux AppImage and `.deb`) are on the
[desktop releases](https://github.com/snehal96/anchorleg/releases?q=desktop&expanded=true). It's
a preview, and not notarized yet: the first time, right-click the app → **Open**.

## What it does

- **Switches before you hit the wall.** It sees exact usage while the task runs and pauses at a
  step boundary at 90% (`--stop-at`), then continues on the next account, with the transcript and
  project memory. Moving to another CLI (Claude → Codex) continues from a handoff file.
- **Never spends money behind your back.** Usage credits (Claude extra usage, Codex credits)
  stay off unless you allow them per account.
- **A session manager.** `anchorleg ui` (or the desktop app) shows every session live, takes new
  tasks and replies, and shows each account's quota. Sessions keep running when you close it.
- **Talk to it while it works.** Type to a working session: Claude Code and Codex read it in the
  turn that's running. Ctrl+X has it read now.
- **You approve what it may do.** A background agent can't show its permission prompt, so
  anchorleg asks you (with the diff for an edit) and waits. Its questions to you show up the same
  way.
- **See what it changed.** Every file the session changed, with its diff; undo in one step.
- **Plans, goals, reviews, budgets.** `--plan` to approve a plan first, `--goal "tests pass"` to
  keep going until it holds, `anchorleg review` for a second opinion from another CLI, `--budget
  2M` to cap tokens.
- **Scriptable.** Every command has `--json` and stable exit codes, for scripts, CI and other
  agents.

All features, with examples: [docs/FEATURES.md](docs/FEATURES.md).

## Everyday commands

| Command | What it does |
|---|---|
| `anchorleg ui` | The session manager (`?` lists the keys) |
| `anchorleg run -- "task"` | Start a task (`--cwd DIR`, `--plan`, `--goal "…"`, `--attach FILE`) |
| `anchorleg run --follow-up 12 -- "…"` | Reply to session #12 |
| `anchorleg send 12 -- "…"` | Message #12 while it works (`--interrupt`: read it now) |
| `anchorleg sessions` | Recent sessions (`show 12` for one) |
| `anchorleg status` | Quota per account, and which runs next |
| `anchorleg usage` | Tokens and cost per account and model |
| `anchorleg diff 12` | What #12 changed |
| `anchorleg permission list` | Tool calls and questions waiting for you |
| `anchorleg stop 12` | Stop #12 and its CLI |
| `anchorleg sessions import` | Bring in the Claude Code and Codex sessions you started yourself |

Every command and option, and the JSON contract: [docs/CLI.md](docs/CLI.md).

## Install

<details>
<summary>Requirements, and other ways to install</summary>

- macOS (Apple silicon or Intel) or Linux (x86_64 or arm64, glibc 2.35+, e.g. Ubuntu 22.04+).
  Linux is newer and less tested with real accounts.
- At least one agent CLI, signed in: [Claude Code](https://docs.claude.com/en/docs/claude-code)
  (`claude`) or Codex (`codex`, or the copy inside the ChatGPT desktop app). Antigravity (`agy`)
  is under development.

The install script downloads the latest release for your machine, checks its SHA-256 and puts
`anchorleg` in `~/.local/bin`. Set `ANCHORLEG_INSTALL_DIR` to install somewhere else, or
`ANCHORLEG_VERSION=v0.8.0` to pin a version. Run it again to update.

**Debian / Ubuntu** (`.deb`):

```bash
arch=$(dpkg --print-architecture)   # amd64 or arm64
curl -fsSLO "https://github.com/snehal96/anchorleg/releases/latest/download/anchorleg_0.8.0-1_${arch}.deb"
sudo apt install "./anchorleg_0.8.0-1_${arch}.deb"
```

**Prebuilt binaries:** every [release](https://github.com/snehal96/anchorleg/releases/latest)
has `anchorleg-<target>.tar.gz` for `aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, with `SHA256SUMS`.

**From source** (needs [Rust](https://rustup.rs) stable):

```bash
cargo install --git https://github.com/snehal96/anchorleg anchorleg
```

</details>

## Accounts

An account is the command you would type to start the CLI, usually with its own config folder
(`CLAUDE_CONFIG_DIR=~/.claude-work claude`, `CODEX_HOME=~/.codex-2 codex`).

```bash
anchorleg accounts login work                # new folder, the CLI's own sign-in in your browser
anchorleg accounts import-aliases            # preview what it would take from your shell aliases
anchorleg accounts add home --cmd "CLAUDE_CONFIG_DIR=~/.claude-home claude" --priority 2
anchorleg accounts list                      # in the order they're tried
```

Accounts live in `~/.config/anchorleg/config.toml`; `anchorleg ui` can reorder, disable and
import them too. Model and effort are set once per provider (`anchorleg settings claude --model
sonnet --effort high`, or `/model` in the UI), so a task keeps the same strength when it moves.

## Which account goes first

Blocked accounts are skipped, and one at 95% or more is used only when nothing healthier is
free. Among the rest, the policy decides:

| Policy | Picks | Good for |
|---|---|---|
| `drain` (default) | priority order | one main account, others as backup |
| `balance` | the least used | several equal subscriptions |
| `save-weekly` | the most weekly room left | weekly limits running out before the week ends |

```bash
anchorleg policy balance                       # the default for every run
anchorleg run --account work -- "…"            # only this account, no switching
anchorleg accounts set work --stop-at 0.4      # move off this account once it's 40% used
anchorleg accounts set home --credits last-resort   # pay from extra usage only when all are out
```

On every pause, anchorleg writes `.handoff/TASK.md` in the task's folder (goal, task list, files
touched, recent commands, `git status`) and a local git checkpoint under `refs/anchorleg/`. Any
agent, or you, can pick the task up from it. More: [docs/CLI.md](docs/CLI.md).

## Ground rules

anchorleg is built to use your subscriptions the way their CLIs are meant to be used.

- It only starts the official binaries (`claude`, `codex`, `agy`; later `cursor-agent`),
  exactly as you would.
- It never extracts subscription tokens, never calls vendor APIs directly and never runs a
  proxy that turns a subscription into an API.
- Inside a session, anchorleg-mod only reads usage, stops at a step boundary and forwards
  permission questions. Switching happens outside, by relaunching the CLI.
- Tokens, if you use them, live in the system keychain (macOS Keychain, or the Secret Service
  such as GNOME Keyring on Linux), never in config, logs or test data.
- Checkpoints stay local. anchorleg never pushes.
- Nothing is approved without you.

You are responsible for using your accounts within each provider's terms.

## Supported CLIs

| CLI | Status |
|---|---|
| Claude Code (`claude`) | Supported |
| Codex (`codex`, also bundled with the ChatGPT app) | Supported |
| Antigravity (`agy`) | Under development: switches on quota errors; quota isn't visible |
| Kimi, Cursor (`cursor-agent`) | Planned |

How each one is driven, and what it can and can't do: [docs/CLI.md](docs/CLI.md).

## FAQ

**Does anchorleg raise my limits?** No. It uses the accounts you already have, one after another,
and stops each at a clean point.

**Is this allowed?** anchorleg only runs the official CLIs, logged in the way you log in to them
yourself; it doesn't share, pool or resell anything. The clearest use is across providers: a
Claude subscription and a ChatGPT one are separate products, and continuing a task on Codex
when Claude runs low is just using both. Several accounts of the same provider are your call:
read that provider's terms (Anthropic's, for example, forbid sharing an account with anyone
else). Don't use anchorleg to share accounts.

**I have one Claude account and my extra-usage bill is high. Will anchorleg help?** Yes, two
ways. By default it never spends credits: when a run starts paying, anchorleg stops it at a clean
point. And with a second provider's plan (ChatGPT for Codex), the task continues there instead
of on credits. `anchorleg usage` shows the cost per account so you can compare. We don't suggest
opening more accounts with the same provider to get around its limits.

**What about usage credits / extra usage?** Never spent unless you allow it per account
(`--credits last-resort` or `allow`); by default anchorleg stops a launch that starts paying.

**Does it work with one account?** Yes, as a session manager with approvals. With one account
it can only wait for the reset (or exit 75 with `--no-wait`).

**What does a switch cost?** Resuming means the next account reads the session once, uncached.
Above `--fresh-above` tokens anchorleg starts a fresh session from the handoff instead.

**Where is my data?** Config in `~/.config/anchorleg/`, sessions and quota in
`~/Library/Application Support/anchorleg/` (`ANCHORLEG_HOME` to move it). Nothing leaves your machine
except through the CLIs themselves.

## Troubleshooting

- **"no account configured" (exit 78):** run `anchorleg accounts import-aliases --yes` or
  `anchorleg accounts add`.
- **A task can read but not write files:** Claude's default permission mode asks before edits.
  Answer in `anchorleg ui`, or set `anchorleg settings claude --args "--permission-mode acceptEdits"`.
- **The mod doesn't load:** your organisation's managed settings may block mods. anchorleg still
  works without it (`--no-mod`): it switches on hard limits only, with no approvals.
- **More detail:** `ANCHORLEG_LOG=debug anchorleg run …`; every run's raw CLI output is in
  `~/Library/Application Support/anchorleg/runs/run-<id>.jsonl`.

## How it works

<p align="center">
  <img src="docs/images/architecture.svg" width="880" alt="anchorleg picks an account, launches the official CLI headless (claude -p with anchorleg-mod, codex app-server, agy -p), watches its events and switches with a handoff and checkpoint">
</p>

anchorleg is one Rust binary. It starts the official CLI headless, reads quota from it (a small
Claude Code mod, [`mod/`](mod), reports Claude's; Codex's app server reports its own), and
relaunches the CLI on another account to switch. Every CLI behaviour it relies on is checked
against real runs; the captured output is in [`fixtures/`](fixtures) and the tests replay it.

## Contributing

```bash
./scripts/check.sh      # fmt, clippy, tests, and the mod's tests when `claude` is installed
```

The tests replay real captured CLI output against a fake CLI, so they spend no quota. See
[CONTRIBUTING.md](CONTRIBUTING.md); coding agents: [AGENTS.md](AGENTS.md). What changed in each
version: [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
