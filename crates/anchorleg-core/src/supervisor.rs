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

use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::adapters::claude::{self, sync_context};
use crate::adapters::{AgentSignal, EpochSecs, QuotaReading, QuotaStatus};
use crate::adapters::{agy, codex};
use crate::output::{Outcome, RunReport, SCHEMA, Switch};
use crate::policy::{self, Pick, PickOptions};
use crate::registry::{Account, Config, SecretStore, Vendor};
use crate::store::{Decision, RunState, Source, Store};
use crate::{checkpoint, handoff};

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
    /// The `anchorleg` binary the mod calls back (`anchorleg report --json`).
    pub relay_bin: Option<PathBuf>,
    /// The mod stops a run at a clean point once any window reaches this fraction.
    pub stop_at: f64,
    /// Send `prompt` as a new message to an existing session instead of starting one.
    pub follow_up: Option<FollowUp>,
    /// At a switch, a session with more context than this (tokens) starts fresh from
    /// `.handoff/TASK.md` instead of being resumed on a cold cache.
    pub fresh_above: u64,
}

/// An existing session to continue with a new message.
#[derive(Debug, Clone)]
pub struct FollowUp {
    pub parent: i64,
    pub session_id: String,
    /// The account the session was last on; its context is copied if another one takes over.
    pub account: String,
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
            stop_at: 0.9,
            follow_up: None,
            fresh_above: 100_000,
        }
    }
}

/// Read every enabled Codex account's current quota from its CLI (`codex app-server`, no model
/// request), all at once, and store it. Returns the accounts that couldn't answer, with why;
/// those keep what was already known.
pub async fn refresh_codex_quota(
    config: &Config,
    store: &Store,
    secrets: &dyn SecretStore,
    home: &Path,
    accounts: &[&Account],
    now: EpochSecs,
) -> Result<Vec<(String, String)>, RunError> {
    let mut probes = tokio::task::JoinSet::new();
    for a in accounts
        .iter()
        .filter(|a| a.enabled && a.vendor == Vendor::Codex && a.shell.is_none())
    {
        let Ok(launch) = config.launch(a, secrets, home) else {
            continue;
        };
        // Only the binary and the login's env: `[vendor.codex] args` are for `exec`.
        let mut cmd = std::process::Command::new(&launch.program);
        for key in &launch.env_remove {
            cmd.env_remove(key);
        }
        cmd.envs(launch.env.iter().map(|(k, v)| (k, v)));
        let name = a.name.clone();
        probes.spawn(async move {
            let cmd = tokio::process::Command::from(cmd);
            (name, codex::read_live_quota(cmd, LIVE_QUOTA_TIMEOUT).await)
        });
    }
    let mut failed = Vec::new();
    while let Some(done) = probes.join_next().await {
        let Ok((name, result)) = done else { continue };
        match result {
            Ok(readings) if !readings.is_empty() => {
                store.replace_quota(&name, &readings, Source::Probe, now)?;
            }
            Ok(_) => {}
            Err(e) => failed.push((name, e)),
        }
    }
    failed.sort();
    Ok(failed)
}

/// How long `codex app-server` gets to report an account's quota.
const LIVE_QUOTA_TIMEOUT: Duration = Duration::from_secs(15);

/// Prompt for a resumed session on the next account.
pub const CONTINUE_PROMPT: &str = "Continue from where you stopped.";

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

/// The CLIs the supervisor can drive; accounts of other vendors are skipped.
pub const DRIVEN: [Vendor; 3] = [Vendor::Claude, Vendor::Codex, Vendor::Antigravity];

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
}

/// How the next launch continues the task.
enum Continue {
    Start,
    /// The user's message, sent into an existing session.
    Reply(String),
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
        let mut report = RunReport {
            schema: SCHEMA,
            run_id: Some(run_id),
            outcome: Outcome::Failed,
            result: None,
            session_id: None,
            accounts_used: Vec::new(),
            switches: Vec::new(),
            wait_until: None,
        };
        let mut ordered = self.config.by_priority();
        ordered.retain(|a| DRIVEN.contains(&a.vendor));
        let pick_opts = PickOptions::default();
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
        // Set when the mod stopped the last launch at a clean point (soft switch).
        let mut soft_stop_from: Option<&Account> = None;
        // An account that keeps running with the mod's stop off: nothing better to switch to.
        let mut stop_off_for: Option<String> = None;
        let mut written: Option<Written> = None;
        let mut handoffs = 0;

        loop {
            self.refresh_codex(&ordered, opts, run_id).await?;
            let quota = self.store.quota(None)?;

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

            let account = match soft_choice {
                Some(account) => account,
                None => match policy::pick(&ordered, &quota, self.now(), &pick_opts) {
                    Pick::Use { account, .. } => {
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
                            self.decide(run_id, None, Some(account), "start", json!({}))?;
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
                Continue::Resume(sid) => (CONTINUE_PROMPT.to_owned(), Some(sid.as_str())),
                Continue::Fresh => (
                    handoff_prompt(written.as_ref().map(|w| w.task_md.as_str()), &opts.prompt),
                    None,
                ),
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
                opts.stop_at
            };
            let attempt = self
                .attempt(run_id, account, &prompt, resume, stop_at, opts)
                .await?;
            if let Some(sid) = &attempt.session_id {
                report.session_id = Some(sid.clone());
                self.store
                    .set_run_account(run_id, &account.name, Some(sid))?;
            }

            if attempt.limit_hit {
                self.note(opts, run_id, &format!("{} hit its limit", account.name));
                handoffs += 1;
                written = self.write_handoff(
                    run_id,
                    opts,
                    account,
                    "limit reached",
                    report.session_id.as_deref(),
                    handoffs,
                );
                next = match report.session_id.clone() {
                    Some(sid) => Continue::Resume(sid),
                    None => Continue::Fresh,
                };
                continue;
            }
            if let Some(reason) = self.store.take_stop(run_id)? {
                self.note(
                    opts,
                    run_id,
                    &format!("{} paused at a clean point ({reason})", account.name),
                );
                handoffs += 1;
                written = self.write_handoff(
                    run_id,
                    opts,
                    account,
                    &format!("paused, {reason}"),
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

        let state = match report.outcome {
            Outcome::Done => RunState::Done,
            _ => RunState::Failed,
        };
        self.store.set_run_state(run_id, state, self.now())?;
        self.store.set_run_pid(run_id, None)?;
        Ok(report)
    }

    /// Codex accounts' quota, read live before every pick so a launch goes to a login that still
    /// has room (D23).
    async fn refresh_codex(
        &self,
        accounts: &[&Account],
        opts: &RunOptions,
        run_id: i64,
    ) -> Result<(), RunError> {
        let failed = refresh_codex_quota(
            self.config,
            self.store,
            self.secrets,
            &self.home,
            accounts,
            self.now(),
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
        if from.vendor != to.vendor && matches!(next, Continue::Resume(_) | Continue::Reply(_)) {
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
            Continue::Resume(sid) | Continue::Reply(sid) => self.sync_session(from, to, sid),
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

    /// Launch the CLI once and follow its stream until it exits.
    async fn attempt(
        &self,
        run_id: i64,
        account: &Account,
        prompt: &str,
        resume: Option<&str>,
        stop_at: f64,
        opts: &RunOptions,
    ) -> Result<Attempt, RunError> {
        let launch = self.config.launch(account, self.secrets, &self.home)?;
        let mut cmd = launch.command();
        // The provider's model and effort, shared by all its accounts (D20); `--model` on
        // `anchorleg run` overrides the model for this run.
        let shared = self.config.vendors.get(&account.vendor);
        let model_args = account.vendor.model_args(
            opts.model
                .as_deref()
                .or(shared.and_then(|s| s.model.as_deref())),
            shared.and_then(|s| s.effort.as_deref()),
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
            cmd.arg("-p").arg(prompt);
            cmd.args(["--output-format", "stream-json"]);
            if !agy::sets_mode(&launch.args) {
                cmd.args(agy::ACCEPT_EDITS);
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
            cmd.arg("exec");
            if resume.is_some() {
                cmd.arg("resume");
            }
            cmd.args(["--json", "--skip-git-repo-check"]);
            // `exec` defaults to a read-only sandbox, so the agent couldn't edit anything.
            if !codex::sets_sandbox(&launch.args) {
                cmd.args(["-c", codex::WORKSPACE_WRITE]);
            }
            cmd.args(&model_args).arg("--");
            if let Some(sid) = resume {
                cmd.arg(sid);
            }
            cmd.arg(prompt);
        } else {
            if let Some(dir) = &opts.mod_dir {
                cmd.arg("--plugin-dir").arg(dir);
                cmd.env("ANCHORLEG_ACCOUNT", &account.name)
                    .env("ANCHORLEG_RUN_ID", run_id.to_string())
                    .env("ANCHORLEG_STOP_AT", stop_at.to_string());
                if let Some(bin) = &opts.relay_bin {
                    cmd.env("ANCHORLEG_BIN", bin);
                }
            }
            cmd.arg("-p").arg(prompt);
            cmd.args(["--output-format", "stream-json", "--verbose"]);
            cmd.args(&model_args);
            if let Some(sid) = resume {
                cmd.args(["--resume", sid]);
            }
        }
        // Without this, `claude -p` waits 3 s for stdin on every launch (Phase 0).
        cmd.current_dir(&opts.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = tokio::process::Command::from(cmd)
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| RunError::Spawn {
                program: launch.program.clone(),
                source,
            })?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let mut lines = BufReader::new(stdout).lines();
        let mut raw_log = self.open_log(opts.log_dir.as_deref(), run_id);
        let mut attempt = Attempt::default();
        let mut deadline: Option<tokio::time::Instant> = None;
        let mut codex_stream = codex::Stream::default();

        loop {
            let line = match deadline {
                Some(d) => match tokio::time::timeout_at(d, lines.next_line()).await {
                    Ok(line) => line?,
                    Err(_) => {
                        tracing::warn!(account = %account.name, "CLI still running after its limit hit; killing it");
                        child.start_kill()?;
                        break;
                    }
                },
                None => lines.next_line().await?,
            };
            let Some(line) = line else { break };
            if let Some(f) = raw_log.as_mut() {
                let _ = writeln!(f, "{line}");
            }
            if line.trim().is_empty() {
                continue;
            }
            let signals = match account.vendor {
                Vendor::Codex => codex_stream.feed(&line),
                Vendor::Antigravity => agy::signals(&line),
                _ => claude::signals(&claude::parse_line(&line)),
            };
            for signal in signals {
                self.on_signal(run_id, account, signal, &mut attempt)?;
            }
            if attempt.limit_hit && deadline.is_none() {
                deadline = Some(tokio::time::Instant::now() + opts.limit_grace);
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
    ) -> Result<(), RunError> {
        match signal {
            AgentSignal::Started { session_id, .. } => {
                // Saved right away so a live session can be watched and replied to.
                self.store
                    .set_run_account(run_id, &account.name, Some(&session_id))?;
                attempt.session_id = Some(session_id);
            }
            AgentSignal::Quota(reading) => {
                self.store
                    .record_quota(&account.name, &reading, Source::Stream, self.now())?;
            }
            AgentSignal::LimitHit { window, resets_at } => {
                attempt.limit_hit = true;
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
            AgentSignal::Finished { ok, text, .. } => attempt.finished = Some((ok, text)),
            AgentSignal::Retrying { .. } => {}
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
