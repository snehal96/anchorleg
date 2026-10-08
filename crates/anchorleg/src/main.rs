mod accounts;
mod permission;
mod run;
mod settings;
mod status;
mod ui;

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
    Run(RunArgs),
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
    /// List recent sessions (runs), newest first.
    Sessions {
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
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
    /// The task, after `--`.
    #[arg(last = true, required = true)]
    pub prompt: Vec<String>,
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
        Command::Sessions { json, limit } => status::sessions(json, limit),
        Command::Report { json: _ } => status::report(),
        Command::Run(args) => {
            return match run::run(args) {
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
