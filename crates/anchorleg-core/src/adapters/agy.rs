//! Antigravity CLI (`agy -p --output-format stream-json`), verified on agy 1.3.1 (fixtures/agy-*).
//!
//! One line per event, keyed by `event`: `init` (conversation id, model), `step_update` (steps of
//! the turn: the person's input, agent text, tool calls) and `result` (status, response, error).
//! The CLI keeps its login and conversations under `$HOME/.gemini/antigravity-cli`, so one `HOME`
//! is one account. Quota isn't exposed anywhere the CLI writes, so only a failed turn tells.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::AgentSignal;

/// Where agy keeps its state, relative to `HOME`.
pub const DATA_DIR: &str = ".gemini/antigravity-cli";

/// Lets print mode edit files (headless mode can't ask, so it refuses anything not allowed).
/// Shell commands still need an allow rule or `--dangerously-skip-permissions`.
pub const ACCEPT_EDITS: [&str; 2] = ["--mode", "accept-edits"];

/// Whether the person's own args already pick how permissions work, so anchorleg leaves it alone.
pub fn sets_mode(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--mode" || a.starts_with("--mode=") || a == "--dangerously-skip-permissions")
}

/// Turns `agy -p --output-format stream-json` lines into signals.
pub fn signals(line: &str) -> Vec<AgentSignal> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let text = |p: &str| {
        v.pointer(p)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    match v.get("event").and_then(Value::as_str) {
        Some("init") => match text("/conversation_id") {
            Some(id) => vec![AgentSignal::Started {
                session_id: id,
                model: text("/init/model"),
            }],
            None => Vec::new(),
        },
        Some("result") => {
            let session_id = text("/result/conversation_id");
            if v.pointer("/result/status").and_then(Value::as_str) == Some("SUCCESS") {
                let response = text("/result/response")
                    .map(|t| t.trim().to_owned())
                    .filter(|t| !t.is_empty());
                return vec![match denied(&v) {
                    // Headless mode refused a tool and the agent stopped without an answer.
                    Some(note) if response.is_none() => AgentSignal::Finished {
                        ok: false,
                        session_id,
                        text: Some(note),
                    },
                    Some(note) => AgentSignal::Finished {
                        ok: true,
                        session_id,
                        text: response.map(|r| format!("{r}\n\n({note})")),
                    },
                    None => AgentSignal::Finished {
                        ok: true,
                        session_id,
                        text: response,
                    },
                }];
            }
            let error = text("/result/error").unwrap_or_else(|| "the turn failed".to_owned());
            if is_limit(&error) {
                vec![AgentSignal::LimitHit {
                    window: None,
                    resets_at: None,
                }]
            } else {
                vec![AgentSignal::Finished {
                    ok: false,
                    session_id,
                    text: Some(error),
                }]
            }
        }
        _ => Vec::new(),
    }
}

/// `result.denied_actions`, as a sentence for the person.
fn denied(v: &Value) -> Option<String> {
    let names: Vec<&str> = v
        .pointer("/result/denied_actions")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|a| {
            a.get("display_name")
                .or_else(|| a.get("action"))
                .and_then(Value::as_str)
        })
        .collect();
    (!names.is_empty()).then(|| {
        format!(
            "agy refused {} in headless mode; allow it under permissions.allow in agy's \
             settings.json, or `anchorleg settings antigravity --args \
             \"--dangerously-skip-permissions\"`",
            names.join(", ")
        )
    })
}

/// Whether an error reads like the account ran out. Not seen on a real limit yet (RESEARCH:
/// unverified); these are the words Google's APIs and the CLI use for quota errors.
fn is_limit(error: &str) -> bool {
    let e = error.to_lowercase();
    [
        "quota",
        "resource_exhausted",
        "resource exhausted",
        "rate limit",
        "rate_limit",
        "too many requests",
        "usage limit",
    ]
    .iter()
    .any(|k| e.contains(k))
}

/// The conversation's database in an account's `HOME`.
pub fn conversation_file(home: &Path, id: &str) -> PathBuf {
    home.join(DATA_DIR)
        .join("conversations")
        .join(format!("{id}.db"))
}

/// Whether this account can resume the conversation. `agy --conversation <unknown id>` doesn't
/// fail: it silently starts a new conversation, so this has to be checked first.
pub fn has_conversation(home: &Path, id: &str) -> bool {
    conversation_file(home, id).is_file()
}

/// Copy a conversation (database, `brain/<id>/` and annotations) into another account's `HOME`.
/// `Ok(false)` when `from` doesn't have it. Whether the other account can then resume it is
/// unverified (needs a second Google login).
pub fn sync_session(from: &Path, to: &Path, id: &str) -> std::io::Result<bool> {
    if !has_conversation(from, id) {
        return Ok(false);
    }
    if from == to {
        return Ok(true);
    }
    let (a, b) = (from.join(DATA_DIR), to.join(DATA_DIR));
    for rel in [
        format!("conversations/{id}.db"),
        format!("annotations/{id}.pbtxt"),
    ] {
        let src = a.join(&rel);
        if src.is_file() {
            let dest = b.join(&rel);
            std::fs::create_dir_all(dest.parent().expect("has a parent"))?;
            std::fs::copy(&src, &dest)?;
        }
    }
    copy_dir(&a.join("brain").join(id), &b.join("brain").join(id))?;
    Ok(true)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let (src, dest) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dest)?;
        } else {
            std::fs::copy(&src, &dest)?;
        }
    }
    Ok(())
}

/// What a finished tool step did, in the terms the handoff and the conversation view use:
/// `("Read" | "Write" | "Edit" | "Bash", path or command)`. `None` for other tools.
pub fn tool_step(v: &Value) -> Option<(&'static str, String)> {
    let step = v.get("step_update")?;
    if step.get("step_type").and_then(Value::as_str) != Some("tool")
        || step.get("state").and_then(Value::as_str) != Some("DONE")
    {
        return None;
    }
    let name = step.pointer("/tool_info/name").and_then(Value::as_str)?;
    let params = step.pointer("/tool_info/parameters")?;
    let param = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| params.get(*k).and_then(Value::as_str))
            .map(str::to_owned)
    };
    const PATH: &[&str] = &["AbsolutePath", "TargetFile", "FilePath", "Path"];
    match name {
        "view_file" => Some(("Read", param(PATH)?)),
        "write_to_file" => Some(("Write", param(PATH)?)),
        "replace_file_content" | "multi_replace_file_content" | "sed_file" | "notebook_edit" => {
            Some(("Edit", param(PATH)?))
        }
        "run_command" => Some(("Bash", param(&["CommandLine", "Command"])?)),
        _ => None,
    }
}

/// Tokens in the context at the step's model request (input + cache read).
pub fn context_tokens(v: &Value) -> Option<u64> {
    let u = v.pointer("/step_update/usage")?;
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    Some(n("input_tokens") + n("cache_read_tokens")).filter(|t| *t > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str =
        include_str!("../../../../fixtures/agy-1.3.1-probe-20261008/print-start.jsonl");
    const BAD_MODEL: &str =
        include_str!("../../../../fixtures/agy-1.3.1-probe-20261008/print-bad-model.jsonl");
    const NO_LOGIN: &str =
        include_str!("../../../../fixtures/agy-1.3.1-probe-20261008/print-not-signed-in.jsonl");

    fn run(text: &str) -> Vec<AgentSignal> {
        text.lines().flat_map(signals).collect()
    }

    #[test]
    fn a_real_run_starts_and_finishes_with_the_answer() {
        let s = run(START);
        assert_eq!(
            s.first(),
            Some(&AgentSignal::Started {
                session_id: "7d04b433-f9d9-4dca-9934-cd34fa38787f".into(),
                model: Some("gemini-3.8-flash-low".into()),
            })
        );
        assert_eq!(
            s.last(),
            Some(&AgentSignal::Finished {
                ok: true,
                session_id: Some("7d04b433-f9d9-4dca-9934-cd34fa38787f".into()),
                text: Some("FIG-4".into()),
            })
        );
    }

    #[test]
    fn errors_fail_and_quota_errors_are_limit_hits() {
        assert!(matches!(
            run(BAD_MODEL).as_slice(),
            [AgentSignal::Finished { ok: false, session_id: None, text: Some(t) }] if t.contains("no-such-model")
        ));
        assert!(matches!(
            run(NO_LOGIN).as_slice(),
            [AgentSignal::Finished { ok: false, text: Some(t), .. }] if t.contains("authentication")
        ));
        let hit = r#"{"event":"result","result":{"conversation_id":"c","status":"ERROR","error":"RESOURCE_EXHAUSTED: quota exceeded for model"}}"#;
        assert_eq!(
            run(hit),
            vec![AgentSignal::LimitHit {
                window: None,
                resets_at: None
            }]
        );
    }

    #[test]
    fn refused_tools_are_reported() {
        let denied = include_str!(
            "../../../../fixtures/agy-1.3.1-anchorleg-20261008/run-write-denied.jsonl"
        );
        assert!(matches!(
            run(denied).last(),
            Some(AgentSignal::Finished { ok: false, text: Some(t), .. }) if t.contains("refused") && t.contains("permissions.allow")
        ));
        let written = include_str!(
            "../../../../fixtures/agy-1.3.1-anchorleg-20261008/print-accept-edits-write.jsonl"
        );
        assert!(matches!(
            run(written).last(),
            Some(AgentSignal::Finished { ok: true, text: Some(t), .. }) if t == "done."
        ));
        assert!(sets_mode(&["--mode".into(), "plan".into()]));
        assert!(!sets_mode(&["--model".into()]));
    }

    #[test]
    fn tools_and_context_from_steps() {
        let steps: Vec<Value> = START
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let tools: Vec<_> = steps.iter().filter_map(tool_step).collect();
        assert_eq!(
            tools,
            [("Read", "/tmp/scratch/agyprobe/work/notes.txt".to_owned())]
        );
        assert_eq!(
            steps.iter().filter_map(context_tokens).next_back(),
            Some(2491 + 32676)
        );
    }

    #[test]
    fn conversations_are_found_and_copied_between_homes() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        let data = a.join(DATA_DIR);
        std::fs::create_dir_all(data.join("conversations")).unwrap();
        std::fs::create_dir_all(data.join("brain/c1/sub")).unwrap();
        std::fs::write(data.join("conversations/c1.db"), "db").unwrap();
        std::fs::write(data.join("brain/c1/sub/plan.md"), "plan").unwrap();
        assert!(has_conversation(&a, "c1"));
        assert!(!has_conversation(&b, "c1"));
        assert!(sync_session(&a, &b, "c1").unwrap());
        assert!(has_conversation(&b, "c1"));
        assert!(b.join(DATA_DIR).join("brain/c1/sub/plan.md").is_file());
        assert!(!sync_session(&a, &b, "nope").unwrap());
    }
}
