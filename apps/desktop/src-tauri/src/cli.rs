//! The `anchorleg` CLI. The app never opens anchorleg's database or runs an agent itself:
//! everything goes through the CLI's `--json` commands (D30), so the CLI stays the only owner of
//! the store. The app ships its own copy of the CLI next to its executable (D31), so installing
//! the app is all a person does; `ANCHORLEG_BIN` overrides it, and builds without the bundled
//! copy fall back to an installed one.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

/// The oldest CLI this app works with (only matters for a fallback or `ANCHORLEG_BIN`).
pub const MIN_VERSION: (u32, u32, u32) = (0, 6, 1);

/// Shown when no CLI can be found at all (a build without the bundled copy).
pub const INSTALL_COMMAND: &str =
    "curl -fsSL https://raw.githubusercontent.com/snehal96/anchorleg/main/install.sh | bash";

#[derive(Debug, Serialize)]
pub struct CliInfo {
    pub path: Option<String>,
    pub version: Option<String>,
    /// The app's own copy.
    pub bundled: bool,
    /// Found and new enough.
    pub ok: bool,
    pub min_version: String,
    pub install_command: &'static str,
    /// Why it isn't usable, for the person.
    pub problem: Option<String>,
}

/// The copy inside the app: `anchorleg.app/Contents/MacOS/anchorleg`, `/usr/bin/anchorleg` from
/// the `.deb`, or `target/<profile>/anchorleg` under `tauri dev`.
pub fn bundled() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join("anchorleg");
    is_executable(&p).then_some(p)
}

/// Installed copies, for builds without the bundled one. An app opened from the Dock or Finder
/// doesn't get the shell's PATH, so the usual install folders are checked too.
fn installed() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(home) = home() {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".cargo/bin"));
    }
    for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        dirs.push(PathBuf::from(d));
    }
    dirs.into_iter()
        .map(|d| d.join("anchorleg"))
        .find(|p| is_executable(p))
}

pub fn find() -> Option<PathBuf> {
    if let Some(bin) = std::env::var_os("ANCHORLEG_BIN") {
        return Some(PathBuf::from(bin));
    }
    bundled().or_else(installed)
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

type Version = (u32, u32, u32);

/// `anchorleg 0.2.0` → (0, 2, 0).
fn parse_version(text: &str) -> Option<Version> {
    let v = text.split_whitespace().nth(1)?;
    let mut parts = v.split('.').map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// `anchorleg --version` of a binary, as text and parsed.
fn version_of(bin: &Path) -> Result<(String, Option<Version>), String> {
    let out = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("couldn't run it: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let parsed = parse_version(&text);
    Ok((text, parsed))
}

fn show((a, b, c): Version) -> String {
    format!("{a}.{b}.{c}")
}

pub fn info() -> CliInfo {
    let mut info = CliInfo {
        path: None,
        version: None,
        bundled: false,
        ok: false,
        min_version: show(MIN_VERSION),
        install_command: INSTALL_COMMAND,
        problem: None,
    };
    let Some(path) = find() else {
        info.problem = Some("the anchorleg CLI isn't installed".into());
        return info;
    };
    info.bundled = bundled().as_deref() == Some(path.as_path());
    info.path = Some(path.display().to_string());
    let (text, parsed) = match version_of(&path) {
        Ok(v) => v,
        Err(e) => {
            info.problem = Some(e);
            return info;
        }
    };
    info.version = parsed.map(show);
    match parsed {
        Some(v) if v >= MIN_VERSION => info.ok = true,
        Some(v) => {
            info.problem = Some(format!(
                "version {} is too old; this app needs {} or newer",
                show(v),
                info.min_version
            ))
        }
        None => info.problem = Some(format!("unexpected `--version` output: {text:?}")),
    }
    info
}

/// The login shell's PATH, asked once. An app opened from the Dock or Finder gets a bare PATH, and
/// the CLI starts agents (`claude`, `codex`, `git`) that a terminal would find through the shell's.
fn shell_path() -> Option<&'static str> {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    PATH.get_or_init(|| ask_shell().map(|(_, path)| path))
        .as_deref()
}

/// Ask the shell for its PATH ahead of the first command (it can take a few seconds).
pub fn warm_up() {
    std::thread::spawn(shell_path);
}

/// Run `anchorleg <args>` and return its stdout; on failure, its error as the CLI put it.
pub fn text<S: AsRef<OsStr>>(args: &[S]) -> Result<String, String> {
    let path = find().ok_or("the anchorleg CLI isn't installed")?;
    let mut cmd = Command::new(&path);
    cmd.args(args).stdin(Stdio::null());
    if let Some(p) = shell_path() {
        cmd.env("PATH", p);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("running anchorleg: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim().lines().last().unwrap_or_default();
        return Err(err.trim_start_matches("anchorleg: ").to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Start `anchorleg <args>` without waiting, its stdout and stderr piped: for the long ones
/// the window can cancel (signing in an account).
pub fn spawn<S: AsRef<OsStr>>(args: &[S]) -> Result<std::process::Child, String> {
    let path = find().ok_or("the anchorleg CLI isn't installed")?;
    let mut cmd = Command::new(&path);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(p) = shell_path() {
        cmd.env("PATH", p);
    }
    cmd.spawn().map_err(|e| format!("running anchorleg: {e}"))
}

/// Run `anchorleg <args>` and parse its JSON output.
pub fn json<S: AsRef<OsStr>>(args: &[S]) -> Result<Value, String> {
    serde_json::from_str(&text(args)?).map_err(|e| format!("reading anchorleg's output: {e}"))
}

/// `!cmd` in the composer: `sh -c cmd` in `folder` (with the shell's PATH), stdout and stderr
/// together, at most 8 KB; stopped after 60 s.
pub fn shell(folder: &Path, command: &str) -> Result<String, String> {
    const MAX: usize = 8 * 1024;
    const LIMIT: Duration = Duration::from_secs(60);
    let mut cmd = Command::new("sh");
    cmd.args(["-c", &format!("({command}) 2>&1")])
        .current_dir(folder)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(p) = shell_path() {
        cmd.env("PATH", p);
    }
    let mut child = cmd.spawn().map_err(|e| format!("running sh: {e}"))?;
    let mut out = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        use std::io::Read as _;
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() < LIMIT => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let buf = reader.join().unwrap_or_default();
    let mut text = String::from_utf8_lossy(&buf[..buf.len().min(MAX)]).into_owned();
    if buf.len() > MAX {
        text.push_str("\n… (output cut at 8 KB)");
    }
    match status {
        None => text.push_str("\n(stopped after 60 s)"),
        Some(s) if !s.success() => text.push_str(&format!("\n({s})")),
        Some(_) => {}
    }
    Ok(text.trim_end().to_owned())
}

/// The files of `folder` for `@` completions, as `anchorleg ui` lists them: `git ls-files`
/// (tracked and untracked, not ignored) in a repository, else a walk three levels deep that skips
/// hidden folders. At most 5000.
pub fn files(folder: &Path) -> Vec<String> {
    const MAX: usize = 5000;
    if let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(folder)
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        && out.status.success()
    {
        return String::from_utf8_lossy(&out.stdout)
            .lines()
            .take(MAX)
            .map(str::to_owned)
            .collect();
    }
    fn walk(base: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            if out.len() >= MAX {
                return;
            }
            let p = e.path();
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if p.is_dir() {
                if depth > 0 {
                    walk(base, &p, depth - 1, out);
                }
            } else if let Ok(rel) = p.strip_prefix(base) {
                out.push(rel.display().to_string());
            }
        }
    }
    let mut out = Vec::new();
    walk(folder, folder, 2, &mut out);
    out.sort();
    out
}

// ---- "Use anchorleg in Terminal" ---------------------------------------------------------------
//
// Optional: the app works without it. A link `~/.local/bin/anchorleg` → the bundled copy, so the
// terminal command updates with the app. It never replaces a CLI that is as new or newer.

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum TerminalState {
    /// Nothing to link (a build without the bundled copy, or no home directory).
    Unavailable { reason: String },
    /// `~/.local/bin/anchorleg` is our link.
    Linked,
    /// Nothing there yet.
    Missing,
    /// Another anchorleg is there, older than the app's (or a broken link): the link can replace it.
    Older { version: Option<String> },
    /// Another anchorleg is there, as new or newer: left alone.
    Other { version: Option<String> },
}

#[derive(Debug, Serialize)]
pub struct Terminal {
    /// Where the link goes.
    pub link: String,
    pub app_version: Option<String>,
    #[serde(flatten)]
    pub state: TerminalState,
    /// What a new terminal runs for `anchorleg`, if the login shell could be asked.
    pub shell_resolves: Option<String>,
    pub shell_version: Option<String>,
    /// `~/.local/bin` isn't on the login shell's PATH.
    pub needs_path: bool,
}

fn link_path() -> Option<PathBuf> {
    Some(home()?.join(".local/bin/anchorleg"))
}

/// An app run straight from the `.dmg` or from macOS's quarantine copy lives at a path that goes
/// away; a link to it would break.
fn unlinkable(app: &Path) -> Option<String> {
    let s = app.to_string_lossy();
    (s.starts_with("/Volumes/") || s.contains("/AppTranslocation/"))
        .then(|| "move anchorleg to Applications and open it from there first".to_owned())
}

/// The login shell's answer to `command -v anchorleg` and its PATH. A shell that hangs (a
/// prompt in a dotfile) is given up on after a few seconds.
fn ask_shell() -> Option<(Option<String>, String)> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let script = r#"printf '\n@@PATH=%s\n' "$PATH"; printf '@@BIN=%s\n' "$(command -v anchorleg)""#;
    let mut child = Command::new(shell)
        .args(["-ilc", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < Duration::from_secs(4) => {
                std::thread::sleep(Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let path = text
        .lines()
        .find_map(|l| l.strip_prefix("@@PATH="))?
        .to_owned();
    let bin = text
        .lines()
        .find_map(|l| l.strip_prefix("@@BIN="))
        .filter(|b| !b.is_empty())
        .map(str::to_owned);
    Some((bin, path))
}

fn link_state(app: &Path, app_version: Option<Version>, link: &Path) -> TerminalState {
    if std::fs::read_link(link).is_ok_and(|t| t == app) {
        return TerminalState::Linked;
    }
    if link.symlink_metadata().is_err() {
        return TerminalState::Missing;
    }
    let theirs = version_of(link).ok().and_then(|(_, v)| v);
    match (theirs, app_version) {
        (Some(t), Some(a)) if t < a => TerminalState::Older {
            version: Some(show(t)),
        },
        // A broken link or something that isn't a program: fixable from the app.
        (None, _) if !is_executable(link) => TerminalState::Older { version: None },
        (t, _) => TerminalState::Other {
            version: t.map(show),
        },
    }
}

pub fn terminal() -> Terminal {
    let link = link_path();
    let app = bundled();
    let app_version = app
        .as_deref()
        .and_then(|a| version_of(a).ok())
        .and_then(|(_, v)| v);
    let state = match (&app, &link) {
        (None, _) => TerminalState::Unavailable {
            reason: "this build doesn't include the CLI".into(),
        },
        (_, None) => TerminalState::Unavailable {
            reason: "no home directory".into(),
        },
        (Some(app), Some(link)) => link_state(app, app_version, link),
    };
    let (shell_resolves, shell_path) = match ask_shell() {
        Some((bin, path)) => (bin, Some(path)),
        None => (None, None),
    };
    let shell_version = shell_resolves
        .as_deref()
        .and_then(|b| version_of(Path::new(b)).ok())
        .and_then(|(_, v)| v)
        .map(show);
    let needs_path = match (&shell_path, link.as_deref().and_then(Path::parent)) {
        (Some(p), Some(dir)) => !std::env::split_paths(p).any(|d| d == dir),
        _ => false,
    };
    Terminal {
        link: link.map(|l| l.display().to_string()).unwrap_or_default(),
        app_version: app_version.map(show),
        state,
        shell_resolves,
        shell_version,
        needs_path,
    }
}

/// Create or replace `~/.local/bin/anchorleg` as a link to the bundled CLI.
pub fn link_terminal() -> Result<Terminal, String> {
    let app = bundled().ok_or("this build doesn't include the CLI")?;
    if let Some(why) = unlinkable(&app) {
        return Err(why);
    }
    let link = link_path().ok_or("no home directory")?;
    let app_version = version_of(&app).ok().and_then(|(_, v)| v);
    match link_state(&app, app_version, &link) {
        TerminalState::Linked => return Ok(terminal()),
        TerminalState::Missing | TerminalState::Older { .. } => {}
        TerminalState::Other { version } => {
            return Err(format!(
                "{} is anchorleg {}, as new as the app's or newer; left as it is",
                link.display(),
                version.as_deref().unwrap_or("(unknown version)")
            ));
        }
        TerminalState::Unavailable { reason } => return Err(reason),
    }
    let dir = link.parent().expect("link has a parent");
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    // Make the new link beside it and rename over, so there's never a moment without one.
    let tmp = dir.join(".anchorleg.link");
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(&app, &tmp).map_err(|e| format!("linking: {e}"))?;
    std::fs::rename(&tmp, &link).map_err(|e| format!("replacing {}: {e}", link.display()))?;
    Ok(terminal())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("anchorleg 0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("anchorleg 1.10.3\n"), Some((1, 10, 3)));
        assert_eq!(parse_version("anchorleg"), None);
        assert!((0, 6, 0) < MIN_VERSION && (0, 6, 1) >= MIN_VERSION);
    }

    #[test]
    fn shell_output_and_exit() {
        let dir = std::env::temp_dir();
        assert_eq!(shell(&dir, "echo hi; echo err >&2").unwrap(), "hi\nerr");
        assert!(shell(&dir, "exit 3").unwrap().contains("exit status: 3"));
    }

    #[test]
    fn apps_run_from_the_dmg_cant_be_linked() {
        let at = |p: &str| unlinkable(Path::new(p)).is_some();
        assert!(at(
            "/Volumes/anchorleg/anchorleg.app/Contents/MacOS/anchorleg"
        ));
        assert!(at(
            "/private/var/folders/x/T/AppTranslocation/A/d/anchorleg.app/Contents/MacOS/anchorleg"
        ));
        assert!(!at("/Applications/anchorleg.app/Contents/MacOS/anchorleg"));
    }

    fn fake(dir: &Path, name: &str, version: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\necho 'anchorleg {version}'\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn the_link_replaces_only_older_clis() {
        let dir = std::env::temp_dir().join(format!("anchorleg-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let app = fake(&dir, "app", "0.3.0");
        let v = Some((0, 3, 0));
        let link = dir.join("anchorleg");

        assert!(matches!(link_state(&app, v, &link), TerminalState::Missing));
        std::os::unix::fs::symlink(&app, &link).unwrap();
        assert!(matches!(link_state(&app, v, &link), TerminalState::Linked));

        std::fs::remove_file(&link).unwrap();
        fake(&dir, "anchorleg", "0.2.0");
        assert!(matches!(
            link_state(&app, v, &link),
            TerminalState::Older { version: Some(ref s) } if s == "0.2.0"
        ));
        fake(&dir, "anchorleg", "0.3.0");
        assert!(matches!(
            link_state(&app, v, &link),
            TerminalState::Other { .. }
        ));
        fake(&dir, "anchorleg", "1.0.0");
        assert!(matches!(
            link_state(&app, v, &link),
            TerminalState::Other { .. }
        ));

        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(dir.join("gone"), &link).unwrap();
        assert!(matches!(
            link_state(&app, v, &link),
            TerminalState::Older { version: None }
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
