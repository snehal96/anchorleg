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

/// The working tree as a git tree id, built in a scratch index (`git add -A` into a copy of the
/// real index): untracked files are in, ignored ones out, the real index is untouched.
fn snapshot_tree(top: &Path) -> Result<String, GitError> {
    let real_index = PathBuf::from(git(
        top,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        None,
    )?);
    let scratch = PathBuf::from(git(
        top,
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
    let tree = git(top, &["add", "-A", "--", "."], Some(&scratch))
        .and_then(|_| git(top, &["write-tree"], Some(&scratch)));
    let _ = std::fs::remove_file(&scratch);
    tree
}

fn head(top: &Path) -> Option<String> {
    git(top, &["rev-parse", "--verify", "-q", "HEAD^{commit}"], None).ok()
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
    let tree = snapshot_tree(&top)?;
    let head = head(&top);
    if let Some(h) = &head
        && git(&top, &["rev-parse", &format!("{h}^{{tree}}")], None)? == tree
    {
        return Ok(None);
    }
    let refname = format!("refs/anchorleg/run-{run_id}/{n}");
    commit_ref(&top, &tree, head.as_deref(), message, &refname).map(Some)
}

fn commit_ref(
    top: &Path,
    tree: &str,
    parent: Option<&str>,
    message: &str,
    refname: &str,
) -> Result<Checkpoint, GitError> {
    let mut args = vec!["commit-tree", tree, "-m", message];
    if let Some(h) = parent {
        args.extend(["-p", h]);
    }
    let commit = git(top, &args, None)?;
    git(top, &["update-ref", refname, &commit], None)?;
    Ok(Checkpoint {
        refname: refname.to_owned(),
        commit,
    })
}

/// The ref holding the working tree as a run found it.
pub fn start_ref(run_id: i64) -> String {
    format!("refs/anchorleg/run-{run_id}/start")
}

/// Save the working tree as the run starts (`refs/anchorleg/run-<id>/start`), even when it
/// matches `HEAD`, so the session's own changes can be shown later ([`diff_since_start`]).
/// `Ok(None)` outside a repository.
pub fn save_start(cwd: &Path, run_id: i64) -> Result<Option<Checkpoint>, GitError> {
    let Some(top) = toplevel(cwd) else {
        return Ok(None);
    };
    let tree = snapshot_tree(&top)?;
    let head = head(&top);
    commit_ref(
        &top,
        &tree,
        head.as_deref(),
        &format!("anchorleg: run {run_id} started"),
        &start_ref(run_id),
    )
    .map(Some)
}

/// What changed in a session: since the oldest of `runs` (a run and its follow-ups, any order of
/// ids) that saved a start. `Ok(None)`: not a repository, or none did.
pub fn session_diff(cwd: &Path, runs: &[i64], stat: bool) -> Result<Option<String>, GitError> {
    let mut ids = runs.to_vec();
    ids.sort_unstable();
    for id in ids {
        if let Some(d) = diff_since_start(cwd, id, stat)? {
            return Ok(Some(d));
        }
    }
    Ok(None)
}

/// What changed in the working tree since run `run_id` started: `git diff` (or `--stat`), new
/// files included. `Ok(None)`: not a repository, or that run saved no start.
pub fn diff_since_start(cwd: &Path, run_id: i64, stat: bool) -> Result<Option<String>, GitError> {
    let Some(top) = toplevel(cwd) else {
        return Ok(None);
    };
    let start = format!("{}^{{tree}}", start_ref(run_id));
    if git(&top, &["rev-parse", "--verify", "-q", &start], None).is_err() {
        return Ok(None);
    }
    let now = snapshot_tree(&top)?;
    let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
    if stat {
        args.push("--stat");
    }
    args.extend([start.as_str(), now.as_str()]);
    git(&top, &args, None).map(Some)
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
    fn the_session_diff_starts_where_the_run_did() {
        let dir = repo();
        let p = dir.path();
        // The person's own uncommitted change before the run isn't the session's.
        std::fs::write(p.join("a.txt"), "mine\n").unwrap();
        save_start(p, 7).unwrap().unwrap();
        assert_eq!(diff_since_start(p, 7, false).unwrap().unwrap(), "");
        std::fs::write(p.join("a.txt"), "mine\nagent\n").unwrap();
        std::fs::write(p.join("new.txt"), "hello\n").unwrap();
        let diff = diff_since_start(p, 7, false).unwrap().unwrap();
        assert!(diff.contains("+agent") && !diff.contains("+mine"), "{diff}");
        assert!(
            diff.contains("new file mode") && diff.contains("+hello"),
            "{diff}"
        );
        let stat = diff_since_start(p, 7, true).unwrap().unwrap();
        assert!(stat.contains("2 files changed"), "{stat}");
        assert_eq!(diff_since_start(p, 8, false).unwrap(), None);
        // Nothing staged by it.
        assert_eq!(
            git(p, &["diff", "--cached", "--name-only"], None).unwrap(),
            ""
        );
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
