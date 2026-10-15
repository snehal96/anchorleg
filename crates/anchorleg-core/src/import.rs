//! Sessions started outside anchorleg (in the CLI itself), brought in so they show in
//! `anchorleg sessions` and the apps and can be continued with a follow-up on any account.
//!
//! Each CLI keeps its sessions per account: Claude Code under `<config dir>/projects/*/<id>.jsonl`,
//! Codex as rollout files under `$CODEX_HOME/sessions/`. Each is turned into an anchorleg run log
//! (the lines [`crate::conversation::from_log`] reads), so the conversation shows as if anchorleg
//! had run it. Antigravity keeps its conversations in binary databases with no readable
//! transcript, so it isn't imported.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

use crate::adapters::EpochSecs;
use crate::registry::Vendor;

/// One session found on disk.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Found {
    pub vendor: Vendor,
    pub account: String,
    pub session_id: String,
    pub cwd: String,
    /// The first thing the person asked: the run's task.
    pub task: String,
    /// The CLI's own title for it (Claude Code's `/rename` or its generated title).
    pub name: Option<String>,
    pub started_at: EpochSecs,
    pub ended_at: EpochSecs,
    /// Messages from the person, for a sense of size.
    pub prompts: usize,
    #[serde(skip)]
    pub file: PathBuf,
    /// The anchorleg run log: one JSON line each.
    #[serde(skip)]
    pub log: Vec<String>,
}

/// Longest tool output kept per call; the conversation only shows an excerpt anyway.
const MAX_OUTPUT: usize = 4_000;

fn epoch(ts: &str) -> Option<EpochSecs> {
    ts.parse::<jiff::Timestamp>().ok().map(|t| t.as_second())
}

fn modified(path: &Path) -> EpochSecs {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as EpochSecs)
}

fn user_line(text: &str) -> String {
    json!({ "type": "anchorleg_user", "text": text }).to_string()
}

fn clip(s: &str) -> String {
    if s.len() <= MAX_OUTPUT {
        return s.to_owned();
    }
    let mut end = MAX_OUTPUT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// What Claude Code shows of a person's message, or `None` for what it adds itself (reminders,
/// command output, notices). Slash commands come back as `/name args`.
fn claude_prompt(v: &Value) -> Option<String> {
    if ["isMeta", "isCompactSummary", "isVisibleInTranscriptOnly"]
        .iter()
        .any(|k| v.get(*k).and_then(Value::as_bool) == Some(true))
    {
        return None;
    }
    let content = v.pointer("/message/content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("<command-name>") {
        let name = rest.split("</command-name>").next()?.trim();
        let args = text
            .split("<command-args>")
            .nth(1)
            .and_then(|a| a.split("</command-args>").next())
            .unwrap_or("")
            .trim();
        return Some(format!("{name} {args}").trim().to_owned());
    }
    let added = [
        "<local-command",
        "<task-notification",
        "<system-reminder",
        "<bash-",
        "<user-prompt-submit-hook",
        "Caveat:",
    ];
    if text.is_empty() || added.iter().any(|p| text.starts_with(p)) {
        return None;
    }
    Some(text.to_owned())
}

/// A Claude Code transcript (`projects/<folder>/<id>.jsonl`) as a run log.
pub fn claude(transcript: &str, account: &str, file: &Path) -> Option<Found> {
    let mut log = Vec::new();
    let (mut session_id, mut cwd) = (None, None);
    let (mut first, mut last) = (None, None);
    let (mut task, mut title, mut custom) = (None, None, None);
    let mut slash = None;
    let mut prompts = 0;
    let mut answered = false;
    for line in transcript.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let s = |k: &str| v.get(k).and_then(Value::as_str);
        if session_id.is_none() {
            session_id = s("sessionId").map(str::to_owned);
        }
        if cwd.is_none() {
            cwd = s("cwd").map(str::to_owned);
        }
        if let Some(t) = s("timestamp").and_then(epoch) {
            first.get_or_insert(t);
            last = Some(t);
        }
        match s("type") {
            Some("ai-title") => title = s("aiTitle").map(str::to_owned),
            Some("custom-title") => custom = s("customTitle").map(str::to_owned),
            _ => {}
        }
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        match s("type") {
            Some("user") => {
                if let Some(p) = claude_prompt(&v) {
                    prompts += 1;
                    // The task is the first real message, not a `/model` or `/clear` before it.
                    if !p.starts_with('/') {
                        task.get_or_insert_with(|| p.clone());
                    }
                    slash.get_or_insert_with(|| p.clone());
                    log.push(user_line(&p));
                    continue;
                }
                // Tool results, with long outputs cut.
                let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else {
                    continue;
                };
                let results: Vec<Value> = blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
                    .map(|b| {
                        let text = match b.get("content") {
                            Some(Value::String(t)) => t.clone(),
                            Some(Value::Array(parts)) => parts
                                .iter()
                                .filter_map(|p| p.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("\n"),
                            _ => String::new(),
                        };
                        json!({
                            "type": "tool_result",
                            "tool_use_id": b.get("tool_use_id"),
                            "is_error": b.get("is_error"),
                            "content": clip(&text),
                        })
                    })
                    .collect();
                if !results.is_empty() {
                    log.push(
                        json!({ "type": "user", "message": { "content": results } }).to_string(),
                    );
                }
            }
            Some("assistant") => {
                let content = v.pointer("/message/content").cloned().unwrap_or(json!([]));
                answered |= content.as_array().is_some_and(|b| {
                    b.iter()
                        .any(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                });
                // The uuid and session are where a rewind or fork can start from (`at`).
                log.push(
                    json!({
                        "type": "assistant",
                        "message": { "model": v.pointer("/message/model"), "content": content },
                        "uuid": v.get("uuid"),
                        "session_id": v.get("sessionId"),
                    })
                    .to_string(),
                );
            }
            Some("system") if s("subtype") == Some("compact_boundary") => log.push(line.to_owned()),
            _ => {}
        }
    }
    let task = task.or(slash)?;
    if !answered {
        return None;
    }
    let ended = last.unwrap_or_else(|| modified(file));
    Some(Found {
        vendor: Vendor::Claude,
        account: account.to_owned(),
        session_id: session_id?,
        cwd: cwd?,
        task,
        name: custom.or(title),
        started_at: first.unwrap_or(ended),
        ended_at: ended,
        prompts,
        file: file.to_owned(),
        log,
    })
}

/// A Codex rollout item (`event_msg` `item_completed`) as the `codex exec --json` item the
/// conversation reader knows; the person's messages become `anchorleg_user` lines.
fn codex_item(item: &Value) -> Option<Value> {
    let text = |key: &str| -> String {
        item.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| c.get("text").and_then(Value::as_str).or_else(|| c.as_str()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let s = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or_default();
    let exec = |v: Value| Some(json!({ "type": "item.completed", "item": v }));
    match s("type") {
        "UserMessage" => {
            let t = text("content");
            let t = t.trim();
            (!t.is_empty() && !t.starts_with('<'))
                .then(|| json!({ "type": "anchorleg_user", "text": t }))
        }
        "AgentMessage" => exec(json!({ "type": "agent_message", "text": text("content") })),
        "Reasoning" => {
            let t = text("summary_text");
            if t.trim().is_empty() {
                return None;
            }
            exec(json!({ "type": "reasoning", "text": t }))
        }
        "ContextCompaction" => exec(json!({ "type": "context_compaction" })),
        "CommandExecution" => {
            let command = match item.get("command") {
                Some(Value::Array(parts)) => parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
                Some(Value::String(c)) => c.clone(),
                _ => String::new(),
            };
            exec(json!({
                "type": "command_execution",
                "command": command,
                "aggregated_output": clip(s("aggregated_output")),
                "exit_code": item.get("exit_code"),
                "status": s("status"),
            }))
        }
        "FileChange" => {
            let changes: Vec<Value> = item
                .get("changes")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .map(|(path, c)| {
                    let kind = match c.get("type").and_then(Value::as_str) {
                        Some("add") => "add",
                        Some("delete") => "delete",
                        _ => "update",
                    };
                    json!({ "path": path, "kind": kind })
                })
                .collect();
            exec(json!({ "type": "file_change", "changes": changes, "status": s("status") }))
        }
        "McpToolCall" => exec(json!({
            "type": "mcp_tool_call",
            "server": s("server"),
            "tool": s("tool"),
            "status": s("status"),
        })),
        _ => None,
    }
}

/// A Codex rollout (`sessions/YYYY/MM/DD/rollout-…-<id>.jsonl`) as a run log.
pub fn codex(rollout: &str, account: &str, file: &Path) -> Option<Found> {
    let mut log = Vec::new();
    let (mut session_id, mut cwd) = (None, None);
    let (mut first, mut last) = (None, None);
    let mut task = None;
    let mut prompts = 0;
    let mut answered = false;
    let mut subagent = false;
    for line in rollout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(t) = v.get("timestamp").and_then(Value::as_str).and_then(epoch) {
            first.get_or_insert(t);
            last = Some(t);
        }
        let payload = v.get("payload").unwrap_or(&Value::Null);
        match v.get("type").and_then(Value::as_str) {
            Some("session_meta") if session_id.is_none() => {
                session_id = payload.get("id").and_then(Value::as_str).map(str::to_owned);
                cwd = payload
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                // A subagent's own thread: its parent shows it.
                subagent = payload
                    .pointer("/source/subagent")
                    .is_some_and(|s| !s.is_null());
            }
            Some("event_msg")
                if payload.get("type").and_then(Value::as_str) == Some("item_completed") =>
            {
                let Some(out) = payload.get("item").and_then(codex_item) else {
                    continue;
                };
                if out["type"] == "anchorleg_user" {
                    prompts += 1;
                    task.get_or_insert_with(|| out["text"].as_str().unwrap_or_default().to_owned());
                }
                answered |=
                    out.pointer("/item/type").and_then(Value::as_str) == Some("agent_message");
                log.push(out.to_string());
            }
            // The end of a turn: where a rewind or fork can start from (`thread/fork`
            // `lastTurnId`).
            Some("event_msg")
                if payload.get("type").and_then(Value::as_str) == Some("task_complete") =>
            {
                if let Some(turn) = payload.get("turn_id").and_then(Value::as_str) {
                    log.push(json!({ "type": "turn.completed", "turn_id": turn }).to_string());
                }
            }
            _ => {}
        }
    }
    if subagent || !answered {
        return None;
    }
    let ended = last.unwrap_or_else(|| modified(file));
    Some(Found {
        vendor: Vendor::Codex,
        account: account.to_owned(),
        session_id: session_id?,
        cwd: cwd?,
        task: task?,
        name: None,
        started_at: first.unwrap_or(ended),
        ended_at: ended,
        prompts,
        file: file.to_owned(),
        log,
    })
}

/// Transcript files of a Claude Code account, changed since `since`.
pub fn claude_files(config_dir: &Path, since: EpochSecs) -> Vec<PathBuf> {
    let Ok(projects) = std::fs::read_dir(config_dir.join("projects")) else {
        return Vec::new();
    };
    projects
        .flatten()
        .filter_map(|p| std::fs::read_dir(p.path()).ok())
        .flat_map(|d| d.flatten().map(|f| f.path()))
        .filter(|f| f.extension().is_some_and(|e| e == "jsonl") && modified(f) >= since)
        .collect()
}

/// Rollout files of a Codex account, changed since `since`.
pub fn codex_files(codex_home: &Path, since: EpochSecs) -> Vec<PathBuf> {
    fn walk(dir: &Path, since: EpochSecs, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, since, out);
            } else if p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
                && modified(&p) >= since
            {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &crate::adapters::codex::sessions_dir(codex_home),
        since,
        &mut out,
    );
    out
}

/// One copy per session: anchorleg's switches copy a session into the next account, so the same
/// id can sit in several. The latest copy wins (it has the most of the conversation).
pub fn dedup(mut found: Vec<Found>) -> Vec<Found> {
    found.sort_by(|a, b| {
        b.ended_at
            .cmp(&a.ended_at)
            .then(b.log.len().cmp(&a.log.len()))
    });
    let mut seen = std::collections::HashSet::new();
    found.retain(|f| seen.insert(f.session_id.clone()));
    found.sort_by_key(|f| f.started_at);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{Role, from_log};

    #[test]
    fn imported_sessions_keep_the_points_to_go_back_to() {
        use crate::conversation::turn_points;
        // Real captures: a Claude Code transcript's assistant uuids, a Codex rollout's turn ids.
        let t =
            include_str!("../../../fixtures/claude-2.1.299-transcript-20261015/transcript.jsonl");
        let f = claude(t, "work", Path::new("/x.jsonl")).unwrap();
        let p = turn_points(&f.log.join("\n"));
        assert!(p.after.iter().all(Option::is_some), "{p:?}");
        assert!(p.before.iter().skip(1).all(Option::is_some), "{p:?}");
        let r = include_str!("../../../fixtures/codex-0.153.4-rollout-20261015/rollout.jsonl");
        let f = codex(r, "work", Path::new("/x.jsonl")).unwrap();
        let p = turn_points(&f.log.join("\n"));
        assert!(
            !p.after.is_empty() && p.after.iter().all(Option::is_some),
            "{p:?}"
        );
    }

    #[test]
    fn a_claude_transcript_reads_like_an_anchorleg_run() {
        let t = [
            r#"{"type":"queue-operation","operation":"enqueue"}"#,
            r#"{"type":"user","sessionId":"s1","cwd":"/w","timestamp":"2026-10-14T10:00:00Z","message":{"role":"user","content":"<local-command-caveat>Caveat: …</local-command-caveat>"},"isMeta":true}"#,
            r#"{"type":"user","sessionId":"s1","cwd":"/w","timestamp":"2026-10-14T10:00:01Z","message":{"role":"user","content":"fix the build"}}"#,
            r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-10-14T10:00:05Z","message":{"model":"claude-haiku-5-5","content":[{"type":"text","text":"Looking."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo build"}}]}}"#,
            r#"{"type":"user","sessionId":"s1","timestamp":"2026-10-14T10:00:09Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"error[E0425]: cannot find value"}]}}"#,
            r#"{"type":"assistant","sessionId":"s1","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent noise"}]}}"#,
            r#"{"type":"user","sessionId":"s1","timestamp":"2026-10-14T10:01:00Z","message":{"content":"<command-name>/model</command-name>\n<command-args>haiku</command-args>"}}"#,
            r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-10-14T10:02:00Z","message":{"content":[{"type":"text","text":"Fixed."}]}}"#,
            r#"{"type":"ai-title","aiTitle":"Build fix","sessionId":"s1"}"#,
        ]
        .join("\n");
        let f = claude(&t, "work", Path::new("/x.jsonl")).unwrap();
        assert_eq!((f.session_id.as_str(), f.cwd.as_str()), ("s1", "/w"));
        assert_eq!(f.task, "fix the build");
        assert_eq!(f.name.as_deref(), Some("Build fix"));
        assert_eq!(f.prompts, 2);
        assert_eq!(f.ended_at - f.started_at, 120);
        let messages = from_log(&f.log.join("\n"));
        let shown: Vec<_> = messages.iter().map(|m| (m.role, m.text.as_str())).collect();
        assert_eq!(shown[0], (Role::User, "fix the build"));
        assert_eq!(shown[1], (Role::Agent, "Looking."));
        assert!(
            shown
                .iter()
                .any(|(r, t)| *r == Role::User && *t == "/model haiku"),
            "{shown:?}"
        );
        assert_eq!(shown.last().unwrap(), &(Role::Agent, "Fixed."));
        assert!(
            !shown
                .iter()
                .any(|(_, t)| t.contains("subagent noise") || t.contains("Caveat"))
        );
    }

    #[test]
    fn a_slash_command_first_is_not_the_task() {
        let t = [
            r#"{"type":"user","sessionId":"s1","cwd":"/w","message":{"content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","sessionId":"s1","cwd":"/w","message":{"content":"plan the release"}}"#,
            r#"{"type":"assistant","sessionId":"s1","message":{"content":[{"type":"text","text":"ok"}]}}"#,
        ]
        .join("\n");
        assert_eq!(
            claude(&t, "a", Path::new("/x")).unwrap().task,
            "plan the release"
        );
    }

    #[test]
    fn a_claude_transcript_nobody_answered_is_left_out() {
        let t = r#"{"type":"user","sessionId":"s1","cwd":"/w","message":{"content":"hi"}}"#;
        assert!(claude(t, "a", Path::new("/x")).is_none());
    }

    #[test]
    fn a_codex_rollout_reads_like_an_anchorleg_run() {
        let r = [
            r#"{"timestamp":"2026-10-14T09:00:00Z","type":"session_meta","payload":{"id":"th1","cwd":"/repo","source":"vscode"}}"#,
            r#"{"timestamp":"2026-10-14T09:00:01Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"u","content":[{"type":"text","text":"<environment_context>…</environment_context>"}]}}}"#,
            r#"{"timestamp":"2026-10-14T09:00:02Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"u2","content":[{"type":"text","text":"add a test"}]}}}"#,
            r#"{"timestamp":"2026-10-14T09:00:03Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"Reasoning","id":"r","summary_text":["Checking the tests"],"raw_content":[]}}}"#,
            r#"{"timestamp":"2026-10-14T09:00:04Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"c","command":["/bin/zsh","-lc","cargo test"],"aggregated_output":"ok","exit_code":0,"status":"completed"}}}"#,
            r#"{"timestamp":"2026-10-14T09:00:05Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"FileChange","id":"f","changes":{"/repo/t.rs":{"type":"add","unified_diff":"+x"}},"status":"completed"}}}"#,
            r#"{"timestamp":"2026-10-14T09:00:06Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","id":"a","content":[{"type":"output_text","text":"Added t.rs."}]}}}"#,
            r#"{"timestamp":"2026-10-14T09:00:07Z","type":"event_msg","payload":{"type":"token_count","info":{}}}"#,
        ]
        .join("\n");
        let f = codex(&r, "codex-main", Path::new("/r.jsonl")).unwrap();
        assert_eq!(
            (f.session_id.as_str(), f.cwd.as_str(), f.task.as_str()),
            ("th1", "/repo", "add a test")
        );
        assert_eq!(f.prompts, 1);
        let messages = from_log(&f.log.join("\n"));
        let roles: Vec<_> = messages.iter().map(|m| m.role).collect();
        assert_eq!(roles[0], Role::User);
        assert!(
            roles.contains(&Role::Thinking) && roles.contains(&Role::Tool),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.text.contains("cargo test")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.text.contains("t.rs")),
            "{messages:?}"
        );
        assert_eq!(messages.last().unwrap().text, "Added t.rs.");
    }

    #[test]
    fn a_codex_subagents_own_thread_is_left_out() {
        let r = [
            r#"{"type":"session_meta","payload":{"id":"th2","cwd":"/repo","source":{"subagent":{"thread_spawn":{}}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"x"}]}}}"#,
            r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","content":[{"type":"text","text":"y"}]}}}"#,
        ]
        .join("\n");
        assert!(codex(&r, "c", Path::new("/r")).is_none());
    }

    #[test]
    fn real_captures_import() {
        let t =
            include_str!("../../../fixtures/claude-2.1.299-transcript-20261015/transcript.jsonl");
        let f = claude(t, "a", Path::new("/t")).unwrap();
        assert_eq!(f.cwd, "/tmp/scratch/opts-check/work");
        assert!(
            f.task
                .starts_with("Read /tmp/scratch/opts-check/extra/secret.txt"),
            "{}",
            f.task
        );
        let shown = from_log(&f.log.join("\n"));
        assert!(shown.iter().any(|m| m.role == Role::Tool), "{shown:?}");
        assert!(
            shown.last().unwrap().text.contains("PINEAPPLE"),
            "{shown:?}"
        );

        let r = include_str!("../../../fixtures/codex-0.153.4-rollout-20261015/rollout.jsonl");
        let f = codex(r, "c", Path::new("/r")).unwrap();
        assert_eq!(f.cwd, "/tmp/scratch/opts-check/work");
        assert!(f.task.starts_with("Create the file"), "{}", f.task);
        let shown = from_log(&f.log.join("\n"));
        assert!(
            shown.iter().any(|m| m.text.contains("out.txt")),
            "{shown:?}"
        );
        assert!(
            shown.last().unwrap().text.contains("PINEAPPLE"),
            "{shown:?}"
        );
    }

    #[test]
    fn the_latest_copy_of_a_session_wins() {
        let one = |account: &str, ended: EpochSecs| Found {
            vendor: Vendor::Claude,
            account: account.into(),
            session_id: "s".into(),
            cwd: "/w".into(),
            task: "t".into(),
            name: None,
            started_at: 0,
            ended_at: ended,
            prompts: 1,
            file: PathBuf::new(),
            log: Vec::new(),
        };
        let kept = dedup(vec![one("a", 10), one("b", 20)]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].account, "b");
    }
}
