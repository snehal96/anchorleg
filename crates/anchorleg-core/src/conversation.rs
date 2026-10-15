//! A run's conversation for people to read, from its raw log (`<store>/runs/run-<id>.jsonl`):
//! the CLI's stream-json lines plus anchorleg's own `anchorleg_user` / `anchorleg` lines.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// What the person sent.
    User,
    /// The agent's visible text.
    Agent,
    /// A tool call the agent made, summarised.
    Tool,
    /// The start of a tool's output, when it failed.
    ToolError,
    /// The first lines of a tool's output.
    Output,
    /// The model's reasoning: its summary or thinking text, else how much it thought.
    Thinking,
    /// Something anchorleg did: switched, paused, stopped.
    Anchorleg,
    /// The run ended with an error.
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// In a session view: the run the message is from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<i64>,
    /// In a session view, where the session can be forked from (`anchorleg rewind RUN --at`,
    /// `anchorleg run --fork RUN --at`): for a message the person sent, the point just before
    /// it; for the reply that ends a turn, the point just after it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

fn msg(role: Role, text: impl Into<String>) -> Message {
    Message {
        role,
        text: text.into(),
        run: None,
        at: None,
    }
}

/// What `/compact` leaves in the conversation.
const COMPACTED: &str = "Context compacted: the session goes on from a summary.";

/// Read a run log. Lines that aren't JSON or aren't for people are skipped.
pub fn from_log(log: &str) -> Vec<Message> {
    let mut out = Vec::new();
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let text_of = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default();
        if v.get("event").is_some() {
            agy_event(&v, &mut out);
            continue;
        }
        // A Claude subagent's own steps: shown as its progress (`subagents`), not as the
        // session's. Its result comes back as the Agent call's output.
        if is_sidechain(&v) {
            continue;
        }
        match v.get("type").and_then(Value::as_str) {
            // `relay*`: logs written before the rename.
            Some("anchorleg_user" | "relay_user") => out.push(msg(Role::User, text_of("text"))),
            Some("anchorleg" | "relay") => out.push(msg(Role::Anchorleg, text_of("text"))),
            Some("assistant") => {
                for block in blocks(&v) {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let t = block.get("text").and_then(Value::as_str).unwrap_or("");
                            if !t.trim().is_empty() {
                                out.push(msg(Role::Agent, t.trim()));
                            }
                        }
                        Some("tool_use") => out.push(msg(Role::Tool, tool_summary(block))),
                        Some("thinking") => {
                            let t = block.get("thinking").and_then(Value::as_str).unwrap_or("");
                            if !t.trim().is_empty() {
                                out.push(msg(Role::Thinking, t.trim()));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for block in blocks(&v) {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
                    let text = content_text(block);
                    if failed {
                        out.push(msg(Role::ToolError, first_line(&text, 160)));
                    } else if let Some(o) = output_excerpt(&text) {
                        out.push(msg(Role::Output, o));
                    }
                }
            }
            // A hook of the person's that did something visible: blocked, failed or said
            // something. Quiet successes stay out of the way.
            Some("system") if text_of("subtype") == "hook_response" => {
                let said = [text_of("stderr"), text_of("output"), text_of("stdout")]
                    .into_iter()
                    .find(|t| !t.trim().is_empty())
                    .map(|t| first_line(t, 160));
                let code = v.get("exit_code").and_then(Value::as_i64).unwrap_or(0);
                let outcome = text_of("outcome");
                let ok = code == 0 && matches!(outcome, "" | "success");
                if !ok || said.is_some() {
                    let what = if ok {
                        String::new()
                    } else if outcome.is_empty() || outcome == "success" {
                        format!(" exited {code}")
                    } else {
                        format!(" {outcome}")
                    };
                    let text = format!(
                        "hook {}{what}{}",
                        text_of("hook_name"),
                        said.map_or(String::new(), |s| format!(": {s}"))
                    );
                    out.push(msg(if ok { Role::Thinking } else { Role::ToolError }, text));
                }
            }
            // Claude's `/compact`.
            Some("system") if text_of("subtype") == "compact_boundary" => {
                let tokens = |k: &str| {
                    v.pointer(&format!("/compact_metadata/{k}"))
                        .and_then(Value::as_u64)
                };
                out.push(msg(
                    Role::Anchorleg,
                    match (tokens("pre_tokens"), tokens("post_tokens")) {
                        (Some(a), Some(b)) => format!(
                            "{COMPACTED} ({}k → {}k tokens)",
                            (a + 500) / 1000,
                            (b + 500) / 1000
                        ),
                        _ => COMPACTED.to_owned(),
                    },
                ));
            }
            // `codex exec --json`
            Some("item.completed") => codex_item(&v, &mut out),
            Some("turn.failed") => {
                let text = v
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("the turn failed");
                out.push(msg(Role::Error, text));
            }
            Some("result") if v.get("is_error").and_then(Value::as_bool) == Some(true) => {
                let text = match v.get("result").and_then(Value::as_str) {
                    Some(t) if !t.is_empty() => t.to_owned(),
                    _ => v
                        .get("errors")
                        .and_then(Value::as_array)
                        .map(|e| {
                            e.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join("; ")
                        })
                        .unwrap_or_else(|| "the run ended with an error".to_owned()),
                };
                out.push(msg(Role::Error, text));
            }
            _ => {}
        }
    }
    out
}

/// `agy` stream-json: finished tool steps, then the turn's response or error.
fn agy_event(v: &Value, out: &mut Vec<Message>) {
    match v.get("event").and_then(Value::as_str) {
        Some("step_update") => {
            if let Some((tool, arg)) = crate::adapters::agy::tool_step(v) {
                let input = match tool {
                    "Bash" => serde_json::json!({ "command": arg }),
                    _ => serde_json::json!({ "file_path": arg }),
                };
                out.push(msg(Role::Tool, describe_tool(tool, &input)));
                if tool == "Bash"
                    && let Some(o) = v
                        .pointer("/step_update/tool_info/output")
                        .and_then(Value::as_str)
                        .and_then(output_excerpt)
                {
                    out.push(msg(Role::Output, o));
                }
            }
            // agy doesn't stream its thinking, only how much there was.
            let step = v.get("step_update").unwrap_or(&Value::Null);
            if step.get("step_type").and_then(Value::as_str) == Some("subagent")
                && step.get("state").and_then(Value::as_str) == Some("DONE")
            {
                for a in agy_subagents(step) {
                    out.push(msg(
                        Role::Tool,
                        describe_tool("Agent", &serde_json::json!({ "description": a.name })),
                    ));
                }
            }
            if step.get("step_type").and_then(Value::as_str) == Some("agent_response")
                && step.get("state").and_then(Value::as_str) == Some("DONE")
                && let Some(n) = step
                    .pointer("/usage/thinking_tokens")
                    .and_then(Value::as_u64)
                    .filter(|n| *n > 0)
            {
                out.push(msg(Role::Thinking, format!("thought for {n} tokens")));
            }
        }
        Some("result") => {
            let field = |k: &str| v.pointer(k).and_then(Value::as_str).unwrap_or_default();
            if field("/result/status") == "SUCCESS" {
                if !field("/result/response").trim().is_empty() {
                    out.push(msg(Role::Agent, field("/result/response").trim()));
                }
            } else {
                let error = field("/result/error");
                let error = if error.is_empty() {
                    "the turn failed"
                } else {
                    error
                };
                out.push(msg(Role::Error, error));
            }
        }
        _ => {}
    }
}

fn codex_item(v: &Value, out: &mut Vec<Message>) {
    let Some(item) = v.get("item") else { return };
    let field = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or_default();
    match field("type") {
        "agent_message" if !field("text").trim().is_empty() => {
            out.push(msg(Role::Agent, field("text").trim()));
        }
        "reasoning" if !field("text").trim().is_empty() => {
            out.push(msg(Role::Thinking, field("text").trim()));
        }
        "context_compaction" => out.push(msg(Role::Anchorleg, COMPACTED)),
        "hook" => {
            let ok = field("status") == "completed";
            let said = first_line(field("text"), 160);
            out.push(msg(
                if ok { Role::Thinking } else { Role::ToolError },
                format!(
                    "hook {}{}{}",
                    field("event"),
                    if ok {
                        String::new()
                    } else {
                        format!(" {}", field("status"))
                    },
                    if said.is_empty() {
                        String::new()
                    } else {
                        format!(": {said}")
                    }
                ),
            ));
        }
        "auto_review" => {
            let risk = item
                .get("risk")
                .and_then(Value::as_str)
                .map(|r| format!(" ({r} risk)"))
                .unwrap_or_default();
            let (role, verdict) = match field("status") {
                "approved" => (Role::Thinking, "approved"),
                "denied" => (Role::ToolError, "refused"),
                "timedOut" => (Role::ToolError, "timed out on"),
                _ => return,
            };
            let why = first_line(field("rationale"), 200);
            out.push(msg(
                role,
                format!("auto-review {verdict} `{}`{risk}: {why}", field("action")),
            ));
        }
        "agent_spawn" => out.push(msg(
            Role::Tool,
            describe_tool(
                "Agent",
                &serde_json::json!({ "description": field("prompt") }),
            ),
        )),
        "agent_wait" => {
            let answers: Vec<&str> = item
                .get("states")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|m| m.values())
                .filter_map(|s| s.get("message").and_then(Value::as_str))
                .collect();
            if let Some(o) = output_excerpt(&answers.join("\n")) {
                out.push(msg(Role::Output, o));
            }
        }
        "command_execution" => {
            let cmd = crate::adapters::codex::unwrap_command(field("command"));
            out.push(msg(
                Role::Tool,
                describe_tool("Bash", &serde_json::json!({ "command": cmd })),
            ));
            let code = item.get("exit_code").and_then(Value::as_i64);
            if code.is_some_and(|c| c != 0) {
                out.push(msg(
                    Role::ToolError,
                    first_line(field("aggregated_output"), 160),
                ));
            } else if let Some(o) = output_excerpt(field("aggregated_output")) {
                out.push(msg(Role::Output, o));
            }
        }
        "file_change" => {
            for change in item
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let path = change.get("path").and_then(Value::as_str).unwrap_or("?");
                let name = match change.get("kind").and_then(Value::as_str) {
                    Some("add") => "Write",
                    _ => "Edit",
                };
                out.push(msg(
                    Role::Tool,
                    describe_tool(name, &serde_json::json!({ "file_path": path })),
                ));
            }
        }
        _ => {}
    }
}

/// The first lines of a tool's output, with how many more there are. `None` when empty.
pub fn output_excerpt(text: &str) -> Option<String> {
    const LINES: usize = 4;
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let mut shown: Vec<String> = lines
        .iter()
        .take(LINES)
        .map(|l| {
            if l.chars().count() > 160 {
                format!("{}…", l.chars().take(159).collect::<String>())
            } else {
                (*l).to_owned()
            }
        })
        .collect();
    if lines.len() > LINES {
        shown.push(format!("… +{} lines", lines.len() - LINES));
    }
    Some(shown.join("\n"))
}

/// What the agent is doing right now, from a run's log: the last tool call that hasn't finished,
/// and its output so far (Codex streams it; Claude and agy send it when the call is done).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Running {
    /// `Bash  cargo test`, as [`describe_tool`] puts it.
    pub what: String,
    /// The last lines of its output so far.
    pub output: String,
}

pub fn running(log: &str) -> Option<Running> {
    // Open calls by id, in the order they started; a finished one is removed.
    let mut open: Vec<(String, String, String)> = Vec::new();
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if is_sidechain(&v) {
            continue;
        }
        let str_at = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or_default();
        match (
            v.get("type").and_then(Value::as_str),
            v.get("event").and_then(Value::as_str),
        ) {
            (Some("assistant"), _) => {
                for b in blocks(&v).filter(|b| b["type"] == "tool_use") {
                    let id = b.get("id").and_then(Value::as_str).unwrap_or_default();
                    open.push((id.to_owned(), tool_summary(b), String::new()));
                }
            }
            (Some("user"), _) => {
                for b in blocks(&v).filter(|b| b["type"] == "tool_result") {
                    let id = b.get("tool_use_id").and_then(Value::as_str);
                    open.retain(|(o, _, _)| Some(o.as_str()) != id);
                }
            }
            (Some("item.started"), _) if str_at("/item/type") == "command_execution" => {
                let cmd = crate::adapters::codex::unwrap_command(str_at("/item/command"));
                open.push((
                    str_at("/item/id").to_owned(),
                    describe_tool("Bash", &serde_json::json!({ "command": cmd })),
                    String::new(),
                ));
            }
            (Some("item.started"), _) if str_at("/item/type") == "agent_spawn" => {
                let what = describe_tool(
                    "Agent",
                    &serde_json::json!({ "description": str_at("/item/prompt") }),
                );
                open.push((str_at("/item/id").to_owned(), what, String::new()));
            }
            (Some("item.started"), _) if str_at("/item/type") == "agent_wait" => {
                let n = v
                    .pointer("/item/agents")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                let what = format!("Wait  for {n} agent{}", if n == 1 { "" } else { "s" });
                open.push((str_at("/item/id").to_owned(), what, String::new()));
            }
            (Some("item.output_delta"), _) => {
                if let Some(o) = open.iter_mut().find(|o| o.0 == str_at("/item_id")) {
                    o.2.push_str(str_at("/delta"));
                }
            }
            (Some("item.completed"), _) => {
                let id = str_at("/item/id");
                open.retain(|o| o.0 != id);
            }
            (Some("result" | "turn.completed" | "turn.failed"), _) | (_, Some("result")) => {
                open.clear();
            }
            (_, Some("step_update")) if str_at("/step_update/step_type") == "tool" => {
                let id = v
                    .pointer("/step_update/step_index")
                    .map(Value::to_string)
                    .unwrap_or_default();
                open.retain(|o| o.0 != id);
                if str_at("/step_update/state") == "ACTIVE" {
                    let name = str_at("/step_update/tool_info/name");
                    let params = v
                        .pointer("/step_update/tool_info/parameters")
                        .cloned()
                        .unwrap_or(Value::Null);
                    let what = match params.get("CommandLine").and_then(Value::as_str) {
                        Some(cmd) => describe_tool("Bash", &serde_json::json!({ "command": cmd })),
                        None => describe_tool(name, &params),
                    };
                    open.push((id, what, String::new()));
                }
            }
            _ => {}
        }
    }
    let (_, what, output) = open.pop()?;
    let tail: Vec<&str> = output
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    let output = tail[tail.len().saturating_sub(4)..].join("\n");
    Some(Running { what, output })
}

/// A line from a Claude subagent's own conversation.
fn is_sidechain(v: &Value) -> bool {
    v.get("isSidechain").and_then(Value::as_bool) == Some(true)
        || v.get("parent_tool_use_id").is_some_and(|p| !p.is_null())
}

/// A subagent the session started, and how far it got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Subagent {
    /// Claude: the Agent call's id; Codex: the subagent's thread; agy: its conversation.
    pub id: String,
    /// What it was asked to do (its description, role or prompt).
    pub name: String,
    pub done: bool,
    /// Tool calls it has made.
    pub steps: usize,
    /// The call it's in (or last made), as `describe_tool` puts it.
    pub doing: Option<String>,
    /// agy: the subagent's own transcript, which `read_transcripts` reads for its progress
    /// (agy runs subagents on their own; their steps aren't in the session's stream).
    #[serde(skip)]
    pub transcript: Option<std::path::PathBuf>,
}

impl Subagent {
    fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: first_line(name, 80),
            done: false,
            steps: 0,
            doing: None,
            transcript: None,
        }
    }
}

/// The subagents in a run log, in the order they started: Claude's Agent calls (their steps
/// carry `parent_tool_use_id`), Codex's `multi_agent` subagents (anchorleg's `subagent`
/// events) and agy's `invoke_subagent` (see `read_transcripts`).
pub fn subagents(log: &str) -> Vec<Subagent> {
    let mut subs: Vec<Subagent> = Vec::new();
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let str_at = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or_default();
        if v.get("event").and_then(Value::as_str) == Some("step_update") {
            let step = &v["step_update"];
            if step["step_type"] == "subagent" && step["state"] == "DONE" {
                subs.extend(agy_subagents(step));
            }
            continue;
        }
        if let Some(parent) = v.get("parent_tool_use_id").and_then(Value::as_str) {
            if let Some(s) = subs.iter_mut().find(|s| s.id == parent)
                && v["type"] == "assistant"
            {
                for b in blocks(&v).filter(|b| b["type"] == "tool_use") {
                    s.steps += 1;
                    s.doing = Some(tool_summary(b));
                }
            }
            continue;
        }
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                for b in blocks(&v).filter(|b| b["type"] == "tool_use") {
                    if matches!(b["name"].as_str(), Some("Agent" | "Task")) {
                        let input = &b["input"];
                        let name = input["description"]
                            .as_str()
                            .or_else(|| input["prompt"].as_str())
                            .unwrap_or("subagent");
                        subs.push(Subagent::new(b["id"].as_str().unwrap_or_default(), name));
                    }
                }
            }
            Some("user") => {
                for b in blocks(&v).filter(|b| b["type"] == "tool_result") {
                    if let Some(s) = subs.iter_mut().find(|s| b["tool_use_id"] == s.id.as_str()) {
                        s.done = true;
                        s.doing = None;
                    }
                }
            }
            Some("item.completed") if str_at("/item/type") == "agent_spawn" => {
                for a in v["item"]["agents"].as_array().into_iter().flatten() {
                    if let Some(id) = a.as_str() {
                        subs.push(Subagent::new(id, str_at("/item/prompt")));
                    }
                }
            }
            Some("subagent") => {
                let Some(s) = subs.iter_mut().find(|s| v["agent"] == s.id.as_str()) else {
                    continue;
                };
                if v.get("status").is_some() {
                    s.done = true;
                    s.doing = None;
                    continue;
                }
                let item = &v["event"]["item"];
                let started = v["event"]["type"] == "item.started";
                let what = match item["type"].as_str() {
                    Some("command_execution") => Some(describe_tool(
                        "Bash",
                        &serde_json::json!({
                            "command": crate::adapters::codex::unwrap_command(
                                item["command"].as_str().unwrap_or_default()
                            )
                        }),
                    )),
                    Some("file_change") => Some("Edit  files".to_owned()),
                    Some("mcp_tool_call") => Some(describe_tool(
                        item["tool"].as_str().unwrap_or("tool"),
                        &Value::Null,
                    )),
                    _ => None,
                };
                if let Some(what) = what {
                    if started || item["type"] != "command_execution" {
                        s.steps += 1;
                    }
                    s.doing = Some(what);
                }
            }
            _ => {}
        }
    }
    subs
}

/// The subagents an agy `subagent` step started.
fn agy_subagents(step: &Value) -> Vec<Subagent> {
    step.pointer("/subagent_info/subagents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let id = a.get("conversation_id").and_then(Value::as_str)?;
            let name = a
                .get("role")
                .and_then(Value::as_str)
                .or_else(|| a.get("initial_prompt").and_then(Value::as_str))
                .unwrap_or("subagent");
            let mut s = Subagent::new(id, name);
            s.transcript = a
                .get("log_uri")
                .and_then(Value::as_str)
                .and_then(|u| u.strip_prefix("file://"))
                .map(std::path::PathBuf::from);
            Some(s)
        })
        .collect()
}

/// Fill in agy subagents' progress from their transcripts (one JSON object per step). A
/// transcript that can't be read leaves the subagent as it was.
pub fn read_transcripts(subs: &mut [Subagent]) {
    for s in subs.iter_mut() {
        if let Some(text) = s
            .transcript
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
        {
            agy_progress(s, &text);
        }
    }
}

/// An agy subagent's transcript: `PLANNER_RESPONSE` steps carry its tool calls (arguments as
/// JSON strings); one with no tool calls is its last word.
fn agy_progress(s: &mut Subagent, transcript: &str) {
    s.steps = 0;
    s.doing = None;
    s.done = false;
    for line in transcript.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["type"] != "PLANNER_RESPONSE" {
            continue;
        }
        let calls = v["tool_calls"].as_array().filter(|c| !c.is_empty());
        let Some(calls) = calls else {
            s.done = v["status"] == "DONE";
            continue;
        };
        s.done = false;
        for c in calls {
            let name = c["name"].as_str().unwrap_or("tool");
            // Each argument is itself JSON: `"\"ls -la\""`.
            let args: serde_json::Map<String, Value> = c["args"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| {
                    let decoded = v
                        .as_str()
                        .and_then(|t| serde_json::from_str::<Value>(t).ok())
                        .unwrap_or_else(|| v.clone());
                    (k.clone(), decoded)
                })
                .collect();
            let args = Value::Object(args);
            s.steps += 1;
            s.doing = Some(match crate::adapters::agy::tool_call(name, &args) {
                Some(("Bash", cmd)) => {
                    describe_tool("Bash", &serde_json::json!({ "command": cmd }))
                }
                Some((tool, path)) => {
                    describe_tool(tool, &serde_json::json!({ "file_path": path }))
                }
                None => describe_tool(name, &Value::Null),
            });
        }
    }
}

/// The slash commands the CLI offered in this log's last `init` (skills, custom commands and
/// built-ins that work in `-p`), without the terminal-only ones.
pub fn slash_commands(log: &str) -> Option<Vec<String>> {
    let init = log
        .lines()
        .filter(|l| l.contains("\"init\""))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .rfind(|v| v.get("subtype").and_then(Value::as_str) == Some("init"))?;
    let names = |key: &str| -> Vec<String> {
        init.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    let terminal_only = names("terminal_slash_commands");
    Some(
        names("slash_commands")
            .into_iter()
            .filter(|c| !terminal_only.contains(c))
            .collect(),
    )
}

fn blocks(v: &Value) -> impl Iterator<Item = &Value> {
    v.pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn tool_summary(block: &Value) -> String {
    let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
    describe_tool(name, block.get("input").unwrap_or(&Value::Null))
}

/// The same call with paths under `cwd` made relative, for showing to people.
pub fn relative_input(input: &Value, cwd: &str) -> Value {
    let prefix = format!("{}/", cwd.trim_end_matches('/'));
    match input {
        Value::String(s) => Value::String(s.replace(&prefix, "")),
        Value::Array(items) => Value::Array(items.iter().map(|v| relative_input(v, cwd)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), relative_input(v, cwd)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `Bash  cargo test`, `Edit  src/lib.rs`, `Grep  "todo"`, else the tool name and its input.
pub fn describe_tool(name: &str, input: &Value) -> String {
    // A question to the person (Claude's own tool, or the one anchorleg-mod adds in `-p`).
    if name == crate::questions::TOOL || name.ends_with("__ask_user") {
        let qs: Vec<&str> = input
            .get("questions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|q| q.get("question").and_then(Value::as_str))
            .collect();
        return format!("Question  {}", first_line(&qs.join(" · "), 120))
            .trim_end()
            .to_owned();
    }
    if name == crate::adapters::codex_server::PERMISSIONS_TOOL {
        return format!(
            "Permissions  {}",
            describe_permissions(input.get("permissions"))
        )
        .trim_end()
        .to_owned();
    }
    let field = |k: &str| input.get(k).and_then(Value::as_str);
    let detail = field("command")
        .or_else(|| field("file_path"))
        .or_else(|| field("path"))
        .or_else(|| field("pattern"))
        .or_else(|| field("url"))
        .or_else(|| field("description"))
        .map(str::to_owned)
        .unwrap_or_else(|| match input {
            Value::Null => String::new(),
            other => other.to_string(),
        });
    format!("{name}  {}", first_line(&detail, 120))
        .trim_end()
        .to_owned()
}

/// Codex's requested sandbox access in a few words: "network · write /a · read /b".
fn describe_permissions(p: Option<&Value>) -> String {
    let Some(p) = p else {
        return String::new();
    };
    let mut parts = Vec::new();
    if p.pointer("/network/enabled").and_then(Value::as_bool) == Some(true) {
        parts.push("network".to_owned());
    }
    let fs = p.get("fileSystem").unwrap_or(&Value::Null);
    let mut by_access: Vec<(String, Vec<String>)> = Vec::new();
    let mut add = |access: &str, path: String| match by_access.iter_mut().find(|(a, _)| a == access)
    {
        Some((_, list)) => list.push(path),
        None => by_access.push((access.to_owned(), vec![path])),
    };
    for e in fs
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let access = e.get("access").and_then(Value::as_str).unwrap_or("access");
        let path = e.get("path").unwrap_or(&Value::Null);
        let shown = path
            .get("path")
            .or_else(|| path.get("pattern"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                path.pointer("/value/kind")
                    .and_then(Value::as_str)
                    .map(|k| format!("<{k}>"))
            })
            .unwrap_or_default();
        add(access, shown);
    }
    for access in ["read", "write"] {
        for path in fs
            .get(access)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(path) = path.as_str() {
                add(access, path.to_owned());
            }
        }
    }
    parts.extend(
        by_access
            .into_iter()
            .map(|(a, list)| format!("{a} {}", list.join(", "))),
    );
    first_line(&parts.join(" · "), 120)
}

fn content_text(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn first_line(s: &str, max: usize) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max).collect::<String>())
    } else {
        line.to_owned()
    }
}

/// Lines shown under an edit's approval: what goes (`-`) and what comes (`+`), from Claude's
/// `old_string`/`new_string`/`content` or Codex's diff; `.` marks context or elided lines.
pub fn change_preview(tool: &str, input: &serde_json::Value) -> Vec<(char, String)> {
    use serde_json::Value;
    const MAX: usize = 10;
    fn edit(old: &str, new: &str, lines: &mut Vec<(char, String)>) {
        lines.extend(old.lines().map(|l| ('-', l.to_owned())));
        lines.extend(new.lines().map(|l| ('+', l.to_owned())));
    }
    let text = |k: &str| input.get(k).and_then(Value::as_str);
    let mut lines: Vec<(char, String)> = Vec::new();
    if let Some(diff) = text("diff") {
        for l in diff.lines() {
            lines.push(match l.chars().next() {
                Some('+') if !l.starts_with("+++") => ('+', l[1..].to_owned()),
                Some('-') if !l.starts_with("---") => ('-', l[1..].to_owned()),
                _ => ('.', l.strip_prefix(' ').unwrap_or(l).to_owned()),
            });
        }
    } else {
        match tool {
            "Edit" => edit(
                text("old_string").unwrap_or_default(),
                text("new_string").unwrap_or_default(),
                &mut lines,
            ),
            "MultiEdit" => {
                for e in input
                    .get("edits")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let s = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or_default();
                    edit(s("old_string"), s("new_string"), &mut lines);
                }
            }
            "Write" => edit("", text("content").unwrap_or_default(), &mut lines),
            _ => {}
        }
    }
    if lines.len() > MAX {
        let more = lines.len() - (MAX - 1);
        lines.truncate(MAX - 1);
        lines.push(('.', format!("… {more} more lines")));
    }
    lines
}

/// Where a run left session `session`, read from its log: the uuid of Claude's last main-thread
/// message in it, or the id of Codex's last completed turn. A rewind forks from there.
pub fn fork_point(log: &str, session: &str) -> Option<String> {
    let mut point = None;
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let field = |k: &str| v.get(k).and_then(serde_json::Value::as_str);
        match field("type") {
            Some("assistant")
                if field("session_id") == Some(session)
                    && v.get("parent_tool_use_id")
                        .is_none_or(serde_json::Value::is_null) =>
            {
                point = field("uuid").map(str::to_owned).or(point);
            }
            Some("turn.completed") => point = field("turn_id").map(str::to_owned).or(point),
            _ => {}
        }
    }
    point
}

/// A session as people see it: the conversation of every run in it, and what its newest run is
/// doing. What `anchorleg ui` draws and `anchorleg sessions show` prints.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionView {
    pub messages: Vec<Message>,
    /// The tool call the newest run is in, if it's working.
    pub running: Option<Running>,
    /// The newest run's subagents.
    pub subagents: Vec<Subagent>,
    /// The agent's last task list.
    pub steps: Vec<crate::handoff::Step>,
    /// Tokens in the context at the last model request, and the model's window.
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    /// The CLI's guess at the person's next message, after the newest run's last turn.
    pub suggestion: Option<String>,
}

/// Claude Code's guess at the person's next message (`--prompt-suggestions`): the
/// `prompt_suggestion` after the last turn, if nothing has been said since.
pub fn suggestion(log: &str) -> Option<String> {
    let mut out = None;
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("prompt_suggestion") => {
                out = v
                    .get("suggestion")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
            }
            Some("anchorleg_user" | "assistant") => out = None,
            _ => {}
        }
    }
    out
}

/// Where a session stood around each message the person sent in a run log: for the k-th, the
/// point just before it (`before[k]`) and the point at the end of its turn (`after[k]`). Points
/// are Claude's assistant message uuids and Codex's turn ids, what [`fork_point`] reads.
#[derive(Debug, Default, PartialEq)]
pub struct TurnPoints {
    pub before: Vec<Option<String>>,
    pub after: Vec<Option<String>>,
}

pub fn turn_points(log: &str) -> TurnPoints {
    let mut out = TurnPoints::default();
    let mut point: Option<String> = None;
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let field = |k: &str| v.get(k).and_then(Value::as_str);
        match field("type") {
            Some("assistant") if !is_sidechain(&v) => {
                point = field("uuid").map(str::to_owned).or(point);
            }
            Some("turn.completed") => point = field("turn_id").map(str::to_owned).or(point),
            Some("anchorleg_user" | "relay_user") => {
                if !out.before.is_empty() {
                    out.after.push(point.clone());
                }
                out.before.push(point.clone());
            }
            _ => {}
        }
    }
    if !out.before.is_empty() {
        out.after.push(point);
    }
    out
}

/// Marks each message with its run and, from `points`, where the session can be forked from
/// around it: the person's k-th message and the last reply of its turn.
fn place(messages: &mut [Message], run: i64, points: &TurnPoints) {
    let mut k = 0;
    let mut last_reply: Option<usize> = None;
    let close = |messages: &mut [Message], last: Option<usize>, k: usize| {
        if let (Some(i), Some(Some(at))) =
            (last, k.checked_sub(1).and_then(|k| points.after.get(k)))
        {
            messages[i].at = Some(at.clone());
        }
    };
    for i in 0..messages.len() {
        messages[i].run = Some(run);
        match messages[i].role {
            Role::User => {
                close(messages, last_reply.take(), k);
                messages[i].at = points.before.get(k).cloned().flatten();
                k += 1;
            }
            Role::Agent => last_reply = Some(i),
            _ => {}
        }
    }
    close(messages, last_reply, k);
}

/// The view of a session from its runs' logs (`<log_dir>/run-<id>.jsonl`), oldest run first.
pub fn session_view(runs: &[&crate::store::Run], log_dir: &std::path::Path) -> SessionView {
    let read = |id: i64| {
        std::fs::read_to_string(log_dir.join(format!("run-{id}.jsonl"))).unwrap_or_default()
    };
    let mut view = SessionView::default();
    let mut whole = String::new();
    let mut newest = String::new();
    for run in runs {
        let log = read(run.id);
        let mut part = from_log(&log);
        // Paths inside the session's folder read better relative to it.
        let prefix = format!("{}/", run.cwd.trim_end_matches('/'));
        for m in part.iter_mut().filter(|m| m.role == Role::Tool) {
            m.text = m.text.replace(&prefix, "");
        }
        // Logs from before anchorleg recorded the prompt: show the task as the user's message.
        if !part.iter().any(|m| m.role == Role::User) {
            part.insert(0, msg(Role::User, run.task.clone()));
        }
        place(&mut part, run.id, &turn_points(&log));
        view.messages.extend(part);
        whole.push_str(&log);
        newest = log;
    }
    view.running = running(&newest);
    view.suggestion = suggestion(&newest);
    view.subagents = subagents(&newest);
    read_transcripts(&mut view.subagents);
    let t = crate::handoff::extract(&whole);
    view.context_tokens = t.context_tokens;
    view.context_window = t.context_window;
    view.steps = t.steps;
    view
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_suggestion_after_the_last_turn_until_someone_speaks() {
        // As Claude Code 2.1.300 prints it, after the turn's `result` (fixture
        // claude-2.1.300-prompt-suggestion-20261015).
        let log = include_str!(
            "../../../fixtures/claude-2.1.300-prompt-suggestion-20261015/stream.jsonl"
        );
        assert_eq!(
            super::suggestion(log).as_deref(),
            Some("fix the add function")
        );
        let later = format!(
            "{log}{}\n",
            r#"{"type":"anchorleg_user","text":"no thanks"}"#
        );
        assert_eq!(super::suggestion(&later), None);
        assert_eq!(super::suggestion(""), None);
    }

    #[test]
    fn messages_say_where_the_session_can_go_back_to() {
        use super::{Role, TurnPoints, turn_points};
        // Claude: assistant uuids; the person's second message goes back to the end of the
        // first turn, and each turn's last reply forks from just after it.
        let claude = [
            r#"{"type":"anchorleg_user","text":"one"}"#,
            r#"{"type":"assistant","uuid":"a1","message":{"content":[{"type":"text","text":"hi"}]}}"#,
            r#"{"type":"assistant","uuid":"a2","parent_tool_use_id":"t9","message":{"content":[{"type":"text","text":"sub"}]}}"#,
            r#"{"type":"assistant","uuid":"a3","message":{"content":[{"type":"text","text":"done"}]}}"#,
            r#"{"type":"anchorleg_user","text":"two"}"#,
            r#"{"type":"assistant","uuid":"b1","message":{"content":[{"type":"text","text":"ok"}]}}"#,
        ]
        .join("\n");
        let p = turn_points(&claude);
        assert_eq!(
            p,
            TurnPoints {
                before: vec![None, Some("a3".into())],
                after: vec![Some("a3".into()), Some("b1".into())],
            }
        );
        let mut m = super::from_log(&claude);
        super::place(&mut m, 7, &p);
        let at: Vec<(Role, Option<&str>)> = m.iter().map(|m| (m.role, m.at.as_deref())).collect();
        assert_eq!(
            at,
            [
                (Role::User, None),
                (Role::Agent, None),
                (Role::Agent, Some("a3")),
                (Role::User, Some("a3")),
                (Role::Agent, Some("b1")),
            ]
        );
        assert!(m.iter().all(|m| m.run == Some(7)));
        // Codex: turn ids.
        let codex = [
            r#"{"type":"anchorleg_user","text":"one"}"#,
            r#"{"type":"turn.completed","turn_id":"u1"}"#,
            r#"{"type":"anchorleg_user","text":"two"}"#,
        ]
        .join("\n");
        assert_eq!(
            turn_points(&codex).before,
            vec![None, Some("u1".to_owned())]
        );
    }

    #[test]
    fn a_session_view_joins_its_runs_and_reads_the_newest() {
        let store = crate::store::Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let first = store.start_run("fix the build", "/w", None, 1).unwrap();
        let second = store
            .start_run("now the tests", "/w", Some(first), 2)
            .unwrap();
        let text = |t: &str| {
            format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{t}"}}]}}}}"#
            )
        };
        let tool = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        std::fs::write(dir.path().join(format!("run-{first}.jsonl")), text("built")).unwrap();
        std::fs::write(
            dir.path().join(format!("run-{second}.jsonl")),
            [text("testing"), tool.to_owned()].join("\n"),
        )
        .unwrap();
        let runs = [
            store.run(first).unwrap().unwrap(),
            store.run(second).unwrap().unwrap(),
        ];
        let view = session_view(&runs.iter().collect::<Vec<_>>(), dir.path());
        let roles: Vec<_> = view
            .messages
            .iter()
            .map(|m| (m.role, m.text.as_str()))
            .collect();
        // Old logs without the prompt get the task as the person's message.
        assert_eq!(roles[0], (Role::User, "fix the build"));
        assert_eq!(roles[1], (Role::Agent, "built"));
        assert_eq!(roles[2], (Role::User, "now the tests"));
        assert_eq!(roles[3], (Role::Agent, "testing"));
        assert!(
            view.running
                .as_ref()
                .is_some_and(|r| r.what.contains("cargo test"))
        );
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["messages"][0]["role"], "user");
    }

    #[test]
    fn a_rewind_forks_from_the_last_main_thread_message_or_turn() {
        let claude = [
            r#"{"type":"assistant","uuid":"a","session_id":"s1","parent_tool_use_id":null}"#,
            r#"{"type":"assistant","uuid":"sub","session_id":"s1","parent_tool_use_id":"t"}"#,
            r#"{"type":"assistant","uuid":"other","session_id":"s0"}"#,
            r#"{"type":"result","session_id":"s1"}"#,
        ]
        .join("\n");
        assert_eq!(fork_point(&claude, "s1").as_deref(), Some("a"));
        let codex = [
            r#"{"type":"turn.completed","turn_id":"u1"}"#,
            r#"{"type":"turn.completed","turn_id":"u2"}"#,
            r#"{"type":"turn.failed"}"#,
        ]
        .join("\n");
        assert_eq!(fork_point(&codex, "t1").as_deref(), Some("u2"));
        assert_eq!(fork_point("", "s1"), None);
    }
    use super::*;

    #[test]
    fn reads_a_relay_run_log() {
        let log = [
            r#"{"type":"anchorleg_user","text":"fix the tests"}"#,
            r#"{"type":"anchorleg","text":"running on claude-personal"}"#,
            r#"{"type":"system","subtype":"init","session_id":"s1"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"Running them.\n"},{"type":"tool_use","name":"Bash","input":{"command":"cargo test\necho done"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","is_error":true,"content":"error: 2 tests failed\nmore"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            "not json",
            r#"{"type":"result","is_error":true,"result":null,"errors":["No conversation found"]}"#,
        ]
        .join("\n");
        assert_eq!(
            from_log(&log),
            vec![
                msg(Role::User, "fix the tests"),
                msg(Role::Anchorleg, "running on claude-personal"),
                msg(Role::Thinking, "hm"),
                msg(Role::Agent, "Running them."),
                msg(Role::Tool, "Bash  cargo test"),
                msg(Role::ToolError, "error: 2 tests failed"),
                msg(Role::Output, "ok"),
                msg(Role::Error, "No conversation found"),
            ]
        );
    }

    #[test]
    fn hooks_show_when_they_block_fail_or_say_something() {
        let log = [
            r#"{"type":"system","subtype":"hook_response","hook_name":"SessionStart:startup","output":"","stdout":"","stderr":"","exit_code":0,"outcome":"success"}"#,
            r#"{"type":"system","subtype":"hook_response","hook_name":"PreToolUse:Bash","stderr":"no rm -rf here\nmore","exit_code":2,"outcome":"blocked"}"#,
            r#"{"type":"system","subtype":"hook_response","hook_name":"Stop","stdout":"lint clean","exit_code":0,"outcome":"success"}"#,
            r#"{"type":"item.completed","item":{"type":"hook","status":"blocked","event":"preToolUse","text":"no network"}}"#,
        ]
        .join("\n");
        assert_eq!(
            from_log(&log),
            vec![
                msg(
                    Role::ToolError,
                    "hook PreToolUse:Bash blocked: no rm -rf here"
                ),
                msg(Role::Thinking, "hook Stop: lint clean"),
                msg(Role::ToolError, "hook preToolUse blocked: no network"),
            ]
        );
    }

    #[test]
    fn reads_an_agy_run_log() {
        let msgs = from_log(include_str!(
            "../../../fixtures/agy-1.3.1-probe-20261008/print-start.jsonl"
        ));
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert_eq!(msgs[0].role, Role::Tool);
        assert!(msgs[0].text.contains("notes.txt"));
        assert_eq!(msgs[1], msg(Role::Agent, "FIG-4"));
        let failed = from_log(include_str!(
            "../../../fixtures/agy-1.3.1-probe-20261008/print-bad-model.jsonl"
        ));
        assert!(matches!(failed.as_slice(), [m] if m.role == Role::Error));
    }

    #[test]
    fn reads_a_codex_run_log() {
        let log = include_str!("../../../fixtures/codex-0.153.4-probe-20261008/exec-start.jsonl");
        let msgs = from_log(log);
        assert_eq!(msgs.last(), Some(&msg(Role::Agent, "PLUM-9")));
        assert!(!msgs.iter().any(|m| m.role == Role::ToolError));
        assert!(
            msgs.iter()
                .any(|m| m.role == Role::Tool && m.text.contains("find . -name notes.txt"))
        );
        let failed = from_log(include_str!(
            "../../../fixtures/codex-0.153.4-probe-20261008/exec-not-logged-in.jsonl"
        ));
        assert!(
            matches!(failed.last(), Some(m) if m.role == Role::Error && m.text.contains("401"))
        );
    }

    #[test]
    fn slash_commands_from_the_last_init() {
        let log = include_str!(
            "../../../fixtures/claude-2.1.294-permission-20261008/run-write-approved.jsonl"
        );
        let cmds = slash_commands(log).unwrap();
        assert!(cmds.iter().any(|c| c == "compact"));
        assert!(cmds.iter().any(|c| c == "code-review"));
        assert!(!cmds.iter().any(|c| c == "doctor"), "terminal-only");
        assert_eq!(slash_commands("{}"), None);
    }

    #[test]
    fn reads_the_real_mod_stop_capture() {
        let log = include_str!(
            "../../../fixtures/claude-2.1.294-mod-stop-20261008/run-personal-then-two.jsonl"
        );
        let m = from_log(log);
        assert!(
            m.iter()
                .any(|x| x.role == Role::Tool && x.text.starts_with("Read  "))
        );
        assert_eq!(m.last(), Some(&msg(Role::Agent, "MANGO-17")));
    }

    #[test]
    fn questions_read_as_questions() {
        let input = serde_json::json!({ "questions": [{ "question": "Red or Blue?" }, { "question": "Size?" }] });
        assert_eq!(
            describe_tool("mcp__anchorleg-mod__ask_user", &input),
            "Question  Red or Blue? · Size?"
        );
        assert_eq!(
            describe_tool("AskUserQuestion", &input),
            "Question  Red or Blue? · Size?"
        );
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    const AGY: &str = include_str!(
        "../../../fixtures/agy-1.3.3-command-subagent-20261012/command-subagent.jsonl"
    );
    const AGY_THINKING: &str =
        include_str!("../../../fixtures/agy-1.3.3-command-subagent-20261012/thinking.jsonl");

    #[test]
    fn claude_subagents_are_progress_not_the_conversation() {
        let log = include_str!("../../../fixtures/claude-2.1.296-subagent-20261012/subagent.jsonl");
        let m = from_log(log);
        // The Agent call and its result; the subagent's own Bash call isn't the session's.
        assert!(m.contains(&msg(Role::Tool, "Agent  List files in working folder")));
        assert!(!m.iter().any(|m| m.text.starts_with("Bash")), "{m:?}");
        let subs = subagents(log);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].name, "List files in working folder");
        assert!(subs[0].done && subs[0].steps == 1);
        // Cut before its result: it's working, on its Bash call.
        let upto: String = log
            .lines()
            .take_while(|l| !l.contains("task_updated"))
            .map(|l| format!("{l}\n"))
            .collect();
        let s = &subagents(&upto)[0];
        assert!(!s.done);
        assert!(
            s.doing.as_deref().unwrap().starts_with("Bash  ls -la"),
            "{s:?}"
        );
        assert!(running(&upto).unwrap().what.starts_with("Agent"));
    }

    #[test]
    fn agy_subagents_are_read_from_their_transcripts() {
        let mut subs = subagents(AGY);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].name, "File Lister");
        assert!(
            subs[0]
                .transcript
                .as_ref()
                .unwrap()
                .ends_with("logs/transcript.jsonl")
        );
        assert!(from_log(AGY).contains(&msg(Role::Tool, "Agent  File Lister")));
        let transcript = include_str!(
            "../../../fixtures/agy-1.3.3-command-subagent-20261012/subagent-transcript.jsonl"
        );
        agy_progress(&mut subs[0], transcript);
        assert!(subs[0].done);
        assert_eq!(subs[0].steps, 2);
        // Partway: after its first tool call.
        let first: String = transcript
            .lines()
            .take(2)
            .map(|l| format!("{l}\n"))
            .collect();
        agy_progress(&mut subs[0], &first);
        assert!(!subs[0].done);
        assert_eq!(subs[0].doing.as_deref(), Some("Bash  ls -la"));
    }

    #[test]
    fn compaction_shows_as_a_note() {
        let claude = from_log(include_str!(
            "../../../fixtures/claude-2.1.296-compact-20261012/compact.jsonl"
        ));
        assert_eq!(
            claude.last().unwrap(),
            &msg(Role::Anchorleg, format!("{COMPACTED} (26k → 5k tokens)"))
        );
        // The summary Claude resumes from isn't shown as a message.
        assert!(!claude.iter().any(|m| m.text.contains("Summary:")));
        let codex =
            from_log(r#"{"type":"item.completed","item":{"id":"c","type":"context_compaction"}}"#);
        assert_eq!(codex, [msg(Role::Anchorleg, COMPACTED)]);
    }

    #[test]
    fn agy_output_thinking_and_running() {
        let m = from_log(AGY);
        let i = m.iter().position(|m| m.text.starts_with("Bash")).unwrap();
        assert_eq!(m[i + 1], msg(Role::Output, "one\ntwo"));
        assert!(from_log(AGY_THINKING).contains(&msg(Role::Thinking, "thought for 253 tokens")));
        // Cut the stream right after the command started: it's running.
        let upto: String = AGY
            .lines()
            .take_while(|l| !l.contains("\"output\""))
            .map(|l| format!("{l}\n"))
            .collect();
        let r = running(&upto).unwrap();
        assert_eq!(r.what, "Bash  echo one; sleep 3; echo two");
        assert_eq!(running(AGY), None);
    }

    #[test]
    fn codex_streams_output_and_reasoning() {
        let started = r#"{"type":"item.started","item":{"id":"c1","type":"command_execution","command":"cargo test","status":"in_progress"}}"#;
        let deltas = [1, 2, 3, 4, 5]
            .map(|n| {
                format!(r#"{{"type":"item.output_delta","item_id":"c1","delta":"line {n}\n"}}"#)
            })
            .join("\n");
        let log = format!("{started}\n{deltas}");
        let r = running(&log).unwrap();
        assert_eq!(r.what, "Bash  cargo test");
        assert_eq!(r.output, "line 2\nline 3\nline 4\nline 5");
        let done = r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution","command":"cargo test","aggregated_output":"ok\n","exit_code":0}}"#;
        let reasoning = r#"{"type":"item.completed","item":{"id":"r1","type":"reasoning","text":"Checking the tests first"}}"#;
        let log = format!("{log}\n{done}\n{reasoning}");
        assert_eq!(running(&log), None);
        let m = from_log(&log);
        assert!(m.contains(&msg(Role::Output, "ok")));
        assert!(m.contains(&msg(Role::Thinking, "Checking the tests first")));
    }

    #[test]
    fn claude_output_and_running() {
        let call = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Run the tests"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        assert_eq!(running(call).unwrap().what, "Bash  cargo test");
        let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"a\nb\nc\nd\ne\nf"}]}}"#;
        let log = format!("{call}\n{result}");
        assert_eq!(running(&log), None);
        let m = from_log(&log);
        assert!(m.contains(&msg(Role::Thinking, "Run the tests")));
        assert!(m.contains(&msg(Role::Output, "a\nb\nc\nd\n… +2 lines")));
    }
}
