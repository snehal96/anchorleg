//! What a task adds to every launch, whichever account or CLI takes it: more folders, its own
//! instructions, an effort level, and Claude Code's own options. Kept with the run (as JSON), so
//! follow-ups, forks and switches launch the same way.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskOptions {
    /// More folders the agent may read and write besides the task's own (Claude Code and
    /// Antigravity `--add-dir`, Codex's writable roots). Absolute paths.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub add_dirs: Vec<PathBuf>,
    /// Instructions for the whole task, on top of the CLI's own: Claude Code
    /// `--append-system-prompt`, Codex developer instructions; Antigravity has neither and gets
    /// them at the top of a new session's first message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Reasoning effort (`low` … `max`), over the CLI settings' own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Options only Claude Code has; other CLIs leave them out (and the run says so).
    #[serde(skip_serializing_if = "ClaudeOptions::is_empty")]
    pub claude: ClaudeOptions,
}

/// Claude Code flags passed as they are (`claude --help`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeOptions {
    /// `--agent`: run the session as this agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// `--agents`: custom agents, as JSON or a file holding it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents: Option<String>,
    /// `--allowedTools`: tool rules allowed without asking, e.g. `Bash(git *)`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// `--disallowedTools`: tool rules refused.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disallowed_tools: Vec<String>,
    /// `--tools`: the built-in tools there are at all.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// `--mcp-config`: MCP servers from these JSON files or strings.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mcp_config: Vec<String>,
    /// `--strict-mcp-config`: only the servers from `mcp_config`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub strict_mcp_config: bool,
    /// `--fallback-model`: models to fall back on when the session's is overloaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
    /// `--settings`: more settings, a file or JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<String>,
}

impl ClaudeOptions {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The flags, in `claude`'s own spelling, for what's set.
    pub fn names(&self) -> Vec<&'static str> {
        [
            (self.agent.is_some(), "--agent"),
            (self.agents.is_some(), "--agents"),
            (!self.allowed_tools.is_empty(), "--allowed-tools"),
            (!self.disallowed_tools.is_empty(), "--disallowed-tools"),
            (!self.tools.is_empty(), "--tools"),
            (!self.mcp_config.is_empty(), "--mcp-config"),
            (self.strict_mcp_config, "--strict-mcp-config"),
            (self.fallback_model.is_some(), "--fallback-model"),
            (self.settings.is_some(), "--settings"),
        ]
        .into_iter()
        .filter_map(|(set, name)| set.then_some(name))
        .collect()
    }

    /// The arguments for `claude`. `allowed` adds rules of anchorleg's own (lasting rules), so
    /// there's one `--allowedTools`. List flags take one comma-separated value each, so none of
    /// them swallows the next argument.
    pub fn args(&self, allowed: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        let mut flag = |name: &str, value: String| {
            out.push(name.to_owned());
            out.push(value);
        };
        if let Some(a) = &self.agent {
            flag("--agent", a.clone());
        }
        if let Some(a) = &self.agents {
            flag("--agents", a.clone());
        }
        let allow: Vec<&str> = allowed
            .iter()
            .chain(&self.allowed_tools)
            .map(String::as_str)
            .collect();
        if !allow.is_empty() {
            flag("--allowedTools", allow.join(","));
        }
        if !self.disallowed_tools.is_empty() {
            flag("--disallowedTools", self.disallowed_tools.join(","));
        }
        if !self.tools.is_empty() {
            flag("--tools", self.tools.join(","));
        }
        for c in &self.mcp_config {
            flag("--mcp-config", c.clone());
        }
        if let Some(m) = &self.fallback_model {
            flag("--fallback-model", m.clone());
        }
        if let Some(s) = &self.settings {
            flag("--settings", s.clone());
        }
        if self.strict_mcp_config {
            out.push("--strict-mcp-config".to_owned());
        }
        out
    }
}

impl TaskOptions {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// From the JSON kept with a run; anything unreadable is no options.
    pub fn from_json(text: Option<&str>) -> Self {
        text.and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or_default()
    }

    pub fn to_json(&self) -> Option<String> {
        (!self.is_empty()).then(|| serde_json::to_string(self).unwrap_or_default())
    }

    /// `--add-dir` for Claude Code and Antigravity, one flag per folder.
    pub fn add_dir_args(&self) -> Vec<String> {
        self.add_dirs
            .iter()
            .flat_map(|d| ["--add-dir".to_owned(), d.display().to_string()])
            .collect()
    }

    /// Codex's `config` overrides on `thread/start`/`resume`/`fork`: the extra folders as
    /// writable roots, as `codex exec --add-dir` does.
    pub fn codex_config(&self) -> Option<Value> {
        (!self.add_dirs.is_empty()).then(|| {
            let roots: Vec<String> = self
                .add_dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect();
            json!({ "sandbox_workspace_write": { "writable_roots": roots } })
        })
    }

    /// A new Antigravity session's first message, with the task's instructions on top.
    pub fn agy_prompt(&self, prompt: &str) -> String {
        match &self.instructions {
            Some(i) => format!(
                "Instructions for this whole task (follow them in every step):\n{i}\n\n---\n\n{prompt}"
            ),
            None => prompt.to_owned(),
        }
    }

    /// `self`, with what `parent` had wherever `self` sets nothing (a follow-up keeps its
    /// session's options; what's given again replaces them).
    pub fn or(mut self, parent: &TaskOptions) -> Self {
        let p = parent.clone();
        if self.add_dirs.is_empty() {
            self.add_dirs = p.add_dirs;
        }
        self.instructions = self.instructions.or(p.instructions);
        self.effort = self.effort.or(p.effort);
        let (c, pc) = (&mut self.claude, p.claude);
        c.agent = c.agent.take().or(pc.agent);
        c.agents = c.agents.take().or(pc.agents);
        for (mine, theirs) in [
            (&mut c.allowed_tools, pc.allowed_tools),
            (&mut c.disallowed_tools, pc.disallowed_tools),
            (&mut c.tools, pc.tools),
            (&mut c.mcp_config, pc.mcp_config),
        ] {
            if mine.is_empty() {
                *mine = theirs;
            }
        }
        c.strict_mcp_config |= pc.strict_mcp_config;
        c.fallback_model = c.fallback_model.take().or(pc.fallback_model);
        c.settings = c.settings.take().or(pc.settings);
        self
    }

    /// What a side question or a review keeps: the folders, to read the same files. They answer
    /// in their own read-only way, so not the rest.
    pub fn for_read_only(&self) -> Self {
        Self {
            add_dirs: self.add_dirs.clone(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_leaves_out_what_is_unset() {
        let o = TaskOptions {
            add_dirs: vec!["/x".into()],
            claude: ClaudeOptions {
                strict_mcp_config: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let j = o.to_json().unwrap();
        assert_eq!(
            j,
            r#"{"add_dirs":["/x"],"claude":{"strict_mcp_config":true}}"#
        );
        assert_eq!(TaskOptions::from_json(Some(&j)), o);
        assert_eq!(TaskOptions::default().to_json(), None);
        assert_eq!(
            TaskOptions::from_json(Some("not json")),
            TaskOptions::default()
        );
    }

    #[test]
    fn a_follow_up_keeps_what_it_does_not_give_again() {
        let parent = TaskOptions {
            add_dirs: vec!["/a".into()],
            instructions: Some("be brief".into()),
            claude: ClaudeOptions {
                agent: Some("reviewer".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mine = TaskOptions {
            instructions: Some("be thorough".into()),
            ..Default::default()
        }
        .or(&parent);
        assert_eq!(mine.add_dirs, vec![PathBuf::from("/a")]);
        assert_eq!(mine.instructions.as_deref(), Some("be thorough"));
        assert_eq!(mine.claude.agent.as_deref(), Some("reviewer"));
    }

    #[test]
    fn claude_args_merge_anchorlegs_rules_and_never_leave_a_list_open() {
        let c = ClaudeOptions {
            allowed_tools: vec!["Bash(git *)".into()],
            disallowed_tools: vec!["WebFetch".into(), "WebSearch".into()],
            mcp_config: vec!["a.json".into(), "b.json".into()],
            strict_mcp_config: true,
            ..Default::default()
        };
        assert_eq!(
            c.args(&["Read".into()]),
            [
                "--allowedTools",
                "Read,Bash(git *)",
                "--disallowedTools",
                "WebFetch,WebSearch",
                "--mcp-config",
                "a.json",
                "--mcp-config",
                "b.json",
                "--strict-mcp-config",
            ]
        );
        assert_eq!(
            c.names(),
            [
                "--allowed-tools",
                "--disallowed-tools",
                "--mcp-config",
                "--strict-mcp-config"
            ]
        );
    }

    #[test]
    fn codex_gets_the_folders_as_writable_roots() {
        let o = TaskOptions {
            add_dirs: vec!["/a".into(), "/b".into()],
            ..Default::default()
        };
        assert_eq!(
            o.codex_config().unwrap(),
            json!({ "sandbox_workspace_write": { "writable_roots": ["/a", "/b"] } })
        );
        assert_eq!(o.add_dir_args(), ["--add-dir", "/a", "--add-dir", "/b"]);
        assert!(TaskOptions::default().codex_config().is_none());
    }
}
