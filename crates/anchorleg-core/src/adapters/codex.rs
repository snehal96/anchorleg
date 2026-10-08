//! Codex CLI (`codex exec --json`), verified on codex-cli 0.153.4 (fixtures/codex-*).
//!
//! The stream has no quota, so it's read from the rollout file the CLI writes for every thread:
//! `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<thread_id>.jsonl`, whose `token_count`
//! events carry `rate_limits`. One `CODEX_HOME` is one login.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{AgentSignal, EpochSecs, QuotaReading, QuotaStatus, Window};

/// Where the ChatGPT desktop app keeps its copy of the official CLI.
pub const BUNDLED_BIN: &str = "/Applications/ChatGPT.app/Contents/Resources/codex";

/// `-c` value that lets `codex exec` edit the working folder (its default is read-only).
pub const WORKSPACE_WRITE: &str = "sandbox_mode=\"workspace-write\"";

/// Whether the person's own args already pick a sandbox, so anchorleg leaves it alone.
pub fn sets_sandbox(args: &[String]) -> bool {
    args.iter().any(|a| {
        a == "-s"
            || a.starts_with("--sandbox")
            || a.contains("sandbox_mode")
            || a.starts_with("--dangerously-bypass-approvals")
            || a == "--full-auto"
    })
}

/// Turns `codex exec --json` lines into signals. Stateful: the answer is the last
/// `agent_message`, and the turn's end only says whether it succeeded.
#[derive(Debug, Default)]
pub struct Stream {
    thread_id: Option<String>,
    last_text: Option<String>,
}

impl Stream {
    /// One stdout line; garbage and unknown events give no signals.
    pub fn feed(&mut self, line: &str) -> Vec<AgentSignal> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        let text = |p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_owned);
        match v.get("type").and_then(Value::as_str) {
            Some("thread.started") => match text("/thread_id") {
                Some(id) => {
                    self.thread_id = Some(id.clone());
                    vec![AgentSignal::Started {
                        session_id: id,
                        model: None,
                    }]
                }
                None => Vec::new(),
            },
            Some("item.completed") => {
                if v.pointer("/item/type").and_then(Value::as_str) == Some("agent_message")
                    && let Some(t) = text("/item/text").filter(|t| !t.trim().is_empty())
                {
                    self.last_text = Some(t.trim().to_owned());
                }
                Vec::new()
            }
            Some("turn.completed") => vec![AgentSignal::Finished {
                ok: true,
                session_id: self.thread_id.clone(),
                text: self.last_text.clone(),
            }],
            Some("turn.failed") => {
                let message = text("/error/message").unwrap_or_default();
                match limit_reset(&message) {
                    Some(resets_at) => vec![AgentSignal::LimitHit {
                        window: None,
                        resets_at,
                    }],
                    None => vec![AgentSignal::Finished {
                        ok: false,
                        session_id: self.thread_id.clone(),
                        text: Some(message),
                    }],
                }
            }
            Some("error") => {
                let message = text("/message").unwrap_or_default();
                if let Some(rest) = message.strip_prefix("Reconnecting... ") {
                    // "Reconnecting... 2/5 (…)"
                    let attempt = rest.split('/').next().and_then(|n| n.trim().parse().ok());
                    vec![AgentSignal::Retrying {
                        category: Some("connection".to_owned()),
                        attempt,
                    }]
                } else if let Some(resets_at) = limit_reset(&message) {
                    vec![AgentSignal::LimitHit {
                        window: None,
                        resets_at,
                    }]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }
}

/// `Some(reset)` when an error says the account is out of quota. Not yet seen on a real limit
/// (RESEARCH: unverified), so it matches the wording Codex and OpenAI use for usage and rate
/// limits; the rollout file gives the exact reset time afterwards.
fn limit_reset(message: &str) -> Option<Option<EpochSecs>> {
    let m = message.to_lowercase();
    // No bare "429": request ids and cf-ray values in these messages can contain it.
    let hit = [
        "usage limit",
        "rate limit",
        "rate_limit",
        "too many requests",
        "quota",
    ]
    .iter()
    .any(|k| m.contains(k));
    hit.then_some(None)
}

/// The account's `CODEX_HOME`.
pub fn sessions_dir(codex_home: &Path) -> PathBuf {
    codex_home.join("sessions")
}

/// The rollout file for a thread, searched under `sessions/` (date folders, a few levels deep).
pub fn rollout_file(codex_home: &Path, thread_id: &str) -> Option<PathBuf> {
    let suffix = format!("-{thread_id}.jsonl");
    find(&sessions_dir(codex_home), &suffix, 4)
}

fn find(dir: &Path, suffix: &str, depth: usize) -> Option<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    // Newest date folders first: a running thread is almost always today's.
    entries.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
    for e in entries {
        let path = e.path();
        if path.is_dir() {
            if depth > 0
                && let Some(found) = find(&path, suffix, depth - 1)
            {
                return Some(found);
            }
        } else if e.file_name().to_string_lossy().ends_with(suffix) {
            return Some(path);
        }
    }
    None
}

/// Quota from a rollout file's last `token_count` with rate limits.
pub fn rollout_quota(rollout: &str) -> Vec<QuotaReading> {
    let Some(limits) = rollout
        .lines()
        .rev()
        .filter(|l| l.contains("\"rate_limits\""))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|v| v.pointer("/payload/rate_limits").cloned())
        .filter(|l| l.is_object())
    else {
        return Vec::new();
    };
    readings(
        &limits,
        [
            "window_minutes",
            "used_percent",
            "resets_at",
            "rate_limit_reached_type",
        ],
    )
}

/// Quota from an `account/rateLimits/read` result (camelCase twin of the rollout's).
pub fn live_quota(result: &Value) -> Vec<QuotaReading> {
    match result.get("rateLimits").filter(|l| l.is_object()) {
        Some(limits) => readings(
            limits,
            [
                "windowDurationMins",
                "usedPercent",
                "resetsAt",
                "rateLimitReachedType",
            ],
        ),
        None => Vec::new(),
    }
}

/// `primary` / `secondary` windows, with the field names `[minutes, used %, resets at, reached]`.
fn readings(
    limits: &Value,
    [minutes_key, used_key, resets_key, reached_key]: [&str; 4],
) -> Vec<QuotaReading> {
    let reached = limits.get(reached_key).is_some_and(|r| !r.is_null());
    let windows: Vec<(Window, f64, Option<EpochSecs>)> = ["primary", "secondary"]
        .iter()
        .filter_map(|k| limits.get(*k).filter(|w| w.is_object()))
        .map(|w| {
            let minutes = w.get(minutes_key).and_then(Value::as_i64).unwrap_or(0);
            let used = w.get(used_key).and_then(Value::as_f64).unwrap_or(0.0) / 100.0;
            (
                window(minutes),
                used.clamp(0.0, 1.0),
                w.get(resets_key).and_then(Value::as_i64),
            )
        })
        .collect();
    // Which window ran out isn't named in a form we've seen yet: the full one, else the first.
    let full = windows.iter().position(|(_, used, _)| *used >= 1.0);
    windows
        .iter()
        .enumerate()
        .map(|(i, (w, used, resets_at))| QuotaReading {
            window: Some(w.clone()),
            status: Some(if *used >= 1.0 || (reached && full.unwrap_or(0) == i) {
                QuotaStatus::Rejected
            } else {
                QuotaStatus::Allowed
            }),
            used: Some(*used),
            resets_at: *resets_at,
        })
        .collect()
}

/// Codex names windows by length: 5 hours and a week map to relay's usual ones.
fn window(minutes: i64) -> Window {
    match minutes {
        300 => Window::FiveHour,
        10_080 => Window::SevenDay,
        m if m > 0 && m % 1440 == 0 => Window::Other(format!("{}d", m / 1440)),
        m => Window::Other(format!("{m}m")),
    }
}

/// Ask the CLI for the login's current quota, without a model request: `codex app-server`
/// (stdio JSON-RPC), `initialize`, then `account/rateLimits/read`. `cmd` is the account's
/// `codex` with its env; this adds the subcommand. ~2-3 s on a real login.
pub async fn read_live_quota(
    mut cmd: tokio::process::Command,
    timeout: std::time::Duration,
) -> Result<Vec<QuotaReading>, String> {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
    cmd.arg("app-server")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("starting codex app-server: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let requests = [
        serde_json::json!({ "id": 1, "method": "initialize",
            "params": { "clientInfo": { "name": "anchorleg", "version": env!("CARGO_PKG_VERSION") } } }),
        serde_json::json!({ "method": "initialized" }),
        serde_json::json!({ "id": 2, "method": "account/rateLimits/read" }),
    ];
    let mut text = String::new();
    for r in requests {
        text.push_str(&format!("{r}\n"));
    }
    stdin
        .write_all(text.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stdin.flush().await.map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(stdout).lines();
    let read = async {
        while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("id").and_then(Value::as_i64) != Some(2) {
                continue;
            }
            if let Some(err) = v.pointer("/error/message").and_then(Value::as_str) {
                return Err(err.to_owned());
            }
            return Ok(live_quota(v.get("result").unwrap_or(&Value::Null)));
        }
        Err("codex app-server closed without an answer".to_owned())
    };
    let out = tokio::time::timeout(timeout, read)
        .await
        .unwrap_or_else(|_| Err("codex app-server didn't answer in time".to_owned()));
    let _ = child.start_kill();
    let _ = child.wait().await;
    out
}

/// Copy a thread's rollout file into another `CODEX_HOME`, at the same relative path, so that
/// login can `codex exec resume` it. `Ok(false)` when the thread isn't in `from`.
pub fn sync_session(from: &Path, to: &Path, thread_id: &str) -> std::io::Result<bool> {
    if from == to {
        return Ok(rollout_file(from, thread_id).is_some());
    }
    let Some(src) = rollout_file(from, thread_id) else {
        return Ok(false);
    };
    let rel = src
        .strip_prefix(from)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let dest = to.join(rel);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::copy(&src, &dest)?;
    Ok(true)
}

/// `/bin/zsh -lc "cargo test"` → `cargo test`: the shell wrapper Codex puts around commands.
pub fn unwrap_command(command: &str) -> String {
    for shell in [
        "/bin/zsh -lc ",
        "/bin/bash -lc ",
        "bash -lc ",
        "zsh -lc ",
        "/bin/sh -c ",
    ] {
        if let Some(rest) = command.strip_prefix(shell) {
            let rest = rest.trim();
            if let Ok(words) = shlex::split(rest).ok_or(())
                && words.len() == 1
            {
                return words[0].clone();
            }
            return rest.trim_matches(|c| c == '"' || c == '\'').to_owned();
        }
    }
    command.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str =
        include_str!("../../../../fixtures/codex-0.153.4-probe-20261008/exec-start.jsonl");
    const NO_LOGIN: &str =
        include_str!("../../../../fixtures/codex-0.153.4-probe-20261008/exec-not-logged-in.jsonl");
    const ROLLOUT: &str =
        include_str!("../../../../fixtures/codex-0.153.4-probe-20261008/rollout-token-count.jsonl");

    fn run(text: &str) -> Vec<AgentSignal> {
        let mut s = Stream::default();
        text.lines().flat_map(|l| s.feed(l)).collect()
    }

    #[test]
    fn a_real_run_starts_and_finishes_with_the_answer() {
        let signals = run(START);
        assert_eq!(
            signals.first(),
            Some(&AgentSignal::Started {
                session_id: "01a11b6d-7aac-7310-a0ee-1cd4c7d43544".into(),
                model: None
            })
        );
        assert_eq!(
            signals.last(),
            Some(&AgentSignal::Finished {
                ok: true,
                session_id: Some("01a11b6d-7aac-7310-a0ee-1cd4c7d43544".into()),
                text: Some("PLUM-9".into()),
            })
        );
    }

    #[test]
    fn not_logged_in_retries_then_fails_without_a_limit() {
        let signals = run(NO_LOGIN);
        assert!(
            signals
                .iter()
                .any(|s| matches!(s, AgentSignal::Retrying { .. }))
        );
        assert!(matches!(
            signals.last(),
            Some(AgentSignal::Finished { ok: false, text: Some(t), .. }) if t.contains("401")
        ));
        assert!(
            !signals
                .iter()
                .any(|s| matches!(s, AgentSignal::LimitHit { .. }))
        );
    }

    #[test]
    fn a_usage_limit_failure_is_a_limit_hit() {
        let line = r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit. Try again in 3 hours."}}"#;
        assert_eq!(
            run(line),
            vec![AgentSignal::LimitHit {
                window: None,
                resets_at: None
            }]
        );
        assert!(run("not json\n{\"type\":\"item.started\"}").is_empty());
    }

    #[test]
    fn quota_from_the_real_rollout() {
        let q = rollout_quota(ROLLOUT);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].window, Some(Window::Other("30d".into())));
        assert_eq!(q[0].status, Some(QuotaStatus::Allowed));
        assert_eq!(q[0].used, Some(0.0));
        assert!(q[0].resets_at.is_some());
    }

    #[test]
    fn quota_windows_and_a_reached_limit() {
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":100.0,"window_minutes":300,"resets_at":500},"secondary":{"used_percent":40.0,"window_minutes":10080,"resets_at":900},"rate_limit_reached_type":"rate_limit_reached"}}}"#;
        let q = rollout_quota(line);
        assert_eq!(q[0].window, Some(Window::FiveHour));
        assert_eq!(q[0].status, Some(QuotaStatus::Rejected));
        assert_eq!(q[0].resets_at, Some(500));
        assert_eq!(q[1].window, Some(Window::SevenDay));
        assert_eq!(q[1].status, Some(QuotaStatus::Allowed));
        assert_eq!(q[1].used, Some(0.4));
        assert!(rollout_quota("{}").is_empty());
    }

    #[test]
    fn live_quota_from_the_app_server() {
        let result = serde_json::json!({ "rateLimits": {
            "limitId": "codex",
            "primary": { "usedPercent": 100, "windowDurationMins": 300, "resetsAt": 500 },
            "secondary": { "usedPercent": 7, "windowDurationMins": 10080, "resetsAt": 900 },
            "planType": "plus",
            "rateLimitReachedType": "rate_limit_reached"
        }});
        let q = live_quota(&result);
        assert_eq!(q[0].window, Some(Window::FiveHour));
        assert_eq!(q[0].status, Some(QuotaStatus::Rejected));
        assert_eq!(q[1].used, Some(0.07));
        assert_eq!(q[1].status, Some(QuotaStatus::Allowed));
        assert!(live_quota(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn live_quota_from_the_real_reply() {
        let text = include_str!(
            "../../../../fixtures/codex-0.153.4-app-server-20261008/rate-limits-read.jsonl"
        );
        let reply: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        let q = live_quota(&reply["result"]);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].window, Some(Window::Other("30d".into())));
        assert_eq!(q[0].used, Some(0.01));
        assert_eq!(q[0].status, Some(QuotaStatus::Allowed));
    }

    #[test]
    fn sessions_are_found_and_copied_between_homes() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        let day = a.join("sessions/2026/10/08");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("rollout-2026-10-08T17-42-02-t1.jsonl"), "{}").unwrap();
        assert!(rollout_file(&a, "t1").is_some());
        assert!(rollout_file(&b, "t1").is_none());
        assert!(sync_session(&a, &b, "t1").unwrap());
        assert!(
            b.join("sessions/2026/10/08/rollout-2026-10-08T17-42-02-t1.jsonl")
                .is_file()
        );
        assert!(!sync_session(&a, &b, "nope").unwrap());
    }

    #[test]
    fn shell_wrappers_are_removed() {
        assert_eq!(unwrap_command(r#"/bin/zsh -lc "cargo test""#), "cargo test");
        assert_eq!(
            unwrap_command(r#"/bin/zsh -lc "find . -name notes.txt""#),
            "find . -name notes.txt"
        );
        assert_eq!(unwrap_command("ls -la"), "ls -la");
    }
}
