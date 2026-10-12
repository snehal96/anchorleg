//! Antigravity CLI (`agy -p --output-format stream-json`), verified on agy 1.3.1 (fixtures/agy-*).
//!
//! One line per event, keyed by `event`: `init` (conversation id, model), `step_update` (steps of
//! the turn: the person's input, agent text, tool calls) and `result` (status, response, error).
//! The CLI keeps its login and conversations under `$HOME/.gemini/antigravity-cli`, so one `HOME`
//! is one account. Quota isn't exposed anywhere the CLI writes, so only a failed turn tells.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{AgentSignal, Usage};

/// Where agy keeps its state, relative to `HOME`.
pub const DATA_DIR: &str = ".gemini/antigravity-cli";

/// Lets print mode edit files (headless mode can't ask, so it refuses anything not allowed).
/// Shell commands still need an allow rule or `--dangerously-skip-permissions`.
pub const ACCEPT_EDITS: [&str; 2] = ["--mode", "accept-edits"];

/// Lets agy run every tool without asking. anchorleg passes it only when its own hook is in
/// place in that account's `HOME` (D26): the hook asks the person and denies what they refuse.
/// In print mode a hook's "allow" can't lift agy's refusal, but its "deny" blocks a call.
pub const SKIP_PERMISSIONS: &str = "--dangerously-skip-permissions";

/// agy's global hooks file, relative to `HOME` (read by agy 1.3.1).
pub const HOOKS_FILE: &str = ".gemini/config/hooks.json";

/// The name of anchorleg's hook in that file; other hooks there are left alone.
pub const HOOK_NAME: &str = "anchorleg";

/// How long agy waits for the hook, in seconds: longer than anchorleg waits for an answer
/// (30 minutes), so the hook always answers first.
pub const HOOK_TIMEOUT_SECS: u64 = 35 * 60;

/// Whether the person's own args already pick how permissions work, so anchorleg leaves it alone.
pub fn sets_mode(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--mode" || a.starts_with("--mode=") || a == SKIP_PERMISSIONS)
}

/// The hook entry anchorleg keeps in `hooks.json`: every tool call goes through
/// `anchorleg permission agy-hook`, and if that can't run the call is denied.
fn hook_entry(bin: &Path) -> Value {
    let quoted = format!("'{}'", bin.display().to_string().replace('\'', r"'\''"));
    serde_json::json!({
        "PreToolUse": [{
            "matcher": "*",
            "hooks": [{
                "type": "command",
                "command": format!(
                    r#"{quoted} permission agy-hook || echo '{{"decision":"deny","reason":"anchorleg could not be asked"}}'"#
                ),
                "timeout": HOOK_TIMEOUT_SECS,
            }],
        }],
    })
}

/// Put anchorleg's hook into this `HOME`'s global `hooks.json`, keeping any other hooks. Fails
/// rather than overwrite a file that isn't a JSON object. Only for an account's own `HOME`,
/// never the person's (D26).
pub fn install_hook(home: &Path, bin: &Path) -> std::io::Result<()> {
    let path = home.join(HOOKS_FILE);
    let mut hooks = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(m)) => m,
            _ => {
                return Err(std::io::Error::other(format!(
                    "{} isn't a JSON object; not touching it",
                    path.display()
                )));
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(e) => return Err(e),
    };
    let entry = hook_entry(bin);
    if hooks.get(HOOK_NAME) == Some(&entry) {
        return Ok(());
    }
    hooks.insert(HOOK_NAME.to_owned(), entry);
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    let tmp = path.with_extension("json.anchorleg-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&Value::Object(hooks))?)?;
    std::fs::rename(&tmp, &path)
}

/// What anchorleg's hook does with one agy tool call.
#[derive(Debug, PartialEq)]
pub enum HookVerdict {
    /// Reads and edits inside the task's folders, and agy's own bookkeeping: no question.
    Allow,
    /// Ask the person, showing the call as `tool` with `input` (Claude's tool names where one
    /// fits, so `anchorleg ui` shows it the same way).
    Ask { tool: String, input: Value },
}

/// agy tools that only read.
const READ_TOOLS: &[&str] = &["view_file", "list_dir", "find_by_name", "grep_search"];
/// agy tools that edit files.
const EDIT_TOOLS: &[&str] = &[
    "write_to_file",
    "replace_file_content",
    "multi_replace_file_content",
    "sed_file",
    "notebook_edit",
];
/// agy's own bookkeeping: nothing outside the conversation changes.
const QUIET_TOOLS: &[&str] = &[
    "command_status",
    "wait",
    "wait_5_seconds",
    "finish",
    "ask_question",
    "list_permissions",
    "manage_task",
];

/// Decide on a `PreToolUse` request (agy's hook input: `toolCall {name, args}`,
/// `workspacePaths`, `artifactDirectoryPath`). Like `--mode accept-edits`, reads and edits
/// inside the workspace (and agy's own artifact folder) go ahead; shell commands, the browser,
/// the web, MCP tools and anything outside those folders are asked about.
pub fn hook_verdict(req: &Value) -> HookVerdict {
    let name = req
        .pointer("/toolCall/name")
        .and_then(Value::as_str)
        .unwrap_or("unknown tool");
    let args = req
        .pointer("/toolCall/args")
        .cloned()
        .unwrap_or(Value::Null);
    if QUIET_TOOLS.contains(&name) {
        return HookVerdict::Allow;
    }
    let mut roots: Vec<PathBuf> = req
        .get("workspacePaths")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(PathBuf::from)
        .collect();
    if let Some(dir) = req.get("artifactDirectoryPath").and_then(Value::as_str) {
        roots.push(PathBuf::from(dir));
    }
    let paths = absolute_paths(&args);
    let inside = |p: &Path| roots.iter().any(|r| within(p, r));
    let kind = if READ_TOOLS.contains(&name) {
        Some("Read")
    } else if EDIT_TOOLS.contains(&name) {
        Some(if name == "write_to_file" {
            "Write"
        } else {
            "Edit"
        })
    } else {
        None
    };
    if let Some(kind) = kind {
        match paths.iter().find(|p| !inside(p)) {
            None => return HookVerdict::Allow,
            Some(outside) => {
                return HookVerdict::Ask {
                    tool: kind.to_owned(),
                    input: serde_json::json!({ "file_path": outside.display().to_string() }),
                };
            }
        }
    }
    if name == "run_command"
        && let Some(command) = args.get("CommandLine").and_then(Value::as_str)
    {
        let mut input = serde_json::json!({ "command": command });
        if let Some(cwd) = args.get("Cwd").and_then(Value::as_str) {
            input["cwd"] = cwd.into();
        }
        return HookVerdict::Ask {
            tool: "Bash".to_owned(),
            input,
        };
    }
    HookVerdict::Ask {
        tool: name.to_owned(),
        input: args,
    }
}

/// Every absolute path among a tool call's string arguments.
fn absolute_paths(args: &Value) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Value::Object(m) = args {
        for v in m.values() {
            match v {
                Value::String(s) if s.starts_with('/') => out.push(PathBuf::from(s)),
                Value::Array(items) => out.extend(
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|s| s.starts_with('/'))
                        .map(PathBuf::from),
                ),
                _ => {}
            }
        }
    }
    out
}

/// Whether `path` is `root` or inside it, after resolving `.`, `..` and symlinks (macOS's
/// `/tmp` is `/private/tmp`) as far as the path exists.
fn within(path: &Path, root: &Path) -> bool {
    let (p, r) = (resolve(path), resolve(root));
    p.starts_with(&r)
}

fn resolve(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut clean = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    // Canonicalize the longest part that exists, then add the rest back.
    let mut existing = clean.clone();
    let mut rest = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(n) => rest.push(n.to_owned()),
            None => return clean,
        }
        if !existing.pop() {
            return clean;
        }
    }
    let mut out = std::fs::canonicalize(&existing).unwrap_or(existing);
    out.extend(rest.iter().rev());
    out
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
            let mut out = usage(&v)
                .map(AgentSignal::Usage)
                .into_iter()
                .collect::<Vec<_>>();
            out.extend(result_signals(&v, &text));
            out
        }
        _ => Vec::new(),
    }
}

/// The run's tokens from `result.usage`. agy counts cache reads inside `input_tokens`.
fn usage(v: &Value) -> Option<Usage> {
    let u = v.pointer("/result/usage")?;
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let cached = n("cache_read_tokens");
    Some(Usage {
        model: None,
        input_tokens: n("input_tokens").saturating_sub(cached),
        output_tokens: n("output_tokens") + n("thinking_tokens"),
        cache_read_tokens: cached,
        cache_write_tokens: 0,
        cost_usd: None,
    })
    .filter(|u| !u.is_empty())
}

/// `Finished` or `LimitHit` from a `result` event.
fn result_signals(v: &Value, text: &dyn Fn(&str) -> Option<String>) -> Vec<AgentSignal> {
    let session_id = text("/result/conversation_id");
    if v.pointer("/result/status").and_then(Value::as_str) == Some("SUCCESS") {
        let response = text("/result/response")
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty());
        return vec![match denied(v) {
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
             settings.json, or give this account its own HOME so anchorleg can ask you",
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

    fn hook_req(name: &str, args: Value) -> Value {
        serde_json::json!({
            "toolCall": { "name": name, "args": args },
            "workspacePaths": ["/w/repo"],
            "artifactDirectoryPath": "/h/.gemini/antigravity-cli/brain/c1",
            "conversationId": "c1",
        })
    }

    #[test]
    fn the_hook_lets_reads_and_edits_in_the_workspace_through_and_asks_about_the_rest() {
        use serde_json::json;
        let allow = |name, args| hook_verdict(&hook_req(name, args)) == HookVerdict::Allow;
        assert!(allow(
            "view_file",
            json!({ "AbsolutePath": "/w/repo/src/lib.rs" })
        ));
        assert!(allow(
            "write_to_file",
            json!({ "TargetFile": "/w/repo/new.rs", "CodeContent": "x" })
        ));
        assert!(allow(
            "write_to_file",
            json!({ "TargetFile": "/h/.gemini/antigravity-cli/brain/c1/plan.md" })
        ));
        assert!(allow("command_status", json!({ "CommandId": "1" })));
        assert_eq!(
            hook_verdict(&hook_req(
                "write_to_file",
                json!({ "TargetFile": "/w/repo/../other/x.rs" })
            )),
            HookVerdict::Ask {
                tool: "Write".into(),
                input: json!({ "file_path": "/w/repo/../other/x.rs" })
            }
        );
        assert_eq!(
            hook_verdict(&hook_req(
                "view_file",
                json!({ "AbsolutePath": "/etc/hosts" })
            )),
            HookVerdict::Ask {
                tool: "Read".into(),
                input: json!({ "file_path": "/etc/hosts" })
            }
        );
        assert_eq!(
            hook_verdict(&hook_req(
                "run_command",
                json!({ "CommandLine": "cargo test", "Cwd": "/w/repo" })
            )),
            HookVerdict::Ask {
                tool: "Bash".into(),
                input: json!({ "command": "cargo test", "cwd": "/w/repo" })
            }
        );
        assert_eq!(
            hook_verdict(&hook_req("search_web", json!({ "query": "x" }))),
            HookVerdict::Ask {
                tool: "search_web".into(),
                input: json!({ "query": "x" })
            }
        );
    }

    #[test]
    fn the_hook_is_added_next_to_other_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(HOOKS_FILE);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"lint":{"PostToolUse":[]}}"#).unwrap();
        install_hook(dir.path(), Path::new("/opt/it's/anchorleg")).unwrap();
        install_hook(dir.path(), Path::new("/opt/it's/anchorleg")).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(v.get("lint").is_some());
        let cmd = v
            .pointer("/anchorleg/PreToolUse/0/hooks/0/command")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(
            cmd.starts_with(r"'/opt/it'\''s/anchorleg' permission agy-hook || echo"),
            "{cmd}"
        );
        assert_eq!(
            v.pointer("/anchorleg/PreToolUse/0/hooks/0/timeout"),
            Some(&HOOK_TIMEOUT_SECS.into())
        );

        std::fs::write(&file, "not json").unwrap();
        assert!(install_hook(dir.path(), Path::new("/bin/anchorleg")).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "not json");
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
