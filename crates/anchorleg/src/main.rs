mod accounts;
mod permission;
mod run;
mod settings;
mod status;
mod ui;
mod usage;

use anchorleg_core::policy::Policy;
use std::path::PathBuf;
use std::process::ExitCode;

use anchorleg_core::output::exit;
use clap::{Args, Parser, Subcommand};

/// Keep headless coding tasks running when an account hits its quota.
#[derive(Parser)]
#[command(name = "anchorleg", version = anchorleg_core::VERSION)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a task, switching accounts when one runs out.
    Run(Box<RunArgs>),
    /// Review the changes a session made, read-only, on another account (a second opinion):
    /// `anchorleg run --review RUN`. Extra instructions after `--`.
    Review {
        run: i64,
        /// Only use accounts of this CLI: claude, codex or antigravity.
        #[arg(long, value_parser = ["claude", "codex", "antigravity"])]
        cli: Option<String>,
        /// Only use this account.
        #[arg(long, value_name = "ACCOUNT")]
        account: Option<String>,
        /// Send the findings to the reviewed session to fix.
        #[arg(long)]
        fix: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        no_wait: bool,
        #[arg(last = true)]
        extra: Vec<String>,
    },
    /// Show quota and blocked state for every account.
    Status {
        #[arg(long)]
        json: bool,
        /// Ask each Codex login for its current quota first (no model request, a few seconds).
        #[arg(long)]
        refresh: bool,
    },
    /// Add, list or remove accounts.
    #[command(subcommand)]
    Accounts(accounts::AccountsCmd),
    /// Session manager: start tasks, watch and reply to sessions, manage accounts.
    Ui,
    /// Tool calls waiting for your approval (asked by anchorleg-mod while a run works).
    #[command(subcommand)]
    Permission(permission::PermissionCmd),
    /// Per-CLI settings shared by all its accounts: model, effort, extra arguments.
    /// No name: list them all.
    Settings {
        /// claude, codex, kimi or cursor.
        vendor: Option<String>,
        /// Model for every account of this CLI (`default`: the CLI's own).
        #[arg(long)]
        model: Option<String>,
        /// Effort for every account of this CLI (Claude: low … max; `default`: the CLI's own).
        #[arg(long)]
        effort: Option<String>,
        /// Arguments to add, as one string, e.g. "--permission-mode acceptEdits".
        #[arg(long, allow_hyphen_values = true)]
        args: Option<String>,
        /// Remove this CLI's settings (applied before the others).
        #[arg(long)]
        clear: bool,
    },
    /// How `anchorleg run` picks among accounts with room: drain, balance or save-weekly.
    /// No name: show the current one.
    Policy {
        #[arg(value_parser = parse_policy)]
        policy: Option<Policy>,
    },
    /// What a session changed in its folder since it started (new files included), as a diff.
    Diff {
        run: i64,
        /// Only the files and line counts.
        #[arg(long)]
        stat: bool,
    },
    /// Put back every file a session changed since it started (files it created are removed).
    /// Without --yes it only shows what would change. A backup is kept: --redo brings it back.
    Undo {
        run: i64,
        #[arg(long)]
        yes: bool,
        /// Bring back what the last undo of this session put back.
        #[arg(long, conflicts_with = "yes")]
        redo: bool,
    },
    /// Notifications when a run needs you (a tool call, a question, a plan) or ends.
    /// No options: show the current settings.
    Notify {
        /// Desktop notifications: on or off.
        #[arg(long, value_name = "on|off", value_parser = parse_on_off)]
        desktop: Option<bool>,
        /// Also run this command (`sh -c`) for every notice: JSON on stdin, and
        /// ANCHORLEG_EVENT / ANCHORLEG_RUN_ID / ANCHORLEG_TITLE / ANCHORLEG_MESSAGE.
        #[arg(long, value_name = "CMD", conflicts_with = "no_command")]
        command: Option<String>,
        /// Stop running the command.
        #[arg(long)]
        no_command: bool,
        /// Send a test notice.
        #[arg(long)]
        test: bool,
    },
    /// Tokens and cost of anchorleg's runs, per account and model, plus credits and quota.
    Usage {
        /// How far back: e.g. 24h, 7d, 30d, or `all`.
        #[arg(long, default_value = "7d", value_parser = usage::parse_since)]
        since: i64,
        #[arg(long)]
        json: bool,
    },
    /// List recent sessions (runs), newest first.
    Sessions {
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Send a message to a session that's still working. Codex takes it into the running turn;
    /// Claude and agy get it when their turn ends. `--interrupt` stops the agent at its next clean point
    /// so the message is read now.
    Send {
        run: i64,
        #[arg(long)]
        interrupt: bool,
        /// Attach a file; repeat for several.
        #[arg(long, value_name = "FILE")]
        attach: Vec<PathBuf>,
        /// The message, after `--`.
        #[arg(last = true, required = true)]
        text: Vec<String>,
    },
    /// Exit 0 if the person asked this run to stop for a new message (called by anchorleg-mod).
    #[command(hide = true)]
    InterruptCheck { run: i64 },
    /// Stop a running session (its `anchorleg run` process and the CLI under it).
    Stop {
        /// The run id, as `anchorleg status`, `anchorleg ui` or `anchorleg run --json` show it.
        run: i64,
    },
    /// Record exact usage from inside a session (called by anchorleg-mod). Reads JSON on stdin.
    Report {
        /// Required: the input is JSON (kept explicit so the format can grow).
        #[arg(long, required = true)]
        json: bool,
    },
}

#[derive(Args)]
pub struct RunArgs {
    /// Send the prompt as a new message to this run's session instead of starting a new one.
    #[arg(long, value_name = "RUN")]
    pub follow_up: Option<i64>,
    /// Start a new session from this run's session history (the original goes on unchanged):
    /// try another direction from the same point. Claude and Codex.
    #[arg(long, value_name = "RUN", conflicts_with = "follow_up")]
    pub fork: Option<i64>,
    /// Directory the task runs in. Defaults to the current one (or the followed-up run's).
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    /// Print one JSON object (see docs/CLI.md) instead of progress text.
    #[arg(long)]
    pub json: bool,
    /// If every account is blocked, exit with code 75 instead of waiting for a reset.
    #[arg(long)]
    pub no_wait: bool,
    /// Model to pass to the CLI.
    #[arg(long)]
    pub model: Option<String>,
    /// Switch at a clean point once any quota window reaches this fraction (needs anchorleg-mod).
    #[arg(long, default_value_t = 0.9, value_parser = parse_fraction)]
    pub stop_at: f64,
    /// Don't load anchorleg-mod into Claude runs (switch only on hard limit hits).
    #[arg(long)]
    pub no_mod: bool,
    /// At a switch, start fresh from .handoff/TASK.md instead of resuming when the session's
    /// context is above this many tokens.
    #[arg(long, default_value_t = 100_000)]
    pub fresh_above: u64,
    /// How to pick among accounts with room, for this run: drain, balance or save-weekly.
    /// Defaults to `anchorleg policy`.
    #[arg(long, value_parser = parse_policy)]
    pub policy: Option<Policy>,
    /// Use this account first whenever it has room.
    #[arg(long, value_name = "ACCOUNT")]
    pub prefer: Option<String>,
    /// Only use this account: no switching (waits for its reset, or exits with `--no-wait`).
    #[arg(long, value_name = "ACCOUNT", conflicts_with = "prefer")]
    pub account: Option<String>,
    /// Attach a file (an image, a log…): copied into the task folder and named in the prompt.
    /// Repeat for several.
    #[arg(long, value_name = "FILE")]
    pub attach: Vec<PathBuf>,
    /// Plan first: the agent only plans, you approve the plan (or say what to change), then the
    /// same session carries it out.
    #[arg(long)]
    pub plan: bool,
    /// Run in a git worktree of its own (`.anchorleg/worktrees/NAME`, new local branch
    /// `anchorleg/NAME`), so it can't collide with you or other sessions in the same repository.
    #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "",
          conflicts_with_all = ["follow_up", "fork"])]
    pub worktree: Option<String>,
    /// How long a question or approval waits for you before the agent goes on without an answer
    /// (it decides itself; a tool call is refused): e.g. 90s, 10m, 2h. Default: questions wait
    /// until you answer, tool calls 30m. Follow-ups keep their session's.
    #[arg(long, value_name = "DURATION", value_parser = parse_wait)]
    pub answer_wait: Option<i64>,
    /// The session's permission mode: `ask` (every question comes to you), `edits` (file edits
    /// go ahead) or `auto` (the CLI's own reviewer approves or refuses instead of asking you;
    /// Claude Code's auto mode, Codex's auto-review). Follow-ups keep their session's mode
    /// unless given.
    #[arg(long, value_parser = ["ask", "edits", "auto"])]
    pub mode: Option<String>,
    /// Keep the session working until this condition holds (e.g. "cargo test passes"), across
    /// turns, accounts and CLIs. Each turn ends with the agent's GOAL: MET / NOT MET line.
    /// Follow-ups keep an unmet goal. With no task, the goal is the task.
    #[arg(long, value_name = "CONDITION")]
    pub goal: Option<String>,
    /// At most this many turns toward the goal (default 10); then the run fails as unmet.
    #[arg(long, value_name = "N")]
    pub goal_turns: Option<u32>,
    /// Review the changes this run's session made, read-only, on another account when one has
    /// room (a second opinion). The task after `--`, if any, adds to the review's instructions.
    #[arg(long, value_name = "RUN",
          conflicts_with_all = ["follow_up", "fork", "worktree", "plan", "goal"])]
    pub review: Option<i64>,
    /// Only use accounts of this CLI: claude, codex or antigravity.
    #[arg(long, value_parser = ["claude", "codex", "antigravity"])]
    pub cli: Option<String>,
    /// With `--review`: once the review is in, send its findings to the reviewed session, which
    /// fixes the ones it agrees with.
    #[arg(long, requires = "review")]
    pub fix: bool,
    /// The task, after `--`.
    #[arg(last = true, required_unless_present_any = ["goal", "review"])]
    pub prompt: Vec<String>,
}

fn parse_wait(s: &str) -> Result<i64, String> {
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: i64 = n.parse().map_err(|_| format!("{s}: like 90s, 10m or 2h"))?;
    let secs = match unit {
        "" | "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => return Err(format!("{s}: like 90s, 10m or 2h")),
    };
    if secs < 10 {
        return Err("at least 10s".to_owned());
    }
    Ok(secs)
}

fn parse_on_off(s: &str) -> Result<bool, String> {
    match s {
        "on" | "true" | "yes" => Ok(true),
        "off" | "false" | "no" => Ok(false),
        _ => Err(format!("{s}: on or off")),
    }
}

fn parse_policy(s: &str) -> Result<Policy, String> {
    s.parse()
}

fn parse_fraction(s: &str) -> Result<f64, String> {
    match s.parse::<f64>() {
        Ok(f) if f > 0.0 && f <= 1.0 => Ok(f),
        _ => Err("expected a fraction above 0 and at most 1, e.g. 0.9".to_owned()),
    }
}

/// Unix seconds now.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("ANCHORLEG_LOG"))
        .with_writer(std::io::stderr)
        .init();

    let result = match Cli::parse().command {
        Command::Accounts(cmd) => accounts::run(cmd),
        Command::Status { json, refresh } => status::status(json, refresh),
        Command::Ui => ui::run(),
        Command::Permission(cmd) => permission::run(cmd),
        Command::Settings {
            vendor,
            model,
            effort,
            args,
            clear,
        } => settings::run(
            vendor,
            settings::Changes {
                args,
                model,
                effort,
                clear,
            },
        ),
        Command::Stop { run } => run::stop(run),
        Command::Send {
            run,
            interrupt,
            attach,
            text,
        } => run::send(run, &text.join(" "), interrupt, &attach),
        Command::InterruptCheck { run } => {
            return match run::interrupt_check(run) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => ExitCode::from(1),
                Err(e) => {
                    eprintln!("anchorleg: {e:#}");
                    ExitCode::from(2)
                }
            };
        }
        Command::Policy { policy } => settings::policy(policy),
        Command::Diff { run, stat } => run::diff(run, stat),
        Command::Undo { run, yes, redo } => run::undo(run, redo, yes),
        Command::Notify {
            desktop,
            command,
            no_command,
            test,
        } => settings::notify(desktop, command, no_command, test),
        Command::Sessions { json, limit } => status::sessions(json, limit),
        Command::Usage { since, json } => usage::run(since, json),
        Command::Report { json: _ } => status::report(),
        Command::Review {
            run,
            cli,
            account,
            fix,
            json,
            no_wait,
            extra,
        } => {
            let mut argv = vec![
                "anchorleg".to_owned(),
                "run".to_owned(),
                "--review".to_owned(),
                run.to_string(),
            ];
            for (flag, value) in [("--cli", cli), ("--account", account)] {
                if let Some(v) = value {
                    argv.extend([flag.to_owned(), v]);
                }
            }
            for (flag, on) in [("--fix", fix), ("--json", json), ("--no-wait", no_wait)] {
                if on {
                    argv.push(flag.to_owned());
                }
            }
            if !extra.is_empty() {
                argv.push("--".to_owned());
                argv.extend(extra);
            }
            let Command::Run(args) = Cli::parse_from(argv).command else {
                unreachable!("parsed as run");
            };
            return match run::run(*args) {
                Ok(report) => ExitCode::from(report.outcome.exit_code()),
                Err(e) => {
                    eprintln!("anchorleg: {e:#}");
                    ExitCode::from(exit::FAILED)
                }
            };
        }
        Command::Run(args) => {
            return match run::run(*args) {
                Ok(report) => ExitCode::from(report.outcome.exit_code()),
                Err(e) => {
                    eprintln!("anchorleg: {e:#}");
                    ExitCode::from(exit::FAILED)
                }
            };
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("anchorleg: {e:#}");
            ExitCode::from(exit::FAILED)
        }
    }
}
