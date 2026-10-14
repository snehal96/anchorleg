//! Telling the person when a run needs them or is over, while they're away from it (D39).
//!
//! Two channels, both set in `config.toml` under `[notify]` (`anchorleg notify`):
//! - a desktop notification (macOS Notification Center through `osascript`, Linux `notify-send`),
//!   on unless turned off;
//! - a command of the person's own (`sh -c`), given the notice as JSON on stdin and as
//!   `ANCHORLEG_EVENT` / `ANCHORLEG_RUN_ID` / `ANCHORLEG_TITLE` / `ANCHORLEG_MESSAGE`, for
//!   anything else (a chat bot, a phone push service, a sound).
//!
//! While anchorleg's desktop app runs it shows the notices itself (D65): it keeps its pid in
//! `<data dir>/desktop.pid`, and notices go to `<data dir>/desktop-notices.jsonl` (one JSON line
//! each) instead of a desktop notification. The command still runs.
//!
//! Notices never carry answers or secrets: only what is waiting, and how a run ended.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifySettings {
    /// Desktop notifications. On by default.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub desktop: bool,
    /// Run this (`sh -c`) for every notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

impl Default for NotifySettings {
    fn default() -> Self {
        Self {
            desktop: true,
            command: None,
        }
    }
}

impl NotifySettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn on() -> bool {
    true
}

fn is_on(b: &bool) -> bool {
    *b
}

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A tool call, question or plan waits for the person.
    Waiting,
    Done,
    Failed,
    /// The task moved to another account.
    Switched,
    /// Every account is out: the run waits for a reset, or is paying from credits.
    Blocked,
    /// `anchorleg notify --test`.
    Test,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Waiting => "waiting",
            Kind::Done => "done",
            Kind::Failed => "failed",
            Kind::Switched => "switched",
            Kind::Blocked => "blocked",
            Kind::Test => "test",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Notice {
    pub event: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
    pub title: String,
    pub message: String,
}

impl Notice {
    pub fn new(event: Kind, run_id: Option<i64>, title: &str, message: &str) -> Self {
        Self {
            event,
            run_id,
            title: shorten(title, 60),
            message: shorten(message, 200),
        }
    }
}

/// The first line of `s`, at most `max` characters.
fn shorten(s: &str, max: usize) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() <= max {
        return line.to_owned();
    }
    let cut: String = line.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Tests set this to a file: notices are appended to it as JSON lines instead of being sent.
pub const LOG_ENV: &str = "ANCHORLEG_NOTIFY_LOG";

/// Send `notice` on every channel that's on. Never fails the caller: a channel that can't be
/// reached is logged and skipped. Returns without waiting for the channels to finish.
pub fn send(settings: &NotifySettings, notice: &Notice) {
    let app = crate::store::Store::default_path()
        .ok()
        .and_then(|db| db.parent().and_then(app_notices));
    if let Some(path) = &app {
        append(path, notice);
    }
    if let Some(log) = std::env::var_os(LOG_ENV) {
        append(Path::new(&log), notice);
        return;
    }
    if settings.desktop
        && app.is_none()
        && let Some(cmd) = desktop_command(notice)
    {
        spawn(cmd, None);
    }
    if let Some(command) = &settings.command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .env("ANCHORLEG_EVENT", notice.event.name())
            .env(
                "ANCHORLEG_RUN_ID",
                notice.run_id.map(|r| r.to_string()).unwrap_or_default(),
            )
            .env("ANCHORLEG_TITLE", &notice.title)
            .env("ANCHORLEG_MESSAGE", &notice.message);
        spawn(cmd, serde_json::to_string(notice).ok());
    }
}

/// The desktop app keeps its pid here while it runs.
pub const APP_PID: &str = "desktop.pid";
/// Where notices go while the app runs, for it to show.
pub const APP_NOTICES: &str = "desktop-notices.jsonl";

/// `dir`'s notices file while the desktop app whose pid is in `dir` runs.
pub fn app_notices(dir: &Path) -> Option<PathBuf> {
    let pid: i64 = std::fs::read_to_string(dir.join(APP_PID))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    pid_alive(pid).then(|| dir.join(APP_NOTICES))
}

fn pid_alive(pid: i64) -> bool {
    pid > 0
        && Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

fn append(path: &Path, notice: &Notice) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{}", serde_json::to_string(notice).unwrap_or_default());
    }
}

fn desktop_command(notice: &Notice) -> Option<Command> {
    if cfg!(target_os = "macos") {
        let script = format!(
            "display notification {} with title {}",
            applescript_string(&notice.message),
            applescript_string(&notice.title)
        );
        let mut cmd = Command::new("osascript");
        cmd.args(["-e", &script]);
        Some(cmd)
    } else if cfg!(target_os = "linux") {
        let mut cmd = Command::new("notify-send");
        cmd.args(["--app-name=anchorleg", &notice.title, &notice.message]);
        Some(cmd)
    } else {
        None
    }
}

/// `s` as an AppleScript string literal.
fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Start `cmd` with `input` on stdin; a thread waits for it so no zombie is left behind.
fn spawn(mut cmd: Command, input: Option<String>) {
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    match cmd.spawn() {
        Ok(mut child) => {
            if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
                let _ = stdin.write_all(input.as_bytes());
            }
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => tracing::warn!(%e, "couldn't send a notification"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_go_to_the_desktop_app_only_while_it_runs() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(app_notices(dir.path()), None);
        std::fs::write(dir.path().join(APP_PID), std::process::id().to_string()).unwrap();
        assert_eq!(app_notices(dir.path()), Some(dir.path().join(APP_NOTICES)));
        // An app that quit without removing its pid file.
        let mut gone = Command::new("true").spawn().unwrap();
        let pid = gone.id();
        gone.wait().unwrap();
        std::fs::write(dir.path().join(APP_PID), pid.to_string()).unwrap();
        assert_eq!(app_notices(dir.path()), None);
    }

    #[test]
    fn notices_are_short_and_quoted_safely() {
        let n = Notice::new(
            Kind::Waiting,
            Some(3),
            "fix the \"auth\" tests\nand more",
            "Allow Bash(rm -rf build)?",
        );
        assert_eq!(n.title, "fix the \"auth\" tests");
        assert_eq!(applescript_string(&n.title), r#""fix the \"auth\" tests""#);
        assert_eq!(shorten(&"x".repeat(300), 10), format!("{}…", "x".repeat(9)));
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            serde_json::json!({ "event": "waiting", "run_id": 3, "title": n.title, "message": "Allow Bash(rm -rf build)?" })
        );
        // On unless turned off; the default isn't written to config.toml.
        let s: NotifySettings = toml::from_str("").unwrap();
        assert!(s.desktop && s.is_default());
    }
}
