//! `anchorleg run`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anchorleg_core::attachments;
use anchorleg_core::notify::{self, Kind, Notice, NotifySettings};
use anchorleg_core::output::SCHEMA;
use anchorleg_core::output::{Outcome, RunReport};
use anchorleg_core::registry::{Config, Keychain};
use anchorleg_core::store::{RunState, Store};
use anchorleg_core::supervisor::{FollowUp, Progress, Quiet, RunOptions, Supervisor};

use crate::{RunArgs, now};

struct Stderr;

impl Progress for Stderr {
    fn event(&self, message: &str) {
        eprintln!("anchorleg: {message}");
    }
}

/// Passes progress on, and notifies the person of the moments that matter while they're away:
/// a switch to another account, every account out.
struct Notifying<'a> {
    inner: &'a dyn Progress,
    settings: &'a NotifySettings,
    title: String,
}

impl Progress for Notifying<'_> {
    fn event(&self, message: &str) {
        self.inner.event(message);
        let kind = if message.starts_with("switching ") {
            Kind::Switched
        } else if message.starts_with("every account is") {
            Kind::Blocked
        } else {
            return;
        };
        notify::send(
            self.settings,
            &Notice::new(kind, None, &self.title, message),
        );
    }
}

/// Run the task; returns the report so `main` can pick the exit code.
pub fn run(args: RunArgs) -> anyhow::Result<RunReport> {
    let config = Config::load(&Config::default_path()?)?;
    let db = Store::default_path()?;
    let store = Store::open(&db)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let parent = match args.follow_up {
        Some(id) => Some(
            store
                .run(id)?
                .ok_or_else(|| anyhow::anyhow!("no run {id}"))?,
        ),
        None => None,
    };
    let cwd = match (args.cwd, &parent) {
        (Some(dir), _) => dir,
        (None, Some(p)) => PathBuf::from(&p.cwd),
        (None, None) => std::env::current_dir()?,
    };

    let staged = attachments::stage(&cwd, &args.attach, now())?;
    let mut opts = RunOptions::new(
        attachments::with_note(&args.prompt.join(" "), &staged),
        &cwd,
    );
    opts.images = staged
        .iter()
        .filter(|p| attachments::is_image(p))
        .map(|p| cwd.join(p))
        .collect();
    opts.plan = args.plan;
    if let Some(p) = &parent {
        let (Some(session_id), Some(account)) = (p.session_id.clone(), p.account.clone()) else {
            anyhow::bail!(
                "run {} never started a session; start a new one instead",
                p.id
            );
        };
        opts.follow_up = Some(FollowUp {
            parent: p.id,
            session_id,
            account,
        });
    }
    opts.model = args.model;
    opts.no_wait = args.no_wait;
    opts.log_dir = db.parent().map(|d| d.join("runs"));
    opts.stop_at = args.stop_at;
    opts.fresh_above = args.fresh_above;
    opts.policy = args.policy.unwrap_or(config.policy);
    for name in [&args.prefer, &args.account].into_iter().flatten() {
        if config.get(name).is_none() {
            anyhow::bail!("no account `{name}`");
        }
    }
    opts.prefer = args.prefer;
    opts.only = args.account;
    opts.relay_bin = Some(std::env::current_exe()?);
    opts.data_dir = db.parent().map(Path::to_owned);
    if !args.no_mod
        && let Some(dir) = db.parent()
    {
        opts.mod_dir = Some(anchorleg_core::anchorleg_mod::install(dir)?);
    }

    let inner: &dyn Progress = if args.json { &Quiet } else { &Stderr };
    let title = opts.prompt.clone();
    let progress = &Notifying {
        inner,
        settings: &config.notify,
        title: title.clone(),
    };
    let supervisor = Supervisor {
        config: &config,
        store: &store,
        secrets: &Keychain,
        home,
        clock: Arc::new(now),
        progress,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let report = runtime.block_on(async {
        let mut signals = Signals::new();
        let report = tokio::select! {
            // `anchorleg stop` signals the whole group: the CLI may exit in the same instant, so
            // the signal is checked first.
            biased;
            // Dropping the run kills the CLI under it (kill_on_drop).
            () = signals.recv() => return stopped(&store, opts.log_dir.as_deref(), None),
            report = supervisor.run(&opts) => report.map_err(anyhow::Error::from)?,
        };
        // The CLI died of the same signal before anchorleg's own arrived.
        if report.outcome != Outcome::Done
            && tokio::time::timeout(Duration::from_millis(200), signals.recv())
                .await
                .is_ok()
        {
            return stopped(&store, opts.log_dir.as_deref(), report.run_id);
        }
        Ok(report)
    })?;

    let ended = match report.outcome {
        Outcome::Done => Some((Kind::Done, report.result.clone().unwrap_or_default())),
        Outcome::Failed => Some((
            Kind::Failed,
            report
                .result
                .clone()
                .unwrap_or_else(|| "the task failed".to_owned()),
        )),
        Outcome::AllBlocked => Some((Kind::Blocked, "every account is blocked".to_owned())),
        // The person stopped it, or there's nothing to run on: they know.
        Outcome::Stopped | Outcome::NoAccounts => None,
    };
    if let Some((kind, text)) = ended {
        let what = match kind {
            Kind::Done => "done",
            Kind::Failed => "failed",
            _ => "blocked",
        };
        let id = report.run_id.map_or(String::new(), |r| format!("#{r} "));
        notify::send(
            &config.notify,
            &Notice::new(kind, report.run_id, &title, &format!("{id}{what}: {text}")),
        );
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        if let Some(text) = &report.result {
            println!("{text}");
        }
        match report.outcome {
            Outcome::Done => {}
            Outcome::Failed => eprintln!("anchorleg: task failed"),
            Outcome::AllBlocked => eprintln!(
                "anchorleg: every account is blocked until {} (unix time)",
                report.wait_until.unwrap_or_default()
            ),
            Outcome::NoAccounts => {
                eprintln!(
                    "anchorleg: no Claude account configured; see `anchorleg accounts --help`"
                )
            }
            Outcome::Stopped => eprintln!("anchorleg: stopped"),
        }
    }
    Ok(report)
}

/// Ctrl-C or SIGTERM (what `anchorleg stop` sends), listened for from the start of the run.
struct Signals {
    term: tokio::signal::unix::Signal,
    int: tokio::signal::unix::Signal,
}

impl Signals {
    fn new() -> Self {
        use tokio::signal::unix::{SignalKind, signal};
        Self {
            term: signal(SignalKind::terminate()).expect("installing SIGTERM handler"),
            int: signal(SignalKind::interrupt()).expect("installing SIGINT handler"),
        }
    }

    async fn recv(&mut self) {
        tokio::select! {
            _ = self.term.recv() => {}
            _ = self.int.recv() => {}
        }
    }
}

/// Mark this process's run as stopped and report it. `run_id` is known once the supervisor has
/// finished (it clears the pid); before that the run is the one carrying this process's pid.
fn stopped(
    store: &Store,
    log_dir: Option<&std::path::Path>,
    run_id: Option<i64>,
) -> anyhow::Result<RunReport> {
    let me = i64::from(std::process::id());
    let run = match run_id {
        Some(id) => store.run(id)?,
        None => store
            .recent_runs(20)?
            .into_iter()
            .find(|r| r.pid == Some(me)),
    };
    if let Some(run) = &run {
        store.set_run_state(run.id, RunState::Stopped, now())?;
        store.set_run_pid(run.id, None)?;
        if let Some(dir) = log_dir {
            use std::io::Write as _;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .append(true)
                .open(dir.join(format!("run-{}.jsonl", run.id)))
            {
                let _ = writeln!(
                    f,
                    "{}",
                    serde_json::json!({ "type": "anchorleg", "text": "stopped" })
                );
            }
        }
    }
    Ok(RunReport {
        schema: SCHEMA,
        run_id: run.as_ref().map(|r| r.id),
        outcome: Outcome::Stopped,
        result: None,
        session_id: run.as_ref().and_then(|r| r.session_id.clone()),
        accounts_used: run
            .as_ref()
            .and_then(|r| r.account.clone())
            .into_iter()
            .collect(),
        switches: Vec::new(),
        wait_until: None,
    })
}

/// `anchorleg stop <run>`: signal the run's process group (started by `anchorleg ui`) or process.
pub fn stop(id: i64) -> anyhow::Result<()> {
    let store = Store::open(&Store::default_path()?)?;
    let run = store
        .run(id)?
        .ok_or_else(|| anyhow::anyhow!("no run {id}"))?;
    let Some(pid) = run.pid else {
        anyhow::bail!("run {id} isn't running");
    };
    let group = std::process::Command::new("kill")
        .args(["-TERM", "--", &format!("-{pid}")])
        .stderr(std::process::Stdio::null())
        .status()?;
    if !group.success() {
        let one = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()?;
        if !one.success() {
            // The process is gone; don't leave the run looking alive.
            store.set_run_state(id, RunState::Stopped, now())?;
            store.set_run_pid(id, None)?;
        }
    }
    println!("stopping run {id}");
    Ok(())
}

/// `anchorleg send`: a message for a session that's still working. Codex takes it into the
/// running turn; Claude gets it as soon as its turn ends (or, with `interrupt`, at its next
/// clean point), in the same run.
pub fn send(id: i64, text: &str, interrupt: bool, attach: &[PathBuf]) -> anyhow::Result<()> {
    let store = Store::open(&Store::default_path()?)?;
    let run = store
        .run(id)?
        .ok_or_else(|| anyhow::anyhow!("no run {id}"))?;
    if run.pid.is_none() || !matches!(run.state, RunState::Running | RunState::Waiting) {
        anyhow::bail!("run {id} isn't working; reply with `anchorleg run --follow-up {id} -- …`");
    }
    if text.trim().is_empty() && attach.is_empty() {
        anyhow::bail!("nothing to send");
    }
    let staged = attachments::stage(Path::new(&run.cwd), attach, now())?;
    store.queue_message(
        id,
        attachments::with_note(text.trim(), &staged).trim(),
        interrupt,
        now(),
    )?;
    println!(
        "{} for run {id}",
        if interrupt {
            "interrupting; sent"
        } else {
            "sent"
        }
    );
    Ok(())
}

/// For anchorleg-mod, before each model request: exit 0 when the person asked this run to stop
/// for a new message, 1 otherwise.
pub fn interrupt_check(id: i64) -> anyhow::Result<bool> {
    let store = Store::open(&Store::default_path()?)?;
    Ok(store.interrupt_requested(id)?)
}

/// `anchorleg diff <run>`: the session's changes since it started.
pub fn diff(id: i64, stat: bool) -> anyhow::Result<()> {
    let store = Store::open(&Store::default_path()?)?;
    let run = store
        .run(id)?
        .ok_or_else(|| anyhow::anyhow!("no run {id}"))?;
    let chain = store.session_runs(id)?;
    match anchorleg_core::checkpoint::session_diff(Path::new(&run.cwd), &chain, stat)? {
        Some(d) if d.is_empty() => println!("no changes since #{id}'s session started"),
        Some(d) => println!("{d}"),
        None => anyhow::bail!(
            "nothing to compare: {} isn't a git repository, or the session started before \
             anchorleg recorded starts",
            run.cwd
        ),
    }
    Ok(())
}
