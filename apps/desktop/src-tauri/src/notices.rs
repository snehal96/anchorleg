//! Notifications (D65). While the app runs, its pid is in `<data dir>/desktop.pid`; runs then
//! append their notices to `<data dir>/desktop-notices.jsonl` instead of notifying themselves,
//! and the app shows them: a system notification, and an event the window uses to select the run.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter as _};
use tauri_plugin_notification::NotificationExt as _;

const PID: &str = "desktop.pid";
const NOTICES: &str = "desktop-notices.jsonl";
const POLL: Duration = Duration::from_millis(700);

/// The folder whose `desktop.pid` is ours, once claimed.
static CLAIMED: OnceLock<PathBuf> = OnceLock::new();

/// One notice, as `anchorleg-core::notify::Notice` writes it.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Notice {
    pub event: String,
    #[serde(default)]
    pub run_id: Option<i64>,
    pub title: String,
    pub message: String,
}

/// The store folder, from `anchorleg notify --json`.
fn data_dir() -> Option<PathBuf> {
    let v = crate::cli::json(&["notify", "--json"]).ok()?;
    v["data_dir"].as_str().map(PathBuf::from)
}

/// Claim the notices and show them as they come. Runs on its own thread for the app's life.
pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let Some(dir) = data_dir() else {
            return;
        };
        if std::fs::create_dir_all(&dir).is_err()
            || std::fs::write(dir.join(PID), std::process::id().to_string()).is_err()
        {
            return;
        }
        let _ = CLAIMED.set(dir.clone());
        let path = dir.join(NOTICES);
        // Older notices were shown already (or by the CLI itself).
        let mut offset = std::fs::metadata(&path).map_or(0, |m| m.len());
        loop {
            std::thread::sleep(POLL);
            for n in read_new(&path, &mut offset) {
                show(&app, &n);
            }
        }
    });
}

/// Give the notices back to the CLI when the app quits: only our own pid file.
pub fn stop() {
    if let Some(dir) = CLAIMED.get() {
        let pid = dir.join(PID);
        if std::fs::read_to_string(&pid).is_ok_and(|p| p.trim() == std::process::id().to_string()) {
            let _ = std::fs::remove_file(pid);
        }
    }
}

/// Complete lines after `offset`; `offset` moves past them. A shorter file starts over.
fn read_new(path: &Path, offset: &mut u64) -> Vec<Notice> {
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map_or(0, |m| m.len());
    if len < *offset {
        *offset = 0;
    }
    if len == *offset || f.seek(SeekFrom::Start(*offset)).is_err() {
        return Vec::new();
    }
    let mut text = String::new();
    if f.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    // A line still being written waits for the next poll.
    let done = text.rfind('\n').map_or(0, |i| i + 1);
    *offset += done as u64;
    text[..done]
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn show(app: &AppHandle, n: &Notice) {
    let _ = app.emit("notice", n);
    let _ = app
        .notification()
        .builder()
        .title(&n.title)
        .body(&n.message)
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn new_complete_lines_only() {
        let dir = std::env::temp_dir().join(format!("anchorleg-notices-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(NOTICES);
        let line = |id: i64| {
            format!(
                r#"{{"event":"done","run_id":{id},"title":"anchorleg","message":"run {id} done"}}"#
            )
        };
        std::fs::write(&path, line(1) + "\n").unwrap();
        let mut offset = std::fs::metadata(&path).unwrap().len();
        assert!(read_new(&path, &mut offset).is_empty());

        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(f, "{}\n{}", line(2), &line(3)[..10]).unwrap();
        let got = read_new(&path, &mut offset);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].run_id, Some(2));
        writeln!(f, "{}", &line(3)[10..]).unwrap();
        assert_eq!(read_new(&path, &mut offset)[0].run_id, Some(3));

        // Truncated: read from the start again.
        std::fs::write(&path, line(4) + "\n").unwrap();
        assert_eq!(read_new(&path, &mut offset)[0].run_id, Some(4));
        let _ = std::fs::remove_dir_all(dir);
    }
}
