//! anchorleg's desktop app: a window over the CLI it ships with (D30, D31). Commands here are thin: run
//! `anchorleg … --json` off the UI thread and hand the result to the React side.

mod cli;

use serde::Deserialize;
use serde_json::Value;

/// Run a blocking CLI call off the UI thread.
async fn blocking<T: Send + 'static>(
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

/// `anchorleg sessions show <run> --since N --json`: one session's conversation from message N.
#[tauri::command]
async fn session(run: i64, since: Option<usize>) -> Result<Value, String> {
    let (run, since) = (run.to_string(), since.unwrap_or(0).to_string());
    blocking(move || cli::json(&["sessions", "show", &run, "--since", &since, "--json"])).await
}

/// A new run in the background (`anchorleg run --detach --json`): a new session in `cwd`, a reply
/// to a finished one (`follow_up`), or a new session from another's history (`fork`).
#[derive(Debug, Deserialize)]
struct Start {
    text: String,
    cwd: Option<String>,
    follow_up: Option<i64>,
    fork: Option<i64>,
    #[serde(default)]
    attach: Vec<String>,
}

#[tauri::command]
async fn start(req: Start) -> Result<Value, String> {
    let mut args: Vec<String> = vec!["run".into(), "--detach".into(), "--json".into()];
    if let Some(cwd) = req.cwd {
        args.extend(["--cwd".into(), cwd]);
    }
    if let Some(id) = req.follow_up {
        args.extend(["--follow-up".into(), id.to_string()]);
    }
    if let Some(id) = req.fork {
        args.extend(["--fork".into(), id.to_string()]);
    }
    for f in req.attach {
        args.extend(["--attach".into(), f]);
    }
    args.extend(["--".into(), req.text]);
    blocking(move || cli::json(&args)).await
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
        .invoke_handler(tauri::generate_handler![
            cli_info,
            sessions,
            session,
            start,
            send,
            stop,
            rename,
            files,
            terminal,
            link_terminal
        ])
        .run(tauri::generate_context!())
        .expect("error while running the anchorleg app");
}
