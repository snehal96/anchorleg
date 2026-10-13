//! Lasting allow rules (`anchorleg rules`, `allow = [...]` in `config.toml`), written the way
//! Claude Code writes permission rules: `Tool` or `Tool(pattern)`, e.g. `Bash(cargo test *)`,
//! `Edit(src/**)`, `WebFetch(domain:docs.rs)`, `mcp__github__get_issue`.
//!
//! Claude Code gets them as `--allowedTools` and applies them itself; for Codex and Antigravity
//! anchorleg checks a tool call against them before asking the person.

use serde_json::Value;

/// Whether `rule` allows a call of `tool` with `input` (anchorleg's names: `Bash {command}`,
/// `Edit|Write|Read {file_path}`, `WebFetch {url}`, MCP tools by name). `cwd` makes relative
/// path patterns relative to the task folder.
pub fn matches(rule: &str, tool: &str, input: &Value, cwd: &str) -> bool {
    let rule = rule.trim();
    let (name, pattern) = match rule.split_once('(') {
        Some((name, rest)) => match rest.strip_suffix(')') {
            Some(p) => (name.trim(), Some(p.trim())),
            None => return false,
        },
        None => (rule, None),
    };
    if name != tool {
        return false;
    }
    let Some(pattern) = pattern.filter(|p| !p.is_empty() && *p != "*") else {
        return true;
    };
    let field = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default();
    match tool {
        // `npm run test:*` (Claude's older prefix form) means `npm run test *`.
        "Bash" => {
            let pattern = pattern
                .strip_suffix(":*")
                .map_or(pattern.to_owned(), |p| format!("{p} *"));
            let command = field("command").trim();
            glob(&pattern, command, false)
                || pattern
                    .strip_suffix(" *")
                    .is_some_and(|prefix| command == prefix)
        }
        "Edit" | "Write" | "Read" | "NotebookEdit" => {
            let path = field("file_path");
            let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
            if pattern.starts_with('/') || pattern.starts_with('~') {
                glob(pattern, path, true)
            } else {
                relative(path, cwd).is_some_and(|rel| glob(pattern, &rel, true))
            }
        }
        "WebFetch" => match pattern.strip_prefix("domain:") {
            Some(domain) => host(field("url"))
                .is_some_and(|h| h == domain || h.ends_with(&format!(".{domain}"))),
            None => glob(pattern, field("url"), false),
        },
        _ => false,
    }
}

/// Whether any of `rules` allows the call.
pub fn allowed(rules: &[String], tool: &str, input: &Value, cwd: &str) -> bool {
    rules.iter().any(|r| matches(r, tool, input, cwd))
}

/// The rule "always allow this, in every session" saves for a call: the command's first words
/// for Bash (`Bash(cargo test *)`), the file's folder for the file tools (`Edit(src/**)`), the
/// domain for WebFetch, the bare tool otherwise.
pub fn suggest(tool: &str, input: &Value, cwd: &str) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default();
    match tool {
        "Bash" => {
            let words: Vec<&str> = field("command").split_whitespace().collect();
            let keep = match words.as_slice() {
                [] => return "Bash".to_owned(),
                [first, second, ..] if !second.starts_with('-') && is_word(second) => {
                    vec![*first, *second]
                }
                [first, ..] => vec![*first],
            };
            format!("Bash({} *)", keep.join(" "))
        }
        "Edit" | "Write" | "Read" | "NotebookEdit" => {
            let path = field("file_path");
            let rel = relative(path, cwd).unwrap_or_else(|| path.to_owned());
            match rel.rsplit_once('/') {
                Some((dir, _)) => format!("{tool}({dir}/**)"),
                None if rel.starts_with('/') => format!("{tool}({rel})"),
                None => format!("{tool}(*)"),
            }
        }
        "WebFetch" => {
            host(field("url")).map_or("WebFetch".to_owned(), |h| format!("WebFetch(domain:{h})"))
        }
        _ => tool.to_owned(),
    }
}

/// `path` relative to `cwd`, also when one of them names it through a symlink (macOS reports
/// `/var/…` as `/private/var/…`).
fn relative(path: &str, cwd: &str) -> Option<String> {
    let strip = |base: &str| {
        path.strip_prefix(&format!("{}/", base.trim_end_matches('/')))
            .map(str::to_owned)
    };
    strip(cwd).or_else(|| {
        let real = std::fs::canonicalize(cwd).ok()?;
        let path = std::fs::canonicalize(path).ok().or_else(|| {
            let p = std::path::Path::new(path);
            Some(
                std::fs::canonicalize(p.parent()?)
                    .ok()?
                    .join(p.file_name()?),
            )
        })?;
        path.strip_prefix(&real)
            .ok()
            .map(|r| r.to_string_lossy().into_owned())
    })
}

/// A subcommand-like word (`test`, `run`), not a path or an argument with quotes.
fn is_word(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':')
}

fn host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    Some(host.split(':').next().unwrap_or(host)).filter(|h| !h.is_empty())
}

/// `*` matches anything (within one path segment when `path`), `**` anything at all.
fn glob(pattern: &str, text: &str, path: bool) -> bool {
    fn go(p: &[u8], t: &[u8], path: bool) -> bool {
        match p {
            [] => t.is_empty(),
            [b'*', b'*', rest @ ..] => {
                let rest = rest.strip_prefix(b"/").unwrap_or(rest);
                (0..=t.len()).any(|i| go(rest, &t[i..], path))
            }
            [b'*', rest @ ..] => (0..=t.len())
                .take_while(|&i| !path || i == 0 || t[i - 1] != b'/')
                .any(|i| go(rest, &t[i..], path)),
            [c, rest @ ..] => t.first() == Some(c) && go(rest, &t[1..], path),
        }
    }
    go(pattern.as_bytes(), text.as_bytes(), path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rules_match_like_claudes() {
        let bash = |c: &str| json!({ "command": c });
        assert!(matches(
            "Bash(cargo test *)",
            "Bash",
            &bash("cargo test --lib"),
            "/w"
        ));
        assert!(matches(
            "Bash(cargo test *)",
            "Bash",
            &bash("cargo test"),
            "/w"
        ));
        assert!(matches(
            "Bash(npm run test:*)",
            "Bash",
            &bash("npm run test unit"),
            "/w"
        ));
        assert!(!matches(
            "Bash(cargo test *)",
            "Bash",
            &bash("cargo build"),
            "/w"
        ));
        assert!(matches("Bash", "Bash", &bash("rm -rf /"), "/w"));
        assert!(!matches("Bash(git *)", "Edit", &bash("git status"), "/w"));

        let file = |p: &str| json!({ "file_path": p });
        assert!(matches(
            "Edit(src/**)",
            "Edit",
            &file("/w/src/a/b.rs"),
            "/w"
        ));
        assert!(!matches(
            "Edit(src/*)",
            "Edit",
            &file("/w/src/a/b.rs"),
            "/w"
        ));
        assert!(matches("Edit(src/*)", "Edit", &file("/w/src/b.rs"), "/w"));
        assert!(!matches(
            "Edit(src/**)",
            "Edit",
            &file("/elsewhere/src/a.rs"),
            "/w"
        ));
        assert!(matches("Write(/tmp/**)", "Write", &file("/tmp/x/y"), "/w"));

        let url = |u: &str| json!({ "url": u });
        assert!(matches(
            "WebFetch(domain:docs.rs)",
            "WebFetch",
            &url("https://docs.rs/serde"),
            "/w"
        ));
        assert!(!matches(
            "WebFetch(domain:docs.rs)",
            "WebFetch",
            &url("https://evil.com/docs.rs"),
            "/w"
        ));
        assert!(matches(
            "mcp__github__get_issue",
            "mcp__github__get_issue",
            &json!({}),
            "/w"
        ));
        assert!(!matches(
            "Bash(cargo test *",
            "Bash",
            &bash("cargo test"),
            "/w"
        ));
    }

    #[test]
    fn suggested_rules_cover_the_call_and_its_kind() {
        let s = suggest("Bash", &json!({ "command": "cargo test --lib" }), "/w");
        assert_eq!(s, "Bash(cargo test *)");
        assert_eq!(
            suggest("Bash", &json!({ "command": "ls -la" }), "/w"),
            "Bash(ls *)"
        );
        let e = suggest("Edit", &json!({ "file_path": "/w/src/a.rs" }), "/w");
        assert_eq!(e, "Edit(src/**)");
        assert!(matches(
            &e,
            "Edit",
            &json!({ "file_path": "/w/src/b/c.rs" }),
            "/w"
        ));
        assert_eq!(
            suggest("WebFetch", &json!({ "url": "https://docs.rs/x" }), "/w"),
            "WebFetch(domain:docs.rs)"
        );
    }
}
