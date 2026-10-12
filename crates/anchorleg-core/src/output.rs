//! The `--json` contract (D10). Callers parse these shapes, so changes that break them bump
//! [`SCHEMA`]. Documented in `docs/CLI.md`.

use serde::{Deserialize, Serialize};

use crate::adapters::{CreditState, EpochSecs};
use crate::policy::{self, Pick, PickOptions, PickReason};
use crate::registry::{Config, Vendor};
use crate::store::{CreditsRow, QuotaRow, RunId, UsageTotal};

pub const SCHEMA: u32 = 1;

/// Process exit codes for `anchorleg run`.
pub mod exit {
    /// The task finished.
    pub const OK: u8 = 0;
    /// The task ran and failed, or anchorleg hit an error.
    pub const FAILED: u8 = 1;
    /// Bad arguments (clap's default).
    pub const USAGE: u8 = 2;
    /// Every account is blocked and `--no-wait` was given (EX_TEMPFAIL).
    pub const ALL_BLOCKED: u8 = 75;
    /// No account is configured for this task (EX_CONFIG).
    pub const NO_ACCOUNTS: u8 = 78;
    /// Stopped by `anchorleg stop` or Ctrl-C (128 + SIGINT).
    pub const STOPPED: u8 = 130;
}

/// `anchorleg status --json`.
#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    pub schema: u32,
    pub now: EpochSecs,
    /// The switch policy `next` was picked with.
    pub policy: policy::Policy,
    /// What `anchorleg run` would do right now.
    pub next: Next,
    /// In the order they're tried.
    pub accounts: Vec<AccountStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Next {
    Use { account: String, reason: PickReason },
    Wait { until: EpochSecs, account: String },
    NoAccounts,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountStatus {
    pub name: String,
    pub vendor: Vendor,
    pub priority: u32,
    pub blocked_until: Option<EpochSecs>,
    /// Latest reading per window.
    pub windows: Vec<WindowStatus>,
    /// Paid usage credits past the plan's limits, when the CLI has reported them.
    pub credits: Option<CreditsStatus>,
    /// Whether anchorleg may spend them: `never`, `last-resort` or `allow`.
    pub credits_use: policy::CreditsUse,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreditsStatus {
    pub state: CreditState,
    pub balance: Option<f64>,
    pub reason: Option<String>,
    pub updated_at: EpochSecs,
}

/// `anchorleg usage --json`: tokens and cost recorded from runs since `since`.
#[derive(Debug, Clone, Serialize)]
pub struct UsageReport {
    pub schema: u32,
    pub now: EpochSecs,
    pub since: EpochSecs,
    /// Per account and model, most expensive first.
    pub totals: Vec<UsageTotal>,
    /// Per UTC day: `[day start, tokens, list-price cost]`.
    pub by_day: Vec<(EpochSecs, u64, f64)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowStatus {
    pub window: String,
    pub status: Option<String>,
    /// Fraction used, 0.0–1.0.
    pub used: Option<f64>,
    pub resets_at: Option<EpochSecs>,
    pub source: String,
    pub updated_at: EpochSecs,
}

pub fn status(
    config: &Config,
    quota: &[QuotaRow],
    credits: &[CreditsRow],
    now: EpochSecs,
    opts: &PickOptions,
) -> StatusReport {
    let mut opts = opts.clone();
    opts.has_credits = credits
        .iter()
        .filter(|c| c.state.spendable())
        .map(|c| c.account.clone())
        .collect();
    let opts = &opts;
    let ordered = config.by_priority();
    let next = match policy::pick(&ordered, quota, now, opts) {
        Pick::Use { account, reason } => Next::Use {
            account: account.name.clone(),
            reason,
        },
        Pick::WaitUntil { at, account } => Next::Wait {
            until: at,
            account: account.name.clone(),
        },
        Pick::NoAccounts => Next::NoAccounts,
    };
    let accounts = ordered
        .into_iter()
        .map(|a| {
            let rows: Vec<&QuotaRow> = quota.iter().filter(|r| r.account == a.name).collect();
            AccountStatus {
                name: a.name.clone(),
                vendor: a.vendor,
                priority: a.priority,
                blocked_until: policy::blocked_until(&rows, now, opts),
                windows: rows
                    .into_iter()
                    .map(|r| WindowStatus {
                        window: r.window.clone(),
                        status: r.status.clone(),
                        used: r.used,
                        resets_at: r.resets_at,
                        source: r.source.clone(),
                        updated_at: r.updated_at,
                    })
                    .collect(),
                credits: credits
                    .iter()
                    .find(|c| c.account == a.name)
                    .map(|c| CreditsStatus {
                        state: c.state,
                        balance: c.balance,
                        reason: c.reason.clone(),
                        updated_at: c.updated_at,
                    }),
                credits_use: a.credits,
            }
        })
        .collect();
    StatusReport {
        schema: SCHEMA,
        now,
        policy: opts.policy,
        next,
        accounts,
    }
}

/// `anchorleg run --json`: one object on stdout when the run ends.
#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub schema: u32,
    pub run_id: Option<RunId>,
    pub outcome: Outcome,
    /// The CLI's final message, if it got that far.
    pub result: Option<String>,
    pub session_id: Option<String>,
    /// Every account that worked on the task, in order.
    pub accounts_used: Vec<String>,
    pub switches: Vec<Switch>,
    /// Set when `outcome` is `all_blocked`: the earliest time an account frees up.
    pub wait_until: Option<EpochSecs>,
    /// The run's goal and how it ended (`anchorleg run --goal`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<GoalReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GoalReport {
    pub condition: String,
    pub status: crate::goal::Status,
    /// Turns spent on it in this run.
    pub turns: u32,
    /// The agent's last word on it.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Failed,
    AllBlocked,
    NoAccounts,
    Stopped,
}

impl Outcome {
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Done => exit::OK,
            Self::Failed => exit::FAILED,
            Self::AllBlocked => exit::ALL_BLOCKED,
            Self::NoAccounts => exit::NO_ACCOUNTS,
            Self::Stopped => exit::STOPPED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Switch {
    pub from: String,
    pub to: String,
    /// Which switch rule fired, e.g. `rejected`.
    pub rule: String,
    pub at: EpochSecs,
}

/// Input to `anchorleg report --json` (stdin). Sent by anchorleg-mod from inside a Claude session;
/// the mod converts its own numbers to this shape.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub account: String,
    pub readings: Vec<ReportReading>,
    /// With `stop_reason`: the mod stopped this run at a clean point and asks anchorleg to switch.
    #[serde(default)]
    pub run_id: Option<i64>,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportReading {
    /// `five_hour`, `seven_day`, or another window name.
    pub window: String,
    /// `allowed`, `warning` or `rejected`.
    #[serde(default)]
    pub status: Option<String>,
    /// Fraction used, 0.0–1.0.
    #[serde(default)]
    pub used: Option<f64>,
    /// Unix seconds.
    #[serde(default)]
    pub resets_at: Option<EpochSecs>,
}
