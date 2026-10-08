//! Local git checkpoints (D5): the working tree at a switch, saved as a commit under
//! `refs/anchorleg/…` without touching the user's branch, index or stash. Never pushed.
//!
//! The commit is built in a scratch index (`git add -A` into a copy of the real index), so
//! untracked files are included and ignored files are not.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Checkpoint {
    /// `refs/anchorleg/run-<id>/<n>`.
    pub refname: String,
    pub commit: String,
}

#[derive(Debug, thiserror::Error)]
#[error("git {args}: {message}")]
pub struct GitError {
    args: String,
    message: String,
}

fn git(cwd: &Path, args: &[&str], index: Option<&Path>) -> Result<String, GitError> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(cwd).args(args);
    if let Some(i) = index {
        cmd.env("GIT_INDEX_FILE", i);
    }
    // The checkpoint is anchorleg's, and must not fail on a machine with no git identity.
    cmd.env("GIT_AUTHOR_NAME", "anchorleg")
        .env("GIT_AUTHOR_EMAIL", "anchorleg@localhost")
        .env("GIT_COMMITTER_NAME", "anchorleg")
        .env("GIT_COMMITTER_EMAIL", "anchorleg@localhost");
    let err = |message: String| GitError {
        args: args.join(" "),
        message,
    };
    let out = cmd.output().map_err(|e| err(e.to_string()))?;
    if !out.status.success() {
        return Err(err(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
}

/// The repository's top folder, or `None` when `cwd` isn't in a git work tree.
pub fn toplevel(cwd: &Path) -> Option<PathBuf> {
    git(cwd, &["rev-parse", "--show-toplevel"], None)
        .ok()
        .map(PathBuf::from)
}

/// `git status --short`, or `None` outside a repository.
pub fn status(cwd: &Path) -> Option<String> {
    git(cwd, &["status", "--short", "--untracked-files=all"], None).ok()
}

/// Save the working tree as `refs/anchorleg/run-<run_id>/<n>`. `Ok(None)`: not a repository, or
/// nothing changed since `HEAD`.
pub fn save(
    cwd: &Path,
    run_id: i64,
    n: usize,
    message: &str,
) -> Result<Option<Checkpoint>, GitError> {
    let Some(top) = toplevel(cwd) else {
        return Ok(None);
    };
    let real_index = PathBuf::from(git(
        &top,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        None,
    )?);
    let scratch = PathBuf::from(git(
        &top,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            &format!("anchorleg-index-{}", std::process::id()),
        ],
        None,
    )?);
    if real_index.exists() {
        std::fs::copy(&real_index, &scratch).map_err(|e| GitError {
            args: "copy index".into(),
            message: e.to_string(),
        })?;
    }
    let result = (|| {
        git(&top, &["add", "-A", "--", "."], Some(&scratch))?;
        let tree = git(&top, &["write-tree"], Some(&scratch))?;
        let head = git(
            &top,
            &["rev-parse", "--verify", "-q", "HEAD^{commit}"],
            None,
        )
        .ok();
        if let Some(h) = &head
            && git(&top, &["rev-parse", &format!("{h}^{{tree}}")], None)? == tree
        {
            return Ok(None);
        }
        let mut args = vec!["commit-tree", tree.as_str(), "-m", message];
        if let Some(h) = &head {
            args.extend(["-p", h.as_str()]);
        }
        let commit = git(&top, &args, None)?;
        let refname = format!("refs/anchorleg/run-{run_id}/{n}");
        git(&top, &["update-ref", &refname, &commit], None)?;
        Ok(Some(Checkpoint { refname, commit }))
    })();
    let _ = std::fs::remove_file(&scratch);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"], None).unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"], None).unwrap();
        git(dir.path(), &["commit", "-q", "-m", "init"], None).unwrap();
        dir
    }

    #[test]
    fn saves_changes_without_touching_the_branch_or_index() {
        let dir = repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        std::fs::write(p.join("new.txt"), "new\n").unwrap();
        std::fs::write(p.join(".gitignore"), "skip.txt\n").unwrap();
        std::fs::write(p.join("skip.txt"), "x\n").unwrap();
        let head_before = git(p, &["rev-parse", "HEAD"], None).unwrap();
        let status_before = status(p).unwrap();

        let cp = save(p, 7, 1, "anchorleg checkpoint").unwrap().unwrap();
        assert_eq!(cp.refname, "refs/anchorleg/run-7/1");
        assert_eq!(git(p, &["rev-parse", "HEAD"], None).unwrap(), head_before);
        assert_eq!(status(p).unwrap(), status_before);
        let files = git(p, &["ls-tree", "--name-only", &cp.commit], None).unwrap();
        assert_eq!(
            files.lines().collect::<Vec<_>>(),
            [".gitignore", "a.txt", "new.txt"]
        );
        assert_eq!(
            git(p, &["show", &format!("{}:a.txt", cp.commit)], None).unwrap(),
            "two"
        );
        assert_eq!(
            git(p, &["rev-parse", &format!("{}^", cp.commit)], None).unwrap(),
            head_before
        );
    }

    #[test]
    fn nothing_to_save() {
        let dir = repo();
        assert_eq!(save(dir.path(), 1, 1, "m").unwrap(), None);
        let plain = tempfile::tempdir().unwrap();
        assert_eq!(save(plain.path(), 1, 1, "m").unwrap(), None);
        assert_eq!(status(plain.path()), None);
    }

    #[test]
    fn works_before_the_first_commit() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"], None).unwrap();
        std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
        let cp = save(dir.path(), 2, 1, "m").unwrap().unwrap();
        assert!(git(dir.path(), &["rev-parse", "--verify", "-q", "HEAD"], None).is_err());
        assert_eq!(
            git(dir.path(), &["ls-tree", "--name-only", &cp.commit], None).unwrap(),
            "a.txt"
        );
    }
}
