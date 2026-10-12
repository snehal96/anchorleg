//! Files the person attaches to a message (a pasted screenshot, a dropped log file).
//!
//! They are copied into the task folder, under `.anchorleg/attachments/`, so every account and
//! every CLI can read them, also after a switch, and the message names them by path. Claude reads
//! them with its Read tool (which shows images); Codex also gets images as `localImage` input.
//! The folder holds a `.gitignore` of `*`, so attachments never show up in git.

use std::path::{Path, PathBuf};

/// Where attachments go, relative to the task folder.
pub const DIR: &str = ".anchorleg/attachments";

const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The folder attachments go to in `cwd`, created with its `.gitignore`.
pub fn dir(cwd: &Path) -> std::io::Result<PathBuf> {
    let dir = cwd.join(DIR);
    std::fs::create_dir_all(&dir)?;
    let ignore = cwd.join(".anchorleg/.gitignore");
    if !ignore.exists() {
        std::fs::write(
            ignore,
            "# anchorleg's own files (attachments); never committed\n*\n",
        )?;
    }
    Ok(dir)
}

/// Copy `files` into `cwd`'s attachment folder. Returns the copies, relative to `cwd`.
/// A file already inside that folder is used where it is.
pub fn stage(cwd: &Path, files: &[PathBuf], now: i64) -> std::io::Result<Vec<PathBuf>> {
    if files.is_empty() {
        // Nothing attached: don't create the folder.
        return Ok(Vec::new());
    }
    let target = dir(cwd)?;
    let mut out = Vec::new();
    for (i, f) in files.iter().enumerate() {
        if f.parent()
            .and_then(|p| p.canonicalize().ok())
            .is_some_and(|p| target.canonicalize().is_ok_and(|t| p == t))
        {
            out.push(Path::new(DIR).join(f.file_name().unwrap_or_default()));
            continue;
        }
        if !f.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no such file: {}", f.display()),
            ));
        }
        let name = f
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment");
        let name = format!("{now}-{i}-{}", sanitize(name));
        std::fs::copy(f, target.join(&name))?;
        out.push(Path::new(DIR).join(name));
    }
    Ok(out)
}

/// Spaces and odd characters make paths awkward for agents to quote; keep names plain.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `text` with a note naming the attachments (paths relative to the task folder).
pub fn with_note(text: &str, staged: &[PathBuf]) -> String {
    if staged.is_empty() {
        return text.to_owned();
    }
    let list: Vec<String> = staged
        .iter()
        .map(|p| format!("- {}", p.display()))
        .collect();
    format!(
        "{text}\n\nAttached (read them; paths are relative to the task folder):\n{}",
        list.join("\n")
    )
}

/// Paths of existing files in `text`, as a terminal pastes a dropped file: absolute or `~/`,
/// possibly quoted or with `\ `-escaped spaces. Returns the files and the text without them.
pub fn dropped_paths(text: &str, home: &Path) -> (Vec<PathBuf>, String) {
    let mut files = Vec::new();
    let mut rest = Vec::new();
    for token in split_shell_words(text) {
        let expanded = match token.strip_prefix("~/") {
            Some(t) => home.join(t),
            None => PathBuf::from(&token),
        };
        if (token.starts_with('/') || token.starts_with("~/")) && expanded.is_file() {
            files.push(expanded);
        } else {
            rest.push(token);
        }
    }
    if files.is_empty() {
        // Nothing dropped: leave the person's text exactly as typed.
        return (files, text.to_owned());
    }
    (files, rest.join(" "))
}

/// Words split on spaces, honouring `'…'`, `"…"` and `\ ` (how terminals paste dropped paths).
fn split_shell_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (None, '\'' | '"') if cur.is_empty() => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            }
            (_, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Save the clipboard's image (if it holds one) as a PNG in `cwd`'s attachment folder.
/// macOS: `osascript`; Linux: `wl-paste`, else `xclip`. `Ok(None)`: no image on the clipboard.
pub fn paste_image(cwd: &Path, now: i64) -> std::io::Result<Option<PathBuf>> {
    let target = dir(cwd)?;
    let name = format!("{now}-pasted.png");
    let file = target.join(&name);
    let saved = if cfg!(target_os = "macos") {
        let script = format!(
            "try\nset d to (the clipboard as «class PNGf»)\non error\nreturn \"none\"\nend try\n\
             set f to (open for access POSIX file \"{}\" with write permission)\n\
             set eof f to 0\nwrite d to f\nclose access f\nreturn \"ok\"",
            file.display()
        );
        let out = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()?;
        String::from_utf8_lossy(&out.stdout).trim() == "ok"
    } else {
        let try_cmd = |prog: &str, args: &[&str]| {
            std::process::Command::new(prog)
                .args(args)
                .output()
                .ok()
                .filter(|o| o.status.success() && o.stdout.starts_with(b"\x89PNG"))
                .map(|o| o.stdout)
        };
        match try_cmd("wl-paste", &["--type", "image/png"]).or_else(|| {
            try_cmd(
                "xclip",
                &["-selection", "clipboard", "-t", "image/png", "-o"],
            )
        }) {
            Some(bytes) => {
                std::fs::write(&file, bytes)?;
                true
            }
            None => false,
        }
    };
    if !saved || std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0) == 0 {
        let _ = std::fs::remove_file(&file);
        return Ok(None);
    }
    Ok(Some(Path::new(DIR).join(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_into_the_task_folder_and_ignored_by_git() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("My Screenshot.png");
        std::fs::write(&src, b"img").unwrap();
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let staged = stage(&work, &[src], 100).unwrap();
        assert_eq!(
            staged,
            [PathBuf::from(
                ".anchorleg/attachments/100-0-My_Screenshot.png"
            )]
        );
        assert!(work.join(&staged[0]).is_file());
        assert_eq!(
            std::fs::read_to_string(work.join(".anchorleg/.gitignore")).unwrap(),
            "# anchorleg's own files (attachments); never committed\n*\n"
        );
        // Already staged (a pasted image): used where it is.
        let again = stage(&work, &[work.join(&staged[0])], 200).unwrap();
        assert_eq!(again, staged);
        assert!(stage(&work, &[tmp.path().join("missing.txt")], 1).is_err());
        assert!(is_image(&staged[0]));
        assert!(
            with_note("look", &staged)
                .ends_with("- .anchorleg/attachments/100-0-My_Screenshot.png")
        );
    }

    #[test]
    fn dropped_paths_are_found_in_the_text() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a b.log");
        std::fs::write(&a, "x").unwrap();
        let escaped = a.display().to_string().replace(' ', "\\ ");
        let (files, rest) = dropped_paths(&format!("why does this fail {escaped}"), tmp.path());
        assert_eq!(files, std::slice::from_ref(&a));
        assert_eq!(rest, "why does this fail");
        let (files, _) = dropped_paths(&format!("'{}' and /no/such/file", a.display()), tmp.path());
        assert_eq!(files, [a]);
        let typed = r#"say "hello  world" /no/such/file"#;
        assert_eq!(dropped_paths(typed, tmp.path()), (vec![], typed.to_owned()));
    }
}
