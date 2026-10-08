//! Claude Code headless output: `claude -p --output-format stream-json --verbose`.
//!
//! One JSON object per line. Parsing never fails: a line we can't read becomes
//! [`ClaudeEvent::Malformed`] or [`ClaudeEvent::Unrecognized`] and the run continues.
//!
//! Field names follow real captures in `fixtures/claude-*`. Every field is optional and unknown
//! fields are ignored, so a newer CLI can add fields without breaking a run.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AgentSignal, EpochSecs, QuotaReading, QuotaStatus, Window};

/// One line of Claude's stream-json output.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ClaudeEvent {
    /// `system/init`: first event of a run.
    Init(Init),
    /// `system/api_retry`: a request failed and will be retried.
    ApiRetry(ApiRetry),
    /// `rate_limit_event`.
    RateLimit(RateLimitInfo),
    /// `assistant`: a model message (text and tool calls).
    Assistant(AssistantMessage),
    /// `result`: last event of a run.
    Result(RunResult),
    /// A well-formed event anchorleg doesn't use (`user`, other `system` subtypes, new types).
    Other { kind: String },
    /// A known event type whose fields didn't have the expected shape.
    Unrecognized { kind: String, error: String },
    /// Not a JSON object with a `type`.
    Malformed { error: String },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Init {
    pub session_id: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, rename = "apiKeySource")]
    pub api_key_source: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ApiRetry {
    #[serde(default)]
    pub attempt: Option<u32>,
    #[serde(default)]
    pub max_retries: Option<u32>,
    #[serde(default)]
    pub retry_delay_ms: Option<u64>,
    #[serde(default)]
    pub error_status: Option<u16>,
    /// Error category, e.g. `rate_limit`.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitInfo {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub resets_at: Option<EpochSecs>,
    #[serde(default)]
    pub rate_limit_type: Option<String>,
    /// 0.0–1.0. Often missing while the status is "allowed" (anthropics/claude-code#78476).
    #[serde(default)]
    pub utilization: Option<f64>,
    #[serde(default)]
    pub surpassed_threshold: Option<f64>,
    /// Per-window detail, keyed by window name (`five_hour`, `seven_day`).
    #[serde(default)]
    pub unified_windows: Option<std::collections::BTreeMap<String, WindowInfo>>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub utilization: Option<f64>,
    #[serde(default)]
    pub resets_at: Option<EpochSecs>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct AssistantMessage {
    #[serde(default)]
    pub session_id: Option<String>,
    pub message: MessageBody,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct MessageBody {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        #[serde(default)]
        id: Option<String>,
        name: String,
        #[serde(default)]
        input: Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct RunResult {
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub num_turns: Option<u32>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    /// Set instead of `result` when the run failed before a turn, e.g. a bad `--resume` id.
    #[serde(default)]
    pub errors: Vec<String>,
}

/// Parse one line of stream-json. Never fails and never panics.
pub fn parse_line(line: &str) -> ClaudeEvent {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return malformed(e.to_string()),
    };
    let Some(kind) = value.get("type").and_then(Value::as_str).map(str::to_owned) else {
        return malformed("no string `type` field".to_owned());
    };
    let subtype = value.get("subtype").and_then(Value::as_str);

    let parsed = match (kind.as_str(), subtype) {
        ("system", Some("init")) => typed(value, ClaudeEvent::Init),
        ("system", Some("api_retry")) => typed(value, ClaudeEvent::ApiRetry),
        ("rate_limit_event", _) => match value.get("rate_limit_info") {
            Some(info) => typed(info.clone(), ClaudeEvent::RateLimit),
            None => Err("no `rate_limit_info`".to_owned()),
        },
        ("assistant", _) => typed(value, ClaudeEvent::Assistant),
        ("result", _) => typed(value, ClaudeEvent::Result),
        _ => {
            let kind = match subtype {
                Some(sub) => format!("{kind}/{sub}"),
                None => kind,
            };
            return ClaudeEvent::Other { kind };
        }
    };
    parsed.unwrap_or_else(|error| {
        tracing::warn!(%kind, %error, "unexpected shape in claude event");
        ClaudeEvent::Unrecognized { kind, error }
    })
}

fn typed<T: serde::de::DeserializeOwned>(
    value: Value,
    wrap: fn(T) -> ClaudeEvent,
) -> Result<ClaudeEvent, String> {
    serde_json::from_value(value)
        .map(wrap)
        .map_err(|e| e.to_string())
}

fn malformed(error: String) -> ClaudeEvent {
    tracing::warn!(%error, "malformed claude stream line");
    ClaudeEvent::Malformed { error }
}

/// What this event means for the supervisor. Most events mean nothing; a rejected rate limit
/// means both a quota reading and a limit hit.
pub fn signals(event: &ClaudeEvent) -> Vec<AgentSignal> {
    match event {
        ClaudeEvent::Init(init) => vec![AgentSignal::Started {
            session_id: init.session_id.clone(),
            model: init.model.clone(),
        }],
        ClaudeEvent::ApiRetry(retry) => vec![AgentSignal::Retrying {
            category: retry.error.clone(),
            attempt: retry.attempt,
        }],
        ClaudeEvent::RateLimit(info) => rate_limit_signals(info),
        ClaudeEvent::Result(result) => result_signals(result),
        ClaudeEvent::Assistant(_)
        | ClaudeEvent::Other { .. }
        | ClaudeEvent::Unrecognized { .. }
        | ClaudeEvent::Malformed { .. } => Vec::new(),
    }
}

fn rate_limit_signals(info: &RateLimitInfo) -> Vec<AgentSignal> {
    let mut out = Vec::new();
    let mut top_window = info.rate_limit_type.as_deref().map(Window::from_name);

    let status = info.status.as_deref().map(QuotaStatus::from_name);

    // Per-window detail first: it's the most precise. Real windows (2.1.294) carry only
    // utilization and resetsAt; the top-level status belongs to the `rateLimitType` window.
    for (name, w) in info.unified_windows.iter().flatten() {
        let window = Window::from_name(name);
        let status = match w.status.as_deref() {
            Some(s) => Some(QuotaStatus::from_name(s)),
            None if top_window.as_ref() == Some(&window) => status.clone(),
            None => None,
        };
        if status == Some(QuotaStatus::Rejected) && top_window.is_none() {
            top_window = Some(window.clone());
        }
        out.push(AgentSignal::Quota(QuotaReading {
            window: Some(window),
            status,
            used: w.utilization,
            resets_at: w.resets_at,
        }));
    }

    // Skip the top-level reading when a per-window reading already covers its window.
    let covered = out.iter().any(
        |s| matches!(s, AgentSignal::Quota(q) if top_window.is_some() && q.window == top_window),
    );
    if out.is_empty() || !covered {
        out.insert(
            0,
            AgentSignal::Quota(QuotaReading {
                window: top_window.clone(),
                status: status.clone(),
                used: info.utilization,
                resets_at: info.resets_at,
            }),
        );
    }

    if status == Some(QuotaStatus::Rejected) {
        out.push(AgentSignal::LimitHit {
            window: top_window,
            resets_at: info.resets_at,
        });
    }
    out
}

fn result_signals(result: &RunResult) -> Vec<AgentSignal> {
    let mut out = Vec::new();
    let text = result
        .result
        .clone()
        .or_else(|| (!result.errors.is_empty()).then(|| result.errors.join("; ")));
    if result.is_error
        && let Some(t) = text.as_deref()
    {
        if let Some(resets_at) = limit_message(t) {
            out.push(AgentSignal::LimitHit {
                window: None,
                resets_at,
            });
        }
        if t.contains("No conversation found with session ID") {
            out.push(AgentSignal::SessionNotFound);
        }
    }
    out.push(AgentSignal::Finished {
        ok: !result.is_error,
        session_id: result.session_id.clone(),
        text,
    });
    out
}

/// If `text` is a usage-limit message, returns `Some(resets_at)` (the reset time when the
/// message carries one as `…|<epoch>`). Unverified wording; Phase 0 will pin it down.
fn limit_message(text: &str) -> Option<Option<EpochSecs>> {
    let lower = text.to_lowercase();
    let is_limit = ["usage limit reached", "hit your limit", "limit reached"]
        .iter()
        .any(|p| lower.contains(p));
    if !is_limit {
        return None;
    }
    let resets_at = text
        .rsplit_once('|')
        .and_then(|(_, tail)| tail.trim().parse::<EpochSecs>().ok());
    Some(resets_at)
}

/// What [`sync_context`] copied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ContextSync {
    /// The session transcript (and its subagent transcripts) now exist in the target dir.
    pub session: bool,
    /// Project memory files copied because they were missing or older in the target.
    pub memory_files: usize,
}

/// Carry a session's context into another account's config dir before resuming there
/// (D11, D14).
///
/// Finds `<from>/projects/*/<id>.jsonl` and copies, to the same place under `to`: the
/// transcript, its `<id>/` folder of subagent transcripts, and the project's `memory/` folder.
/// Memory is merged: a file is copied only when it's missing in `to` or older there; nothing is
/// deleted. Does nothing when the session isn't in `from` or both dirs are the same.
pub fn sync_context(from: &Path, to: &Path, session_id: &str) -> std::io::Result<ContextSync> {
    let mut done = ContextSync::default();
    if from == to {
        return Ok(done);
    }
    let file_name = format!("{session_id}.jsonl");
    let Ok(entries) = std::fs::read_dir(from.join("projects")) else {
        return Ok(done);
    };
    for project in entries.flatten() {
        let src = project.path().join(&file_name);
        if !src.is_file() {
            continue;
        }
        let dest_dir = to.join("projects").join(project.file_name());
        std::fs::create_dir_all(&dest_dir)?;
        std::fs::copy(&src, dest_dir.join(&file_name))?;
        let sub = project.path().join(session_id);
        if sub.is_dir() {
            merge_dir(&sub, &dest_dir.join(session_id))?;
        }
        done.session = true;
        let memory = project.path().join("memory");
        if memory.is_dir() {
            done.memory_files = merge_dir(&memory, &dest_dir.join("memory"))?;
        }
        return Ok(done);
    }
    Ok(done)
}

/// `<config_dir>/projects/*/<id>.jsonl`, if the session is there.
pub fn session_file(config_dir: &Path, session_id: &str) -> Option<std::path::PathBuf> {
    let file_name = format!("{session_id}.jsonl");
    std::fs::read_dir(config_dir.join("projects"))
        .ok()?
        .flatten()
        .map(|p| p.path().join(&file_name))
        .find(|f| f.is_file())
}

/// Copy files from `from` into `to` that are missing there or older there. Returns how many
/// were copied. Never deletes.
fn merge_dir(from: &Path, to: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(to)?;
    let mut copied = 0;
    for entry in std::fs::read_dir(from)?.flatten() {
        let dest = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copied += merge_dir(&entry.path(), &dest)?;
        } else if kind.is_file() {
            let newer = match std::fs::metadata(&dest) {
                Ok(d) => entry.metadata()?.modified()? > d.modified()?,
                Err(_) => true,
            };
            if newer {
                std::fs::copy(entry.path(), &dest)?;
                copied += 1;
            }
        }
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syncs_session_and_memory_between_config_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        let pa = a.join("projects/-repo");
        let pb = b.join("projects/-repo");
        std::fs::create_dir_all(pa.join("s1/subagents")).unwrap();
        std::fs::create_dir_all(pa.join("memory")).unwrap();
        std::fs::create_dir_all(pb.join("memory")).unwrap();
        std::fs::write(pa.join("s1.jsonl"), "{}\n").unwrap();
        std::fs::write(pa.join("s1/subagents/x.jsonl"), "{}\n").unwrap();
        // b's own note is newer than a's copy: keep b's. a's other note is new to b: copy it.
        std::fs::write(pa.join("memory/shared.md"), "from a").unwrap();
        std::fs::write(pa.join("memory/only-a.md"), "a only").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(pb.join("memory/shared.md"), "newer in b").unwrap();
        std::fs::write(pb.join("memory/only-b.md"), "b only").unwrap();

        let sync = sync_context(&a, &b, "s1").unwrap();
        assert_eq!(
            sync,
            ContextSync {
                session: true,
                memory_files: 1
            }
        );
        assert!(pb.join("s1.jsonl").is_file());
        assert!(pb.join("s1/subagents/x.jsonl").is_file());
        let read = |p: &str| std::fs::read_to_string(pb.join(p)).unwrap();
        assert_eq!(read("memory/shared.md"), "newer in b");
        assert_eq!(read("memory/only-a.md"), "a only");
        assert_eq!(read("memory/only-b.md"), "b only");

        assert_eq!(
            sync_context(&a, &b, "missing").unwrap(),
            ContextSync::default()
        );
        assert_eq!(sync_context(&a, &a, "s1").unwrap(), ContextSync::default());
    }

    #[test]
    fn garbage_never_panics() {
        for line in [
            "",
            "   ",
            "not json",
            "[]",
            "42",
            "{}",
            r#"{"type":7}"#,
            r#"{"type":"result","is_error":"yes"}"#,
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"rate_limit_event"}"#,
        ] {
            let event = parse_line(line);
            let _ = signals(&event);
        }
    }

    #[test]
    fn unknown_type_is_other() {
        let e = parse_line(r#"{"type":"brand_new","x":1}"#);
        assert_eq!(
            e,
            ClaudeEvent::Other {
                kind: "brand_new".into()
            }
        );
    }

    #[test]
    fn rejected_rate_limit_is_a_limit_hit() {
        let e = parse_line(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1791460800,"rateLimitType":"five_hour"}}"#,
        );
        let s = signals(&e);
        assert!(s.contains(&AgentSignal::LimitHit {
            window: Some(Window::FiveHour),
            resets_at: Some(1791460800),
        }));
    }

    #[test]
    fn allowed_without_utilization_is_not_a_limit_hit() {
        let e = parse_line(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#);
        let s = signals(&e);
        assert_eq!(s.len(), 1);
        assert!(matches!(&s[0], AgentSignal::Quota(q) if q.used.is_none()));
    }

    #[test]
    fn window_detail_is_not_duplicated() {
        let e = parse_line(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","rateLimitType":"five_hour","utilization":0.91,"unifiedWindows":{"five_hour":{"utilization":0.91},"seven_day":{"utilization":0.4}}}}"#,
        );
        let windows: Vec<_> = signals(&e)
            .into_iter()
            .filter_map(|s| match s {
                AgentSignal::Quota(q) => q.window,
                _ => None,
            })
            .collect();
        assert_eq!(windows, vec![Window::FiveHour, Window::SevenDay]);
    }

    #[test]
    fn limit_text_in_result() {
        assert_eq!(
            limit_message("Claude AI usage limit reached|1791460800"),
            Some(Some(1791460800))
        );
        assert_eq!(
            limit_message("You've hit your limit · resets 3pm"),
            Some(None)
        );
        assert_eq!(limit_message("Done. All tests pass."), None);
    }

    #[test]
    fn successful_result_finishes_without_limit() {
        let e = parse_line(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"usage limit reached in docs only","session_id":"s1"}"#,
        );
        assert_eq!(
            signals(&e),
            vec![AgentSignal::Finished {
                ok: true,
                session_id: Some("s1".into()),
                text: Some("usage limit reached in docs only".into()),
            }]
        );
    }
}
