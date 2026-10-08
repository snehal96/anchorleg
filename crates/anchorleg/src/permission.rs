//! `anchorleg permission …`: tool calls a headless CLI needs the person to approve.
//!
//! anchorleg-mod sees the CLI about to ask (Claude's `tool.check` answering "ask", which `-p` would
//! turn into a refusal) and calls `anchorleg permission ask`, which waits until someone answers in
//! `anchorleg ui` or with `anchorleg permission answer`. Nothing is approved without the person.

use std::io::Read as _;
use std::time::{Duration, Instant};

use anchorleg_core::conversation::{describe_tool, relative_input};
use anchorleg_core::store::{PermissionState, RunState, Store};
use anyhow::{Context, bail};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::now;

/// After this long without an answer the call is refused, as the CLI would have.
pub const GIVE_UP_SECS: i64 = 30 * 60;

#[derive(Subcommand)]
pub enum PermissionCmd {
    /// Wait for the person to answer a tool call (called by anchorleg-mod). JSON on stdin.
    Ask {
        #[arg(long, required = true)]
        json: bool,
        /// Answer "pending" after this many seconds; the caller asks again with the id.
        #[arg(long, default_value_t = 540)]
        wait: u64,
    },
    /// List tool calls waiting for an answer.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Answer one: yes, always (this tool, for the rest of the session) or no.
    Answer {
        id: i64,
        #[arg(value_parser = ["yes", "always", "no"])]
        answer: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskInput {
    run_id: i64,
    tool: String,
    #[serde(default)]
    input: Value,
    reason: Option<String>,
    /// Keep waiting on a request made earlier.
    id: Option<i64>,
}

#[derive(Serialize)]
struct AskOutput {
    decision: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<i64>,
    reason: String,
}

pub fn run(cmd: PermissionCmd) -> anyhow::Result<()> {
    let db = Store::default_path()?;
    let store = Store::open(&db)?;
    match cmd {
        PermissionCmd::Ask { json: _, wait } => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            let input: AskInput = serde_json::from_str(&text).context("reading the request")?;
            let out = ask(&store, &input, Duration::from_secs(wait))?;
            println!("{}", serde_json::to_string(&out)?);
        }
        PermissionCmd::List { json } => {
            let pending = store.pending_permissions()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&pending)?);
                return Ok(());
            }
            if pending.is_empty() {
                println!("nothing is waiting for you");
            }
            for p in &pending {
                println!(
                    "{:>4}  run #{}  {}",
                    p.id,
                    p.run_id,
                    describe(&p.tool, &p.input)
                );
            }
        }
        PermissionCmd::Answer { id, answer } => {
            let state = match answer.as_str() {
                "yes" => PermissionState::Allowed,
                "always" => PermissionState::Always,
                _ => PermissionState::Denied,
            };
            if !store.answer_permission(id, state, now())? {
                bail!("request {id} isn't waiting for an answer");
            }
            println!("answered {id}: {answer}");
        }
    }
    Ok(())
}

/// One line for a tool call, paths shown relative to the current folder.
fn describe(tool: &str, input: &Value) -> String {
    match std::env::current_dir() {
        Ok(cwd) => describe_tool(tool, &relative_input(input, &cwd.display().to_string())),
        Err(_) => describe_tool(tool, input),
    }
}

fn ask(store: &Store, req: &AskInput, wait: Duration) -> anyhow::Result<AskOutput> {
    let runs = store.session_runs(req.run_id)?;
    if runs.is_empty() {
        bail!("no run {}", req.run_id);
    }
    let what = describe(&req.tool, &req.input);
    let id = match req.id {
        Some(id) => id,
        None => {
            if store.always_allowed(&runs, &req.tool)? {
                return Ok(AskOutput {
                    decision: "allow",
                    id: None,
                    reason: format!("{} allowed for this session", req.tool),
                });
            }
            let id = store.ask_permission(
                req.run_id,
                &req.tool,
                &req.input,
                req.reason.as_deref(),
                now(),
            )?;
            note(req.run_id, &format!("waiting for you to allow {what}"));
            id
        }
    };
    let started = Instant::now();
    loop {
        let p = store
            .permission(id)?
            .with_context(|| format!("no request {id}"))?;
        let (decision, reason, said) = match p.state {
            PermissionState::Allowed => ("allow", "the user allowed it".to_owned(), "allowed"),
            PermissionState::Always => (
                "allow",
                format!("the user allowed {} for this session", req.tool),
                "allowed for this session",
            ),
            PermissionState::Denied => ("deny", "the user said no".to_owned(), "refused"),
            PermissionState::Expired => ("deny", "nobody answered".to_owned(), "expired"),
            PermissionState::Pending => {
                let run = store.run(req.run_id)?;
                let ended = run.is_none_or(|r| {
                    !matches!(r.state, RunState::Running | RunState::Waiting) || r.pid.is_none()
                });
                if ended || now() - p.asked_at > GIVE_UP_SECS {
                    store.answer_permission(id, PermissionState::Expired, now())?;
                    continue;
                }
                if started.elapsed() >= wait {
                    return Ok(AskOutput {
                        decision: "pending",
                        id: Some(id),
                        reason: "still waiting".to_owned(),
                    });
                }
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
        };
        note(req.run_id, &format!("{what}: {said}"));
        return Ok(AskOutput {
            decision,
            id: Some(id),
            reason,
        });
    }
}

/// Add an anchorleg line to the run's log, where `anchorleg ui` shows it.
fn note(run_id: i64, text: &str) {
    use std::io::Write as _;
    let Some(dir) = Store::default_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("runs")))
    else {
        return;
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join(format!("run-{run_id}.jsonl")))
    {
        let _ = writeln!(f, "{}", json!({ "type": "anchorleg", "text": text }));
    }
}
