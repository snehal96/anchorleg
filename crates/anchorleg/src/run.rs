//! `anchorleg run`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anchorleg_core::attachments;
use anchorleg_core::notify::{self, Kind, Notice, NotifySettings};
use anchorleg_core::output::SCHEMA;
use anchorleg_core::output::{Outcome, RunReport};
use anchorleg_core::registry::{Config, Keychain, Vendor};
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

/// How much of a session's diff goes into the review prompt; beyond it the reviewer reads the
/// rest with git.
const REVIEW_DIFF_CAP: usize = 60 * 1024;

/// What a reviewer is asked: the session's task and its diff since the session started.
fn review_prompt(id: i64, first: i64, diff: &str, extra: &str) -> String {
    let start = anchorleg_core::checkpoint::start_ref(first);
    let shown = if diff.len() > REVIEW_DIFF_CAP {
        let mut cut = REVIEW_DIFF_CAP;
        while !diff.is_char_boundary(cut) {
            cut -= 1;
        }
        format!(
            "{}\n… (cut; run `git diff {start}` for the rest)",
            &diff[..cut]
        )
    } else {
        diff.to_owned()
    };
    let extra = if extra.trim().is_empty() {
        String::new()
    } else {
        format!("\nAlso: {}\n", extra.trim())
    };
    format!(
        "Review the changes anchorleg session #{id} made in this folder (its diff since it \
         started is below; `git diff {start}` shows it too). Don't change any files, and \
         you don't need commands beyond reading files and git diff/log/show/status (run each git \
         command on its own, not chained, so nobody has to approve it).\n\
         Look for bugs, regressions, missing tests and risky changes. List each finding as \
         `- [P0|P1|P2|P3] <title> — <path>:<line>` with a short explanation and the fix; P0 is \
         the worst. Skip style nits. End with one line: `VERDICT: approve` or \
         `VERDICT: changes requested`.\n{extra}\n```diff\n{shown}\n```"
    )
}

/// Opens a side question (`anchorleg btw`), so the fork answers instead of carrying on.
const BTW_NOTE: &str = "A side question about this session, asked while you work on the task. \
     Answer it briefly from what you know and can read; don't change any files and don't go on \
     with the task here.";

/// For `--rewind RUN`: the run before it in its session and the point there to fork from.
fn rewind_point(store: &Store, db: &Path, id: i64) -> anyhow::Result<(i64, String)> {
    let run = store
        .run(id)?
        .ok_or_else(|| anyhow::anyhow!("no run {id}"))?;
    let Some(parent) = run.parent_id.and_then(|p| store.run(p).ok().flatten()) else {
        anyhow::bail!(
            "#{id} is its session's first run: nothing to go back to (`anchorleg undo {id}` puts \
             the files back)"
        );
    };
    let Some(session) = parent.session_id.as_deref() else {
        anyhow::bail!("#{} never started a session", parent.id);
    };
    let log = db
        .with_file_name("runs")
        .join(format!("run-{}.jsonl", parent.id));
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let at = anchorleg_core::conversation::fork_point(&text, session).ok_or_else(|| {
        anyhow::anyhow!(
            "#{}'s log doesn't say where its session stood; only Claude and Codex sessions rewind",
            parent.id
        )
    })?;
    Ok((parent.id, at))
}

/// Run the task; returns the report so `main` can pick the exit code.
/// How long `--detach` waits for the background run to record itself.
const DETACH_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// `anchorleg run --detach …`: start this same command without `--detach` in its own process
/// group (so it outlives this one, and `anchorleg stop` ends it whole), wait until it has a run
/// id, print it and return. What the run prints goes nowhere; until it has a run id its errors
/// are read back from a file and reported here.
pub fn detach(json: bool) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt as _;
    // Drop `--detach` from the options only: after `--` it's part of the task.
    let mut options = true;
    let mut args: Vec<std::ffi::OsString> = std::env::args_os()
        .skip(1)
        .filter(|a| {
            if a == "--" {
                options = false;
            }
            !(options && a == "--detach")
        })
        .collect();
    // The run reports in JSON (its run id, if it ends before it's seen working), into a file.
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    if !args[..end].iter().any(|a| a == "--json")
        && let Some(i) = args[..end].iter().position(|a| a == "run")
    {
        args.insert(i + 1, "--json".into());
    }
    let db = Store::default_path()?;
    let store = Store::open(&db)?;
    let dir = db.parent().map(|d| d.join("runs")).unwrap_or_default();
    std::fs::create_dir_all(&dir)?;
    let base = dir.join(format!(".detach-{}-{}", std::process::id(), now()));
    let (out_path, err_path) = (base.with_extension("out"), base.with_extension("err"));
    let out = std::fs::File::create(&out_path)?;
    let err = std::fs::File::create(&err_path)?;
    let exe = std::env::current_exe()?;
    let mut child = std::process::Command::new(&exe)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err)
        .process_group(0)
        .spawn()
        .map_err(|e| anyhow::anyhow!("starting {}: {e}", exe.display()))?;
    let pid = i64::from(child.id());
    let started = std::time::Instant::now();
    let result = loop {
        if let Some(run) = store
            .recent_runs(20)?
            .into_iter()
            .find(|r| r.pid == Some(pid))
        {
            break Ok(run.id);
        }
        if let Some(status) = child.try_wait()? {
            // Ended before it was seen working: done already (its report has the run id), or
            // it never started a run, and its error says why.
            let report = std::fs::read_to_string(&out_path).unwrap_or_default();
            if let Some(id) = serde_json::from_str::<serde_json::Value>(&report)
                .ok()
                .and_then(|r| r["run_id"].as_i64())
            {
                break Ok(id);
            }
            let msg = std::fs::read_to_string(&err_path).unwrap_or_default();
            let msg = msg.trim().trim_start_matches("anchorleg: ");
            break Err(if msg.is_empty() {
                anyhow::anyhow!("the run ended at once ({status})")
            } else {
                anyhow::anyhow!("{msg}")
            });
        }
        if started.elapsed() > DETACH_WAIT {
            break Err(anyhow::anyhow!(
                "the run (pid {pid}) hasn't started a session after {}s; it goes on in the background",
                DETACH_WAIT.as_secs()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    // The run keeps writing to the files it has open; the names can go.
    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_file(&err_path);
    let run_id = result?;
    if json {
        println!("{}", serde_json::json!({ "run_id": run_id, "pid": pid }));
    } else {
        println!(
            "started run #{run_id} (pid {pid}); follow it with `anchorleg sessions show {run_id}`"
        );
    }
    Ok(())
}

pub fn run(args: RunArgs) -> anyhow::Result<RunReport> {
    let config = Config::load(&Config::default_path()?)?;
    let db = Store::default_path()?;
    let store = Store::open(&db)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    // `--rewind R`: R's session as it was when R started, i.e. where R's parent left it.
    let rewound = match args.rewind {
        Some(id) => Some(rewind_point(&store, &db, id)?),
        None => None,
    };
    let parent_id = args
        .follow_up
        .or(args.fork)
        .or(args.btw)
        .or(rewound.as_ref().map(|(p, _)| *p));
    let parent = match parent_id {
        Some(id) => Some(
            store
                .run(id)?
                .ok_or_else(|| anyhow::anyhow!("no run {id}"))?,
        ),
        None => None,
    };
    let cwd_given = args.cwd.is_some();
    let mut cwd = match (args.cwd, &parent) {
        (Some(dir), _) => dir,
        (None, Some(p)) => PathBuf::from(&p.cwd),
        (None, None) => std::env::current_dir()?,
    };
    if let Some(name) = &args.worktree {
        let name = if name.trim().is_empty() {
            format!("task-{}", now())
        } else {
            name.trim().to_owned()
        };
        cwd = anchorleg_core::checkpoint::add_worktree(&cwd, &name)?;
        if !args.json {
            eprintln!(
                "anchorleg: working in {} (branch anchorleg/{name})",
                cwd.display()
            );
        }
    }

    if let Some(id) = args.rewind {
        for r in store.session_runs(id)? {
            if let Some(run) = store.run(r)?
                && matches!(run.state, RunState::Running | RunState::Waiting)
                && run.pid.is_some_and(pid_alive)
            {
                anyhow::bail!(
                    "#{} is still working; stop it first (anchorleg stop {})",
                    run.id,
                    run.id
                );
            }
        }
        if !args.keep_files {
            let line = match anchorleg_core::checkpoint::rewind_files(&cwd, id)? {
                Some(r) => format!(
                    "files back to how they were when #{id} started: {} put back, {} removed \
                     (backup: {})",
                    r.restored.len(),
                    r.removed.len(),
                    r.backup
                ),
                None => format!(
                    "the files stay as they are: {} isn't a git repository, or #{id} started \
                     before anchorleg recorded starts",
                    cwd.display()
                ),
            };
            if !args.json {
                eprintln!("anchorleg: {line}");
            }
        }
    }
    let staged = attachments::stage(&cwd, &args.attach, now())?;
    // An unmet goal carries on into the session's follow-ups (D48).
    let goal = args.goal.clone().or_else(|| {
        parent
            .as_ref()
            .filter(|p| {
                p.goal_status
                    .as_deref()
                    .and_then(anchorleg_core::goal::Status::parse)
                    .is_some_and(anchorleg_core::goal::Status::open)
            })
            .and_then(|p| p.goal.clone())
    });
    // `--review RUN`: the session's changes, read-only, on another account (D49).
    let reviewed = match args.review {
        Some(id) => {
            let r = store
                .run(id)?
                .ok_or_else(|| anyhow::anyhow!("no run {id}"))?;
            if matches!(r.state, RunState::Running | RunState::Waiting)
                && r.pid.is_some_and(pid_alive)
            {
                anyhow::bail!("#{id} is still working; review it once it's done");
            }
            if !cwd_given {
                cwd = PathBuf::from(&r.cwd);
            }
            let chain = store.session_runs(id)?;
            let diff = match anchorleg_core::checkpoint::session_diff(&cwd, &chain, false)? {
                Some(d) if d.trim().is_empty() => anyhow::bail!("#{id}'s session changed nothing"),
                Some(d) => d,
                None => anyhow::bail!(
                    "nothing to review: {} isn't a git repository, or #{id}'s session started \
                     before anchorleg recorded starts",
                    r.cwd
                ),
            };
            let first = chain.iter().min().copied().unwrap_or(id);
            Some((r, review_prompt(id, first, &diff, &args.prompt.join(" "))))
        }
        None => None,
    };
    let task = match (args.prompt.join(" "), &goal, &reviewed) {
        (_, _, Some((_, prompt))) => prompt.clone(),
        (q, _, _) if args.btw.is_some() => format!("{q}\n\n({BTW_NOTE})"),
        (t, Some(g), _) if t.trim().is_empty() => g.clone(),
        (t, _, _) => t,
    };
    let mut opts = RunOptions::new(attachments::with_note(&task, &staged), &cwd);
    opts.goal = goal;
    opts.vendor = args.cli.as_deref().map(|c| match c {
        "codex" => Vendor::Codex,
        "antigravity" => Vendor::Antigravity,
        _ => Vendor::Claude,
    });
    if let Some((r, _)) = &reviewed {
        opts.review = true;
        opts.review_of = Some(r.id);
        opts.avoid = r.account.clone();
    }
    if let Some(n) = args.goal_turns {
        opts.goal_turns = n.max(1);
    }
    opts.budget = args.budget;
    opts.json_schema = args.json_schema;
    opts.images = staged
        .iter()
        .filter(|p| attachments::is_image(p))
        .map(|p| cwd.join(p))
        .collect();
    opts.plan = args.plan;
    opts.answer_wait = args
        .answer_wait
        .or_else(|| parent.as_ref().and_then(|p| p.answer_wait));
    opts.mode = args
        .mode
        .clone()
        .or_else(|| parent.as_ref().and_then(|p| p.mode.clone()))
        .as_deref()
        .and_then(anchorleg_core::supervisor::Mode::parse);
    if let Some(p) = &parent {
        let (Some(session_id), Some(account)) = (p.session_id.clone(), p.account.clone()) else {
            anyhow::bail!(
                "run {} never started a session; start a new one instead",
                p.id
            );
        };
        let fork = args.fork.is_some() || args.btw.is_some() || rewound.is_some();
        if fork
            && config
                .get(&account)
                .is_some_and(|a| a.vendor == anchorleg_core::registry::Vendor::Antigravity)
        {
            anyhow::bail!(
                "forking or a side question needs a Claude or Codex session; #{} ran on {account}",
                p.id
            );
        }
        opts.follow_up = Some(FollowUp {
            parent: p.id,
            session_id,
            account,
            fork,
            at: rewound.as_ref().map(|(_, at)| at.clone()),
        });
        opts.side = args.btw.is_some();
    }
    // A follow-up keeps its session's name; a fork is a new session.
    opts.name = match (args.name.filter(|n| !n.trim().is_empty()), &parent) {
        (Some(n), _) => Some(n.trim().to_owned()),
        (None, Some(p)) if args.follow_up.is_some() || args.rewind.is_some() => {
            store.session_name(p.id)?
        }
        (None, Some(p)) if args.btw.is_some() => {
            store.session_name(p.id)?.map(|n| format!("btw: {n}"))
        }
        (None, _) => None,
    };
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
    opts.config_path = Some(Config::default_path()?);
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
        Outcome::Failed | Outcome::OverBudget => Some((
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

    // `--fix`: the reviewed session gets the findings, in a follow-up of its own.
    if args.fix
        && report.outcome == Outcome::Done
        && let (Some((r, _)), Some(findings)) = (&reviewed, &report.result)
    {
        let text = format!(
            "A reviewer ({}) looked at the changes this session made. Its findings:\n\n{findings}\n\n\
             Fix the ones you agree with; for each one you skip, say why.",
            report
                .accounts_used
                .last()
                .map_or("another account", String::as_str)
        );
        let mut cmd = std::process::Command::new(std::env::current_exe()?);
        cmd.args([
            "run",
            "--json",
            "--follow-up",
            &r.id.to_string(),
            "--",
            &text,
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
        {
            use std::os::unix::process::CommandExt as _;
            cmd.process_group(0);
        }
        cmd.spawn()?;
        if !args.json {
            eprintln!("anchorleg: sent the findings to #{} to fix", r.id);
        }
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
            Outcome::OverBudget => eprintln!("anchorleg: the task's token budget is used up"),
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
        budget: None,
        structured_output: None,
        goal: None,
    })
}

/// `anchorleg rename <run> <name>`: name the session (an empty name clears it). The CLI learns
/// the new name with the session's next message.
pub fn rename(id: i64, name: &str) -> anyhow::Result<()> {
    let store = Store::open(&Store::default_path()?)?;
    store
        .run(id)?
        .ok_or_else(|| anyhow::anyhow!("no run {id}"))?;
    let name = name.trim();
    store.set_run_name(id, (!name.is_empty()).then_some(name))?;
    match store.session_name(id)? {
        Some(n) => println!("#{id}: {n}"),
        None => println!("#{id}: no name"),
    }
    Ok(())
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
/// running turn; Claude and agy get it as soon as their turn ends (or, with `interrupt`, at its next
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

/// Undo (or `redo`) the files a session changed since it started; the result as one line, also
/// logged in the session's conversation. The caller makes sure the session isn't working.
pub fn undo_session(store: &Store, log_dir: &Path, id: i64, redo: bool) -> anyhow::Result<String> {
    let run = store
        .run(id)?
        .ok_or_else(|| anyhow::anyhow!("no run {id}"))?;
    let chain = store.session_runs(id)?;
    let cwd = Path::new(&run.cwd);
    let done = if redo {
        anchorleg_core::checkpoint::redo_session(cwd, &chain)?
    } else {
        anchorleg_core::checkpoint::undo_session(cwd, &chain)?
    };
    let Some(r) = done else {
        anyhow::bail!(if redo {
            "nothing to redo for this session".to_owned()
        } else {
            format!(
                "nothing to compare with: {} isn't a git repository, or the session started \
                 before anchorleg recorded starts",
                run.cwd
            )
        });
    };
    let what = if redo { "redid" } else { "undid" };
    let line = if r.restored.is_empty() && r.removed.is_empty() {
        format!("{what} nothing: no changes")
    } else {
        format!(
            "{what} the session's changes: {} file(s) put back, {} removed",
            r.restored.len(),
            r.removed.len()
        )
    };
    // The newest run of the session shows it in its conversation.
    let newest = chain.iter().max().copied().unwrap_or(id);
    {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .append(true)
            .open(log_dir.join(format!("run-{newest}.jsonl")))
        {
            let _ = writeln!(
                f,
                "{}",
                serde_json::json!({ "type": "anchorleg", "text": line })
            );
        }
    }
    Ok(line)
}

/// `anchorleg undo <run>`.
pub fn undo(id: i64, redo: bool, yes: bool) -> anyhow::Result<()> {
    let db = Store::default_path()?;
    let store = Store::open(&db)?;
    let log_dir = db.with_file_name("runs");
    let chain = store.session_runs(id)?;
    for r in &chain {
        if let Some(run) = store.run(*r)?
            && matches!(run.state, RunState::Running | RunState::Waiting)
            && run.pid.is_some_and(pid_alive)
        {
            anyhow::bail!(
                "#{} is still working; stop it first (anchorleg stop {})",
                run.id,
                run.id
            );
        }
    }
    if !yes && !redo {
        let run = store
            .run(id)?
            .ok_or_else(|| anyhow::anyhow!("no run {id}"))?;
        match anchorleg_core::checkpoint::session_diff(Path::new(&run.cwd), &chain, true)? {
            Some(stat) if !stat.trim().is_empty() => {
                println!("{stat}");
                println!(
                    "\nThese files go back to how they were when the session started (a backup is \
                     kept; `--redo` brings them back). Run again with --yes."
                );
            }
            Some(_) => println!("no changes since the session started"),
            None => anyhow::bail!("nothing to compare with for #{id}"),
        }
        return Ok(());
    }
    println!("{}", undo_session(&store, &log_dir, id, redo)?);
    if !redo {
        println!(
            "`anchorleg undo {id} --redo` brings the changes back. The agent isn't told; say so in your next message."
        );
    }
    Ok(())
}

fn pid_alive(pid: i64) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
