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
    /// Something anchorleg did: switched, paused, stopped.
    Anchorleg,
    /// The run ended with an error.
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

fn msg(role: Role, text: impl Into<String>) -> Message {
    Message {
        role,
        text: text.into(),
    }
}

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
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for block in blocks(&v) {
                    let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
                    if block.get("type").and_then(Value::as_str) == Some("tool_result") && failed {
                        out.push(msg(Role::ToolError, first_line(&content_text(block), 160)));
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_relay_run_log() {
        let log = [
            r#"{"type":"anchorleg_user","text":"fix the tests"}"#,
            r#"{"type":"anchorleg","text":"running on claude-sm"}"#,
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
                msg(Role::Anchorleg, "running on claude-sm"),
                msg(Role::Agent, "Running them."),
                msg(Role::Tool, "Bash  cargo test"),
                msg(Role::ToolError, "error: 2 tests failed"),
                msg(Role::Error, "No conversation found"),
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
            "../../../fixtures/claude-2.1.294-mod-stop-20261008/run-sm-then-two.jsonl"
        );
        let m = from_log(log);
        assert!(
            m.iter()
                .any(|x| x.role == Role::Tool && x.text.starts_with("Read  "))
        );
        assert_eq!(m.last(), Some(&msg(Role::Agent, "MANGO-17")));
    }
}
