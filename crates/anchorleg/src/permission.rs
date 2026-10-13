//! `anchorleg permission …`: tool calls a headless CLI needs the person to approve.
//!
//! anchorleg-mod sees the CLI about to ask (Claude's `tool.check` answering "ask", which `-p` would
//! turn into a refusal) and calls `anchorleg permission ask`, which waits until someone answers in
//! `anchorleg ui` or with `anchorleg permission answer`. Nothing is approved without the person.
//! For agy, anchorleg's `PreToolUse` hook calls `anchorleg permission agy-hook` the same way (D26).

use std::io::Read as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anchorleg_core::adapters::agy::{self, HookVerdict};
use anchorleg_core::conversation::{describe_tool, relative_input};
use anchorleg_core::notify::{self, Kind, Notice};
use anchorleg_core::questions;
use anchorleg_core::registry::Config;
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
    /// agy's `PreToolUse` hook (D26): reads agy's request on stdin, asks the person when the call
    /// needs it, and prints agy's `{decision, reason}`.
    #[command(hide = true)]
    AgyHook,
    /// A stdio MCP server with one tool, `ask_user`, that puts the agent's questions to the
    /// person (agy's `-p` drops its own question tool). Installed in an agy account's own `HOME`
    /// next to the hook.
    #[command(hide = true)]
    McpAsk,
    /// List tool calls waiting for an answer.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Answer one. A tool call: yes, always (this tool, for the rest of the session), rule
    /// (always, in every session: saves a rule like `Bash(cargo test *)`) or no.
    /// A question: one answer per question, in order: an option number (`2`, or `1,3` when
    /// several may be picked), an option label, or your own words; `-` for a secret one
    /// (anchorleg never passes secrets); `--skip` declines.
    /// A plan: `edit` opens it in `$VISUAL`/`$EDITOR`; the plan as you save it is approved.
    Answer {
        id: i64,
        #[arg(required_unless_present_any = ["skip", "plan_file"])]
        answer: Vec<String>,
        /// Decline a question without answering.
        #[arg(long)]
        skip: bool,
        /// Approve the plan as written in this file (your edit of it).
        #[arg(long, conflicts_with_all = ["skip", "answer"])]
        plan_file: Option<PathBuf>,
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
    /// A question's answers: question id → labels or typed text.
    #[serde(skip_serializing_if = "Option::is_none")]
    answers: Option<questions::Answers>,
}

pub fn run(cmd: PermissionCmd) -> anyhow::Result<()> {
    let db = Store::default_path()?;
    let store = match Store::open(&db) {
        Ok(store) => store,
        // agy's hook must always answer, and a refusal is the safe answer.
        Err(e) if matches!(cmd, PermissionCmd::AgyHook) => {
            println!(
                "{}",
                json!({ "decision": "deny", "reason": format!("anchorleg: {e}") })
            );
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    match cmd {
        PermissionCmd::Ask { json: _, wait } => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            let input: AskInput = serde_json::from_str(&text).context("reading the request")?;
            let out = ask(&store, &input, Duration::from_secs(wait))?;
            println!("{}", serde_json::to_string(&out)?);
        }
        PermissionCmd::AgyHook => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            let out = agy_hook(&store, &text);
            println!("{}", serde_json::to_string(&out)?);
        }
        PermissionCmd::McpAsk => {
            use std::io::{BufRead as _, Write as _};
            let run_id = std::env::var("ANCHORLEG_RUN_ID")
                .ok()
                .and_then(|v| v.parse::<i64>().ok());
            let mut out = std::io::stdout().lock();
            for line in std::io::stdin().lock().lines() {
                let Ok(msg) = serde_json::from_str::<Value>(&line?) else {
                    continue;
                };
                if let Some(reply) = mcp_reply(&store, run_id, &msg) {
                    writeln!(out, "{reply}")?;
                    out.flush()?;
                }
            }
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
                if p.is_question() {
                    println!("{:>4}  run #{}  question:", p.id, p.run_id);
                    for (i, q) in questions::from_input(&p.input).iter().enumerate() {
                        println!("        {}. {}", i + 1, q.question);
                        for (n, o) in q.options.iter().enumerate() {
                            let more = o
                                .description
                                .as_deref()
                                .map_or(String::new(), |d| format!("  ({d})"));
                            println!("           {}) {}{more}", n + 1, o.label);
                        }
                    }
                    continue;
                }
                println!(
                    "{:>4}  run #{}  {}",
                    p.id,
                    p.run_id,
                    describe(&p.tool, &p.input)
                );
                for (mark, line) in anchorleg_core::conversation::change_preview(&p.tool, &p.input)
                {
                    println!("        {mark} {line}");
                }
            }
        }
        PermissionCmd::Answer {
            id,
            mut answer,
            skip,
            plan_file,
        } => {
            let p = store
                .permission(id)?
                .filter(|p| p.state == PermissionState::Pending)
                .with_context(|| format!("request {id} isn't waiting for an answer"))?;
            if p.is_question() {
                if skip {
                    store.answer_permission(id, PermissionState::Denied, now())?;
                    println!("skipped {id}");
                    return Ok(());
                }
                let qs = questions::from_input(&p.input);
                let plan = questions::plan_of(&p.input)
                    .filter(|_| qs.len() == 1 && qs[0].id == questions::plan_question().id);
                if let Some(file) = &plan_file {
                    plan.context("only a plan can be answered with --plan-file")?;
                    let text = std::fs::read_to_string(file)
                        .with_context(|| format!("reading {}", file.display()))?;
                    answer = vec![questions::plan_edited(&text)];
                } else if let Some(plan) = plan
                    && answer == ["edit"]
                {
                    let edited = edit_text(plan)?;
                    answer = vec![if edited.trim() == plan.trim() {
                        questions::PLAN_APPROVE.to_owned()
                    } else {
                        questions::plan_edited(&edited)
                    }];
                }
                if answer.len() != qs.len() {
                    bail!(
                        "{} question(s), {} answer(s): give one per question",
                        qs.len(),
                        answer.len()
                    );
                }
                if qs.iter().zip(&answer).any(|(q, a)| q.secret && a != "-") {
                    bail!("{}", questions::SECRET_NOTE);
                }
                let answers: questions::Answers = qs
                    .iter()
                    .zip(&answer)
                    .filter(|(q, _)| !q.secret)
                    .map(|(q, a)| (q.id.clone(), questions::parse_answer(q, a)))
                    .collect();
                if !store.answer_question(id, &answers, now())? {
                    bail!("request {id} isn't waiting for an answer");
                }
                println!("answered {id}: {}", questions::summary(&qs, &answers));
                return Ok(());
            }
            let answer = answer.join(" ");
            let state = match (skip, answer.as_str()) {
                (false, "yes") => PermissionState::Allowed,
                (false, "always") => PermissionState::Always,
                (false, "rule") => {
                    let cwd = store.run(p.run_id)?.map(|r| r.cwd).unwrap_or_default();
                    let rule = save_rule(&anchorleg_core::rules::suggest(&p.tool, &p.input, &cwd))?;
                    println!("saved the rule {rule} for every session");
                    PermissionState::Always
                }
                (_, "no") | (true, _) => PermissionState::Denied,
                _ => bail!("answer a tool call with yes, always, rule or no"),
            };
            if !store.answer_permission(id, state, now())? {
                bail!("request {id} isn't waiting for an answer");
            }
            println!("answered {id}: {}", if skip { "no" } else { &answer });
        }
    }
    Ok(())
}

/// Answer one agy `PreToolUse` request. Outside an anchorleg run (no `ANCHORLEG_RUN_ID`) agy
/// asks as usual; inside one, anything the policy doesn't allow waits for the person, and any
/// failure denies the call.
fn agy_hook(store: &Store, text: &str) -> Value {
    let Some(run_id) = std::env::var("ANCHORLEG_RUN_ID")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
    else {
        return json!({ "decision": "ask" });
    };
    let req: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            return json!({ "decision": "deny", "reason": format!("anchorleg: bad request: {e}") });
        }
    };
    let mode =
        std::env::var(agy::MODE_ENV).map_or(agy::HookMode::Edits, |m| agy::HookMode::parse(&m));
    let (tool, input) = match agy::hook_verdict_in(&req, mode) {
        HookVerdict::Allow => return json!({ "decision": "allow" }),
        HookVerdict::Deny(reason) => return json!({ "decision": "deny", "reason": reason }),
        HookVerdict::Ask { tool, input } => (tool, input),
    };
    let ask_req = AskInput {
        run_id,
        tool,
        input,
        reason: None,
        id: None,
    };
    // Wait as long as anyone may answer; agy's hook timeout is longer still.
    let wait = Duration::from_secs(GIVE_UP_SECS as u64 + 60);
    match ask(store, &ask_req, wait) {
        Ok(out) if out.decision == "allow" => json!({ "decision": "allow", "reason": out.reason }),
        Ok(out) => json!({
            "decision": "deny",
            "reason": format!("{} in anchorleg", out.reason),
        }),
        Err(e) => json!({ "decision": "deny", "reason": format!("anchorleg: {e}") }),
    }
}

/// The tool `anchorleg permission mcp-ask` offers: the same as anchorleg-mod's `ask_user`.
fn ask_user_tool() -> Value {
    json!({
        "name": "ask_user",
        "description": "Ask the person running this task one to four questions and wait for \
            their answers. Use it when you need a decision only they can make (a choice between \
            approaches, a missing requirement); otherwise pick a sensible default and say so. Give \
            2-4 options per question when you can; they may also answer in their own words.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array", "minItems": 1, "maxItems": 4,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string", "description": "The full question, ending with a question mark." },
                            "header": { "type": "string", "description": "A short label, up to 12 characters." },
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": { "label": { "type": "string" }, "description": { "type": "string" } },
                                    "required": ["label"],
                                },
                            },
                            "multiSelect": { "type": "boolean", "description": "Several options may be picked." },
                        },
                        "required": ["question"],
                    },
                },
            },
            "required": ["questions"],
        },
    })
}

/// The answer to one MCP message (JSON-RPC), `None` for notifications.
fn mcp_reply(store: &Store, run_id: Option<i64>, msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let method = msg
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let ok = |result: Value| json!({ "jsonrpc": "2.0", "id": id.clone(), "result": result });
    let text = |text: String, error: bool| {
        ok(json!({ "content": [{ "type": "text", "text": text }], "isError": error }))
    };
    Some(match method {
        "initialize" => ok(json!({
            "protocolVersion": msg
                .pointer("/params/protocolVersion")
                .cloned()
                .unwrap_or_else(|| "2025-06-18".into()),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "anchorleg", "version": env!("CARGO_PKG_VERSION") },
        })),
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({ "tools": [ask_user_tool()] })),
        "tools/call" if msg.pointer("/params/name") == Some(&json!("ask_user")) => {
            let Some(run_id) = run_id else {
                return Some(text(
                    "Not in an anchorleg run: nobody to ask. Decide yourself or ask in your reply."
                        .to_owned(),
                    true,
                ));
            };
            let args = msg.pointer("/params/arguments").unwrap_or(&Value::Null);
            let qs = questions::from_claude(args);
            if qs.is_empty() {
                return Some(text("No questions given.".to_owned(), true));
            }
            let req = AskInput {
                run_id,
                tool: questions::TOOL.to_owned(),
                input: questions::to_input(&qs),
                reason: None,
                id: None,
            };
            match ask(store, &req, Duration::from_secs(u64::MAX / 4)) {
                Ok(AskOutput {
                    decision: "allow",
                    answers: Some(answers),
                    ..
                }) => text(questions::summary(&qs, &answers), false),
                Ok(out) => text(
                    format!(
                        "The user didn't answer: {} (asked through anchorleg). Decide yourself \
                         or ask in your reply.",
                        out.reason
                    ),
                    true,
                ),
                Err(e) => text(format!("anchorleg couldn't ask: {e}"), true),
            }
        }
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("anchorleg doesn't answer {method}") },
        }),
    })
}

/// `text` as the person edits it in `$VISUAL` / `$EDITOR` (`vi` without either).
pub fn edit_text(text: &str) -> anyhow::Result<String> {
    let path = std::env::temp_dir().join(format!("anchorleg-plan-{}.md", std::process::id()));
    std::fs::write(&path, text)?;
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_owned());
    // `$EDITOR` may carry arguments (`code --wait`).
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(&path)
        .status()
        .with_context(|| format!("starting {editor}"));
    let edited = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    let status = status?;
    if !status.success() {
        bail!("{editor} exited with {status}: nothing was answered");
    }
    Ok(edited?)
}

/// Add `rule` to the lasting allow rules (once) and return it.
pub fn save_rule(rule: &str) -> anyhow::Result<String> {
    let path = Config::default_path()?;
    let mut config = Config::load(&path)?;
    if !config.allow.iter().any(|r| r == rule) {
        config.allow.push(rule.to_owned());
        config.save(&path)?;
    }
    Ok(rule.to_owned())
}

/// One line for a tool call, paths shown relative to the current folder.
fn describe(tool: &str, input: &Value) -> String {
    match std::env::current_dir() {
        Ok(cwd) => describe_tool(tool, &relative_input(input, &cwd.display().to_string())),
        Err(_) => describe_tool(tool, input),
    }
}

/// A desktop notification (or the person's command) for a new request, so they come back.
fn notify_waiting(store: &Store, req: &AskInput, what: &str, question: bool) {
    let Ok(config) = Config::default_path().and_then(|p| Config::load(&p)) else {
        return;
    };
    let task = store.run(req.run_id).ok().flatten().map(|r| r.task);
    let title = task.as_deref().unwrap_or("anchorleg");
    let plan = question
        && questions::from_input(&req.input)
            .first()
            .is_some_and(|q| q.id == questions::plan_question().id);
    let message = match () {
        _ if plan => format!("#{} plan ready: approve it?", req.run_id),
        _ if question => format!("#{} asks: {what}", req.run_id),
        _ => format!("#{} waits: allow {what}?", req.run_id),
    };
    notify::send(
        &config.notify,
        &Notice::new(Kind::Waiting, Some(req.run_id), title, &message),
    );
}

fn ask(store: &Store, req: &AskInput, wait: Duration) -> anyhow::Result<AskOutput> {
    let runs = store.session_runs(req.run_id)?;
    if runs.is_empty() {
        bail!("no run {}", req.run_id);
    }
    let question = req.tool == questions::TOOL;
    // The session's own wait (`run --answer-wait`). Without one a question waits for the person
    // as long as the run lives (D44); a tool call is refused after GIVE_UP_SECS, as the CLI would.
    let give_up = match store.run(req.run_id)?.and_then(|r| r.answer_wait) {
        Some(secs) => Some(secs),
        None if question => None,
        None => Some(GIVE_UP_SECS),
    };
    let what = if question {
        let qs = questions::from_input(&req.input);
        match qs.as_slice() {
            [q] => format!("“{}”", q.question),
            _ => format!("{} questions", qs.len()),
        }
    } else {
        describe(&req.tool, &req.input)
    };
    let id = match req.id {
        Some(id) => id,
        None => {
            if !question && store.always_allowed(&runs, &req.tool)? {
                return Ok(AskOutput {
                    decision: "allow",
                    id: None,
                    reason: format!("{} allowed for this session", req.tool),
                    answers: None,
                });
            }
            // The person's lasting rules (`anchorleg rules`); a review stays read-only.
            if !question && let Some(run) = store.run(req.run_id)?.filter(|r| r.review_of.is_none())
            {
                let rules = Config::default_path()
                    .and_then(|p| Config::load(&p))
                    .map(|c| c.allow)
                    .unwrap_or_default();
                if let Some(rule) = rules
                    .iter()
                    .find(|r| anchorleg_core::rules::matches(r, &req.tool, &req.input, &run.cwd))
                {
                    return Ok(AskOutput {
                        decision: "allow",
                        id: None,
                        reason: format!("allowed by the rule {rule}"),
                        answers: None,
                    });
                }
            }
            let id = store.ask_permission(
                req.run_id,
                &req.tool,
                &req.input,
                req.reason.as_deref(),
                now(),
            )?;
            note(
                req.run_id,
                &if question {
                    format!("the agent asks: {what}")
                } else {
                    format!("waiting for you to allow {what}")
                },
            );
            notify_waiting(store, req, &what, question);
            id
        }
    };
    let started = Instant::now();
    loop {
        let p = store
            .permission(id)?
            .with_context(|| format!("no request {id}"))?;
        if question && p.state != PermissionState::Pending {
            let answered = p.state == PermissionState::Allowed;
            let said = match &p.answers {
                Some(a) if answered => {
                    format!(
                        "answered: {}",
                        questions::summary(&questions::from_input(&p.input), a)
                    )
                }
                _ if p.state == PermissionState::Expired => "nobody answered".to_owned(),
                _ => "the user skipped it".to_owned(),
            };
            note(req.run_id, &format!("{what}: {said}"));
            return Ok(AskOutput {
                decision: if answered { "allow" } else { "deny" },
                id: Some(id),
                reason: said,
                answers: if answered { p.answers } else { None },
            });
        }
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
                if ended || give_up.is_some_and(|g| now() - p.asked_at > g) {
                    store.answer_permission(id, PermissionState::Expired, now())?;
                    continue;
                }
                if started.elapsed() >= wait {
                    return Ok(AskOutput {
                        decision: "pending",
                        id: Some(id),
                        reason: "still waiting".to_owned(),
                        answers: None,
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
            answers: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_waits_for_ever_a_tool_call_does_not() {
        let store = Store::open_in_memory().unwrap();
        let run = store.start_run("task", "/repo", None, 100).unwrap();
        store.set_run_state(run, RunState::Running, 100).unwrap();
        store
            .set_run_pid(run, Some(i64::from(std::process::id())))
            .unwrap();
        let long_ago = now() - 2 * GIVE_UP_SECS;
        let q = questions::to_input(&[questions::plan_question()]);
        let asked = |tool: &str, input: &Value| {
            let id = store
                .ask_permission(run, tool, input, None, long_ago)
                .unwrap();
            let req = AskInput {
                run_id: run,
                tool: tool.to_owned(),
                input: input.clone(),
                reason: None,
                id: Some(id),
            };
            ask(&store, &req, Duration::from_millis(200))
                .unwrap()
                .decision
        };
        assert_eq!(asked(questions::TOOL, &q), "pending");
        assert_eq!(asked("Bash", &json!({ "command": "ls" })), "deny");
        // With a wait set for the session, a question expires too.
        store.set_run_answer_wait(run, Some(60)).unwrap();
        assert_ne!(asked(questions::TOOL, &q), "pending");
    }
}
