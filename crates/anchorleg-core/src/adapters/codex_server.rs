//! Codex turns through `codex app-server` (stdio JSON-RPC), verified on codex-cli 0.153.4
//! (fixtures/codex-0.153.4-app-server-turn-20261011). This replaces `codex exec --json` because
//! only the app server can ask before a command or an edit (D27).
//!
//! anchorleg keeps one format for Codex runs: every app-server message is translated into the
//! `codex exec --json` events the rest of anchorleg already reads (`thread.started`,
//! `item.completed`, `turn.completed`, `turn.failed`, `error`), so run logs, the handoff and the
//! conversation view don't change. Approval requests are answered by the person through
//! `anchorleg permission ask`, the same queue Claude runs use.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use super::codex::{self, live_quota};
use super::{Credits, QuotaReading};
use crate::questions;

/// `codex app-server` request ids anchorleg uses.
const INITIALIZE: i64 = 1;
const THREAD: i64 = 2;
const TURN: i64 = 3;
/// `turn/interrupt` for a soft stop; far above the turn ids.
const INTERRUPT: i64 = 1000;
/// `turn/steer` requests count up from here.
const STEER: i64 = 2000;
/// `thread/name/set`; its answer needs nothing.
const NAME: i64 = 1100;

/// How many times a turn stopped by a refusal is continued with a note about it.
const MAX_REFUSAL_TURNS: usize = 3;

/// What anchorleg starts the turn with.
#[derive(Debug, Clone)]
pub struct Turn {
    pub cwd: String,
    pub prompt: String,
    /// Continue this thread instead of starting one.
    pub resume: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `None` leaves Codex's own setting (the person's args or config chose one).
    pub sandbox: Option<String>,
    /// `on-request` asks the person through anchorleg; `never` when no one can be asked.
    pub approval: Option<String>,
    /// Stop the turn once any quota window reaches this fraction (`--stop-at`); 0 is off.
    pub stop_at: f64,
    /// Images attached to the prompt, sent as `localImage` input with the first turn.
    pub images: Vec<String>,
    /// Plan only: Codex's plan collaboration mode (experimental API), read-only sandbox.
    pub plan: bool,
    /// A review: the first turn is Codex's review mode (`review/start`) with the prompt as its
    /// instructions.
    pub review: bool,
    /// Fork `resume` into a new thread (`thread/fork`) instead of continuing it.
    pub fork: bool,
    /// With `fork`: the last turn the new thread keeps (`lastTurnId`), for a rewind.
    pub fork_at: Option<String>,
    /// Approval requests go to Codex's own reviewer (`approvalsReviewer: auto_review`), not to
    /// the person (`--mode auto`).
    pub auto_review: bool,
    /// Tokens the task may still use (`--budget`): past them the turn stops after the model call
    /// that went over, like the quota stop.
    pub budget_left: Option<u64>,
    /// A JSON Schema for the turn's final message (`outputSchema`, `--json-schema`).
    pub output_schema: Option<Value>,
    /// The task's own instructions (`developerInstructions` on the thread request).
    pub instructions: Option<String>,
    /// Config overrides for the thread (`config`): e.g. the task's extra writable folders.
    pub config: Option<Value>,
    /// The session's name, set on the thread (`thread/name/set`).
    pub name: Option<String>,
}

/// Why anchorleg stopped a Codex turn when the task's budget ran out.
pub const BUDGET_STOP: &str = "token budget used up";

/// What the driver passes back to the supervisor.
#[derive(Debug)]
pub enum Event {
    /// A `codex exec --json` event line, to log and parse like before.
    Line(String),
    /// Current quota, pushed by Codex after model calls.
    Quota(Vec<QuotaReading>),
    /// Usage-credit state, pushed with the quota.
    Credits(Credits),
    /// `thread/resume` of a thread this login doesn't have.
    NotFound,
    /// anchorleg stopped the turn because quota reached `--stop-at` (why, for the run log).
    Stopped(String),
    /// Codex took the person's message into the running turn (the queued message's id).
    Steered(i64),
}

/// What the person sends a running Codex launch (`anchorleg send`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Steer {
    /// Add this message to the running turn (`turn/steer`); `id` is the queued message's.
    Text { id: i64, text: String },
    /// Stop the turn now (`turn/interrupt`); the queued message follows as a new turn.
    Interrupt,
}

async fn next_steer(rx: &mut Option<tokio::sync::mpsc::UnboundedReceiver<Steer>>) -> Steer {
    match rx {
        Some(rx) => match rx.recv().await {
            Some(s) => s,
            None => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
}

/// Why to stop, when a window has reached `stop_at` (e.g. "five_hour quota at 92%").
pub fn stop_reason(readings: &[QuotaReading], stop_at: f64) -> Option<String> {
    if stop_at <= 0.0 {
        return None;
    }
    readings
        .iter()
        .filter_map(|r| Some((r.used?, r.window.as_ref())))
        .filter(|(used, _)| *used >= stop_at)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(used, window)| {
            let name = window.map_or("", |w| w.name());
            format!("{name} quota at {:.0}%", used * 100.0)
                .trim_start()
                .to_owned()
        })
}

/// The JSON-RPC messages anchorleg sends.
/// `experimental` opts in to Codex's experimental API (needed for plan mode).
pub fn initialize(experimental: bool) -> Value {
    let mut params =
        json!({ "clientInfo": { "name": "anchorleg", "version": env!("CARGO_PKG_VERSION") } });
    if experimental {
        params["capabilities"] = json!({ "experimentalApi": true });
    }
    json!({ "id": INITIALIZE, "method": "initialize", "params": params })
}

pub fn thread_request(turn: &Turn) -> Value {
    let mut params = json!({ "cwd": turn.cwd });
    if let Some(m) = &turn.model {
        params["model"] = m.as_str().into();
    }
    if let Some(s) = &turn.sandbox {
        params["sandbox"] = s.as_str().into();
    }
    if let Some(a) = &turn.approval {
        params["approvalPolicy"] = a.as_str().into();
    }
    if turn.auto_review {
        params["approvalsReviewer"] = "auto_review".into();
    }
    if let Some(i) = &turn.instructions {
        params["developerInstructions"] = i.as_str().into();
    }
    if let Some(c) = &turn.config {
        params["config"] = c.clone();
    }
    match &turn.resume {
        Some(id) => {
            params["threadId"] = id.as_str().into();
            params["excludeTurns"] = true.into();
            let method = if turn.fork {
                if let Some(at) = &turn.fork_at {
                    params["lastTurnId"] = at.as_str().into();
                }
                "thread/fork"
            } else {
                "thread/resume"
            };
            json!({ "id": THREAD, "method": method, "params": params })
        }
        None => json!({ "id": THREAD, "method": "thread/start", "params": params }),
    }
}

pub fn turn_request(id: i64, thread: &str, text: &str, effort: Option<&str>) -> Value {
    let mut params = json!({
        "threadId": thread,
        "input": [{ "type": "text", "text": text }],
        // Short reasoning summaries, shown dimmed in the conversation.
        "summary": "auto",
    });
    if let Some(e) = effort {
        params["effort"] = e.into();
    }
    json!({ "id": id, "method": "turn/start", "params": params })
}

/// `review/start` on the thread, inline, with `instructions` as a custom target.
pub fn review_request(id: i64, thread: &str, instructions: &str) -> Value {
    json!({
        "id": id,
        "method": "review/start",
        "params": {
            "threadId": thread,
            "target": { "type": "custom", "instructions": instructions },
            "delivery": "inline",
        },
    })
}

/// `thread/compact/start`: Codex summarises the thread so far to free its context.
pub fn compact_request(id: i64, thread: &str) -> Value {
    json!({ "id": id, "method": "thread/compact/start", "params": { "threadId": thread } })
}

/// A `turn/start` request in Codex's plan collaboration mode: the agent investigates and ends
/// the turn with a `plan` item instead of changing anything. `model` is required by Codex.
pub fn with_plan_mode(mut request: Value, model: &str, effort: Option<&str>) -> Value {
    request["params"]["collaborationMode"] = json!({
        "mode": "plan",
        "settings": { "model": model, "reasoning_effort": effort, "developer_instructions": null },
    });
    request
}

/// A `turn/start` request with images added to its input (`localImage`, by path).
pub fn with_images(mut request: Value, images: &[String]) -> Value {
    if let Some(input) = request
        .pointer_mut("/params/input")
        .and_then(Value::as_array_mut)
    {
        input.extend(
            images
                .iter()
                .map(|p| json!({ "type": "localImage", "path": p })),
        );
    }
    request
}

/// An approval request, as anchorleg shows it: Claude's tool names where one fits.
#[derive(Debug, PartialEq)]
pub struct Approval {
    pub tool: String,
    pub input: Value,
    pub reason: Option<String>,
    /// How to say no: `decline` lets the turn go on; `cancel` (when Codex doesn't offer
    /// `decline`) stops it, and anchorleg continues it with a note.
    pub refuse: &'static str,
}

/// The tool name Codex's requests for more sandbox access (`item/permissions/requestApproval`)
/// are asked under.
pub const PERMISSIONS_TOOL: &str = "Permissions";

/// How much of one command's output is passed on live; the rest arrives with the finished item.
const LIVE_OUTPUT_CAP: usize = 16 * 1024;

/// Translates app-server messages for one launch.
#[derive(Debug, Default)]
pub struct Translator {
    thread_id: Option<String>,
    /// The latest model call's token usage (`last` of `thread/tokenUsage/updated`).
    last_usage: Option<Value>,
    /// The model's context window, as Codex reports it with token usage.
    context_window: Option<u64>,
    /// Command output already passed on, per item: live output is capped (D45).
    output_sent: HashMap<String, usize>,
    /// Commands started and not finished yet: still running when the turn ends means they run
    /// in the background.
    running: HashMap<String, String>,
    /// This turn's tokens: the sum of every model call's `last`.
    turn_usage: HashMap<&'static str, u64>,
    /// Files of each file change, from `item/started`, for its approval request.
    changes: HashMap<String, Vec<String>>,
    /// Their diffs, shown with the approval request.
    diffs: HashMap<String, String>,
}

impl Translator {
    pub fn thread_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
    }

    /// This turn's tokens that count toward a budget: new input, output, cache writes and a
    /// tenth of cached input (as `Store::run_tokens` counts them).
    pub fn turn_tokens(&self) -> u64 {
        let n = |k: &str| self.turn_usage.get(k).copied().unwrap_or(0);
        let cached = n("cached_input_tokens");
        n("input_tokens")
            .saturating_sub(cached)
            .saturating_add(n("output_tokens"))
            .saturating_add(n("cache_write_input_tokens"))
            .saturating_add(cached / 10)
    }

    /// The `thread/start` or `thread/resume` answer.
    pub fn thread_started(&mut self, result: &Value) -> Vec<Value> {
        let Some(id) = result.pointer("/thread/id").and_then(Value::as_str) else {
            return Vec::new();
        };
        self.thread_id = Some(id.to_owned());
        vec![json!({ "type": "thread.started", "thread_id": id })]
    }

    /// A notification, as `codex exec --json` events.
    pub fn notification(&mut self, method: &str, params: &Value) -> Vec<Value> {
        match method {
            "turn/started" => vec![json!({ "type": "turn.started" })],
            "item/started" => {
                let Some(item) = params.get("item") else {
                    return Vec::new();
                };
                match item.get("type").and_then(Value::as_str) {
                    Some("fileChange") => {
                        if let Some(id) = item.get("id").and_then(Value::as_str) {
                            self.changes.insert(id.to_owned(), change_paths(item));
                            self.diffs.insert(id.to_owned(), change_diff(item));
                        }
                        Vec::new()
                    }
                    // A command starting, or subagents started or waited on: shown as running
                    // until it completes.
                    Some("commandExecution" | "collabAgentToolCall") => {
                        if let (Some(id), Some(cmd)) = (
                            item.get("id").and_then(Value::as_str),
                            item.get("command").and_then(Value::as_str),
                        ) {
                            self.running
                                .insert(id.to_owned(), codex::unwrap_command(cmd).to_owned());
                        }
                        exec_item(item)
                            .map(|i| json!({ "type": "item.started", "item": i }))
                            .into_iter()
                            .collect()
                    }
                    _ => Vec::new(),
                }
            }
            "item/commandExecution/outputDelta" => {
                let (Some(id), Some(delta)) = (
                    params.get("itemId").and_then(Value::as_str),
                    params.get("delta").and_then(Value::as_str),
                ) else {
                    return Vec::new();
                };
                let sent = self.output_sent.entry(id.to_owned()).or_default();
                if *sent >= LIVE_OUTPUT_CAP {
                    return Vec::new();
                }
                let room = LIVE_OUTPUT_CAP - *sent;
                let mut cut = delta.len().min(room);
                while !delta.is_char_boundary(cut) {
                    cut -= 1;
                }
                *sent += cut;
                vec![json!({ "type": "item.output_delta", "item_id": id, "delta": &delta[..cut] })]
            }
            "item/completed" => params
                .get("item")
                .inspect(|i| {
                    if let Some(id) = i.get("id").and_then(Value::as_str) {
                        self.running.remove(id);
                    }
                })
                .and_then(exec_item)
                .map(|item| json!({ "type": "item.completed", "item": item }))
                .into_iter()
                .collect(),
            // A hook of the person's that did something visible (blocked, failed, said something).
            "hook/completed" => {
                let run = params.get("run").unwrap_or(&Value::Null);
                let status = run
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let text: Vec<&str> = run
                    .get("entries")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.get("text").and_then(Value::as_str))
                    .collect();
                if status == "completed" && text.is_empty() {
                    return Vec::new();
                }
                vec![json!({ "type": "item.completed", "item": {
                    "id": run.get("id"), "type": "hook", "status": status,
                    "event": run.get("eventName"), "text": text.join("\n"),
                } })]
            }
            // Codex answers this turn with another model than asked for.
            "model/rerouted" => vec![json!({
                "type": "model.rerouted",
                "from": params.get("fromModel"),
                "to": params.get("toModel"),
                "reason": params.get("reason"),
            })],
            // Codex's reviewer decided on something it would have asked about (`--mode auto`).
            "item/autoApprovalReview/completed" => {
                let field = |k: &str| {
                    params
                        .pointer(k)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                };
                let action = match field("/action/type") {
                    "command" => codex::unwrap_command(field("/action/command")).to_owned(),
                    "execve" => field("/action/program").to_owned(),
                    other => other.replace('_', " "),
                };
                vec![json!({
                    "type": "item.completed",
                    "item": {
                        "id": field("/reviewId"),
                        "type": "auto_review",
                        "status": field("/review/status"),
                        "risk": params.pointer("/review/riskLevel"),
                        "action": action,
                        "rationale": field("/review/rationale"),
                    },
                })]
            }
            "turn/plan/updated" => {
                let items: Vec<Value> = params
                    .get("plan")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|s| {
                        let text = s.get("step").and_then(Value::as_str)?;
                        let done = s.get("status").and_then(Value::as_str) == Some("completed");
                        Some(json!({ "text": text, "completed": done }))
                    })
                    .collect();
                vec![json!({
                    "type": "item.updated",
                    "item": { "id": "plan", "type": "todo_list", "items": items },
                })]
            }
            "thread/tokenUsage/updated" => {
                self.last_usage = params.pointer("/tokenUsage/last").cloned();
                if let Some(w) = params
                    .pointer("/tokenUsage/modelContextWindow")
                    .and_then(Value::as_u64)
                {
                    self.context_window = Some(w);
                }
                for (from, to) in [
                    ("inputTokens", "input_tokens"),
                    ("cachedInputTokens", "cached_input_tokens"),
                    ("cacheWriteInputTokens", "cache_write_input_tokens"),
                    ("outputTokens", "output_tokens"),
                ] {
                    let n = self
                        .last_usage
                        .as_ref()
                        .and_then(|u| u.get(from))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let total = self.turn_usage.entry(to).or_default();
                    *total = total.saturating_add(n);
                }
                Vec::new()
            }
            "error" => {
                let message = params
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("error");
                if params.get("willRetry").and_then(Value::as_bool) == Some(true) {
                    vec![
                        json!({ "type": "error", "message": format!("Reconnecting... {message}") }),
                    ]
                } else {
                    Vec::new() // `turn/completed` reports it as the turn's error.
                }
            }
            _ => Vec::new(),
        }
    }

    /// The thread a message is about, when it isn't this session's: a subagent's.
    pub fn other_thread(&self, msg: &Value) -> Option<String> {
        let thread = msg.pointer("/params/threadId").and_then(Value::as_str)?;
        let main = self.thread_id.as_deref()?;
        (thread != main).then(|| thread.to_owned())
    }

    /// A notification from a subagent's thread, as `{"type":"subagent","agent":<thread>,…}`:
    /// its items as `event`, its end as `status`. Its tokens count toward this turn's.
    pub fn subagent_notification(
        &mut self,
        thread: &str,
        method: &str,
        params: &Value,
    ) -> Vec<Value> {
        let wrap = |event: Value| json!({ "type": "subagent", "agent": thread, "event": event });
        match method {
            "item/started" => params
                .get("item")
                .filter(|i| i["type"] == "commandExecution")
                .and_then(exec_item)
                .map(|item| wrap(json!({ "type": "item.started", "item": item })))
                .into_iter()
                .collect(),
            "item/completed" => params
                .get("item")
                .filter(|i| i["type"] != "userMessage")
                .and_then(exec_item)
                .map(|item| wrap(json!({ "type": "item.completed", "item": item })))
                .into_iter()
                .collect(),
            "thread/tokenUsage/updated" => {
                let last = params.pointer("/tokenUsage/last");
                for (from, to) in [
                    ("inputTokens", "input_tokens"),
                    ("cachedInputTokens", "cached_input_tokens"),
                    ("cacheWriteInputTokens", "cache_write_input_tokens"),
                    ("outputTokens", "output_tokens"),
                ] {
                    let n = last
                        .and_then(|u| u.get(from))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let total = self.turn_usage.entry(to).or_default();
                    *total = total.saturating_add(n);
                }
                Vec::new()
            }
            "turn/completed" => {
                let status = match params.pointer("/turn/status").and_then(Value::as_str) {
                    Some("completed") => "completed",
                    Some("interrupted") => "interrupted",
                    _ => "failed",
                };
                vec![json!({ "type": "subagent", "agent": thread, "status": status })]
            }
            _ => Vec::new(),
        }
    }

    /// `turn/completed`: `turn.completed` or `turn.failed`.
    pub fn turn_completed(&self, params: &Value) -> Value {
        let turn = params.get("turn").unwrap_or(&Value::Null);
        match turn.get("status").and_then(Value::as_str) {
            Some("completed") => {
                let n = |k: &str| {
                    self.last_usage
                        .as_ref()
                        .and_then(|u| u.get(k))
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                };
                let mut done = json!({
                    "type": "turn.completed",
                    "turn_id": turn.get("id").cloned().unwrap_or(Value::Null),
                    "usage": {
                        "input_tokens": n("inputTokens"),
                        "cached_input_tokens": n("cachedInputTokens"),
                        "output_tokens": n("outputTokens"),
                    },
                    "turn_usage": self.turn_usage,
                });
                if !self.running.is_empty() {
                    done["background"] = json!(self.running.values().collect::<Vec<_>>());
                }
                if let Some(w) = self.context_window {
                    done["usage"]["model_context_window"] = w.into();
                }
                done
            }
            status => {
                let error = turn.get("error").unwrap_or(&Value::Null);
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| match status {
                        Some("interrupted") => "the turn was stopped".to_owned(),
                        _ => "the turn failed".to_owned(),
                    });
                let mut failed = json!({ "type": "turn.failed", "error": { "message": message } });
                // A stopped or failed turn still used tokens.
                if !self.turn_usage.is_empty() {
                    failed["turn_usage"] = json!(self.turn_usage);
                }
                if let Some(code) = error.get("codexErrorInfo").and_then(error_code) {
                    failed["error"]["codex_error_info"] = code.into();
                }
                failed
            }
        }
    }

    /// The turn anchorleg stopped at a clean point: only its tokens, as `turn.stopped`.
    pub fn turn_stopped(&self) -> Value {
        json!({ "type": "turn.stopped", "turn_usage": self.turn_usage })
    }

    /// An approval request from Codex. `None` for requests anchorleg doesn't answer.
    pub fn approval(&self, method: &str, params: &Value) -> Option<Approval> {
        let reason = params
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let refuse = match params.get("availableDecisions").and_then(Value::as_array) {
            Some(d) if !d.iter().any(|x| x == "decline") => "cancel",
            _ => "decline",
        };
        let (tool, input) = match method {
            "item/commandExecution/requestApproval" => {
                let command = params
                    .pointer("/commandActions/0/command")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        params
                            .get("command")
                            .and_then(Value::as_str)
                            .map(|c| super::codex::unwrap_command(c).to_owned())
                    })
                    .unwrap_or_default();
                let mut input = json!({ "command": command });
                if let Some(cwd) = params.get("cwd").and_then(Value::as_str) {
                    input["cwd"] = cwd.into();
                }
                ("Bash".to_owned(), input)
            }
            "item/fileChange/requestApproval" => {
                let item = params.get("itemId").and_then(Value::as_str).unwrap_or("");
                let paths = self.changes.get(item).cloned().unwrap_or_default();
                let mut input = json!({ "file_path": paths.first().cloned().unwrap_or_default() });
                if paths.len() > 1 {
                    input["files"] = paths.into();
                }
                if let Some(diff) = self.diffs.get(item).filter(|d| !d.is_empty()) {
                    input["diff"] = diff.as_str().into();
                }
                if let Some(root) = params.get("grantRoot").and_then(Value::as_str) {
                    input["grant_root"] = root.into();
                }
                ("Edit".to_owned(), input)
            }
            // Codex asks before every MCP tool call through an MCP prompt with an empty form.
            "mcpServer/elicitation/request"
                if params
                    .pointer("/_meta/codex_approval_kind")
                    .and_then(Value::as_str)
                    == Some("mcp_tool_call") =>
            {
                let server = params
                    .get("serverName")
                    .and_then(Value::as_str)
                    .unwrap_or("mcp");
                let tool = params
                    .get("message")
                    .and_then(Value::as_str)
                    .and_then(|m| m.split('"').nth(1))
                    .unwrap_or("tool");
                let input = params
                    .pointer("/_meta/tool_params")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                return Some(Approval {
                    tool: format!("mcp__{server}__{tool}"),
                    input,
                    reason: params
                        .pointer("/_meta/tool_description")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    refuse: "decline",
                });
            }
            "item/permissions/requestApproval" => {
                let mut input = json!({ "permissions": params.get("permissions").cloned().unwrap_or(Value::Null) });
                if let Some(cwd) = params.get("cwd").and_then(Value::as_str) {
                    input["cwd"] = cwd.into();
                }
                (PERMISSIONS_TOOL.to_owned(), input)
            }
            _ => return None,
        };
        Some(Approval {
            tool,
            input,
            reason,
            refuse,
        })
    }
}

/// `codexErrorInfo` as one word: `"usageLimitExceeded"` or the key of an object variant.
fn error_code(info: &Value) -> Option<String> {
    match info {
        Value::String(s) => Some(s.clone()),
        Value::Object(m) => m.keys().next().cloned(),
        _ => None,
    }
}

fn change_paths(item: &Value) -> Vec<String> {
    item.get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| c.get("path").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

/// A file change's diffs as one unified diff: a new file's text as added lines, a header per
/// file when there are several.
fn change_diff(item: &Value) -> String {
    let changes: Vec<&Value> = item
        .get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let mut out = String::new();
    for c in &changes {
        let path = c.get("path").and_then(Value::as_str).unwrap_or_default();
        let diff = c.get("diff").and_then(Value::as_str).unwrap_or_default();
        if changes.len() > 1 {
            out += &format!("--- {path}\n");
        }
        match c.pointer("/kind/type").and_then(Value::as_str) {
            Some("add") => {
                for line in diff.lines() {
                    out += &format!("+{line}\n");
                }
            }
            Some("delete") => out += &format!("-(deletes {path})\n"),
            _ => {
                out += diff;
                if !diff.is_empty() && !diff.ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// An app-server item as the `codex exec --json` item anchorleg's readers know. `None` for
/// items they don't use (the prompt, reasoning, …).
fn exec_item(item: &Value) -> Option<Value> {
    let id = item.get("id").cloned().unwrap_or(Value::Null);
    let status = item.get("status").cloned().unwrap_or(Value::Null);
    match item.get("type").and_then(Value::as_str)? {
        // The model's reasoning summary, when it wrote one.
        "reasoning" => {
            let text = item
                .get("summary")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty())
                .then(|| json!({ "id": id, "type": "reasoning", "text": text.trim() }))
        }
        // Plan mode's result: shown and kept like the agent's final message.
        "plan" => Some(json!({
            "id": id,
            "type": "agent_message",
            "text": item.get("text").cloned().unwrap_or_default(),
        })),
        // Subagents (`multi_agent`): started (`spawnAgent`) or waited on (`wait`, whose
        // `agentsStates` carry their final messages). Their own threads arrive as `subagent`.
        "collabAgentToolCall" => {
            let agents = item.get("receiverThreadIds").cloned().unwrap_or(json!([]));
            match item.get("tool").and_then(Value::as_str)? {
                "spawnAgent" => Some(json!({
                    "id": id,
                    "type": "agent_spawn",
                    "prompt": item.get("prompt").cloned().unwrap_or_default(),
                    "agents": agents,
                    "status": status,
                })),
                "wait" => Some(json!({
                    "id": id,
                    "type": "agent_wait",
                    "agents": agents,
                    "states": item.get("agentsStates").cloned().unwrap_or(json!({})),
                    "status": status,
                })),
                _ => None,
            }
        }
        // `/compact` (`thread/compact/start`) finished.
        "contextCompaction" => Some(json!({ "id": id, "type": "context_compaction" })),
        "agentMessage" => Some(json!({
            "id": id,
            "type": "agent_message",
            "text": item.get("text").cloned().unwrap_or_default(),
        })),
        "commandExecution" => Some(json!({
            "id": id,
            "type": "command_execution",
            "command": item.get("command").cloned().unwrap_or_default(),
            "aggregated_output": item.get("aggregatedOutput").cloned().unwrap_or_default(),
            "exit_code": item.get("exitCode").cloned().unwrap_or(Value::Null),
            "status": status,
        })),
        "fileChange" => {
            let changes: Vec<Value> = item
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|c| {
                    let kind = c
                        .pointer("/kind/type")
                        .or_else(|| c.get("kind"))
                        .and_then(Value::as_str)
                        .unwrap_or("update");
                    json!({ "path": c.get("path").cloned().unwrap_or_default(), "kind": kind })
                })
                .collect();
            Some(json!({ "id": id, "type": "file_change", "changes": changes, "status": status }))
        }
        _ => None,
    }
}

/// Ask the person through `anchorleg permission ask` (the same queue as Claude runs) and wait.
/// `false` when anchorleg can't be asked.
async fn ask_person(bin: &Path, run_id: i64, approval: &Approval) -> bool {
    let req = json!({
        "run_id": run_id,
        "tool": approval.tool,
        "input": approval.input,
        "reason": approval.reason,
    });
    ask(bin, req)
        .await
        .is_some_and(|a| a.get("decision").and_then(Value::as_str) == Some("allow"))
}

/// Codex's questions (`item/tool/requestUserInput`), asked the same way. Empty answers when
/// the person skipped them or anchorleg can't be asked: Codex then goes on without.
async fn ask_questions(bin: &Path, run_id: i64, params: &Value) -> Value {
    let qs = questions::from_codex(params);
    let req = json!({
        "run_id": run_id,
        "tool": questions::TOOL,
        "input": questions::to_input(&qs),
        "reason": null,
    });
    let answers: questions::Answers = ask(bin, req)
        .await
        .and_then(|a| a.get("answers").cloned())
        .and_then(|a| serde_json::from_value(a).ok())
        .unwrap_or_default();
    questions::codex_response(&answers)
}

/// An MCP server's request for input (`mcpServer/elicitation/request`), asked as questions.
/// Declined when the person skips it or anchorleg can't be asked.
async fn ask_elicitation(bin: &Path, run_id: i64, params: &Value) -> Value {
    let qs = questions::from_elicitation(params);
    if qs.is_empty() {
        return json!({ "action": "decline" });
    }
    let req = json!({
        "run_id": run_id,
        "tool": questions::TOOL,
        "input": questions::to_input(&qs),
        "reason": null,
    });
    let answers: questions::Answers = ask(bin, req)
        .await
        .and_then(|a| a.get("answers").cloned())
        .and_then(|a| serde_json::from_value(a).ok())
        .unwrap_or_default();
    questions::elicitation_response(params, &answers)
}

/// Ask the person `qs` (through `anchorleg permission ask`). `None` when anchorleg can't be
/// asked or the request expired; skipped questions have no answer.
pub(crate) async fn ask_questions_of(
    bin: &Path,
    run_id: i64,
    qs: &[questions::Question],
    plan: Option<&str>,
) -> Option<questions::Answers> {
    let mut input = questions::to_input(qs);
    if let Some(plan) = plan {
        input["plan"] = plan.into();
    }
    let req = json!({
        "run_id": run_id,
        "tool": questions::TOOL,
        "input": input,
        "reason": null,
    });
    ask(bin, req)
        .await
        .and_then(|a| a.get("answers").cloned())
        .and_then(|a| serde_json::from_value(a).ok())
}

/// One `anchorleg permission ask --json` request, asked again while the answer is "pending".
/// The final answer, or `None` when anchorleg can't be asked.
async fn ask(bin: &Path, mut req: Value) -> Option<Value> {
    let mut id: Option<i64> = None;
    loop {
        if let Some(id) = id {
            req["id"] = id.into();
        }
        let mut child = tokio::process::Command::new(bin)
            .args(["permission", "ask", "--json"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(req.to_string().as_bytes()).await;
        }
        let out = child.wait_with_output().await.ok()?;
        let answer: Value = serde_json::from_slice(&out.stdout).ok()?;
        if answer.get("decision").and_then(Value::as_str) != Some("pending") {
            return Some(answer);
        }
        id = Some(answer.get("id").and_then(Value::as_i64)?);
    }
}

/// Who answers Codex's approval requests.
#[derive(Debug, Clone)]
pub struct Asker {
    /// The `anchorleg` binary (`anchorleg permission ask`).
    pub bin: std::path::PathBuf,
    pub run_id: i64,
}

/// Run one launch over the app server's stdin/stdout: start or resume the thread, run the turn,
/// answer approvals, and send everything back as [`Event`]s. Returns when the turn is over (or
/// the server goes away); dropping `stdin` then lets the server exit.
pub async fn drive<R, W>(
    stdout: R,
    mut stdin: W,
    turn: Turn,
    asker: Option<Asker>,
    events: tokio::sync::mpsc::UnboundedSender<Event>,
    mut steer: Option<tokio::sync::mpsc::UnboundedReceiver<Steer>>,
) -> std::io::Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    async fn send<W: tokio::io::AsyncWrite + Unpin>(w: &mut W, v: &Value) -> std::io::Result<()> {
        w.write_all(format!("{v}\n").as_bytes()).await?;
        w.flush().await
    }
    let emit = |v: Value| {
        let _ = events.send(Event::Line(v.to_string()));
    };
    let mut lines = stdout.lines();
    let mut t = Translator::default();
    let mut refused: Vec<String> = Vec::new();
    let mut refusal_turns = 0;
    let mut next_id = TURN;
    // The turn in progress, and why anchorleg is stopping it (soft stop).
    let mut turn_id: Option<String> = None;
    let mut stopping: Option<String> = None;
    // `turn/steer` requests in flight: request id → queued message id.
    let mut steering: HashMap<i64, i64> = HashMap::new();
    let mut next_steer_id = STEER;

    send(&mut stdin, &initialize(turn.plan)).await?;
    // The model plan mode needs: anchorleg's setting, else the one Codex picked for the thread.
    let mut plan_model = turn.model.clone();
    let start_turn = |id: i64, thread: &str, text: &str, model: Option<&str>| {
        if text.trim() == "/compact" {
            // Codex's own `/compact`: summarise the thread to free context, as a turn.
            return compact_request(id, thread);
        }
        if turn.review && id == TURN {
            // Codex's review mode; it streams like a turn and ends with the findings as the
            // agent's message (0.153.4).
            return review_request(id, thread, text);
        }
        let mut r = turn_request(id, thread, text, turn.effort.as_deref());
        if let Some(schema) = &turn.output_schema {
            r["params"]["outputSchema"] = schema.clone();
        }
        match (turn.plan, model) {
            (true, Some(m)) => with_plan_mode(r, m, turn.effort.as_deref()),
            _ => r,
        }
    };
    loop {
        let line = tokio::select! {
            line = lines.next_line() => match line? {
                Some(line) => line,
                None => break,
            },
            s = next_steer(&mut steer) => {
                let (Some(tid), Some(thread)) = (turn_id.as_deref(), t.thread_id()) else {
                    // Between turns: the message waits and starts the next one.
                    continue;
                };
                match s {
                    Steer::Text { id, text } => {
                        steering.insert(next_steer_id, id);
                        send(&mut stdin, &json!({
                            "id": next_steer_id,
                            "method": "turn/steer",
                            "params": {
                                "threadId": thread,
                                "expectedTurnId": tid,
                                "input": [{ "type": "text", "text": text }],
                            },
                        }))
                        .await?;
                        next_steer_id += 1;
                    }
                    Steer::Interrupt if stopping.is_none() => {
                        send(&mut stdin, &json!({
                            "id": INTERRUPT,
                            "method": "turn/interrupt",
                            "params": { "threadId": thread, "turnId": tid },
                        }))
                        .await?;
                    }
                    Steer::Interrupt => {}
                }
                continue;
            }
        };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").cloned();
        // A subagent's own thread (Codex's `multi_agent`) streams on this connection too: its
        // events are progress to show, never this turn's (its `turn/completed` isn't ours).
        if let (Some(m), None) = (method, &id)
            && let Some(thread) = t.other_thread(&msg)
        {
            let params = msg.get("params").unwrap_or(&Value::Null);
            t.subagent_notification(&thread, m, params)
                .into_iter()
                .for_each(emit);
            continue;
        }
        match (method, id) {
            // Our requests' answers.
            (None, Some(id)) => {
                let error = msg.pointer("/error/message").and_then(Value::as_str);
                if id == INITIALIZE {
                    send(&mut stdin, &json!({ "method": "initialized" })).await?;
                    send(&mut stdin, &thread_request(&turn)).await?;
                } else if id == THREAD {
                    match (error, msg.get("result")) {
                        (Some(e), _) if turn.resume.is_some() && e.contains("no rollout found") => {
                            let _ = events.send(Event::NotFound);
                            return Ok(());
                        }
                        (Some(e), _) => {
                            emit(json!({ "type": "turn.failed", "error": { "message": e } }));
                            return Ok(());
                        }
                        (None, Some(result)) => {
                            t.thread_started(result).into_iter().for_each(emit);
                            if plan_model.is_none() {
                                plan_model = result
                                    .get("model")
                                    .or_else(|| result.pointer("/thread/model"))
                                    .and_then(Value::as_str)
                                    .map(str::to_owned);
                            }
                            let thread = t.thread_id().unwrap_or_default().to_owned();
                            if let Some(name) = &turn.name {
                                send(
                                    &mut stdin,
                                    &json!({ "id": NAME, "method": "thread/name/set",
                                        "params": { "threadId": thread, "name": name } }),
                                )
                                .await?;
                            }
                            send(
                                &mut stdin,
                                &with_images(
                                    start_turn(
                                        next_id,
                                        &thread,
                                        &turn.prompt,
                                        plan_model.as_deref(),
                                    ),
                                    &turn.images,
                                ),
                            )
                            .await?;
                        }
                        (None, None) => return Ok(()),
                    }
                } else if id == INTERRUPT || id == NAME {
                    // The answer to a soft stop (`turn/completed` follows), or to the name.
                } else if let Some(message) = id.as_i64().and_then(|i| steering.remove(&i)) {
                    // A refused steer (the turn ended meanwhile) stays queued for the next turn.
                    if error.is_none() {
                        let _ = events.send(Event::Steered(message));
                    }
                } else if let Some(e) = error {
                    // `turn/start` refused.
                    emit(json!({ "type": "turn.failed", "error": { "message": e } }));
                    return Ok(());
                }
            }
            // Requests from Codex.
            (Some(method), Some(id)) => {
                let params = msg.get("params").unwrap_or(&Value::Null);
                let reply = match (t.approval(method, params), &asker) {
                    _ if method == "item/tool/requestUserInput" => {
                        let result = match &asker {
                            Some(asker) => ask_questions(&asker.bin, asker.run_id, params).await,
                            None => questions::codex_response(&questions::Answers::new()),
                        };
                        json!({ "id": id, "result": result })
                    }
                    (Some(a), asker) if method == "mcpServer/elicitation/request" => {
                        // An MCP tool call to approve.
                        let allowed = match asker {
                            Some(asker) => ask_person(&asker.bin, asker.run_id, &a).await,
                            None => false,
                        };
                        let action = if allowed { "accept" } else { "decline" };
                        json!({ "id": id, "result": { "action": action, "content": {} } })
                    }
                    (None, _) if method == "mcpServer/elicitation/request" => {
                        let result = match &asker {
                            Some(asker) => ask_elicitation(&asker.bin, asker.run_id, params).await,
                            None => json!({ "action": "decline" }),
                        };
                        json!({ "id": id, "result": result })
                    }
                    (Some(a), asker) if a.tool == PERMISSIONS_TOOL => {
                        // Granted for this turn as asked, or nothing.
                        let granted = match asker {
                            Some(asker) => ask_person(&asker.bin, asker.run_id, &a).await,
                            None => false,
                        };
                        let result = if granted {
                            json!({ "permissions": a.input["permissions"], "scope": "turn" })
                        } else {
                            json!({ "permissions": {} })
                        };
                        json!({ "id": id, "result": result })
                    }
                    (Some(a), Some(asker)) => {
                        let decision = if ask_person(&asker.bin, asker.run_id, &a).await {
                            "accept"
                        } else {
                            if a.refuse == "cancel" {
                                refused.push(
                                    match a.input.get("command").and_then(Value::as_str) {
                                        Some(c) => format!("`{c}`"),
                                        None => {
                                            crate::conversation::describe_tool(&a.tool, &a.input)
                                        }
                                    },
                                );
                            }
                            a.refuse
                        };
                        json!({ "id": id, "result": { "decision": decision } })
                    }
                    (Some(a), None) => json!({ "id": id, "result": { "decision": a.refuse } }),
                    (None, _) => json!({
                        "id": id,
                        "error": { "code": -32601, "message": format!("anchorleg doesn't answer {method}") },
                    }),
                };
                send(&mut stdin, &reply).await?;
            }
            (Some("turn/completed"), None) => {
                let params = msg.get("params").unwrap_or(&Value::Null);
                let status = params.pointer("/turn/status").and_then(Value::as_str);
                if let Some(reason) = stopping.take() {
                    // Our soft stop. If the turn finished anyway, it's simply done.
                    if status == Some("interrupted") {
                        emit(t.turn_stopped());
                        let _ = events.send(Event::Stopped(reason));
                        return Ok(());
                    }
                }
                if status == Some("interrupted")
                    && !refused.is_empty()
                    && refusal_turns < MAX_REFUSAL_TURNS
                {
                    // Codex stops the turn when a request is cancelled; like a Claude run told
                    // "no", carry on without it.
                    refusal_turns += 1;
                    next_id += 1;
                    let text = format!(
                        "I refused this, so it didn't run: {}. Carry on without it if you can; \
                         otherwise stop and say what you need.",
                        refused.join("; ")
                    );
                    refused.clear();
                    let thread = t.thread_id().unwrap_or_default().to_owned();
                    send(
                        &mut stdin,
                        &start_turn(next_id, &thread, &text, plan_model.as_deref()),
                    )
                    .await?;
                    continue;
                }
                emit(t.turn_completed(params));
                return Ok(());
            }
            (Some("account/rateLimits/updated"), None) => {
                if let Some(params) = msg.get("params") {
                    let readings = live_quota(params);
                    // Sent after each model call, before the tools it asked for run: a clean
                    // point to stop, like anchorleg-mod's stop before the next step (D28).
                    if stopping.is_none()
                        && let (Some(reason), Some(tid), Some(thread)) = (
                            stop_reason(&readings, turn.stop_at),
                            turn_id.as_deref(),
                            t.thread_id(),
                        )
                    {
                        send(
                            &mut stdin,
                            &json!({
                                "id": INTERRUPT,
                                "method": "turn/interrupt",
                                "params": { "threadId": thread, "turnId": tid },
                            }),
                        )
                        .await?;
                        stopping = Some(reason);
                    }
                    if !readings.is_empty() {
                        let _ = events.send(Event::Quota(readings));
                    }
                    if let Some(c) = params.get("rateLimits").and_then(codex::credits) {
                        let _ = events.send(Event::Credits(c));
                    }
                }
            }
            (Some(method), None) => {
                let params = msg.get("params").unwrap_or(&Value::Null);
                if method == "turn/started" {
                    turn_id = params
                        .pointer("/turn/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                t.notification(method, params).into_iter().for_each(emit);
                // Token usage comes after each model call: the same clean point as the quota stop.
                if method == "thread/tokenUsage/updated"
                    && stopping.is_none()
                    && turn.budget_left.is_some_and(|left| t.turn_tokens() >= left)
                    && let (Some(tid), Some(thread)) = (turn_id.as_deref(), t.thread_id())
                {
                    send(
                        &mut stdin,
                        &json!({
                            "id": INTERRUPT,
                            "method": "turn/interrupt",
                            "params": { "threadId": thread, "turnId": tid },
                        }),
                    )
                    .await?;
                    stopping = Some(BUDGET_STOP.to_owned());
                }
            }
            (None, None) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::adapters::codex::Stream;
    use crate::adapters::{AgentSignal, QuotaStatus};

    const ACCEPT: &str = include_str!(
        "../../../../fixtures/codex-0.153.4-app-server-turn-20261011/turn-accept.jsonl"
    );
    const CANCEL: &str = include_str!(
        "../../../../fixtures/codex-0.153.4-app-server-turn-20261011/resume-cancel.jsonl"
    );

    const SUBAGENT: &str =
        include_str!("../../../../fixtures/codex-0.153.4-subagent-20261012/subagent.jsonl");

    const MCP: &str =
        include_str!("../../../../fixtures/codex-0.153.4-mcp-elicitation-20261012/requests.jsonl");

    const AUTO_REVIEW: &str =
        include_str!("../../../../fixtures/codex-0.153.4-auto-review-20261012/auto-review.jsonl");

    #[test]
    fn a_turns_budget_tokens_count_cache_reads_at_a_tenth() {
        let mut t = Translator::default();
        for m in incoming(AUTO_REVIEW) {
            if let Some(method) = m.get("method").and_then(Value::as_str) {
                t.notification(method, &m["params"]);
            }
        }
        // Two model calls: 34,205 input (22,016 of it cached) and 113 output in all.
        assert_eq!(t.turn_tokens(), 34_205 - 22_016 + 113 + 2_201);
    }

    #[test]
    fn codex_reviewer_decisions_show_in_the_conversation() {
        let log: String = translate(AUTO_REVIEW)
            .iter()
            .map(|e| e.to_string() + "\n")
            .collect();
        let messages = crate::conversation::from_log(&log);
        let review = messages
            .iter()
            .find(|m| m.text.starts_with("auto-review"))
            .expect("the reviewer's decision");
        assert_eq!(review.role, crate::conversation::Role::Thinking);
        assert!(
            review
                .text
                .starts_with("auto-review approved `curl -sI https://example.com | head -1` (low risk): The user explicitly authorized"),
            "{}",
            review.text
        );
    }

    #[test]
    fn plan_mode_turns() {
        // Codex 0.153.4 ends a plan-mode turn with a `plan` item (seen on a real run).
        let item = json!({ "type": "plan", "id": "p-plan", "text": "1. Create `hello.txt`.\n" });
        assert_eq!(
            exec_item(&item).unwrap(),
            json!({ "id": "p-plan", "type": "agent_message", "text": "1. Create `hello.txt`.\n" })
        );
        assert_eq!(
            initialize(true)["params"]["capabilities"]["experimentalApi"],
            true
        );
        assert!(initialize(false)["params"].get("capabilities").is_none());
        let r = with_plan_mode(
            turn_request(3, "t1", "plan it", Some("low")),
            "m",
            Some("low"),
        );
        assert_eq!(
            r["params"]["collaborationMode"],
            json!({ "mode": "plan", "settings": { "model": "m", "reasoning_effort": "low", "developer_instructions": null } })
        );
    }

    #[test]
    fn real_mcp_tool_approval_and_form() {
        let reqs: Vec<Value> = MCP
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|m| m["method"] == "mcpServer/elicitation/request")
            .collect();
        let t = Translator::default();
        // The tool-call approval is asked like any tool call.
        let a = t
            .approval("mcpServer/elicitation/request", &reqs[0]["params"])
            .unwrap();
        assert_eq!(a.tool, "mcp__picker__pick_project");
        assert_eq!(a.input, json!({}));
        // The server's own form is asked as questions.
        assert!(
            t.approval("mcpServer/elicitation/request", &reqs[1]["params"])
                .is_none()
        );
        let qs = questions::from_elicitation(&reqs[1]["params"]);
        assert_eq!(qs.len(), 1);
        let answers: questions::Answers =
            [("project".to_owned(), questions::parse_answer(&qs[0], "2"))].into();
        assert_eq!(
            questions::elicitation_response(&reqs[1]["params"], &answers),
            json!({ "action": "accept", "content": { "project": "OPS" } })
        );
    }

    /// The messages Codex sent in a capture.
    fn incoming(capture: &str) -> Vec<Value> {
        capture
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v["dir"] == "in")
            .map(|v| v["msg"].clone())
            .collect()
    }

    /// Translate a capture the way `drive` does, without the approvals.
    pub(crate) fn translate(capture: &str) -> Vec<Value> {
        let mut t = Translator::default();
        let mut out = Vec::new();
        for m in incoming(capture) {
            if let (Some(method), None, Some(thread)) = (
                m.get("method").and_then(Value::as_str),
                m.get("id"),
                t.other_thread(&m),
            ) {
                out.extend(t.subagent_notification(&thread, method, &m["params"]));
                continue;
            }
            match (m.get("method").and_then(Value::as_str), m.get("id")) {
                (None, Some(id)) if id == THREAD => out.extend(t.thread_started(&m["result"])),
                (Some("turn/completed"), None) => out.push(t.turn_completed(&m["params"])),
                (Some(method), None) => out.extend(t.notification(method, &m["params"])),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn a_real_turn_reads_like_codex_exec() {
        let events = translate(ACCEPT);
        let mut stream = Stream::default();
        let signals: Vec<_> = events
            .iter()
            .flat_map(|e| stream.feed(&e.to_string()))
            .collect();
        assert_eq!(
            signals,
            [
                AgentSignal::Started {
                    session_id: "01a12ad3-384a-7890-b1fb-184383f3668f".into(),
                    model: None
                },
                // Three model calls, summed: cached input is counted apart from new input.
                AgentSignal::Usage(crate::adapters::Usage {
                    model: None,
                    input_tokens: 12083,
                    output_tokens: 239,
                    cache_read_tokens: 38144,
                    cache_write_tokens: 0,
                    cost_usd: None,
                }),
                AgentSignal::Finished {
                    ok: true,
                    session_id: Some("01a12ad3-384a-7890-b1fb-184383f3668f".into()),
                    text: Some("HTTP/2 200".into())
                }
            ]
        );
        let kinds: Vec<&str> = events
            .iter()
            .filter(|e| e["type"] == "item.completed")
            .filter_map(|e| e.pointer("/item/type").and_then(Value::as_str))
            .collect();
        // The command was also shown as running while it ran.
        assert!(
            events
                .iter()
                .any(|e| e["type"] == "item.started" && e["item"]["type"] == "command_execution")
        );
        assert_eq!(
            kinds,
            [
                "agent_message",
                "file_change",
                "command_execution",
                "agent_message"
            ]
        );
        let change = events
            .iter()
            .find(|e| e.pointer("/item/type") == Some(&"file_change".into()))
            .unwrap();
        assert_eq!(change.pointer("/item/changes/0/kind"), Some(&"add".into()));
        assert_eq!(
            events.last().unwrap().pointer("/usage/input_tokens"),
            Some(&16892.into())
        );
    }

    #[test]
    fn a_subagents_thread_is_its_progress_not_the_turn() {
        // A real `multi_agent` turn: the subagent's thread streams on the same connection,
        // with its own `turn/completed` before the session's.
        let events = translate(SUBAGENT);
        let finished: Vec<&Value> = events
            .iter()
            .filter(|e| e["type"] == "turn.completed")
            .collect();
        assert_eq!(finished.len(), 1);
        let child = "01a13025-d58e-7162-863a-2fdcf55e0020";
        let spawn = events
            .iter()
            .find(|e| e["type"] == "item.completed" && e["item"]["type"] == "agent_spawn")
            .unwrap();
        assert_eq!(spawn["item"]["agents"], json!([child]));
        assert_eq!(
            events.iter().rfind(|e| e["type"] == "subagent").unwrap(),
            &json!({ "type": "subagent", "agent": child, "status": "completed" })
        );
        // Its messages aren't the session's: only the parent's answer is.
        let mut stream = Stream::default();
        let signals: Vec<_> = events
            .iter()
            .flat_map(|e| stream.feed(&e.to_string()))
            .collect();
        let Some(AgentSignal::Finished { text, .. }) = signals.last() else {
            panic!("{signals:?}");
        };
        assert_eq!(
            text.as_deref(),
            Some("a.txt  \nb.txt  \nprobe.err  \nsubagent.jsonl")
        );
        // Its tokens count toward the turn's: three parent calls and two of the subagent's.
        let Some(AgentSignal::Usage(u)) =
            signals.iter().rfind(|s| matches!(s, AgentSignal::Usage(_)))
        else {
            panic!("{signals:?}");
        };
        assert!(u.input_tokens + u.cache_read_tokens > 56_664, "{u:?}");

        let log: String = events.iter().map(|e| format!("{e}\n")).collect();
        let subs = crate::conversation::subagents(&log);
        assert_eq!(subs.len(), 1);
        assert_eq!(
            subs[0].name,
            "List the files in the current folder and return the filenames only."
        );
        assert!(subs[0].done);
        assert_eq!(subs[0].steps, 1);
    }

    #[test]
    fn approval_requests_become_questions() {
        let req = incoming(ACCEPT)
            .into_iter()
            .find(|m| m["method"] == "item/commandExecution/requestApproval")
            .unwrap();
        let t = Translator::default();
        let a = t
            .approval(req["method"].as_str().unwrap(), &req["params"])
            .unwrap();
        assert_eq!(a.tool, "Bash");
        assert_eq!(a.input["command"], "curl -sI https://example.com | head -1");
        assert_eq!(a.input["cwd"], "/tmp/scratch/codexturn/work");
        assert_eq!(
            a.reason.as_deref(),
            Some("May I run the exact curl command with network access?")
        );
        // Codex offered no "decline" here, so a refusal cancels the turn.
        assert_eq!(a.refuse, "cancel");

        let mut t = Translator::default();
        t.notification(
            "item/started",
            &json!({ "item": { "type": "fileChange", "id": "f1", "changes": [{ "path": "/w/a.rs", "kind": { "type": "update" }, "diff": "@@ -1 +1 @@\n-a\n+b\n" }] } }),
        );
        let a = t
            .approval(
                "item/fileChange/requestApproval",
                &json!({ "itemId": "f1", "threadId": "t", "turnId": "u", "startedAtMs": 1 }),
            )
            .unwrap();
        assert_eq!((a.tool.as_str(), a.refuse), ("Edit", "decline"));
        assert_eq!(a.input["file_path"], "/w/a.rs");
        assert_eq!(a.input["diff"], "@@ -1 +1 @@\n-a\n+b\n");
        assert!(
            t.approval("item/tool/requestUserInput", &json!({}))
                .is_none()
        );
    }

    #[test]
    fn a_cancelled_turn_and_limit_errors() {
        let events = translate(CANCEL);
        // A stopped turn still reports the tokens it used.
        assert_eq!(
            events.last().unwrap(),
            &json!({ "type": "turn.failed", "error": { "message": "the turn was stopped" },
                "turn_usage": { "input_tokens": 16932, "cached_input_tokens": 16128,
                    "cache_write_input_tokens": 0, "output_tokens": 154 } })
        );
        let t = Translator::default();
        let failed = t.turn_completed(&json!({ "turn": { "status": "failed", "error": {
            "message": "You've hit your usage limit.", "codexErrorInfo": "usageLimitExceeded" } } }));
        assert_eq!(failed["error"]["codex_error_info"], "usageLimitExceeded");
        let mut stream = Stream::default();
        assert!(matches!(
            stream.feed(&failed.to_string()).as_slice(),
            [AgentSignal::LimitHit { .. }]
        ));
        let retry = Translator::default().notification(
            "error",
            &json!({ "error": { "message": "stream disconnected" }, "willRetry": true }),
        );
        assert!(matches!(
            stream.feed(&retry[0].to_string()).as_slice(),
            [AgentSignal::Retrying { .. }]
        ));
    }

    #[test]
    fn quota_updates_are_readings() {
        let update = incoming(ACCEPT)
            .into_iter()
            .find(|m| m["method"] == "account/rateLimits/updated")
            .unwrap();
        let readings = live_quota(&update["params"]);
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].used, Some(0.01));
        assert_ne!(readings[0].status, Some(QuotaStatus::Rejected));
    }

    #[test]
    fn stops_when_a_window_reaches_the_threshold() {
        use crate::adapters::Window;
        let r = |w: Window, used| QuotaReading {
            window: Some(w),
            status: None,
            used: Some(used),
            resets_at: None,
        };
        let readings = [r(Window::FiveHour, 0.92), r(Window::SevenDay, 0.4)];
        assert_eq!(
            stop_reason(&readings, 0.9).as_deref(),
            Some("five_hour quota at 92%")
        );
        assert_eq!(stop_reason(&readings, 0.95), None);
        assert_eq!(stop_reason(&readings, 0.0), None);
    }

    #[test]
    fn requests_carry_the_settings() {
        let turn = Turn {
            cwd: "/w".into(),
            prompt: "hi".into(),
            resume: Some("t1".into()),
            model: Some("gpt-5.6-luna".into()),
            effort: Some("low".into()),
            sandbox: Some("workspace-write".into()),
            approval: Some("on-request".into()),
            stop_at: 0.9,
            images: Vec::new(),
            plan: false,
            review: false,
            fork: false,
            fork_at: None,
            auto_review: true,
            budget_left: None,
            output_schema: None,
            instructions: None,
            config: None,
            name: None,
        };
        let r = thread_request(&turn);
        assert_eq!(r["method"], "thread/resume");
        assert_eq!(r["params"]["threadId"], "t1");
        assert_eq!(r["params"]["approvalPolicy"], "on-request");
        assert_eq!(r["params"]["sandbox"], "workspace-write");
        assert_eq!(r["params"]["approvalsReviewer"], "auto_review");
        let r = turn_request(3, "t1", "hi", Some("low"));
        assert_eq!(r["params"]["effort"], "low");
        assert_eq!(r["params"]["input"][0]["text"], "hi");
    }
}
