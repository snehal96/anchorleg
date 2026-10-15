//! anchorleg's desktop app: a window over the CLI it ships with (D30, D31). Commands here are thin: run
//! `anchorleg … --json` off the UI thread and hand the result to the React side.

mod appmenu;
mod cli;
mod notices;
mod paste;
mod settings;
mod signin;
mod tray;

use serde::Deserialize;
use serde_json::Value;

/// Run a blocking CLI call off the UI thread.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn cli_info() -> cli::CliInfo {
    tauri::async_runtime::spawn_blocking(cli::info)
        .await
        .expect("cli_info task")
}

#[tauri::command]
async fn sessions(limit: Option<u32>) -> Result<Value, String> {
    let limit = limit.unwrap_or(50).to_string();
    blocking(move || cli::json(&["sessions", "--json", "--limit", &limit])).await
}

/// `anchorleg sessions agents <run> --json`: the session's subagents in full.
#[tauri::command]
async fn agents(run: i64) -> Result<Value, String> {
    let run = run.to_string();
    blocking(move || cli::json(&["sessions", "agents", &run, "--json"])).await
}

/// `anchorleg sessions show <run> --since N --json`: one session's conversation from message N.
#[tauri::command]
async fn session(run: i64, since: Option<usize>) -> Result<Value, String> {
    let (run, since) = (run.to_string(), since.unwrap_or(0).to_string());
    blocking(move || cli::json(&["sessions", "show", &run, "--since", &since, "--json"])).await
}

/// A new run in the background (`anchorleg run --detach --json`): a new session in `cwd`, a reply
/// to a finished one (`follow_up`), or a new session from another's history (`fork`).
/// Also: a side question (`btw`), going back to before a run (`rewind`), a review of a session
/// (`review`, `fix` sends the findings back), and a new session's goal, mode or plan-first.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Start {
    text: String,
    cwd: Option<String>,
    follow_up: Option<i64>,
    fork: Option<i64>,
    btw: Option<i64>,
    rewind: Option<i64>,
    /// With `rewind` or `fork`: a point inside that run (a message's `at`).
    at: Option<String>,
    keep_files: bool,
    review: Option<i64>,
    fix: bool,
    goal: Option<String>,
    mode: Option<String>,
    plan: bool,
    /// Start on this account (`--prefer`), or use only it (`--account`).
    prefer: Option<String>,
    account: Option<String>,
    /// In a git worktree of its own.
    worktree: bool,
    name: Option<String>,
    attach: Vec<String>,
    /// The model (`--model`), the CLI to use (`--cli`), how to pick accounts (`--policy`).
    model: Option<String>,
    cli: Option<String>,
    policy: Option<String>,
    /// Most tokens the task may use, e.g. `500k` (`--budget`).
    budget: Option<String>,
    /// At most this many turns toward the goal (`--goal-turns`).
    goal_turns: Option<u32>,
    /// How long a question waits for the person, e.g. `10m` (`--answer-wait`).
    answer_wait: Option<String>,
    /// More folders the agent may use (`--add-dir`), the task's own instructions, effort.
    add_dir: Vec<String>,
    instructions: Option<String>,
    effort: Option<String>,
    /// Claude Code's own options (other CLIs leave them out).
    agent: Option<String>,
    allowed_tools: Vec<String>,
    disallowed_tools: Vec<String>,
    mcp_config: Vec<String>,
    strict_mcp_config: bool,
    fallback_model: Option<String>,
}

impl Start {
    fn args(self) -> Vec<String> {
        let mut args: Vec<String> = vec!["run".into(), "--detach".into(), "--json".into()];
        let mut opt = |flag: &str, v: Option<String>| {
            if let Some(v) = v {
                args.extend([flag.to_owned(), v]);
            }
        };
        opt("--cwd", self.cwd);
        opt("--follow-up", self.follow_up.map(|i| i.to_string()));
        opt("--fork", self.fork.map(|i| i.to_string()));
        opt("--btw", self.btw.map(|i| i.to_string()));
        opt("--rewind", self.rewind.map(|i| i.to_string()));
        opt("--at", self.at);
        opt("--review", self.review.map(|i| i.to_string()));
        opt("--goal", self.goal);
        opt("--mode", self.mode);
        opt("--prefer", self.prefer);
        opt("--account", self.account);
        opt("--name", self.name);
        opt("--model", self.model);
        opt("--cli", self.cli);
        opt("--policy", self.policy);
        opt("--budget", self.budget);
        opt("--goal-turns", self.goal_turns.map(|n| n.to_string()));
        opt("--answer-wait", self.answer_wait);
        opt("--instructions", self.instructions);
        opt("--effort", self.effort);
        opt("--agent", self.agent);
        opt("--fallback-model", self.fallback_model);
        for (flag, values) in [
            ("--add-dir", self.add_dir),
            ("--allowed-tools", self.allowed_tools),
            ("--disallowed-tools", self.disallowed_tools),
            ("--mcp-config", self.mcp_config),
        ] {
            for v in values.into_iter().filter(|v| !v.trim().is_empty()) {
                args.extend([flag.to_owned(), v]);
            }
        }
        if self.strict_mcp_config {
            args.push("--strict-mcp-config".into());
        }
        for f in self.attach {
            args.extend(["--attach".into(), f]);
        }
        if self.keep_files {
            args.push("--keep-files".into());
        }
        if self.fix {
            args.push("--fix".into());
        }
        if self.plan {
            args.push("--plan".into());
        }
        // Last: `--worktree` takes an optional name, and `--` ends it.
        if self.worktree {
            args.push("--worktree".into());
        }
        args.extend(["--".into(), self.text]);
        args
    }
}

#[tauri::command]
async fn start(req: Start) -> Result<Value, String> {
    let args = req.args();
    blocking(move || cli::json(&args)).await
}

/// Open a web link (`http(s)` only) in the browser, or an existing file in the app the system
/// picks for it. Anything else is refused: the target comes from what an agent or MCP server said.
#[tauri::command]
async fn open_target(target: String) -> Result<(), String> {
    let ok = target.starts_with("https://")
        || target.starts_with("http://")
        || std::path::Path::new(&target).is_file();
    if !ok {
        return Err(format!("not a web link or a file: {target}"));
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&target)
        .spawn()
        .map(drop)
        .map_err(|e| e.to_string())
}

/// The session's folder compared with how it found it (`anchorleg diff --git`), as a unified
/// diff: catches what shell commands changed too. The files the CLIs reported come with the
/// session (`changes`).
#[tauri::command]
async fn diff(run: i64) -> Result<String, String> {
    blocking(move || cli::text(&["diff".to_owned(), run.to_string(), "--git".to_owned()])).await
}

/// `preview` (the files, nothing changes), `undo` or `redo` a session's changes.
#[tauri::command]
async fn undo(run: i64, action: String) -> Result<Value, String> {
    blocking(move || {
        let mut args = vec!["undo".to_owned(), run.to_string(), "--json".to_owned()];
        match action.as_str() {
            "preview" => {}
            "undo" => args.push("--yes".into()),
            "redo" => args.push("--redo".into()),
            other => return Err(format!("unknown undo action {other}")),
        }
        cli::json(&args)
    })
    .await
}

/// `!cmd`: run a command in the session's folder, as `anchorleg ui` does: `sh -c`, up to 60 s,
/// stdout and stderr together, at most 8 KB. Its output goes with the next message.
#[tauri::command]
async fn shell(cwd: String, command: String) -> Result<String, String> {
    blocking(move || cli::shell(std::path::Path::new(&cwd), &command)).await
}

/// A message for a session that is working (`anchorleg send`); `interrupt` stops its turn first.
#[tauri::command]
async fn send(run: i64, text: String, interrupt: bool, attach: Vec<String>) -> Result<(), String> {
    let mut args: Vec<String> = vec!["send".into(), run.to_string()];
    if interrupt {
        args.push("--interrupt".into());
    }
    for f in attach {
        args.extend(["--attach".into(), f]);
    }
    args.extend(["--".into(), text]);
    blocking(move || cli::text(&args).map(drop)).await
}

#[tauri::command]
async fn stop(run: i64) -> Result<(), String> {
    blocking(move || cli::text(&["stop".to_owned(), run.to_string()]).map(drop)).await
}

/// Name a session; an empty name clears it.
#[tauri::command]
async fn rename(run: i64, name: String) -> Result<(), String> {
    blocking(move || cli::text(&["rename".to_owned(), run.to_string(), name]).map(drop)).await
}

/// Tool calls, questions and plans waiting for the person (`anchorleg permission list --json`).
#[tauri::command]
async fn permissions() -> Result<Value, String> {
    blocking(|| cli::json(&["permission", "list", "--json"])).await
}

/// Answer a request: `yes`, `always`, `rule` or `no` for a tool call; one answer per question
/// (an option label, several joined with commas, or the person's own words; `-` for a secret).
/// `skip` answers no / skips the questions.
#[tauri::command]
async fn answer(id: i64, answers: Vec<String>, skip: bool) -> Result<(), String> {
    let mut args: Vec<String> = vec!["permission".into(), "answer".into(), id.to_string()];
    if skip {
        args.push("--skip".into());
    } else {
        args.push("--".into());
        args.extend(answers);
    }
    blocking(move || cli::text(&args).map(drop)).await
}

/// Approve a plan as the person edited it (`permission answer <id> --plan-file`).
#[tauri::command]
async fn approve_plan(id: i64, plan: String) -> Result<(), String> {
    blocking(move || {
        let file =
            std::env::temp_dir().join(format!("anchorleg-plan-{}-{id}.md", std::process::id()));
        std::fs::write(&file, plan).map_err(|e| e.to_string())?;
        let out = cli::text(&[
            "permission".as_ref(),
            "answer".as_ref(),
            id.to_string().as_ref(),
            "--plan-file".as_ref(),
            file.as_os_str(),
        ]);
        let _ = std::fs::remove_file(&file);
        out.map(drop)
    })
    .await
}

/// The files of a session's folder, for `@` completions.
#[tauri::command]
async fn files(cwd: String) -> Result<Vec<String>, String> {
    blocking(move || Ok(cli::files(std::path::Path::new(&cwd)))).await
}

#[tauri::command]
async fn terminal() -> cli::Terminal {
    tauri::async_runtime::spawn_blocking(cli::terminal)
        .await
        .expect("terminal task")
}

#[tauri::command]
async fn link_terminal() -> Result<cli::Terminal, String> {
    blocking(cli::link_terminal).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    cli::warm_up();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            notices::start(app.handle().clone());
            tray::create(app.handle())?;
            #[cfg(target_os = "macos")]
            app.set_menu(appmenu::create(app.handle())?)?;
            Ok(())
        })
        .on_menu_event(|app, e| appmenu::handle(app, e.id.as_ref()))
        // Closing the window hides it: sessions, notices and the menu bar item go on until Quit.
        .on_window_event(|w, e| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = e {
                let _ = w.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            cli_info,
            sessions,
            session,
            start,
            diff,
            open_target,
            agents,
            undo,
            shell,
            send,
            stop,
            rename,
            files,
            permissions,
            answer,
            approve_plan,
            settings::status,
            settings::accounts,
            settings::import_sessions,
            settings::vendors,
            settings::usage,
            settings::policy,
            settings::settings,
            settings::set_accounts,
            settings::add_account,
            settings::remove_account,
            settings::import_aliases,
            settings::rules,
            settings::notify,
            settings::mcp_sync,
            tray::update_tray,
            signin::login_account,
            signin::cancel_login,
            signin::login_in_terminal,
            paste::save_paste,
            terminal,
            link_terminal
        ])
        .build(tauri::generate_context!())
        .expect("error while building the anchorleg app")
        .run(|app, event| match event {
            tauri::RunEvent::Exit => notices::stop(),
            // macOS: a click on the Dock icon brings the hidden window back.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => tray::show(app),
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_args_options() {
        let s: Start = serde_json::from_str(
            r#"{"text":"t","goal":"tests pass","goal_turns":5,"model":"sonnet","cli":"codex","policy":"drain","budget":"500k","answer_wait":"10m"}"#,
        )
        .unwrap();
        assert_eq!(
            s.args(),
            [
                "run",
                "--detach",
                "--json",
                "--goal",
                "tests pass",
                "--model",
                "sonnet",
                "--cli",
                "codex",
                "--policy",
                "drain",
                "--budget",
                "500k",
                "--goal-turns",
                "5",
                "--answer-wait",
                "10m",
                "--",
                "t"
            ]
        );
    }

    #[test]
    fn start_args_task_options() {
        let s: Start = serde_json::from_str(
            r#"{"text":"t","add_dir":["/a","/b"],"instructions":"be brief","effort":"high","agent":"rev","allowed_tools":["Bash(git *)",""],"strict_mcp_config":true}"#,
        )
        .unwrap();
        assert_eq!(
            s.args(),
            [
                "run",
                "--detach",
                "--json",
                "--instructions",
                "be brief",
                "--effort",
                "high",
                "--agent",
                "rev",
                "--add-dir",
                "/a",
                "--add-dir",
                "/b",
                "--allowed-tools",
                "Bash(git *)",
                "--strict-mcp-config",
                "--",
                "t"
            ]
        );
    }

    #[test]
    fn start_args() {
        let s: Start =
            serde_json::from_str(r#"{"text":"-why?","btw":4,"attach":["/a b"]}"#).unwrap();
        assert_eq!(
            s.args(),
            [
                "run", "--detach", "--json", "--btw", "4", "--attach", "/a b", "--", "-why?"
            ]
        );
        let s: Start = serde_json::from_str(
            r#"{"text":"t","cwd":"/p","plan":true,"worktree":true,"prefer":"work","name":"n"}"#,
        )
        .unwrap();
        assert_eq!(
            s.args(),
            [
                "run",
                "--detach",
                "--json",
                "--cwd",
                "/p",
                "--prefer",
                "work",
                "--name",
                "n",
                "--plan",
                "--worktree",
                "--",
                "t"
            ]
        );
        let s: Start =
            serde_json::from_str(r#"{"text":"go on","rewind":7,"keep_files":true}"#).unwrap();
        assert_eq!(
            s.args(),
            [
                "run",
                "--detach",
                "--json",
                "--rewind",
                "7",
                "--keep-files",
                "--",
                "go on"
            ]
        );
    }
}
