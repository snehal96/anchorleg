//! Vendor-neutral handoff (D4, D5): what a session did, read from its transcript with no LLM
//! call, written to `.handoff/TASK.md` in the working folder so any agent can pick the task up.
//!
//! The transcript is Claude's session file (`projects/<dir>/<id>.jsonl`) or anchorleg's run log;
//! both carry `assistant` / `user` lines with `message.content` blocks. A run log can also hold
//! `codex exec --json` lines (`item.completed`, `turn.completed`) and `agy` stream-json lines
//! (keyed by `event`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The folder anchorleg writes in the working directory.
pub const DIR: &str = ".handoff";
/// The section agents keep themselves; anchorleg carries it over when it rewrites the file.
const NOTES: &str = "## Notes";
const NOTES_HINT: &str = "Decisions made, approaches tried and dropped (and why). \
Keep this section up to date as you work; anchorleg keeps it when it rewrites the file.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    InProgress,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub text: String,
    pub status: StepStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transcript {
    /// Files the agent wrote or edited, first touch first, with the tool that touched them.
    pub files: Vec<(String, &'static str)>,
    /// Files the agent read, first read first.
    pub read: Vec<String>,
    /// Shell commands, oldest first.
    pub commands: Vec<String>,
    /// The agent's last task list (TodoWrite, or TaskCreate/TaskUpdate).
    pub steps: Vec<Step>,
    /// The agent's last visible text.
    pub last_message: Option<String>,
    /// Tokens in the context at the last model request (input + cache read + cache write).
    pub context_tokens: Option<u64>,
}

/// Read a transcript. Lines that aren't JSON, and subagent (sidechain) lines, are skipped.
pub fn extract(log: &str) -> Transcript {
    let mut t = Transcript::default();
    // TaskCreate: tool_use id → subject, then the result's "Task #N" → that subject.
    let mut pending_tasks: HashMap<String, String> = HashMap::new();
    let mut tasks: Vec<(String, Step)> = Vec::new();
    let mut todo_steps: Option<Vec<Step>> = None;

    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || v.get("parent_tool_use_id").is_some_and(|p| !p.is_null())
        {
            continue;
        }
        match v.get("event").and_then(Value::as_str) {
            Some("step_update") => {
                if let Some(n) = crate::adapters::agy::context_tokens(&v) {
                    t.context_tokens = Some(n);
                }
                if let Some((tool, arg)) = crate::adapters::agy::tool_step(&v) {
                    match tool {
                        "Read" => {
                            if !t.read.contains(&arg) {
                                t.read.push(arg);
                            }
                        }
                        "Bash" => t.commands.push(arg.trim().to_owned()),
                        op => {
                            if !t.files.iter().any(|(p, _)| *p == arg) {
                                t.files
                                    .push((arg, if op == "Write" { "written" } else { "edited" }));
                            }
                        }
                    }
                }
                continue;
            }
            Some("result") => {
                if let Some(s) = v
                    .pointer("/result/response")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                {
                    t.last_message = Some(s.trim().to_owned());
                }
                continue;
            }
            _ => {}
        }
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                if let Some(u) = v.pointer("/message/usage") {
                    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                    let total = n("input_tokens")
                        + n("cache_read_input_tokens")
                        + n("cache_creation_input_tokens");
                    if total > 0 {
                        t.context_tokens = Some(total);
                    }
                }
                for block in blocks(&v) {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let s = block.get("text").and_then(Value::as_str).unwrap_or("");
                            if !s.trim().is_empty() {
                                t.last_message = Some(s.trim().to_owned());
                            }
                        }
                        Some("tool_use") => tool_use(
                            block,
                            &mut t,
                            &mut pending_tasks,
                            &mut tasks,
                            &mut todo_steps,
                        ),
                        _ => {}
                    }
                }
            }
            Some("item.completed" | "item.updated") => codex_item(&v, &mut t, &mut todo_steps),
            Some("turn.completed") => {
                // Codex counts cached input inside `input_tokens`.
                if let Some(n) = v.pointer("/usage/input_tokens").and_then(Value::as_u64)
                    && n > 0
                {
                    t.context_tokens = Some(n);
                }
            }
            Some("user") => {
                for block in blocks(&v) {
                    let id = block.get("tool_use_id").and_then(Value::as_str);
                    if let Some(subject) = id.and_then(|id| pending_tasks.remove(id))
                        && let Some(n) = task_number(&content_text(block))
                    {
                        tasks.push((
                            n,
                            Step {
                                text: subject,
                                status: StepStatus::Pending,
                            },
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    // Whichever list the agent used last wins; TodoWrite replaces its whole list each time.
    t.steps = todo_steps.unwrap_or_else(|| tasks.into_iter().map(|(_, s)| s).collect());
    t
}

fn codex_item(v: &Value, t: &mut Transcript, todo_steps: &mut Option<Vec<Step>>) {
    let Some(item) = v.get("item") else { return };
    let field = |k: &str| item.get(k).and_then(Value::as_str);
    let done = v.get("type").and_then(Value::as_str) == Some("item.completed");
    match field("type") {
        Some("agent_message") if done => {
            if let Some(s) = field("text").filter(|s| !s.trim().is_empty()) {
                t.last_message = Some(s.trim().to_owned());
            }
        }
        Some("command_execution") if done => {
            if let Some(cmd) = field("command") {
                t.commands.push(
                    crate::adapters::codex::unwrap_command(cmd)
                        .trim()
                        .to_owned(),
                );
            }
        }
        Some("file_change") if done => {
            for change in item
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(path) = change.get("path").and_then(Value::as_str) else {
                    continue;
                };
                let op = match change.get("kind").and_then(Value::as_str) {
                    Some("add") => "written",
                    Some("delete") => "deleted",
                    _ => "edited",
                };
                if !t.files.iter().any(|(p, _)| p == path) {
                    t.files.push((path.to_owned(), op));
                }
            }
        }
        Some("todo_list") => {
            let steps = item
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|todo| {
                    let text = todo.get("text").and_then(Value::as_str)?;
                    let done = todo.get("completed").and_then(Value::as_bool) == Some(true);
                    Some(Step {
                        text: text.to_owned(),
                        status: if done {
                            StepStatus::Done
                        } else {
                            StepStatus::Pending
                        },
                    })
                })
                .collect();
            *todo_steps = Some(steps);
        }
        _ => {}
    }
}

fn tool_use(
    block: &Value,
    t: &mut Transcript,
    pending_tasks: &mut HashMap<String, String>,
    tasks: &mut Vec<(String, Step)>,
    todo_steps: &mut Option<Vec<Step>>,
) {
    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
    let input = block.get("input").unwrap_or(&Value::Null);
    let field = |k: &str| input.get(k).and_then(Value::as_str);
    match name {
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
            if let Some(path) = field("file_path").or_else(|| field("notebook_path")) {
                let op = if name == "Write" { "written" } else { "edited" };
                if !t.files.iter().any(|(p, _)| p == path) {
                    t.files.push((path.to_owned(), op));
                }
            }
        }
        "Read" => {
            if let Some(path) = field("file_path")
                && !t.read.iter().any(|p| p == path)
            {
                t.read.push(path.to_owned());
            }
        }
        "Bash" => {
            if let Some(cmd) = field("command") {
                t.commands.push(cmd.trim().to_owned());
            }
        }
        "TodoWrite" => {
            let steps = input
                .get("todos")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|todo| {
                    let text = todo.get("content").and_then(Value::as_str)?;
                    Some(Step {
                        text: text.to_owned(),
                        status: status(todo.get("status").and_then(Value::as_str)),
                    })
                })
                .collect();
            *todo_steps = Some(steps);
        }
        "TaskCreate" => {
            if let (Some(id), Some(subject)) =
                (block.get("id").and_then(Value::as_str), field("subject"))
            {
                pending_tasks.insert(id.to_owned(), subject.to_owned());
            }
            *todo_steps = None;
        }
        "TaskUpdate" => {
            let Some(id) = input.get("taskId").map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            }) else {
                return;
            };
            if field("status") == Some("deleted") {
                tasks.retain(|(n, _)| *n != id);
            } else if let Some((_, step)) = tasks.iter_mut().find(|(n, _)| *n == id) {
                if let Some(s) = field("status") {
                    step.status = status(Some(s));
                }
                if let Some(subject) = field("subject") {
                    subject.clone_into(&mut step.text);
                }
            }
            *todo_steps = None;
        }
        _ => {}
    }
}

fn status(s: Option<&str>) -> StepStatus {
    match s {
        Some("completed") => StepStatus::Done,
        Some("in_progress") => StepStatus::InProgress,
        _ => StepStatus::Pending,
    }
}

/// "Task #23 created successfully" → "23".
fn task_number(text: &str) -> Option<String> {
    let rest = &text[text.find('#')? + 1..];
    let n: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!n.is_empty()).then_some(n)
}

fn blocks(v: &Value) -> impl Iterator<Item = &Value> {
    v.pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
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

/// Everything `TASK.md` is made of.
#[derive(Debug)]
pub struct Handoff<'a> {
    /// The task, then each message sent into the session after it.
    pub goals: &'a [String],
    pub transcript: &'a Transcript,
    /// The working folder; paths inside it are shown relative to it.
    pub cwd: &'a Path,
    /// The account that stopped, and why.
    pub from: &'a str,
    pub reason: &'a str,
    /// `git status --short` of the working tree.
    pub git_status: Option<&'a str>,
    /// The checkpoint ref holding the working tree at the stop.
    pub checkpoint: Option<&'a str>,
    /// The agents' own notes from the previous `TASK.md`.
    pub notes: Option<&'a str>,
}

const MAX_COMMANDS: usize = 15;
const MAX_STATUS_LINES: usize = 40;

pub fn render(h: &Handoff) -> String {
    let rel = |p: &str| -> String {
        let prefix = format!("{}/", h.cwd.display().to_string().trim_end_matches('/'));
        p.strip_prefix(&prefix).unwrap_or(p).to_owned()
    };
    let mut s = String::from("# Task handoff\n\n");
    s += &format!(
        "<!-- Written by anchorleg when {} stopped ({}). Any agent can continue from this file: \
         read it, check the working tree, finish the task. -->\n\n",
        h.from, h.reason
    );

    s += "## Goal\n\n";
    match h.goals {
        [] => s += "(not recorded)\n",
        [task, rest @ ..] => {
            s += task.trim();
            s += "\n";
            if !rest.is_empty() {
                s += "\nLater messages from the user, in order:\n\n";
                for m in rest {
                    s += &format!("- {}\n", one_line(m, 300));
                }
            }
        }
    }

    let t = h.transcript;
    s += "\n## Steps\n\n";
    if t.steps.is_empty() {
        s += "(the agent kept no task list)\n";
    }
    for step in &t.steps {
        let (mark, tail) = match step.status {
            StepStatus::Done => ("x", ""),
            StepStatus::InProgress => (" ", " (in progress when it stopped)"),
            StepStatus::Pending => (" ", ""),
        };
        s += &format!("- [{mark}] {}{tail}\n", one_line(&step.text, 200));
    }

    s += "\n## Where it stopped\n\n";
    match &t.last_message {
        Some(m) => {
            s += "The agent's last message:\n\n";
            for line in m.lines().take(30) {
                s += &format!("> {line}\n");
            }
        }
        None => s += "(no message from the agent)\n",
    }

    s += "\n## Files changed\n\n";
    if t.files.is_empty() {
        s += "(none through the agent's edit tools)\n";
    }
    for (path, op) in &t.files {
        s += &format!("- `{}` ({op})\n", rel(path));
    }
    if let Some(status) = h.git_status.filter(|st| !st.trim().is_empty()) {
        s += "\n`git status --short`:\n\n```\n";
        for line in status.lines().take(MAX_STATUS_LINES) {
            s += line;
            s += "\n";
        }
        if status.lines().count() > MAX_STATUS_LINES {
            s += "…\n";
        }
        s += "```\n";
    }

    if !t.read.is_empty() {
        s += "\n## Files read\n\n";
        let skip = t.read.len().saturating_sub(MAX_COMMANDS);
        if skip > 0 {
            s += &format!("({skip} earlier files left out)\n");
        }
        for path in &t.read[skip..] {
            s += &format!("- `{}`\n", rel(path));
        }
    }

    s += "\n## Commands run\n\n";
    if t.commands.is_empty() {
        s += "(none)\n";
    }
    let skip = t.commands.len().saturating_sub(MAX_COMMANDS);
    if skip > 0 {
        s += &format!("({skip} earlier commands left out)\n");
    }
    for c in &t.commands[skip..] {
        s += &format!("- `{}`\n", one_line(c, 200).replace('`', "'"));
    }

    if let Some(r) = h.checkpoint {
        s += &format!(
            "\n## Checkpoint\n\nThe working tree at the stop is saved as `{r}` (local only). \
             `git diff {r}` shows what changed since.\n"
        );
    }

    s += &format!("\n{NOTES}\n\n");
    match h.notes.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => s += n,
        None => s += NOTES_HINT,
    }
    s += "\n";
    s
}

/// The agents' notes in an existing `TASK.md`: everything after `## Notes`, minus anchorleg's hint.
pub fn notes_of(task_md: &str) -> Option<String> {
    let at = task_md
        .match_indices(NOTES)
        .find(|(i, _)| *i == 0 || task_md[..*i].ends_with('\n'))?
        .0;
    let notes = task_md[at + NOTES.len()..].trim().replace(NOTES_HINT, "");
    let notes = notes.trim();
    (!notes.is_empty()).then(|| notes.to_owned())
}

/// Write `<cwd>/.handoff/TASK.md`, keeping the notes from the one already there. The folder
/// ignores itself in git so the handoff never ends up in the user's commits.
pub fn write(
    cwd: &Path,
    render_with_notes: impl FnOnce(Option<&str>) -> String,
) -> std::io::Result<PathBuf> {
    let dir = cwd.join(DIR);
    std::fs::create_dir_all(&dir)?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n")?;
    }
    let path = dir.join("TASK.md");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let content = render_with_notes(notes_of(&old).as_deref());
    let tmp = dir.join("TASK.md.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

fn one_line(s: &str, max: usize) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() > max {
        format!("{}…", joined.chars().take(max).collect::<String>())
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(content: &str, usage: &str) -> String {
        format!(r#"{{"type":"assistant","message":{{"content":[{content}],"usage":{usage}}}}}"#)
    }

    #[test]
    fn extracts_files_commands_tasks_and_context() {
        let log = [
            assistant(
                r#"{"type":"text","text":"Plan first."},{"type":"tool_use","id":"t1","name":"TaskCreate","input":{"subject":"Fix parser"}},{"type":"tool_use","id":"t2","name":"TaskCreate","input":{"subject":"Add tests"}}"#,
                r#"{"input_tokens":5,"cache_read_input_tokens":1000}"#,
            ),
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"Task #4 created successfully: Fix parser"},{"type":"tool_result","tool_use_id":"t2","content":[{"type":"text","text":"Task #5 created successfully"}]}]}}"#.to_owned(),
            assistant(
                r#"{"type":"tool_use","id":"t3","name":"TaskUpdate","input":{"taskId":"4","status":"completed"}},{"type":"tool_use","id":"t4","name":"TaskUpdate","input":{"taskId":"5","status":"in_progress"}},{"type":"tool_use","id":"t5","name":"Edit","input":{"file_path":"/repo/src/parse.rs"}},{"type":"tool_use","id":"t6","name":"Write","input":{"file_path":"/repo/tests/p.rs"}},{"type":"tool_use","id":"t7","name":"Edit","input":{"file_path":"/repo/src/parse.rs"}},{"type":"tool_use","id":"t8","name":"Bash","input":{"command":"cargo test\n"}}"#,
                r#"{"input_tokens":2,"cache_read_input_tokens":1000,"cache_creation_input_tokens":500}"#,
            ),
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent"}]}}"#.to_owned(),
            assistant(r#"{"type":"text","text":"Tests compile; two still fail."}"#, "{}"),
            "garbage".to_owned(),
        ]
        .join("\n");
        let t = extract(&log);
        assert_eq!(
            t.files,
            vec![
                ("/repo/src/parse.rs".to_owned(), "edited"),
                ("/repo/tests/p.rs".to_owned(), "written")
            ]
        );
        assert_eq!(t.commands, vec!["cargo test"]);
        assert_eq!(
            t.steps,
            vec![
                Step {
                    text: "Fix parser".into(),
                    status: StepStatus::Done
                },
                Step {
                    text: "Add tests".into(),
                    status: StepStatus::InProgress
                },
            ]
        );
        assert_eq!(
            t.last_message.as_deref(),
            Some("Tests compile; two still fail.")
        );
        assert_eq!(t.context_tokens, Some(1502));
    }

    #[test]
    fn reads_the_real_mod_stop_capture() {
        let t = extract(include_str!(
            "../../../fixtures/claude-2.1.294-mod-stop-20261008/run-sm-then-two.jsonl"
        ));
        assert_eq!(t.last_message.as_deref(), Some("MANGO-17"));
        assert_eq!(t.read.len(), 1);
        assert!(t.context_tokens.is_some_and(|n| n > 10_000));
    }

    #[test]
    fn reads_a_codex_run_log() {
        let log = [
            include_str!("../../../fixtures/codex-0.153.4-probe-20261008/exec-start.jsonl"),
            r#"{"type":"item.completed","item":{"id":"i9","type":"file_change","changes":[{"path":"/repo/a.rs","kind":"update"},{"path":"/repo/b.rs","kind":"add"}],"status":"completed"}}"#,
            r#"{"type":"item.updated","item":{"id":"i8","type":"todo_list","items":[{"text":"read","completed":true},{"text":"fix","completed":false}]}}"#,
        ]
        .join("\n");
        let t = extract(&log);
        assert_eq!(t.last_message.as_deref(), Some("PLUM-9"));
        assert_eq!(
            t.commands.last().map(String::as_str),
            Some(r"find . -name notes.txt -type f -exec sed -n '1,120p' {} \;")
        );
        assert_eq!(
            t.files,
            [
                ("/repo/a.rs".to_owned(), "edited"),
                ("/repo/b.rs".to_owned(), "written")
            ]
        );
        assert_eq!(t.steps[0].status, StepStatus::Done);
        assert_eq!(t.steps[1].text, "fix");
        assert_eq!(t.context_tokens, Some(45373));
    }

    #[test]
    fn reads_an_agy_run() {
        let t = extract(include_str!(
            "../../../fixtures/agy-1.3.1-probe-20261008/print-start.jsonl"
        ));
        assert_eq!(t.read, ["/tmp/scratch/agyprobe/work/notes.txt"]);
        assert_eq!(t.last_message.as_deref(), Some("FIG-4"));
        assert_eq!(t.context_tokens, Some(2491 + 32676));
        let edit = r#"{"event":"step_update","step_update":{"state":"DONE","step_type":"tool","tool_info":{"name":"write_to_file","parameters":{"TargetFile":"/w/a.txt"}}}}"#;
        assert_eq!(extract(edit).files, [("/w/a.txt".to_owned(), "written")]);
    }

    #[test]
    fn reads_a_real_codex_run_through_anchorleg() {
        let t = extract(include_str!(
            "../../../fixtures/codex-0.153.4-anchorleg-20261008/run-write-file.jsonl"
        ));
        assert_eq!(
            t.files,
            [(
                "/tmp/scratch/codex-real/work/answer.txt".to_owned(),
                "written"
            )]
        );
        assert_eq!(t.commands.len(), 2);
        assert_eq!(t.last_message.as_deref(), Some("FIG-4"));
        assert_eq!(t.context_tokens, Some(64519));
    }

    #[test]
    fn todo_write_replaces_the_list() {
        let log = [
            assistant(
                r#"{"type":"tool_use","name":"TodoWrite","input":{"todos":[{"content":"a","status":"pending"}]}}"#,
                "{}",
            ),
            assistant(
                r#"{"type":"tool_use","name":"TodoWrite","input":{"todos":[{"content":"a","status":"completed"},{"content":"b","status":"pending"}]}}"#,
                "{}",
            ),
        ]
        .join("\n");
        let steps = extract(&log).steps;
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].status, StepStatus::Done);
    }

    #[test]
    fn renders_and_keeps_notes() {
        let t = Transcript {
            files: vec![("/repo/src/a.rs".into(), "edited")],
            read: vec!["/repo/README.md".into()],
            commands: vec!["cargo test".into()],
            steps: vec![Step {
                text: "Fix a".into(),
                status: StepStatus::InProgress,
            }],
            last_message: Some("Halfway.".into()),
            context_tokens: None,
        };
        let goals = ["fix the tests".to_owned(), "also run clippy".to_owned()];
        let md = render(&Handoff {
            goals: &goals,
            transcript: &t,
            cwd: Path::new("/repo"),
            from: "claude-sm",
            reason: "limit reached",
            git_status: Some(" M src/a.rs\n"),
            checkpoint: Some("refs/anchorleg/run-3/1"),
            notes: None,
        });
        insta::assert_snapshot!(md);
        assert_eq!(notes_of(&md), None);

        let edited = format!("{md}\n- Tried a regex; too slow.\n");
        assert_eq!(
            notes_of(&edited).as_deref(),
            Some("- Tried a regex; too slow.")
        );
    }

    #[test]
    fn write_keeps_the_agents_notes() {
        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::default();
        let h = |notes: Option<&str>| {
            render(&Handoff {
                goals: &[],
                transcript: &t,
                cwd: dir.path(),
                from: "a",
                reason: "r",
                git_status: None,
                checkpoint: None,
                notes,
            })
        };
        let path = write(dir.path(), h).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{first}\nDropped approach X.\n")).unwrap();
        write(dir.path(), h).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .ends_with("Dropped approach X.\n")
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".handoff/.gitignore")).unwrap(),
            "*\n"
        );
    }
}
