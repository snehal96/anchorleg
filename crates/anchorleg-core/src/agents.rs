//! A session's subagents in full, from what each CLI keeps on disk: what each was asked, every
//! step it took, and what it reported (`anchorleg sessions agents`).
//!
//! - Claude Code writes a subagent's own transcript next to the session's:
//!   `<config dir>/projects/<folder>/<session>/subagents/agent-<id>.jsonl`, with
//!   `agent-<id>.meta.json` (`description`, `agentType`, `toolUseId`). Its first message is the
//!   task; it reports back with a `SubagentHandback` call (or its last words).
//! - Codex gives a subagent a thread of its own, a rollout whose `session_meta` names the parent
//!   (`source.subagent.thread_spawn.parent_thread_id`); `task_complete` holds its answer.
//!
//! The live view (`sessions show`, `subagents`) says which are working; this reads the rest.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

use crate::adapters::EpochSecs;
use crate::conversation::{Message, Role, from_log};
use crate::import::{clip, codex_item, epoch, modified};

/// A transcript untouched for this long, with no report, is taken as stopped.
const QUIET: EpochSecs = 120;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Working,
    Done,
    Stopped,
}

#[derive(Debug, Clone, Serialize)]
pub struct Agent {
    /// Claude's agent id, Codex's thread id.
    pub id: String,
    /// The parent's call that started it (Claude's tool_use id), to match the live view.
    pub tool_use_id: Option<String>,
    pub name: String,
    /// Its type or role (`general-purpose`, `Explore`, a Codex role), when the CLI says.
    pub kind: Option<String>,
    pub status: Status,
    /// What it was asked to do.
    pub prompt: String,
    /// What it reported back.
    pub result: Option<String>,
    /// Its tool calls.
    pub steps: usize,
    /// What it did, as the conversation shows a session's (tools, their output, its words).
    pub messages: Vec<Message>,
    pub started_at: EpochSecs,
    pub updated_at: EpochSecs,
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.trim().lines().next().unwrap_or_default();
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max).collect::<String>())
    } else {
        line.to_owned()
    }
}

fn status(result: bool, updated: EpochSecs, now: EpochSecs) -> Status {
    if result {
        Status::Done
    } else if now - updated < QUIET {
        Status::Working
    } else {
        Status::Stopped
    }
}

fn steps(messages: &[Message]) -> usize {
    messages.iter().filter(|m| m.role == Role::Tool).count()
}

fn timestamp(v: &Value) -> Option<EpochSecs> {
    v.get("timestamp").and_then(Value::as_str).and_then(epoch)
}

/// The folders holding a Claude Code session's subagent transcripts (one per project folder the
/// session was seen in, normally one).
fn claude_dirs(config_dir: &Path, session: &str) -> Vec<PathBuf> {
    let Ok(projects) = std::fs::read_dir(config_dir.join("projects")) else {
        return Vec::new();
    };
    projects
        .flatten()
        .map(|p| p.path().join(session).join("subagents"))
        .filter(|d| d.is_dir())
        .collect()
}

/// One Claude Code subagent from its transcript and meta file.
pub fn claude_agent(
    transcript: &str,
    meta: &Value,
    id: &str,
    now: EpochSecs,
    updated: EpochSecs,
) -> Agent {
    let mut prompt = None;
    let mut result = None;
    let mut log = Vec::new();
    // The handback calls: their acknowledgement isn't one of its steps either.
    let mut handbacks: Vec<String> = Vec::new();
    let (mut started, mut last) = (None, None);
    for line in transcript.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(t) = timestamp(&v) {
            started.get_or_insert(t);
            last = Some(t);
        }
        match v.get("type").and_then(Value::as_str) {
            Some("user") => match v.pointer("/message/content") {
                // The task is its first message; later plain text is Claude Code's reminders.
                Some(Value::String(s)) => {
                    prompt.get_or_insert_with(|| s.trim().to_owned());
                }
                Some(Value::Array(blocks)) => {
                    let results: Vec<Value> = blocks
                        .iter()
                        .filter(|b| {
                            b.get("type").and_then(Value::as_str) == Some("tool_result")
                                && !b
                                    .get("tool_use_id")
                                    .and_then(Value::as_str)
                                    .is_some_and(|id| handbacks.iter().any(|h| h == id))
                        })
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
                            json!({ "type": "tool_result", "tool_use_id": b.get("tool_use_id"),
                                    "is_error": b.get("is_error"), "content": clip(&text) })
                        })
                        .collect();
                    if !results.is_empty() {
                        log.push(json!({ "type": "user", "message": { "content": results } }));
                    }
                }
                _ => {}
            },
            Some("assistant") => {
                let blocks: Vec<Value> = v
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|b| {
                        // Its report goes back to the parent: that's the result, not a step.
                        let handback = b.get("type").and_then(Value::as_str) == Some("tool_use")
                            && b.get("name").and_then(Value::as_str) == Some("SubagentHandback");
                        if handback {
                            handbacks
                                .extend(b.get("id").and_then(Value::as_str).map(str::to_owned));
                            result = b
                                .pointer("/input/message")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .or(result.take());
                        }
                        !handback && b.get("type").and_then(Value::as_str) != Some("thinking")
                    })
                    .cloned()
                    .collect();
                if !blocks.is_empty() {
                    log.push(json!({ "type": "assistant", "message": { "content": blocks } }));
                }
            }
            _ => {}
        }
    }
    let text: String = log.iter().map(|l| format!("{l}\n")).collect();
    let messages = from_log(&text);
    // Without a handback, its last words are its report.
    let result = result.or_else(|| {
        messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Agent)
            .filter(|_| {
                transcript
                    .lines()
                    .rev()
                    .find_map(|l| {
                        serde_json::from_str::<Value>(l)
                            .ok()
                            .filter(|v| v["type"] == "assistant")
                    })
                    .is_some_and(|v| {
                        v.pointer("/message/stop_reason").and_then(Value::as_str)
                            == Some("end_turn")
                    })
            })
            .map(|m| m.text.clone())
    });
    let prompt = prompt.unwrap_or_default();
    let s = |k: &str| meta.get(k).and_then(Value::as_str).map(str::to_owned);
    let updated = last.unwrap_or(updated).max(updated);
    Agent {
        id: id.to_owned(),
        tool_use_id: s("toolUseId"),
        name: s("description").unwrap_or_else(|| first_line(&prompt, 60)),
        kind: s("agentType"),
        status: status(result.is_some(), updated, now),
        steps: steps(&messages),
        prompt,
        result,
        messages,
        started_at: started.unwrap_or(updated),
        updated_at: updated,
    }
}

/// A Claude Code session's subagents, oldest first.
pub fn claude_agents(config_dir: &Path, session: &str, now: EpochSecs) -> Vec<Agent> {
    let mut out = Vec::new();
    for dir in claude_dirs(config_dir, session) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            let Some(id) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix("agent-"))
                .and_then(|n| n.strip_suffix(".jsonl"))
            else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let meta = std::fs::read_to_string(dir.join(format!("agent-{id}.meta.json")))
                .ok()
                .and_then(|m| serde_json::from_str(&m).ok())
                .unwrap_or(Value::Null);
            out.push(claude_agent(&text, &meta, id, now, modified(&path)));
        }
    }
    out.sort_by_key(|a| a.started_at);
    out
}

/// A Codex rollout's subagent, when it is one of `parent`'s.
pub fn codex_agent(
    rollout: &str,
    parent: &str,
    now: EpochSecs,
    updated: EpochSecs,
) -> Option<Agent> {
    let mut lines = rollout
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok());
    let meta = lines.find(|v| v["type"] == "session_meta")?;
    let p = &meta["payload"];
    let spawn = p.pointer("/source/subagent/thread_spawn")?;
    if spawn.get("parent_thread_id").and_then(Value::as_str) != Some(parent) {
        return None;
    }
    let id = p.get("id").and_then(Value::as_str)?.to_owned();
    let (mut prompt, mut result, mut log) = (None, None, Vec::new());
    let (mut started, mut last) = (timestamp(&meta), timestamp(&meta));
    for v in lines {
        if let Some(t) = timestamp(&v) {
            started.get_or_insert(t);
            last = Some(t);
        }
        let payload = &v["payload"];
        if v["type"] != "event_msg" {
            continue;
        }
        match payload.get("type").and_then(Value::as_str) {
            Some("item_completed") => {
                let Some(out) = payload.get("item").and_then(codex_item) else {
                    continue;
                };
                // Its task is the first message it was given; it isn't shown again as a step.
                if out["type"] == "anchorleg_user" {
                    prompt
                        .get_or_insert_with(|| out["text"].as_str().unwrap_or_default().to_owned());
                    continue;
                }
                log.push(out);
            }
            Some("task_complete") => {
                result = payload
                    .get("last_agent_message")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            _ => {}
        }
    }
    let text: String = log.iter().map(|l| format!("{l}\n")).collect();
    let messages = from_log(&text);
    let prompt = prompt.unwrap_or_default();
    let nick = spawn.get("agent_nickname").and_then(Value::as_str);
    let role = spawn
        .get("agent_role")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let updated = last.unwrap_or(updated).max(updated);
    Some(Agent {
        id,
        tool_use_id: None,
        name: match nick {
            Some(n) => format!("{n}: {}", first_line(&prompt, 60)),
            None => first_line(&prompt, 60),
        },
        kind: role,
        status: status(result.is_some(), updated, now),
        steps: steps(&messages),
        prompt,
        result,
        messages,
        started_at: started.unwrap_or(updated),
        updated_at: updated,
    })
}

/// A Codex thread's subagents, from the rollouts changed since `since` (the session's start).
pub fn codex_agents(
    codex_home: &Path,
    parent: &str,
    since: EpochSecs,
    now: EpochSecs,
) -> Vec<Agent> {
    let mut out: Vec<Agent> = crate::import::codex_files(codex_home, since)
        .into_iter()
        .filter_map(|f| {
            let text = std::fs::read_to_string(&f).ok()?;
            // Cheap check before reading it whole.
            if !text.lines().next().is_some_and(|l| l.contains(parent)) {
                return None;
            }
            codex_agent(&text, parent, now, modified(&f))
        })
        .collect();
    out.sort_by_key(|a| a.started_at);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/claude-2.1.300-subagents-20261015"
    );

    #[test]
    fn claude_subagents_from_their_transcripts() {
        // Laid out as Claude Code keeps them: next to the session's own transcript.
        let config = tempfile::tempdir().unwrap();
        let dir = config.path().join("projects/-w/s1/subagents");
        std::fs::create_dir_all(&dir).unwrap();
        for f in std::fs::read_dir(FIXTURE).unwrap().flatten() {
            if f.file_name() != "README.md" {
                std::fs::copy(f.path(), dir.join(f.file_name())).unwrap();
            }
        }
        let agents = claude_agents(config.path(), "s1", 2_000_000_000);
        let got: Vec<_> = agents
            .iter()
            .map(|a| {
                (
                    a.name.as_str(),
                    a.kind.as_deref(),
                    &a.status,
                    a.steps,
                    a.result.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (
                    "Count files",
                    Some("general-purpose"),
                    &Status::Done,
                    1,
                    Some("2 files in /tmp/scratch/sbx/subagents:\nCount: 2\nFiles: a.txt, b.txt")
                ),
                (
                    "Read a.txt",
                    Some("general-purpose"),
                    &Status::Done,
                    1,
                    Some("alpha")
                ),
            ]
        );
        let a = &agents[1];
        assert!(a.prompt.starts_with("Read the file a.txt"), "{}", a.prompt);
        assert_eq!(
            a.tool_use_id.as_deref(),
            Some("toolu_01PiFVDT9TSYVewwqohpNHhN")
        );
        // Its steps, without the handback or its acknowledgement.
        let roles: Vec<Role> = a.messages.iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::Tool, Role::Output]);
        assert!(claude_agents(config.path(), "other", 0).is_empty());
    }

    #[test]
    fn a_claude_subagent_still_at_work_or_stopped() {
        let t = r#"{"type":"user","timestamp":"2026-10-15T10:00:00Z","message":{"content":"look around"}}
{"type":"assistant","timestamp":"2026-10-15T10:00:05Z","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#;
        let at = epoch("2026-10-15T10:00:05Z").unwrap();
        let fresh = claude_agent(t, &Value::Null, "a1", at + 10, at);
        assert_eq!(
            (fresh.status, fresh.name.as_str(), fresh.steps),
            (Status::Working, "look around", 1)
        );
        assert_eq!(
            claude_agent(t, &Value::Null, "a1", at + 600, at).status,
            Status::Stopped
        );
    }

    #[test]
    fn a_codex_subagent_from_its_rollout() {
        let r =
            include_str!("../../../fixtures/codex-0.153.4-subagent-rollout-20261015/rollout.jsonl");
        let a = codex_agent(r, "01a13f37-0ab4-70c3-87d2-b14a3cf9dd4f", 2_000_000_000, 0).unwrap();
        assert_eq!(a.id, "01a13f37-2a72-7d20-84d7-7c5f9913125e");
        assert!(a.name.starts_with("Raman: Read a.txt"), "{}", a.name);
        assert_eq!(
            (&a.status, a.steps, a.result.as_deref()),
            (&Status::Done, 1, Some("alpha"))
        );
        assert!(a.prompt.contains("a.txt"));
        // Another thread's subagent isn't this one's.
        assert!(codex_agent(r, "someone-else", 0, 0).is_none());
    }
}
