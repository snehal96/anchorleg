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

use super::QuotaReading;
use super::codex::live_quota;

/// `codex app-server` request ids anchorleg uses.
const INITIALIZE: i64 = 1;
const THREAD: i64 = 2;
const TURN: i64 = 3;
/// `turn/interrupt` for a soft stop; far above the turn ids.
const INTERRUPT: i64 = 1000;

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
}

/// What the driver passes back to the supervisor.
#[derive(Debug)]
pub enum Event {
    /// A `codex exec --json` event line, to log and parse like before.
    Line(String),
    /// Current quota, pushed by Codex after model calls.
    Quota(Vec<QuotaReading>),
    /// `thread/resume` of a thread this login doesn't have.
    NotFound,
    /// anchorleg stopped the turn because quota reached `--stop-at` (why, for the run log).
    Stopped(String),
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
pub fn initialize() -> Value {
    json!({
        "id": INITIALIZE,
        "method": "initialize",
        "params": { "clientInfo": { "name": "anchorleg", "version": env!("CARGO_PKG_VERSION") } },
    })
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
    match &turn.resume {
        Some(id) => {
            params["threadId"] = id.as_str().into();
            params["excludeTurns"] = true.into();
            json!({ "id": THREAD, "method": "thread/resume", "params": params })
        }
        None => json!({ "id": THREAD, "method": "thread/start", "params": params }),
    }
}

pub fn turn_request(id: i64, thread: &str, text: &str, effort: Option<&str>) -> Value {
    let mut params = json!({
        "threadId": thread,
        "input": [{ "type": "text", "text": text }],
    });
    if let Some(e) = effort {
        params["effort"] = e.into();
    }
    json!({ "id": id, "method": "turn/start", "params": params })
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

/// Translates app-server messages for one launch.
#[derive(Debug, Default)]
pub struct Translator {
    thread_id: Option<String>,
    /// The latest model call's token usage (`last` of `thread/tokenUsage/updated`).
    last_usage: Option<Value>,
    /// Files of each file change, from `item/started`, for its approval request.
    changes: HashMap<String, Vec<String>>,
}

impl Translator {
    pub fn thread_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
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
                if let Some(item) = params.get("item")
                    && item.get("type").and_then(Value::as_str) == Some("fileChange")
                    && let Some(id) = item.get("id").and_then(Value::as_str)
                {
                    self.changes.insert(id.to_owned(), change_paths(item));
                }
                Vec::new()
            }
            "item/completed" => params
                .get("item")
                .and_then(exec_item)
                .map(|item| json!({ "type": "item.completed", "item": item }))
                .into_iter()
                .collect(),
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
                json!({
                    "type": "turn.completed",
                    "usage": {
                        "input_tokens": n("inputTokens"),
                        "cached_input_tokens": n("cachedInputTokens"),
                        "output_tokens": n("outputTokens"),
                    },
                })
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
                if let Some(code) = error.get("codexErrorInfo").and_then(error_code) {
                    failed["error"]["codex_error_info"] = code.into();
                }
                failed
            }
        }
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
                if let Some(root) = params.get("grantRoot").and_then(Value::as_str) {
                    input["grant_root"] = root.into();
                }
                ("Edit".to_owned(), input)
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

/// An app-server item as the `codex exec --json` item anchorleg's readers know. `None` for
/// items they don't use (the prompt, reasoning, …).
fn exec_item(item: &Value) -> Option<Value> {
    let id = item.get("id").cloned().unwrap_or(Value::Null);
    let status = item.get("status").cloned().unwrap_or(Value::Null);
    match item.get("type").and_then(Value::as_str)? {
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
    let mut id: Option<i64> = None;
    loop {
        let mut req = json!({
            "run_id": run_id,
            "tool": approval.tool,
            "input": approval.input,
            "reason": approval.reason,
        });
        if let Some(id) = id {
            req["id"] = id.into();
        }
        let child = tokio::process::Command::new(bin)
            .args(["permission", "ask", "--json"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else { return false };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(req.to_string().as_bytes()).await;
        }
        let Ok(out) = child.wait_with_output().await else {
            return false;
        };
        let answer: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        match answer.get("decision").and_then(Value::as_str) {
            Some("allow") => return true,
            Some("pending") => id = answer.get("id").and_then(Value::as_i64),
            _ => return false,
        }
        if id.is_none() {
            return false;
        }
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

    send(&mut stdin, &initialize()).await?;
    while let Some(line) = lines.next_line().await? {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").cloned();
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
                            let thread = t.thread_id().unwrap_or_default().to_owned();
                            send(
                                &mut stdin,
                                &turn_request(
                                    next_id,
                                    &thread,
                                    &turn.prompt,
                                    turn.effort.as_deref(),
                                ),
                            )
                            .await?;
                        }
                        (None, None) => return Ok(()),
                    }
                } else if id == INTERRUPT {
                    // The answer to a soft stop; `turn/completed` follows.
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
                        &turn_request(next_id, &thread, &text, turn.effort.as_deref()),
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
            }
            (None, None) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::codex::Stream;
    use crate::adapters::{AgentSignal, QuotaStatus};

    const ACCEPT: &str = include_str!(
        "../../../../fixtures/codex-0.153.4-app-server-turn-20261011/turn-accept.jsonl"
    );
    const CANCEL: &str = include_str!(
        "../../../../fixtures/codex-0.153.4-app-server-turn-20261011/resume-cancel.jsonl"
    );

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
    fn translate(capture: &str) -> Vec<Value> {
        let mut t = Translator::default();
        let mut out = Vec::new();
        for m in incoming(capture) {
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
                AgentSignal::Finished {
                    ok: true,
                    session_id: Some("01a12ad3-384a-7890-b1fb-184383f3668f".into()),
                    text: Some("HTTP/2 200".into())
                }
            ]
        );
        let kinds: Vec<&str> = events
            .iter()
            .filter_map(|e| e.pointer("/item/type").and_then(Value::as_str))
            .collect();
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
            &json!({ "item": { "type": "fileChange", "id": "f1", "changes": [{ "path": "/w/a.rs", "kind": { "type": "update" } }] } }),
        );
        let a = t
            .approval(
                "item/fileChange/requestApproval",
                &json!({ "itemId": "f1", "threadId": "t", "turnId": "u", "startedAtMs": 1 }),
            )
            .unwrap();
        assert_eq!((a.tool.as_str(), a.refuse), ("Edit", "decline"));
        assert_eq!(a.input["file_path"], "/w/a.rs");
        assert!(
            t.approval("item/tool/requestUserInput", &json!({}))
                .is_none()
        );
    }

    #[test]
    fn a_cancelled_turn_and_limit_errors() {
        let events = translate(CANCEL);
        assert_eq!(
            events.last().unwrap(),
            &json!({ "type": "turn.failed", "error": { "message": "the turn was stopped" } })
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
        };
        let r = thread_request(&turn);
        assert_eq!(r["method"], "thread/resume");
        assert_eq!(r["params"]["threadId"], "t1");
        assert_eq!(r["params"]["approvalPolicy"], "on-request");
        assert_eq!(r["params"]["sandbox"], "workspace-write");
        let r = turn_request(3, "t1", "hi", Some("low"));
        assert_eq!(r["params"]["effort"], "low");
        assert_eq!(r["params"]["input"][0]["text"], "hi");
    }
}
