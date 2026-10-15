//! What a session changed, as the CLIs report it: Claude Code's Edit/Write results
//! (`tool_use_result` with `structuredPatch`, or a new file's `content`), Codex's file changes
//! (each with its diff), Antigravity's file tools (the path only: agy reports no diff). This
//! works in any folder, git or not; changes made by shell commands aren't reported, so they
//! aren't here.

use serde::Serialize;
use serde_json::Value;

use crate::adapters::agy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Created,
    Modified,
    Deleted,
}

/// One file's changes over the session.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Change {
    /// As the CLI named it (absolute, as all three report paths).
    pub path: String,
    pub kind: Kind,
    /// Every change in order, as unified-diff hunks.
    pub diff: String,
    pub added: usize,
    pub removed: usize,
    /// False when the CLI named the file but showed no diff (Antigravity).
    pub has_diff: bool,
}

/// One reported change to one file.
struct Edit {
    path: String,
    kind: Kind,
    hunks: Option<String>,
}

/// `text` as added (`+`) or removed (`-`) lines, in one hunk.
fn whole(text: &str, sign: char) -> String {
    let n = text.lines().count();
    let mut out = match sign {
        '+' => format!("@@ -0,0 +1,{n} @@\n"),
        _ => format!("@@ -1,{n} +0,0 @@\n"),
    };
    for line in text.lines() {
        out.push(sign);
        out += line;
        out.push('\n');
    }
    out
}

/// Claude Code's `structuredPatch` as unified-diff hunks.
fn structured(patch: &[Value]) -> String {
    let mut out = String::new();
    for h in patch {
        let n = |k: &str| h.get(k).and_then(Value::as_u64).unwrap_or(0);
        out += &format!(
            "@@ -{},{} +{},{} @@\n",
            n("oldStart"),
            n("oldLines"),
            n("newStart"),
            n("newLines")
        );
        for line in h
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            out += line.as_str().unwrap_or_default();
            out.push('\n');
        }
    }
    out
}

/// The file changes one log line reports.
fn edits(v: &Value) -> Vec<Edit> {
    // Claude Code: the tool's result on the `user` event that carries it.
    if v.get("type").and_then(Value::as_str) == Some("user")
        && let Some(r) = v.get("tool_use_result").filter(|r| r.is_object())
        && let Some(path) = r.get("filePath").and_then(Value::as_str)
    {
        let edit = if r.get("type").and_then(Value::as_str) == Some("create") {
            let content = r.get("content").and_then(Value::as_str).unwrap_or_default();
            Edit {
                path: path.to_owned(),
                kind: Kind::Created,
                hunks: Some(whole(content, '+')),
            }
        } else {
            let Some(patch) = r.get("structuredPatch").and_then(Value::as_array) else {
                return Vec::new();
            };
            Edit {
                path: path.to_owned(),
                kind: Kind::Modified,
                hunks: Some(structured(patch)),
            }
        };
        return vec![edit];
    }
    // Codex: a completed file change (`codex_server` adds each change's diff).
    if v.get("type").and_then(Value::as_str) == Some("item.completed")
        && let Some(item) = v.get("item")
        && item.get("type").and_then(Value::as_str) == Some("file_change")
        && item.get("status").and_then(Value::as_str) == Some("completed")
    {
        return item
            .get("changes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let path = c.get("path").and_then(Value::as_str)?.to_owned();
                let diff = c.get("diff").and_then(Value::as_str);
                let (kind, hunks) = match c.get("kind").and_then(Value::as_str) {
                    Some("add") => (Kind::Created, diff.map(|d| whole(d, '+'))),
                    Some("delete") => (Kind::Deleted, diff.map(|d| whole(d, '-'))),
                    _ => (Kind::Modified, diff.map(str::to_owned)),
                };
                Some(Edit { path, kind, hunks })
            })
            .collect();
    }
    // Antigravity: a file tool that finished. The path, no diff.
    if v.get("event").and_then(Value::as_str) == Some("step_update")
        && let Some(step) = v.get("step_update")
        && step.get("state").and_then(Value::as_str) == Some("DONE")
        && let Some(name) = step.get("tool_name").and_then(Value::as_str)
        && let Some(params) = step.pointer("/tool_info/parameters")
        && let Some((tool, path)) = agy::tool_call(name, params)
    {
        let kind = match tool {
            "Write" => Kind::Created,
            "Edit" => Kind::Modified,
            _ => return Vec::new(),
        };
        return vec![Edit {
            path,
            kind,
            hunks: None,
        }];
    }
    Vec::new()
}

/// What one run's log reports, per file, in the order files were first touched.
pub fn from_log(log: &str) -> Vec<Change> {
    let mut out: Vec<Change> = Vec::new();
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        for e in edits(&v) {
            add(&mut out, e);
        }
    }
    out
}

fn add(out: &mut Vec<Change>, e: Edit) {
    let (added, removed) = e.hunks.as_deref().map_or((0, 0), count);
    let i = match out.iter().position(|c| c.path == e.path) {
        Some(i) => i,
        None => {
            out.push(Change {
                path: e.path.clone(),
                kind: e.kind,
                diff: String::new(),
                added: 0,
                removed: 0,
                has_diff: true,
            });
            out.len() - 1
        }
    };
    if out[i].kind == Kind::Created && e.kind == Kind::Deleted {
        // Made and removed again: nothing left to show.
        out.remove(i);
        return;
    }
    let c = &mut out[i];
    c.kind = match (c.kind, e.kind) {
        // Made and then edited in the session: still a file it created.
        (Kind::Created, Kind::Modified) => Kind::Created,
        (_, k) => k,
    };
    match e.hunks {
        Some(h) => {
            c.diff += &h;
            c.added += added;
            c.removed += removed;
        }
        None => c.has_diff = false,
    }
}

/// Added and removed lines in hunks.
fn count(hunks: &str) -> (usize, usize) {
    hunks.lines().fold((0, 0), |(a, r), l| {
        if l.starts_with('+') && !l.starts_with("+++") {
            (a + 1, r)
        } else if l.starts_with('-') && !l.starts_with("---") {
            (a, r + 1)
        } else {
            (a, r)
        }
    })
}

/// A session's changes from its runs' logs (`<log_dir>/run-<id>.jsonl`), oldest run first: one
/// entry per file. A file created and deleted again in the session isn't listed.
pub fn session(log_dir: &std::path::Path, runs: &[i64]) -> Vec<Change> {
    let mut runs = runs.to_vec();
    runs.sort_unstable();
    let mut out: Vec<Change> = Vec::new();
    for id in runs {
        let log =
            std::fs::read_to_string(log_dir.join(format!("run-{id}.jsonl"))).unwrap_or_default();
        for c in from_log(&log) {
            if let Some(i) = out
                .iter()
                .position(|o| o.path == c.path && o.kind == Kind::Created)
                .filter(|_| c.kind == Kind::Deleted)
            {
                out.remove(i);
                continue;
            }
            match out.iter_mut().find(|o| o.path == c.path) {
                Some(o) => {
                    o.kind = match (o.kind, c.kind) {
                        (Kind::Created, Kind::Modified) => Kind::Created,
                        (_, k) => k,
                    };
                    o.diff += &c.diff;
                    o.added += c.added;
                    o.removed += c.removed;
                    o.has_diff &= c.has_diff;
                }
                None => out.push(c),
            }
        }
    }
    out
}

/// `path` relative to `cwd` when it's inside it.
pub fn relative<'a>(path: &'a str, cwd: &str) -> &'a str {
    path.strip_prefix(&format!("{}/", cwd.trim_end_matches('/')))
        .unwrap_or(path)
}

/// The session's changes as one unified diff, a `--- a/` / `+++ b/` header per file.
pub fn unified(changes: &[Change], cwd: &str) -> String {
    let mut out = String::new();
    for c in changes {
        let p = relative(&c.path, cwd);
        let (from, to) = match c.kind {
            Kind::Created => ("/dev/null".to_owned(), format!("b/{p}")),
            Kind::Deleted => (format!("a/{p}"), "/dev/null".to_owned()),
            Kind::Modified => (format!("a/{p}"), format!("b/{p}")),
        };
        out += &format!("--- {from}\n+++ {to}\n");
        if c.has_diff {
            out += &c.diff;
        } else {
            out += "(changed; this CLI doesn't report the diff)\n";
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claudes_edit_and_write_results() {
        let log = include_str!("../../../fixtures/claude-2.1.300-edit-write-20261015/stream.jsonl");
        let c = from_log(log);
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(relative(&c[0].path, "/tmp/scratch/edits"), "calc.py");
        assert_eq!(c[0].kind, Kind::Modified);
        assert_eq!((c[0].added, c[0].removed), (1, 1));
        assert_eq!(
            c[0].diff,
            "@@ -1,2 +1,2 @@\n def add(a, b):\n-    return a - b\n+    return a + b\n"
        );
        assert_eq!(relative(&c[1].path, "/tmp/scratch/edits"), "notes.md");
        assert_eq!(c[1].kind, Kind::Created);
        assert_eq!(c[1].diff, "@@ -0,0 +1,1 @@\n+fixed add\n");
        let u = unified(&c, "/tmp/scratch/edits");
        assert!(u.starts_with("--- a/calc.py\n+++ b/calc.py\n@@"), "{u}");
        assert!(u.contains("--- /dev/null\n+++ b/notes.md\n"), "{u}");
    }

    #[test]
    fn codex_and_agy_changes() {
        let codex = serde_json::json!({ "type": "item.completed", "item": { "type": "file_change",
            "status": "completed", "changes": [
                { "path": "/w/new.txt", "kind": "add", "diff": "hello\n" },
                { "path": "/w/a.rs", "kind": "update", "diff": "@@ -1 +1 @@\n-a\n+b\n" }] } });
        let declined = serde_json::json!({ "type": "item.completed", "item": {
            "type": "file_change", "status": "declined",
            "changes": [{ "path": "/w/x", "kind": "add", "diff": "x" }] } });
        let agy = serde_json::json!({ "event": "step_update", "step_update": { "state": "DONE",
            "step_type": "tool", "tool_name": "write_to_file",
            "tool_info": { "name": "write_to_file", "parameters": { "TargetFile": "/w/out.md" } } } });
        let log = format!("{codex}\n{declined}\n{agy}\n");
        let c = from_log(&log);
        let summary: Vec<_> = c
            .iter()
            .map(|c| (c.path.as_str(), c.kind, c.added, c.removed, c.has_diff))
            .collect();
        assert_eq!(
            summary,
            [
                ("/w/new.txt", Kind::Created, 1, 0, true),
                ("/w/a.rs", Kind::Modified, 1, 1, true),
                ("/w/out.md", Kind::Created, 0, 0, false),
            ]
        );
    }

    #[test]
    fn a_session_adds_up_its_runs() {
        let dir = tempfile::tempdir().unwrap();
        let line = |path: &str, kind: &str, diff: &str| {
            serde_json::json!({ "type": "item.completed", "item": { "type": "file_change",
                "status": "completed", "changes": [{ "path": path, "kind": kind, "diff": diff }] } })
                .to_string()
        };
        std::fs::write(
            dir.path().join("run-1.jsonl"),
            line("/w/n.txt", "add", "one\n"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("run-2.jsonl"),
            line("/w/n.txt", "update", "@@ -1 +1 @@\n-one\n+two\n"),
        )
        .unwrap();
        let c = session(dir.path(), &[2, 1]);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].kind, Kind::Created);
        assert_eq!((c[0].added, c[0].removed), (2, 1));
    }
}
