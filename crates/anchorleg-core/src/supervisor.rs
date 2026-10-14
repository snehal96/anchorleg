//! Runs one task to completion across accounts: pick an account, launch its CLI, read the
//! event stream, and on a limit hit block that account and continue on the next one.
//!
//! Drives Claude, Codex and Antigravity accounts. Whenever a launch stops early, anchorleg writes
//! `.handoff/TASK.md` and a local git checkpoint (D4, D5). Switch flow (D11, D22): within one
//! CLI, copy the session file into the next account's folder and resume it; across CLIs, when
//! the context is large, or when that account still can't see the session, start fresh from the
//! handoff instead.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::adapters::claude::{self, sync_context};
use crate::adapters::codex_server::{self, Asker, Event, Steer};
use crate::adapters::{
    AgentSignal, CreditState, Credits, Decoder, EpochSecs, QuotaReading, QuotaStatus,
};
use crate::adapters::{agy, claude_usage, codex};
use crate::output::{GoalReport, Outcome, RunReport, SCHEMA, Switch};
use crate::policy::{self, Pick, PickOptions};
use crate::registry::{Account, Config, SecretStore, Vendor};
use crate::store::{Decision, RunState, Source, Store};
use crate::task_options::TaskOptions;
use crate::{checkpoint, goal, handoff};

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error(transparent)]
    Store(#[from] crate::store::StoreError),
    #[error(transparent)]
    Registry(#[from] crate::registry::RegistryError),
    #[error("starting `{program}`: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("reading CLI output: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub prompt: String,
    pub cwd: PathBuf,
    pub model: Option<String>,
    /// All accounts blocked → return `AllBlocked` instead of sleeping until a reset.
    pub no_wait: bool,
    /// Give up after this many account switches (guards against a detection loop).
    pub max_switches: usize,
    /// How long a CLI may keep running after it reported a limit hit before it's killed.
    pub limit_grace: Duration,
    /// Where raw CLI output is kept, one file per run. `None` keeps nothing.
    pub log_dir: Option<PathBuf>,
    /// anchorleg-mod's folder, passed as `--plugin-dir` to Claude runs. `None` runs without it.
    pub mod_dir: Option<PathBuf>,
    /// The `anchorleg` binary the mod and agy's hook call back (`anchorleg report`,
    /// `anchorleg permission`).
    pub relay_bin: Option<PathBuf>,
    /// anchorleg's data folder, passed as `ANCHORLEG_HOME` to agy's hook: an agy account runs
    /// with its own `HOME`, so the hook couldn't find anchorleg's database from it.
    pub data_dir: Option<PathBuf>,
    /// anchorleg's `config.toml`, passed as `ANCHORLEG_CONFIG` to agy's hook for the same
    /// reason, so it finds the person's lasting rules.
    pub config_path: Option<PathBuf>,
    /// The mod stops a run at a clean point once any window reaches this fraction.
    pub stop_at: f64,
    /// Send `prompt` as a new message to an existing session instead of starting one.
    pub follow_up: Option<FollowUp>,
    /// At a switch, a session with more context than this (tokens) starts fresh from
    /// `.handoff/TASK.md` instead of being resumed on a cold cache.
    pub fresh_above: u64,
    /// How to choose among accounts with room (D29).
    pub policy: policy::Policy,
    /// Use this account first whenever it's healthy.
    pub prefer: Option<String>,
    /// Only ever use this account.
    pub only: Option<String>,
    /// Attached images (absolute paths). Codex gets them as image input; every CLI also finds
    /// them named in the prompt.
    pub images: Vec<PathBuf>,
    /// Plan first: the agent only plans (Claude's plan mode, Codex's plan collaboration mode)
    /// until the person approves the plan; then the same session carries it out (D38).
    pub plan: bool,
    /// The session's permission mode, over the CLI settings' own (D41).
    pub mode: Option<Mode>,
    /// How long questions and approvals wait for the person (seconds) before the agent goes on.
    pub answer_wait: Option<i64>,
    /// Keep the session going until this holds (D48), for at most `goal_turns` turns.
    pub goal: Option<String>,
    pub goal_turns: u32,
    /// The most tokens the task may use, across every launch and account (`--budget`); counted
    /// as input, output, cache writes and a tenth of cache reads.
    pub budget: Option<u64>,
    /// A JSON Schema the final answer must follow (`--json-schema`); the answer comes back as
    /// `structured_output` in the report. Not used while planning.
    pub json_schema: Option<Value>,
    /// The session's name (`--name`, or the one it already has): shown in anchorleg and given
    /// to the CLI (Claude `--name`, Codex `thread/name/set`).
    pub name: Option<String>,
    /// A review (`anchorleg review`, D49): read-only on every CLI, and Codex runs it in its
    /// review mode.
    pub review: bool,
    /// The session under review, recorded on the run.
    pub review_of: Option<i64>,
    /// A side question (`anchorleg btw`): a read-only fork of the session answers it, and the
    /// session itself goes on untouched.
    pub side: bool,
    /// Start on another account than this one when one has room (a second opinion).
    pub avoid: Option<String>,
    /// Only accounts of this CLI.
    pub vendor: Option<Vendor>,
    /// What the task adds to every launch: folders, instructions, effort, Claude's options.
    pub task: TaskOptions,
}

/// How much the agent may do without asking, for one session (D41). Claude maps it to its
/// `--permission-mode`; Codex already accepts edits inside the task folder, so both are the same
/// there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Everything the CLI would ask about comes to the person.
    Ask,
    /// File edits go ahead; commands and the rest are asked.
    Edits,
    /// The CLI's own reviewer approves or refuses what it would ask about, so nobody has to be
    /// there: Claude Code's auto mode, Codex's `auto_review`. agy has none and runs as `edits`.
    Auto,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Ask, Mode::Edits, Mode::Auto];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::Edits => "edits",
            Mode::Auto => "auto",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.name() == s)
    }

    fn claude(self) -> &'static str {
        match self {
            Mode::Ask => "default",
            Mode::Edits => "acceptEdits",
            Mode::Auto => "auto",
        }
    }
}

/// The env var that gives anchorleg-mod the tokens the task may still use (`--budget`).
const BUDGET_ENV: &str = "ANCHORLEG_BUDGET_LEFT";

/// The env var that tells anchorleg-mod the launch's permission mode, when it's `auto`.
const PERMISSION_MODE_ENV: &str = "ANCHORLEG_PERMISSION_MODE";

/// The `--permission-mode` Claude's `args` give, if any.
fn claude_mode(args: &[String]) -> Option<&str> {
    args.iter().enumerate().rev().find_map(|(i, a)| {
        a.strip_prefix("--permission-mode=")
            .or_else(|| (a == "--permission-mode").then(|| args.get(i + 1).map(String::as_str))?)
    })
}

/// `args` with Claude's `--permission-mode` set to `mode`, whatever the settings said.
fn with_claude_mode(args: Vec<String>, mode: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--permission-mode" {
            it.next();
        } else if !a.starts_with("--permission-mode=") {
            out.push(a);
        }
    }
    out.extend(["--permission-mode".to_owned(), mode.to_owned()]);
    out
}

/// An existing session to continue with a new message.
#[derive(Debug, Clone)]
pub struct FollowUp {
    pub parent: i64,
    pub session_id: String,
    /// The account the session was last on; its context is copied if another one takes over.
    pub account: String,
    /// Start a new session from this one's history instead of continuing it (Claude
    /// `--fork-session`, Codex `thread/fork`).
    pub fork: bool,
    /// Fork from this point of the session instead of its end (`anchorleg rewind`): the last
    /// message kept, Claude's message uuid (`--resume-session-at`) or Codex's turn id
    /// (`thread/fork` `lastTurnId`).
    pub at: Option<String>,
}

impl RunOptions {
    pub fn new(prompt: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            prompt: prompt.into(),
            cwd: cwd.into(),
            model: None,
            no_wait: false,
            max_switches: 5,
            limit_grace: Duration::from_secs(30),
            log_dir: None,
            mod_dir: None,
            relay_bin: None,
            data_dir: None,
            config_path: None,
            stop_at: 0.9,
            follow_up: None,
            fresh_above: 100_000,
            policy: policy::Policy::Drain,
            prefer: None,
            only: None,
            images: Vec::new(),
            plan: false,
            mode: None,
            answer_wait: None,
            goal: None,
            goal_turns: crate::goal::DEFAULT_TURNS,
            budget: None,
            json_schema: None,
            name: None,
            review: false,
            review_of: None,
            side: false,
            avoid: None,
            vendor: None,
            task: TaskOptions::default(),
        }
    }
}

/// Where a launch's events come from: the CLI's stdout, or the Codex app-server driver.
enum Feed {
    Lines(tokio::io::Lines<BufReader<tokio::process::ChildStdout>>),
    Codex(tokio::sync::mpsc::UnboundedReceiver<Event>),
}

impl Feed {
    async fn next(&mut self) -> std::io::Result<Option<Event>> {
        match self {
            Self::Lines(lines) => Ok(lines.next_line().await?.map(Event::Line)),
            Self::Codex(rx) => Ok(rx.recv().await),
        }
    }
}

/// The next event, or `None` once `deadline` passes first.
async fn next_item(
    source: &mut Feed,
    deadline: Option<tokio::time::Instant>,
) -> Option<std::io::Result<Option<Event>>> {
    match deadline {
        Some(d) => tokio::time::timeout_at(d, source.next()).await.ok(),
        None => Some(source.next().await),
    }
}

/// Read every enabled Claude and Codex account's current quota from its CLI, all at once, and
/// store it: `claude -p /usage` and `codex app-server`, neither a model request (D23, D64).
/// Accounts a run reported on less than `fresh_for` seconds ago (the mod's or the stream's
/// reading, as current as asking) are left alone. Returns the accounts that couldn't answer,
/// with why; those keep what was already known.
pub async fn refresh_quota(
    config: &Config,
    store: &Store,
    secrets: &dyn SecretStore,
    home: &Path,
    accounts: &[&Account],
    now: EpochSecs,
    fresh_for: EpochSecs,
) -> Result<Vec<(String, String)>, RunError> {
    let quota = store.quota(None)?;
    let fresh = |name: &str| {
        fresh_for > 0
            && quota
                .iter()
                .any(|r| r.account == name && r.source != "probe" && now - r.updated_at < fresh_for)
    };
    let mut probes = tokio::task::JoinSet::new();
    for a in accounts.iter().filter(|a| {
        a.enabled
            && a.vendor.caps().quota == crate::vendors::QuotaSource::Live
            && a.shell.is_none()
            && !fresh(&a.name)
    }) {
        let Ok(launch) = config.launch(a, secrets, home) else {
            continue;
        };
        // Only the binary and the login's env: the configured args are for runs.
        let mut cmd = std::process::Command::new(&launch.program);
        for key in &launch.env_remove {
            cmd.env_remove(key);
        }
        cmd.envs(launch.env.iter().map(|(k, v)| (k, v)));
        let name = a.name.clone();
        let vendor = a.vendor;
        probes.spawn(async move {
            let cmd = tokio::process::Command::from(cmd);
            let result = match vendor {
                Vendor::Claude => claude_usage::read_usage(cmd, LIVE_QUOTA_TIMEOUT, now)
                    .await
                    .map(|r| (r, None)),
                _ => codex::read_live_quota(cmd, LIVE_QUOTA_TIMEOUT).await,
            };
            (name, result)
        });
    }
    let mut failed = Vec::new();
    while let Some(done) = probes.join_next().await {
        let Ok((name, result)) = done else { continue };
        match result {
            Ok((readings, credits)) => {
                if !readings.is_empty() {
                    store.replace_quota(&name, &readings, Source::Probe, now)?;
                }
                if let Some(c) = credits {
                    store.record_credits(&name, &c, Source::Probe, now)?;
                }
            }
            Err(e) => failed.push((name, e)),
        }
    }
    failed.sort();
    Ok(failed)
}

/// A quota reading this recent (seconds) isn't asked for again before a pick.
pub const FRESH_QUOTA: EpochSecs = 120;

/// How long a CLI gets to report an account's quota.
const LIVE_QUOTA_TIMEOUT: Duration = Duration::from_secs(15);

/// Prompt for a resumed session on the next account.
pub const CONTINUE_PROMPT: &str = "Continue from where you stopped.";

/// Added to the task when planning on a CLI without a plan hand-over of its own (Claude's
/// ExitPlanMode isn't offered in `-p`; agy has no plan mode).
const PLAN_NOTE: &str = "Plan only for now: don't change any files. Investigate as needed, \
     then end your reply with the complete plan. anchorleg will ask me to approve it; you'll \
     get my answer as the next message.";

/// A Claude review can't edit, and may read the history without asking.
const REVIEW_DENIED_TOOLS: &str = "Edit,Write,NotebookEdit";
const REVIEW_ALLOWED_TOOLS: &str =
    "Bash(git diff *),Bash(git log *),Bash(git show *),Bash(git status *),Bash(git status)";

/// The message that ends planning.
const PLAN_APPROVED: &str =
    "The plan is approved. Plan mode is over: carry out the plan now, step by step.";

/// The message that ends planning with the plan the person edited.
const PLAN_EDITED: &str = "The plan is approved, with my edits. Plan mode is over: carry out \
     this version of the plan now, step by step, in place of yours:";

/// The person's answer to a finished plan.
enum PlanAnswer {
    Approved,
    Edited(String),
    Change(String),
}

/// Prompt for a fresh session that takes over from a stopped one.
fn handoff_prompt(task_md: Option<&str>, task: &str) -> String {
    match task_md {
        Some(md) => format!(
            "You are taking over a task another agent session started and couldn't finish. \
             Its handoff is in .handoff/TASK.md, copied below. Check the working tree \
             (git status, git diff) to see what was already done, then finish the task. \
             Add decisions and dead ends to the Notes section of .handoff/TASK.md as you go.\n\n{md}"
        ),
        None => format!(
            "A previous session working on this task was interrupted and can't be resumed. \
             Check the working tree (git status, git diff) to see what was already done, \
             then finish the task.\n\nTask:\n{task}"
        ),
    }
}

/// The last handoff written in this run.
struct Written {
    task_md: String,
    context_tokens: Option<u64>,
}

/// Progress callbacks, so the CLI can print what's happening.
pub trait Progress {
    fn event(&self, message: &str);
}

/// Prints nothing.
pub struct Quiet;

impl Progress for Quiet {
    fn event(&self, _: &str) {}
}

pub struct Supervisor<'a> {
    pub config: &'a Config,
    pub store: &'a Store,
    pub secrets: &'a dyn SecretStore,
    pub home: PathBuf,
    /// Unix seconds now; injectable for tests.
    pub clock: Arc<dyn Fn() -> EpochSecs + Send + Sync>,
    pub progress: &'a dyn Progress,
}

/// What one launch of the CLI ended with.
#[derive(Debug, Default)]
struct Attempt {
    session_id: Option<String>,
    limit_hit: bool,
    session_not_found: bool,
    finished: Option<(bool, Option<String>)>,
    exit_status: Option<std::process::ExitStatus>,
    /// anchorleg stopped a Codex turn at `--stop-at` (the mod records Claude's in the store).
    soft_stop: Option<String>,
    /// This launch may run on usage credits (the account allows it, or it was picked for them).
    may_spend: bool,
    /// The credit state the CLI last reported during this launch.
    credits: Option<CreditState>,
    /// Requests are being paid from credits.
    on_credits: bool,
    /// A limit hit that credits cover; it counts only if the run then fails.
    limit_deferred: bool,
    /// Stopped because the CLI began paying from credits it may not use: stop at once.
    stop_now: bool,
    /// The answer as JSON (`--json-schema`), when the CLI gave it.
    structured: Option<Value>,
    /// The model the session answers with, as far as anchorleg knows.
    model: Option<String>,
    /// Commands the agent left running in the background; they end with the CLI.
    background: Vec<String>,
}

/// How far a launch may go: where the soft stop sits, and whether it may run on usage credits.
#[derive(Debug, Clone, Copy)]
struct Spend {
    stop_at: f64,
    /// Tokens the task may still use, when it has a budget.
    budget_left: Option<u64>,
    may_spend: bool,
    /// Plan only: nothing may change yet.
    plan: bool,
    /// Fork the session being resumed instead of continuing it.
    fork: bool,
}

/// How the next launch continues the task.
enum Continue {
    Start,
    /// The user's message, sent into an existing session.
    Reply(String),
    /// Messages the person sent while the session was working (session id, text).
    Message(String, String),
    Resume(String),
    Fresh,
}

impl Supervisor<'_> {
    fn now(&self) -> EpochSecs {
        (self.clock)()
    }

    pub async fn run(&self, opts: &RunOptions) -> Result<RunReport, RunError> {
        let run_id = self.store.start_run(
            &opts.prompt,
            &opts.cwd.display().to_string(),
            opts.follow_up.as_ref().map(|f| f.parent),
            self.now(),
        )?;
        if let Some(m) = opts.mode {
            self.store.set_run_mode(run_id, Some(m.name()))?;
        }
        // A follow-up's name lives on the run that gave it; a new one goes on this run.
        if let Some(n) = &opts.name
            && opts.follow_up.as_ref().is_none_or(|f| {
                self.store.session_name(f.parent).ok().flatten().as_ref() != Some(n)
            })
        {
            self.store.set_run_name(run_id, Some(n))?;
        }
        if opts.answer_wait.is_some() {
            self.store.set_run_answer_wait(run_id, opts.answer_wait)?;
        }
        if let Some(j) = opts.task.to_json() {
            self.store.set_run_options(run_id, Some(&j))?;
        }
        if let Some(g) = &opts.goal {
            self.store
                .set_run_goal(run_id, Some(g), Some(goal::Status::Active.name()))?;
        }
        if opts.review_of.is_some() {
            self.store.set_run_review_of(run_id, opts.review_of)?;
        }
        // The folder as the run found it, so the session's own changes can be shown (D40).
        if let Err(e) = crate::checkpoint::save_start(&opts.cwd, run_id) {
            tracing::warn!(%e, "couldn't save the run's start");
        }
        let mut report = RunReport {
            schema: SCHEMA,
            run_id: Some(run_id),
            outcome: Outcome::Failed,
            result: None,
            session_id: None,
            accounts_used: Vec::new(),
            switches: Vec::new(),
            wait_until: None,
            budget: None,
            structured_output: None,
            goal: opts.goal.as_ref().map(|g| GoalReport {
                condition: g.clone(),
                status: goal::Status::Active,
                turns: 0,
                reason: None,
            }),
        };
        let mut ordered = self.config.by_priority();
        ordered.retain(|a| a.vendor.caps().driven);
        let mut pick_opts = PickOptions {
            policy: opts.policy,
            // A follow-up or fork stays on the session's account while it has room: the session
            // lives there, and another CLI could only start fresh from a handoff (D42).
            prefer: opts
                .prefer
                .clone()
                .or_else(|| opts.follow_up.as_ref().map(|f| f.account.clone())),
            only: opts.only.clone(),
            vendor: opts.vendor,
            ..PickOptions::default()
        };
        let mut current: Option<&Account> = None;
        let mut next = match &opts.follow_up {
            Some(f) => Continue::Reply(f.session_id.clone()),
            None => Continue::Start,
        };
        self.store
            .set_run_pid(run_id, Some(i64::from(std::process::id())))?;
        self.log_line(
            opts,
            run_id,
            &json!({ "type": "anchorleg_user", "text": opts.prompt }),
        );
        let mut fresh_tries = 0;
        let mut launches = 0;
        // Plan first (D38): only planning until the person approves the plan.
        let mut planning = opts.plan;
        // Set when the mod stopped the last launch at a clean point (soft switch).
        let mut soft_stop_from: Option<&Account> = None;
        // An account that keeps running with the mod's stop off: nothing better to switch to.
        let mut stop_off_for: Option<String> = None;
        let mut written: Option<Written> = None;
        let mut handoffs = 0;
        // The last launch's background commands, for the next handoff.
        let mut background_note: Option<String> = None;

        loop {
            // The budget is checked between launches; inside one, anchorleg-mod (Claude) and the
            // Codex driver stop the turn at a clean point once it's used up.
            if let Some(budget) = opts.budget {
                let spent = self.store.run_tokens(run_id)?;
                if spent >= budget {
                    report.outcome = Outcome::OverBudget;
                    report.result = Some(format!(
                        "stopped: the task used {spent} of its {budget}-token budget"
                    ));
                    self.decide(
                        run_id,
                        current,
                        None,
                        "budget",
                        json!({ "budget": budget, "spent": spent }),
                    )?;
                    self.note(opts, run_id, "the task's token budget is used up");
                    break;
                }
            }
            self.refresh_quota(&ordered, opts, run_id).await?;
            let quota = self.store.quota(None)?;
            pick_opts.has_credits = self
                .store
                .credits(None)?
                .into_iter()
                .filter(|c| c.state.spendable())
                .map(|c| c.account)
                .collect();
            let mut picked_for_credits = false;

            // After a soft stop, only an account with real room left is worth switching to;
            // otherwise keep going where we are, with the stop off, until the hard limit.
            let soft_choice =
                match soft_stop_from.take() {
                    None => None,
                    Some(from) => {
                        let roomier = PickOptions {
                            exclude: vec![from.name.clone()],
                            near_limit: opts.stop_at,
                            ..pick_opts.clone()
                        };
                        match policy::pick(&ordered, &quota, self.now(), &roomier) {
                            Pick::Use {
                                account,
                                reason: policy::PickReason::Available,
                            } => {
                                self.switch(
                                    run_id,
                                    (from, account, "quota_stop"),
                                    &mut next,
                                    written.as_ref(),
                                    &mut report,
                                    opts,
                                )?;
                                Some(account)
                            }
                            // A strict account past its own limit doesn't carry on: the normal
                            // pick below treats it as blocked and finds another or waits.
                            _ if from.strict
                                && policy::strict_until(
                                    from,
                                    &quota
                                        .iter()
                                        .filter(|r| r.account == from.name)
                                        .collect::<Vec<_>>(),
                                    self.now(),
                                    &pick_opts,
                                )
                                .is_some() =>
                            {
                                self.decide(run_id, Some(from), None, "strict_limit", json!({}))?;
                                self.note(
                                    opts,
                                    run_id,
                                    &format!("{} reached its strict limit", from.name),
                                );
                                None
                            }
                            _ => {
                                self.decide(
                                    run_id,
                                    Some(from),
                                    Some(from),
                                    "quota_stop_stay",
                                    json!({}),
                                )?;
                                self.note(opts, run_id, &format!(
                                "no account with more room; continuing on {} until its limit",
                                from.name
                            ));
                                stop_off_for = Some(from.name.clone());
                                Some(from)
                            }
                        }
                    }
                };

            // A review starts on another account than the session's when one has room.
            let avoided = match (current, &opts.avoid) {
                (None, Some(avoid)) => {
                    let other = PickOptions {
                        exclude: vec![avoid.clone()],
                        ..pick_opts.clone()
                    };
                    match policy::pick(&ordered, &quota, self.now(), &other) {
                        p @ Pick::Use {
                            reason: policy::PickReason::Available,
                            ..
                        } => Some(p),
                        _ => None,
                    }
                }
                _ => None,
            };
            let account = match soft_choice {
                Some(account) => account,
                None => match avoided
                    .unwrap_or_else(|| policy::pick(&ordered, &quota, self.now(), &pick_opts))
                {
                    Pick::Use { account, reason } => {
                        if reason == policy::PickReason::Credits {
                            picked_for_credits = true;
                            self.decide(
                                run_id,
                                current,
                                Some(account),
                                "credits",
                                json!({ "setting": account.credits.name() }),
                            )?;
                            self.note(
                                opts,
                                run_id,
                                &format!(
                                    "every account is out; continuing on {} with usage credits",
                                    account.name
                                ),
                            );
                        }
                        if let Some(prev) = current.filter(|p| p.name != account.name) {
                            if report.switches.len() >= opts.max_switches {
                                self.decide(run_id, Some(prev), None, "max_switches", json!({}))?;
                                report.result = Some(format!(
                                    "gave up after {} account switches",
                                    opts.max_switches
                                ));
                                break;
                            }
                            self.switch(
                                run_id,
                                (prev, account, "rejected"),
                                &mut next,
                                written.as_ref(),
                                &mut report,
                                opts,
                            )?;
                        } else if current.is_none() {
                            self.decide(
                                run_id,
                                None,
                                Some(account),
                                "start",
                                json!({ "policy": opts.policy.name() }),
                            )?;
                            self.carry_follow_up(opts, run_id, account);
                        }
                        account
                    }
                    Pick::NoAccounts => {
                        report.outcome = Outcome::NoAccounts;
                        break;
                    }
                    Pick::WaitUntil { at, account } => {
                        self.decide(run_id, current, None, "all_blocked", json!({ "until": at }))?;
                        if opts.no_wait {
                            report.outcome = Outcome::AllBlocked;
                            report.wait_until = Some(at);
                            break;
                        }
                        self.note(
                            opts,
                            run_id,
                            &format!(
                                "every account is blocked; waiting {}s for {}",
                                at - self.now(),
                                account.name
                            ),
                        );
                        self.store
                            .set_run_state(run_id, RunState::Waiting, self.now())?;
                        let secs = u64::try_from(at - self.now()).unwrap_or(0) + 5;
                        tokio::time::sleep(Duration::from_secs(secs)).await;
                        continue;
                    }
                },
            };

            launches += 1;
            if launches > opts.max_switches + 2 {
                self.decide(run_id, Some(account), None, "max_launches", json!({}))?;
                report.result = Some(format!("gave up after {} launches", launches - 1));
                break;
            }
            current = Some(account);
            if report.accounts_used.last() != Some(&account.name) {
                report.accounts_used.push(account.name.clone());
            }
            self.store.set_run_account(run_id, &account.name, None)?;
            self.store
                .set_run_state(run_id, RunState::Running, self.now())?;

            let (prompt, resume) = match &next {
                Continue::Start => (opts.prompt.clone(), None),
                Continue::Reply(sid) => (opts.prompt.clone(), Some(sid.as_str())),
                Continue::Message(sid, text) => (text.clone(), Some(sid.as_str())),
                Continue::Resume(sid) => (CONTINUE_PROMPT.to_owned(), Some(sid.as_str())),
                Continue::Fresh => {
                    let mut prompt =
                        handoff_prompt(written.as_ref().map(|w| w.task_md.as_str()), &opts.prompt);
                    // On another CLI than the session started on: the instructions it won't
                    // read by itself.
                    let started_on = report
                        .accounts_used
                        .first()
                        .and_then(|n| self.config.get(n))
                        .map(|a| a.vendor);
                    if started_on.is_some_and(|v| v != account.vendor)
                        && let Some(more) =
                            crate::handoff::instructions_for(&opts.cwd, account.vendor)
                    {
                        prompt += &more;
                    }
                    (prompt, None)
                }
            };
            self.note(
                opts,
                run_id,
                &format!(
                    "running on {}{}",
                    account.name,
                    resume.map_or(String::new(), |s| format!(" (resuming {s})"))
                ),
            );
            let stop_at = if stop_off_for.as_deref() == Some(account.name.as_str()) {
                0.0
            } else {
                account.stop_at(opts.stop_at)
            };
            let may_spend = picked_for_credits || account.credits == policy::CreditsUse::Allow;
            let attempt = self
                .attempt(
                    run_id,
                    account,
                    &prompt,
                    resume,
                    Spend {
                        stop_at,
                        budget_left: opts
                            .budget
                            .map(|b| b.saturating_sub(self.store.run_tokens(run_id).unwrap_or(0))),
                        may_spend,
                        plan: planning,
                        fork: matches!(next, Continue::Reply(_))
                            && opts.follow_up.as_ref().is_some_and(|f| f.fork),
                    },
                    opts,
                )
                .await?;
            if let Some(v) = attempt.structured.clone() {
                report.structured_output = Some(v);
            }
            // Background commands end with the CLI: say so, and tell the next launch.
            if !attempt.background.is_empty() {
                let list = attempt.background.join("; ");
                self.note(
                    opts,
                    run_id,
                    &format!(
                        "background commands the agent started on {} ended with it: {list}",
                        account.name
                    ),
                );
                background_note = Some(format!(
                    "background commands that were running and have stopped: {list}"
                ));
            }
            if let Some(sid) = &attempt.session_id {
                report.session_id = Some(sid.clone());
                self.store
                    .set_run_account(run_id, &account.name, Some(sid))?;
            }

            if attempt.limit_hit {
                let why = if attempt.stop_now {
                    "started paying from usage credits; stopped (credits: never)"
                } else {
                    "hit its limit"
                };
                self.note(opts, run_id, &format!("{} {why}", account.name));
                handoffs += 1;
                let why = match background_note.take() {
                    Some(b) => format!("limit reached; {b}"),
                    None => "limit reached".to_owned(),
                };
                written = self.write_handoff(
                    run_id,
                    opts,
                    account,
                    &why,
                    report.session_id.as_deref(),
                    handoffs,
                );
                next = match report.session_id.clone() {
                    Some(sid) => Continue::Resume(sid),
                    None => Continue::Fresh,
                };
                continue;
            }
            let stopped = match attempt.soft_stop.clone() {
                Some(reason) => Some(reason),
                None => self.store.take_stop(run_id)?,
            };
            if let Some(reason) = stopped {
                self.note(
                    opts,
                    run_id,
                    &format!("{} paused at a clean point ({reason})", account.name),
                );
                handoffs += 1;
                let why = match background_note.take() {
                    Some(b) => format!("paused, {reason}; {b}"),
                    None => format!("paused, {reason}"),
                };
                written = self.write_handoff(
                    run_id,
                    opts,
                    account,
                    &why,
                    report.session_id.as_deref(),
                    handoffs,
                );
                next = match report.session_id.clone() {
                    Some(sid) => Continue::Resume(sid),
                    None => Continue::Fresh,
                };
                soft_stop_from = Some(account);
                continue;
            }
            if attempt.session_not_found && fresh_tries == 0 {
                fresh_tries += 1;
                self.decide(
                    run_id,
                    Some(account),
                    Some(account),
                    "resume_failed",
                    json!({ "session_id": report.session_id }),
                )?;
                self.note(
                    opts,
                    run_id,
                    "session not found on this account; starting fresh with a handoff",
                );
                if written.is_none() {
                    handoffs += 1;
                    written = self.write_handoff(
                        run_id,
                        opts,
                        account,
                        "session not found",
                        None,
                        handoffs,
                    );
                }
                next = Continue::Fresh;
                continue;
            }
            // Messages the person sent while it worked continue the same session, here and now.
            if let Some(sid) = report.session_id.clone()
                && let Some(text) = self.take_queued(opts, run_id)?
            {
                launches = 0;
                next = Continue::Message(sid, text);
                continue;
            }
            // A finished plan waits for the person: approved → the same session carries it out;
            // their words → it revises the plan. No answer: the run ends with the plan.
            if planning
                && let Some((true, Some(plan))) = &attempt.finished
                && let Some(sid) = report.session_id.clone()
                && let Some(answer) = self.approve_plan(opts, run_id, plan).await
            {
                launches = 0;
                let text = match answer {
                    PlanAnswer::Approved => {
                        planning = false;
                        PLAN_APPROVED.to_owned()
                    }
                    PlanAnswer::Edited(plan) => {
                        planning = false;
                        format!("{PLAN_EDITED}\n\n{plan}")
                    }
                    PlanAnswer::Change(words) => format!("Change the plan: {words}"),
                };
                self.log_line(
                    opts,
                    run_id,
                    &json!({ "type": "anchorleg_user", "text": text }),
                );
                next = Continue::Message(sid, text);
                continue;
            }
            // A goal: the agent's `GOAL:` line decides whether the session goes on (D48).
            if !planning
                && let Some(g) = report.goal.as_mut()
                && let Some((true, text)) = &attempt.finished
            {
                g.turns += 1;
                let verdict = goal::verdict(text.as_deref().unwrap_or(""));
                let (status, reason) = match verdict {
                    Some(goal::Verdict::Met(r)) => (goal::Status::Met, r),
                    Some(goal::Verdict::Impossible(r)) => (goal::Status::Impossible, r),
                    Some(goal::Verdict::NotMet(r)) => (goal::Status::Active, r),
                    None => (goal::Status::Active, String::new()),
                };
                g.reason = (!reason.is_empty()).then(|| reason.clone());
                let status = if status == goal::Status::Active && g.turns >= opts.goal_turns {
                    goal::Status::Unmet
                } else {
                    status
                };
                g.status = status;
                self.store
                    .set_run_goal(run_id, Some(&g.condition), Some(status.name()))?;
                let turns = g.turns;
                let with_reason = |s: &str| {
                    if reason.is_empty() {
                        s.to_owned()
                    } else {
                        format!("{s}: {reason}")
                    }
                };
                match status {
                    goal::Status::Met => self.note(opts, run_id, &with_reason("goal met")),
                    goal::Status::Impossible => {
                        self.note(opts, run_id, &with_reason("goal can't be met"));
                    }
                    goal::Status::Unmet => self.note(
                        opts,
                        run_id,
                        &with_reason(&format!("goal not met after {turns} turns")),
                    ),
                    goal::Status::Active => {
                        if let Some(sid) = report.session_id.clone() {
                            self.note(
                                opts,
                                run_id,
                                &with_reason(&format!(
                                    "goal not met yet ({turns}/{})",
                                    opts.goal_turns
                                )),
                            );
                            let text = goal::continue_text(&reason);
                            self.log_line(
                                opts,
                                run_id,
                                &json!({ "type": "anchorleg_user", "text": text }),
                            );
                            launches = 0;
                            next = Continue::Message(sid, text);
                            continue;
                        }
                    }
                }
            }
            match attempt.finished {
                Some((ok, text)) => {
                    report.result = text;
                    if ok {
                        report.outcome = Outcome::Done;
                    }
                }
                None => {
                    report.result = Some(format!(
                        "{} exited without a result ({})",
                        account.name,
                        attempt
                            .exit_status
                            .map_or("killed".to_owned(), |s| s.to_string())
                    ));
                }
            }
            break;
        }

        // A goal that ended unmet fails the run, so scripts can tell.
        if report.outcome == Outcome::Done
            && report
                .goal
                .as_ref()
                .is_some_and(|g| matches!(g.status, goal::Status::Impossible | goal::Status::Unmet))
        {
            report.outcome = Outcome::Failed;
        }
        if opts.json_schema.is_some()
            && report.structured_output.is_none()
            && report.outcome == Outcome::Done
        {
            report.structured_output = report
                .result
                .as_deref()
                .and_then(|t| serde_json::from_str::<Value>(t.trim()).ok());
        }
        if let Some(tokens) = opts.budget {
            report.budget = Some(crate::output::BudgetReport {
                tokens,
                spent: self.store.run_tokens(run_id)?,
            });
        }
        let state = match report.outcome {
            Outcome::Done => RunState::Done,
            _ => RunState::Failed,
        };
        self.store.set_run_state(run_id, state, self.now())?;
        self.store.set_run_pid(run_id, None)?;
        Ok(report)
    }

    /// Claude and Codex accounts' quota, read live before every pick so a launch goes to a login
    /// that still has room (D23, D64). A run's own reading from the last two minutes is used as
    /// it is.
    async fn refresh_quota(
        &self,
        accounts: &[&Account],
        opts: &RunOptions,
        run_id: i64,
    ) -> Result<(), RunError> {
        let failed = refresh_quota(
            self.config,
            self.store,
            self.secrets,
            &self.home,
            accounts,
            self.now(),
            FRESH_QUOTA,
        )
        .await?;
        for (name, e) in failed {
            self.note(opts, run_id, &format!("{name}: couldn't read quota ({e})"));
        }
        Ok(())
    }

    /// Record a switch and prepare the next account to continue the session.
    fn switch(
        &self,
        run_id: i64,
        (from, to, rule): (&Account, &Account, &str),
        next: &mut Continue,
        written: Option<&Written>,
        report: &mut RunReport,
        opts: &RunOptions,
    ) -> Result<(), RunError> {
        let at = self.now();
        if from.vendor != to.vendor
            && matches!(
                next,
                Continue::Resume(_) | Continue::Reply(_) | Continue::Message(..)
            )
        {
            self.note(
                opts,
                run_id,
                &format!(
                    "{} runs another CLI; it starts fresh from the handoff",
                    to.name
                ),
            );
            *next = Continue::Fresh;
        }
        // A big context costs the next account a full uncached read; the handoff is small.
        let tokens = written.and_then(|w| w.context_tokens);
        if let (Continue::Resume(_), Some(n)) = (&*next, tokens)
            && n > opts.fresh_above
        {
            self.note(
                opts,
                run_id,
                &format!(
                    "context is {n} tokens; {} starts fresh from the handoff",
                    to.name
                ),
            );
            *next = Continue::Fresh;
        }
        let synced = match &*next {
            Continue::Resume(sid) | Continue::Reply(sid) | Continue::Message(sid, _) => {
                self.sync_session(from, to, sid)
            }
            _ => None,
        };
        self.decide(
            run_id,
            Some(from),
            Some(to),
            rule,
            json!({
                "context_synced": synced,
                "context_tokens": tokens,
                "fresh": matches!(next, Continue::Fresh),
            }),
        )?;
        report.switches.push(Switch {
            from: from.name.clone(),
            to: to.name.clone(),
            rule: rule.to_owned(),
            at,
        });
        self.note(
            opts,
            run_id,
            &format!("switching {} → {}", from.name, to.name),
        );
        Ok(())
    }

    /// Copy a session's files to the account taking it over, so it can resume there.
    fn sync_session(&self, from: &Account, to: &Account, sid: &str) -> Option<serde_json::Value> {
        if let (Some(a), Some(b)) = (
            from.claude_config_dir(&self.home),
            to.claude_config_dir(&self.home),
        ) {
            let synced = sync_context(&a, &b, sid).unwrap_or_else(|e| {
                tracing::warn!(%e, "copying context of session {sid}");
                claude::ContextSync::default()
            });
            return Some(json!(synced));
        }
        if let (Some(a), Some(b)) = (from.codex_home(&self.home), to.codex_home(&self.home)) {
            let copied = codex::sync_session(&a, &b, sid).unwrap_or_else(|e| {
                tracing::warn!(%e, "copying codex thread {sid}");
                false
            });
            return Some(json!({ "rollout": copied }));
        }
        if let (Some(a), Some(b)) = (from.agy_home(&self.home), to.agy_home(&self.home)) {
            let copied = agy::sync_session(&a, &b, sid).unwrap_or_else(|e| {
                tracing::warn!(%e, "copying agy conversation {sid}");
                false
            });
            return Some(json!({ "conversation": copied }));
        }
        None
    }

    fn decide(
        &self,
        run_id: i64,
        from: Option<&Account>,
        to: Option<&Account>,
        rule: &str,
        detail: serde_json::Value,
    ) -> Result<(), RunError> {
        self.store.record_decision(&Decision {
            run_id: Some(run_id),
            at: self.now(),
            from_account: from.map(|a| a.name.clone()),
            to_account: to.map(|a| a.name.clone()),
            rule: rule.to_owned(),
            detail,
        })?;
        Ok(())
    }

    /// Put anchorleg's approval hook into an agy account's own `HOME` (D26). `false` (and the
    /// run uses `--mode accept-edits` instead) for an account on the person's own `HOME`, when
    /// anchorleg's binary or data folder is unknown, or when the hook can't be written.
    fn agy_hook(&self, run_id: i64, account: &Account, opts: &RunOptions) -> bool {
        let (Some(home), Some(bin), Some(_)) = (
            account.agy_home(&self.home),
            opts.relay_bin.as_deref(),
            opts.data_dir.as_deref(),
        ) else {
            return false;
        };
        if home == self.home {
            return false;
        }
        // Questions: agy's `-p` drops its own tool, so anchorleg's MCP server offers `ask_user`.
        if let Err(e) = agy::install_mcp(&home, bin) {
            self.note(
                opts,
                run_id,
                &format!(
                    "couldn't add anchorleg's question tool for {} ({e}); the agent can't ask you",
                    account.name
                ),
            );
        }
        match agy::install_hook(&home, bin) {
            Ok(()) => true,
            Err(e) => {
                self.note(
                    opts,
                    run_id,
                    &format!(
                        "couldn't add anchorleg's approval hook for {} ({e}); edits only",
                        account.name
                    ),
                );
                false
            }
        }
    }

    /// Launch the CLI once and follow its stream until it exits.
    async fn attempt(
        &self,
        run_id: i64,
        account: &Account,
        prompt: &str,
        resume: Option<&str>,
        Spend {
            stop_at,
            budget_left,
            may_spend,
            plan,
            fork,
        }: Spend,
        opts: &RunOptions,
    ) -> Result<Attempt, RunError> {
        let mut launch = self.config.launch(account, self.secrets, &self.home)?;
        if account.vendor == Vendor::Codex {
            // An exec-only flag `codex app-server` rejects; anchorleg's defaults do the same.
            launch.args.retain(|a| a != "--full-auto");
        }
        // Planning and reviewing change nothing.
        let read_only = plan || opts.review || opts.side;
        if account.vendor == Vendor::Claude {
            // Plan mode, then the session's own mode, replace the settings' permission mode. A
            // review isn't a plan: Claude Code answers plan mode with a bigger model than the
            // one picked (Sonnet for Haiku, seen on 2.1.296), so it runs in the normal mode with
            // the editing tools taken away and read-only git commands allowed.
            let mode = if opts.review || opts.side {
                Some("default")
            } else if read_only {
                Some("plan")
            } else {
                opts.mode.map(Mode::claude)
            };
            if let Some(m) = mode {
                launch.args = with_claude_mode(std::mem::take(&mut launch.args), m);
            }
            if opts.review {
                launch.args.extend(
                    [
                        "--disallowedTools",
                        REVIEW_DENIED_TOOLS,
                        "--allowedTools",
                        REVIEW_ALLOWED_TOOLS,
                    ]
                    .map(str::to_owned),
                );
            } else if opts.side {
                launch
                    .args
                    .extend(["--disallowedTools", REVIEW_DENIED_TOOLS].map(str::to_owned));
            }
            if let Some(schema) = opts.json_schema.as_ref().filter(|_| !plan) {
                launch.args.push("--json-schema".to_owned());
                launch.args.push(schema.to_string());
            }
            if let Some(name) = &opts.name {
                launch.args.push("--name".to_owned());
                launch.args.push(name.clone());
            }
            // The person's lasting allow rules (Claude applies them itself) and the task's own
            // Claude options. Not while planning or reviewing, which stay read-only.
            if !read_only {
                launch
                    .args
                    .extend(opts.task.claude.args(&self.config.allow));
            }
            launch.args.extend(opts.task.add_dir_args());
            if let Some(i) = &opts.task.instructions {
                launch.args.push("--append-system-prompt".to_owned());
                launch.args.push(i.clone());
            }
        } else if !opts.task.claude.is_empty() {
            self.note(
                opts,
                run_id,
                &format!(
                    "{} has no {}: left out on {}",
                    account.vendor.caps().label,
                    opts.task.claude.names().join(", "),
                    account.name
                ),
            );
        }
        // Claude's plan mode can't end with its ExitPlanMode tool in `-p`, and agy has no plan
        // mode: say how a plan is handed over.
        let planned;
        let prompt = if plan && account.vendor != Vendor::Codex {
            planned = format!("{prompt}\n\n{PLAN_NOTE}");
            planned.as_str()
        } else if !read_only && let Some(g) = &opts.goal {
            // Every goal turn is told the goal, so it holds on any account or CLI (D48).
            planned = format!("{prompt}\n\n{}", goal::note(g));
            planned.as_str()
        } else {
            prompt
        };
        let mut cmd = launch.command();
        let mut codex_turn: Option<codex_server::Turn> = None;
        // The provider's model and effort, shared by all its accounts (D20); `--model` on
        // `anchorleg run` overrides the model for this run.
        let shared = self.config.vendors.get(&account.vendor);
        let model_args = account.vendor.model_args(
            opts.model
                .as_deref()
                .or(shared.and_then(|s| s.model.as_deref())),
            opts.task
                .effort
                .as_deref()
                .or(shared.and_then(|s| s.effort.as_deref())),
        );
        if account.vendor == Vendor::Antigravity {
            // `agy --conversation <unknown id>` silently starts a new conversation instead.
            if let (Some(sid), Some(home)) = (resume, account.agy_home(&self.home))
                && !agy::has_conversation(&home, sid)
            {
                return Ok(Attempt {
                    session_not_found: true,
                    ..Attempt::default()
                });
            }
            let gated = !agy::sets_mode(&launch.args) && self.agy_hook(run_id, account, opts);
            if gated {
                cmd.arg(agy::SKIP_PERMISSIONS);
                // The hook keeps the task's files alone while planning and asks before edits in
                // `--mode ask` (agy's own `--mode plan` goes on to carry the plan out once
                // permissions are skipped; seen on 1.3.3).
                let mode = if opts.review || opts.side {
                    agy::HookMode::Review
                } else if plan {
                    agy::HookMode::Plan
                } else if opts.mode == Some(Mode::Ask) {
                    agy::HookMode::Ask
                } else {
                    agy::HookMode::Edits
                };
                cmd.env("ANCHORLEG_ACCOUNT", &account.name)
                    .env("ANCHORLEG_RUN_ID", run_id.to_string())
                    .env(agy::MODE_ENV, mode.name());
                if let Some(dir) = &opts.data_dir {
                    cmd.env("ANCHORLEG_HOME", dir);
                }
                if let Some(path) = &opts.config_path {
                    cmd.env("ANCHORLEG_CONFIG", path);
                }
            }
            // agy has no instructions flag: a new conversation starts with them.
            let prompt = match resume {
                None => opts.task.agy_prompt(prompt),
                Some(_) => prompt.to_owned(),
            };
            cmd.arg("-p").arg(prompt);
            cmd.args(["--output-format", "stream-json"]);
            cmd.args(opts.task.add_dir_args());
            if let Some(schema) = opts.json_schema.as_ref().filter(|_| !plan) {
                cmd.arg("--json-schema").arg(schema.to_string());
            }
            if !gated && !agy::sets_mode(&launch.args) {
                // Without the hook, agy's own plan mode keeps the files as they are: in `-p` it
                // reads, writes its plan and has its edits refused (seen on 1.3.3).
                if read_only {
                    cmd.args(["--mode", "plan"]);
                } else {
                    cmd.args(agy::ACCEPT_EDITS);
                }
            }
            cmd.args(&model_args);
            if let Some(sid) = resume {
                cmd.args(["--conversation", sid]);
            }
        } else if account.vendor == Vendor::Codex {
            // `codex exec resume` of a thread this login has never seen exits with no JSON at all.
            if let (Some(sid), Some(home)) = (resume, account.codex_home(&self.home))
                && codex::rollout_file(&home, sid).is_none()
            {
                return Ok(Attempt {
                    session_not_found: true,
                    ..Attempt::default()
                });
            }
            // The app server can ask before a command or an edit; `exec` can't (D27). Model,
            // effort, sandbox and approvals go in the thread and turn requests.
            let args = &launch.args;
            // Codex offers `request_user_input` outside plan mode only with this feature (D36).
            // `-c`, not `--enable`: a Codex that doesn't know the feature ignores it.
            if opts.relay_bin.is_some() && !args.iter().any(|a| a.contains(codex::ASK_FEATURE)) {
                cmd.arg("-c")
                    .arg(format!("features.{}=true", codex::ASK_FEATURE));
            }
            cmd.arg("app-server");
            let picks_approvals = args.iter().any(|a| {
                a == "-a"
                    || a.starts_with("--ask-for-approval")
                    || a.contains("approval_policy")
                    || a.starts_with("--dangerously-bypass-approvals")
            });
            codex_turn = Some(codex_server::Turn {
                cwd: opts.cwd.display().to_string(),
                prompt: prompt.to_owned(),
                resume: resume.map(str::to_owned),
                model: opts
                    .model
                    .clone()
                    .or_else(|| shared.and_then(|s| s.model.clone())),
                effort: opts
                    .task
                    .effort
                    .clone()
                    .or_else(|| shared.and_then(|s| s.effort.clone())),
                sandbox: if read_only {
                    Some("read-only".to_owned())
                } else {
                    (!codex::sets_sandbox(args)).then(|| "workspace-write".to_owned())
                },
                // Nobody to ask without anchorleg's binary: Codex then decides alone, as `exec` did.
                approval: (!picks_approvals).then(|| {
                    if opts.relay_bin.is_some() {
                        "on-request"
                    } else {
                        "never"
                    }
                    .to_owned()
                }),
                stop_at,
                // Only the person's own message carries them, not a "continue" after a switch.
                images: match resume {
                    Some(_) if prompt == CONTINUE_PROMPT => Vec::new(),
                    _ => opts
                        .images
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect(),
                },
                plan,
                review: opts.review,
                fork,
                fork_at: opts
                    .follow_up
                    .as_ref()
                    .and_then(|f| f.at.clone())
                    .filter(|_| fork),
                auto_review: !read_only && opts.mode == Some(Mode::Auto),
                budget_left,
                output_schema: opts.json_schema.clone().filter(|_| !plan),
                name: opts.name.clone(),
                instructions: opts.task.instructions.clone(),
                config: opts.task.codex_config(),
            });
        } else {
            if let Some(dir) = &opts.mod_dir {
                cmd.arg("--plugin-dir").arg(dir);
                cmd.env("ANCHORLEG_ACCOUNT", &account.name)
                    .env("ANCHORLEG_RUN_ID", run_id.to_string())
                    .env("ANCHORLEG_STOP_AT", stop_at.to_string());
                if let Some(left) = budget_left {
                    cmd.env(BUDGET_ENV, left.to_string());
                }
                // In auto mode Claude's classifier answers what it would ask about; the mod
                // leaves those to it instead of asking the person.
                if claude_mode(&launch.args) == Some("auto") {
                    cmd.env(PERMISSION_MODE_ENV, "auto");
                }
                if let Some(bin) = &opts.relay_bin {
                    cmd.env("ANCHORLEG_BIN", bin);
                }
            }
            cmd.arg("-p").arg(prompt);
            cmd.args(["--output-format", "stream-json", "--verbose"]);
            // The person's hooks: what they block or say shows in the conversation.
            if !launch.args.iter().any(|a| a == "--include-hook-events") {
                cmd.arg("--include-hook-events");
            }
            cmd.args(&model_args);
            if let Some(sid) = resume {
                cmd.args(["--resume", sid]);
                if fork {
                    if let Some(at) = opts.follow_up.as_ref().and_then(|f| f.at.as_deref()) {
                        cmd.args(["--resume-session-at", at]);
                    }
                    cmd.arg("--fork-session");
                }
            }
        }
        // Without this, `claude -p` waits 3 s for stdin on every launch (Phase 0).
        cmd.current_dir(&opts.cwd)
            .stdin(if codex_turn.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            // The app server logs its internals (e.g. "TurnAborted" after a refusal) to stderr;
            // everything anchorleg needs comes over the protocol.
            .stderr(if codex_turn.is_some() {
                Stdio::null()
            } else {
                Stdio::inherit()
            });
        let mut child = tokio::process::Command::from(cmd)
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| RunError::Spawn {
                program: launch.program.clone(),
                source,
            })?;
        let stdout = child.stdout.take().expect("stdout is piped");
        // Messages the person sends while a Codex turn runs go straight into it (`turn/steer`).
        let mut steer_tx: Option<tokio::sync::mpsc::UnboundedSender<Steer>> = None;
        let mut source = match codex_turn {
            Some(turn) => {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                let (stx, srx) = tokio::sync::mpsc::unbounded_channel();
                steer_tx = Some(stx);
                let stdin = child.stdin.take().expect("stdin is piped");
                let asker = opts.relay_bin.clone().map(|bin| Asker { bin, run_id });
                tokio::spawn(async move {
                    if let Err(e) = codex_server::drive(
                        BufReader::new(stdout),
                        stdin,
                        turn,
                        asker,
                        tx,
                        Some(srx),
                    )
                    .await
                    {
                        tracing::warn!(%e, "talking to codex app-server");
                    }
                });
                Feed::Codex(rx)
            }
            None => Feed::Lines(BufReader::new(stdout).lines()),
        };
        let mut raw_log = self.open_log(opts.log_dir.as_deref(), run_id);
        let mut attempt = Attempt {
            may_spend,
            ..Attempt::default()
        };
        let mut deadline: Option<tokio::time::Instant> = None;
        let mut decoder = Decoder::new(account.vendor);
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        // Queued messages already handed to the Codex driver.
        let mut forwarded: Vec<i64> = Vec::new();
        // agy was stopped for a message sent with "interrupt".
        let mut interrupted = false;

        loop {
            let item = tokio::select! {
                item = next_item(&mut source, deadline) => match item {
                    Some(item) => item?,
                    None => {
                        tracing::warn!(account = %account.name, "CLI still running after its limit hit; killing it");
                        child.start_kill()?;
                        break;
                    }
                },
                _ = tick.tick(), if steer_tx.is_some() || account.vendor == Vendor::Antigravity => {
                    // agy takes nothing mid-turn: "interrupt" stops it, and the session goes on
                    // with the message (as Claude's does at its next step, through the mod).
                    if account.vendor == Vendor::Antigravity
                        && !interrupted
                        && self.store.interrupt_requested(run_id)?
                    {
                        interrupted = true;
                        self.note(opts, run_id, "stopping here: the user sent a new message");
                        child.start_kill()?;
                    }
                    if let Some(tx) = &steer_tx {
                        for m in self.store.queued_messages(run_id)? {
                            if forwarded.contains(&m.id) {
                                continue;
                            }
                            forwarded.push(m.id);
                            let _ = tx.send(if m.interrupt {
                                Steer::Interrupt
                            } else {
                                Steer::Text { id: m.id, text: m.text }
                            });
                        }
                    }
                    continue;
                }
            };
            let line = match item {
                None => break,
                Some(Event::Line(line)) => line,
                Some(Event::Quota(readings)) => {
                    // A full, current snapshot pushed by Codex during the turn.
                    self.store.replace_quota(
                        &account.name,
                        &readings,
                        Source::Stream,
                        self.now(),
                    )?;
                    continue;
                }
                Some(Event::Credits(c)) => {
                    self.on_signal(run_id, account, AgentSignal::Credits(c), &mut attempt, opts)?;
                    continue;
                }
                Some(Event::NotFound) => {
                    attempt.session_not_found = true;
                    continue;
                }
                Some(Event::Stopped(reason)) => {
                    attempt.soft_stop = Some(reason);
                    continue;
                }
                Some(Event::Steered(id)) => {
                    if let Some(m) = self
                        .store
                        .queued_messages(run_id)?
                        .into_iter()
                        .find(|m| m.id == id)
                        && self.store.take_message(id, self.now())?
                    {
                        self.log_line(
                            opts,
                            run_id,
                            &json!({ "type": "anchorleg_user", "text": m.text }),
                        );
                    }
                    continue;
                }
            };
            if let Some(f) = raw_log.as_mut() {
                let _ = writeln!(f, "{line}");
            }
            if line.trim().is_empty() {
                continue;
            }
            let mut signals = decoder.feed(&line);
            // Claude says nothing when it answers with another model; compare.
            if matches!(decoder, Decoder::Claude)
                && let Some(model) = claude::reply_model(&line)
                && attempt.model.as_ref().is_some_and(|m| *m != model)
            {
                signals.push(AgentSignal::Rerouted {
                    from: attempt.model.clone(),
                    to: model,
                    reason: None,
                });
            }
            for signal in signals {
                self.on_signal(run_id, account, signal, &mut attempt, opts)?;
            }
            if attempt.limit_hit && deadline.is_none() {
                // Every extra second on unpermitted credits costs money; otherwise give the CLI
                // time to wrap up.
                let grace = if attempt.stop_now {
                    Duration::ZERO
                } else {
                    opts.limit_grace
                };
                deadline = Some(tokio::time::Instant::now() + grace);
            }
        }
        if matches!(source, Feed::Codex(_)) {
            // The turn is over and the driver closed the server's stdin; give it a moment to
            // exit, then make sure it does.
            if tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .is_err()
            {
                child.start_kill()?;
            }
        }
        attempt.exit_status = Some(child.wait().await?);
        if account.vendor == Vendor::Codex {
            self.codex_quota(account, resume, &mut attempt)?;
        }
        Ok(attempt)
    }

    /// Codex's stream has no quota: read it from the thread's rollout file once the CLI exits.
    /// A window the rollout marks as reached turns a failed launch into a limit hit.
    fn codex_quota(
        &self,
        account: &Account,
        resume: Option<&str>,
        attempt: &mut Attempt,
    ) -> Result<(), RunError> {
        let Some(sid) = attempt.session_id.as_deref().or(resume) else {
            return Ok(());
        };
        let Some(text) = account
            .codex_home(&self.home)
            .and_then(|home| codex::rollout_file(&home, sid))
            .and_then(|f| std::fs::read_to_string(f).ok())
        else {
            return Ok(());
        };
        if let Some(c) = codex::rollout_credits(&text) {
            self.store
                .record_credits(&account.name, &c, Source::Stream, self.now())?;
        }
        let mut rejected = false;
        for reading in codex::rollout_quota(&text) {
            rejected |= reading.status == Some(QuotaStatus::Rejected);
            self.store
                .record_quota(&account.name, &reading, Source::Stream, self.now())?;
        }
        if rejected && !matches!(attempt.finished, Some((true, _))) {
            attempt.limit_hit = true;
        }
        Ok(())
    }

    fn on_signal(
        &self,
        run_id: i64,
        account: &Account,
        signal: AgentSignal,
        attempt: &mut Attempt,
        opts: &RunOptions,
    ) -> Result<(), RunError> {
        match signal {
            AgentSignal::Background(cmd) => {
                if !attempt.background.contains(&cmd) {
                    attempt.background.push(cmd);
                }
            }
            AgentSignal::Rerouted { from, to, reason } => {
                self.note(
                    opts,
                    run_id,
                    &format!(
                        "{} answers with {to}{}{}",
                        account.name,
                        from.as_deref()
                            .map_or(String::new(), |f| format!(" instead of {f}")),
                        reason
                            .as_deref()
                            .map_or(String::new(), |r| format!(" ({r})"))
                    ),
                );
                self.decide(
                    run_id,
                    Some(account),
                    Some(account),
                    "model_rerouted",
                    json!({ "from": from, "to": to, "reason": reason }),
                )?;
                attempt.model = Some(to);
            }
            AgentSignal::Started { session_id, model } => {
                attempt.model = attempt.model.take().or(model);
                // Saved right away so a live session can be watched and replied to.
                self.store
                    .set_run_account(run_id, &account.name, Some(&session_id))?;
                attempt.session_id = Some(session_id);
            }
            AgentSignal::Quota(reading) => {
                self.store
                    .record_quota(&account.name, &reading, Source::Stream, self.now())?;
            }
            AgentSignal::Credits(c) => self.on_credits(account, c, attempt)?,
            AgentSignal::Usage(u) => {
                self.store.record_usage(
                    Some(run_id),
                    &account.name,
                    &u,
                    attempt.on_credits,
                    self.now(),
                )?;
            }
            AgentSignal::LimitHit { window, resets_at } => {
                // Credits the account may spend cover the limit: let the CLI go on. If it then
                // fails anyway, the limit counts after all.
                if attempt.may_spend
                    && attempt.credits.is_some_and(CreditState::spendable)
                    && attempt.finished.is_none()
                {
                    attempt.limit_deferred = true;
                } else {
                    attempt.limit_hit = true;
                }
                let reading = QuotaReading {
                    window,
                    status: Some(QuotaStatus::Rejected),
                    used: None,
                    resets_at,
                };
                self.store
                    .record_quota(&account.name, &reading, Source::Stream, self.now())?;
            }
            AgentSignal::SessionNotFound => attempt.session_not_found = true,
            AgentSignal::Structured(v) => attempt.structured = Some(v),
            AgentSignal::Finished { ok, text, .. } => {
                if !ok && attempt.limit_deferred {
                    attempt.limit_hit = true;
                }
                attempt.finished = Some((ok, text));
            }
            AgentSignal::Retrying { .. } => {}
        }
        Ok(())
    }

    /// Ask the person to approve the plan the agent just finished. `None`: nobody to ask, or
    /// no answer (skipped, expired).
    async fn approve_plan(&self, opts: &RunOptions, run_id: i64, plan: &str) -> Option<PlanAnswer> {
        let bin = opts.relay_bin.as_ref()?;
        self.note(opts, run_id, "plan ready; waiting for your approval");
        let q = crate::questions::plan_question();
        let answers =
            codex_server::ask_questions_of(bin, run_id, std::slice::from_ref(&q), Some(plan))
                .await?;
        let answer = answers.get(&q.id)?.first()?.trim().to_owned();
        match answer.as_str() {
            "" => None,
            crate::questions::PLAN_APPROVE => Some(PlanAnswer::Approved),
            crate::questions::PLAN_REVISE => Some(PlanAnswer::Change(
                "look again and improve it; ask me if anything is unclear.".to_owned(),
            )),
            _ => match answer.strip_prefix(crate::questions::PLAN_EDITED) {
                Some(edited) => Some(PlanAnswer::Edited(edited.trim().to_owned())),
                None => Some(PlanAnswer::Change(answer)),
            },
        }
    }

    /// Every message waiting for this run, marked delivered and logged as the person's words.
    fn take_queued(&self, opts: &RunOptions, run_id: i64) -> Result<Option<String>, RunError> {
        let mut texts = Vec::new();
        for m in self.store.queued_messages(run_id)? {
            if self.store.take_message(m.id, self.now())? {
                self.log_line(
                    opts,
                    run_id,
                    &json!({ "type": "anchorleg_user", "text": m.text }),
                );
                texts.push(m.text);
            }
        }
        Ok((!texts.is_empty()).then(|| texts.join("\n\n")))
    }

    /// A credit report: remember it, and stop a launch that began paying without permission.
    fn on_credits(
        &self,
        account: &Account,
        credits: Credits,
        attempt: &mut Attempt,
    ) -> Result<(), RunError> {
        self.store
            .record_credits(&account.name, &credits, Source::Stream, self.now())?;
        attempt.credits = Some(credits.state);
        if credits.state == CreditState::InUse {
            if attempt.may_spend {
                attempt.on_credits = true;
            } else if !attempt.limit_hit {
                tracing::info!(account = %account.name, "CLI is paying from usage credits; stopping");
                attempt.limit_hit = true;
                attempt.stop_now = true;
                // The plan's windows are spent even if the CLI still says "allowed": block the
                // account so the next pick doesn't come straight back to it.
                let reading = QuotaReading {
                    window: None,
                    status: Some(QuotaStatus::Rejected),
                    used: None,
                    resets_at: None,
                };
                self.store
                    .record_quota(&account.name, &reading, Source::Stream, self.now())?;
            }
        }
        Ok(())
    }

    /// Write `.handoff/TASK.md` and a git checkpoint for a launch that stopped early. Failures
    /// are logged, never fatal: the switch still happens without them.
    fn write_handoff(
        &self,
        run_id: i64,
        opts: &RunOptions,
        from: &Account,
        reason: &str,
        session_id: Option<&str>,
        n: usize,
    ) -> Option<Written> {
        let transcript = self.transcript(run_id, opts, from, session_id);
        let cp = checkpoint::save(
            &opts.cwd,
            run_id,
            n,
            &format!(
                "anchorleg checkpoint {n} of run {run_id}: {} {reason}",
                from.name
            ),
        )
        .unwrap_or_else(|e| {
            tracing::warn!(%e, "git checkpoint");
            None
        });
        let git_status = checkpoint::status(&opts.cwd);
        let goals = self.goals(run_id);
        let written = handoff::write(&opts.cwd, |notes| {
            handoff::render(&handoff::Handoff {
                goals: &goals,
                transcript: &transcript,
                cwd: &opts.cwd,
                from: &from.name,
                reason,
                git_status: git_status.as_deref(),
                checkpoint: cp.as_ref().map(|c| c.refname.as_str()),
                notes,
                done_when: opts.goal.as_deref(),
            })
        });
        let path = match written {
            Ok(path) => path,
            Err(e) => {
                tracing::warn!(%e, "writing the handoff");
                return None;
            }
        };
        let task_md = std::fs::read_to_string(&path).ok()?;
        let _ = self.decide(
            run_id,
            Some(from),
            None,
            "handoff",
            json!({
                "path": path,
                "checkpoint": cp,
                "context_tokens": transcript.context_tokens,
            }),
        );
        self.note(
            opts,
            run_id,
            &format!(
                "handoff written to {}/TASK.md{}",
                handoff::DIR,
                cp.as_ref()
                    .map_or(String::new(), |c| format!(", checkpoint {}", c.refname))
            ),
        );
        Some(Written {
            task_md,
            context_tokens: transcript.context_tokens,
        })
    }

    /// What the session did: Claude's session file when it has the history, else this run's log.
    fn transcript(
        &self,
        run_id: i64,
        opts: &RunOptions,
        account: &Account,
        session_id: Option<&str>,
    ) -> handoff::Transcript {
        let from_session = session_id
            .zip(account.claude_config_dir(&self.home))
            .and_then(|(sid, dir)| claude::session_file(&dir, sid))
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|s| handoff::extract(&s))
            .filter(|t| t.last_message.is_some() || t.context_tokens.is_some());
        from_session.unwrap_or_else(|| {
            // This run's log and those of the runs it follows up, oldest first, so a handoff to
            // another CLI keeps what earlier messages did.
            let log: String = self
                .chain(run_id)
                .into_iter()
                .filter_map(|id| {
                    let dir = opts.log_dir.as_ref()?;
                    std::fs::read_to_string(dir.join(format!("run-{id}.jsonl"))).ok()
                })
                .collect::<Vec<_>>()
                .join("\n");
            handoff::extract(&log)
        })
    }

    /// The task, then each follow-up message, oldest first.
    fn goals(&self, run_id: i64) -> Vec<String> {
        self.chain(run_id)
            .into_iter()
            .filter_map(|id| self.store.run(id).ok().flatten().map(|r| r.task))
            .collect()
    }

    /// The run and the runs it follows up, oldest first.
    fn chain(&self, run_id: i64) -> Vec<i64> {
        let mut ids = Vec::new();
        let mut id = Some(run_id);
        while let Some(run) = id.and_then(|i| self.store.run(i).ok().flatten()) {
            ids.push(run.id);
            id = run.parent_id;
            if ids.len() > 100 {
                break;
            }
        }
        ids.reverse();
        ids
    }

    /// A follow-up picked up by a different account than the session's last: copy the context.
    fn carry_follow_up(&self, opts: &RunOptions, run_id: i64, to: &Account) {
        let Some(f) = &opts.follow_up else { return };
        if f.account == to.name {
            return;
        }
        let Some(from) = self.config.get(&f.account) else {
            return;
        };
        if from.vendor == to.vendor && self.sync_session(from, to, &f.session_id).is_some() {
            self.note(
                opts,
                run_id,
                &format!("continuing {}'s session on {}", f.account, to.name),
            );
        }
    }

    /// Tell the person (stderr) and keep it in the run log for `anchorleg ui`.
    fn note(&self, opts: &RunOptions, run_id: i64, message: &str) {
        self.progress.event(message);
        self.log_line(
            opts,
            run_id,
            &json!({ "type": "anchorleg", "text": message }),
        );
    }

    fn log_line(&self, opts: &RunOptions, run_id: i64, line: &serde_json::Value) {
        if let Some(mut f) = self.open_log(opts.log_dir.as_deref(), run_id) {
            let _ = writeln!(f, "{line}");
        }
    }

    fn open_log(&self, dir: Option<&Path>, run_id: i64) -> Option<std::fs::File> {
        let dir = dir?;
        std::fs::create_dir_all(dir).ok()?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("run-{run_id}.jsonl")))
            .ok()
    }
}
