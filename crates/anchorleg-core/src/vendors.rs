//! What each vendor CLI can do with anchorleg, in one place. Code that asks "can this vendor
//! …?" reads it here instead of matching on the vendor, and `anchorleg vendors --json` gives it
//! to the desktop app, which shows only what an account's CLI supports. How a CLI is launched
//! and how its events are read stay in its adapter (`adapters/`).

use serde::Serialize;

use crate::registry::Vendor;

/// Where an account's quota comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaSource {
    /// Read from the CLI on demand (`claude -p /usage`, Codex's rate-limit request) and from runs.
    Live,
    /// Only what runs report as they go.
    Runs,
    /// No reading: anchorleg switches when a run fails on a limit.
    None,
}

/// How a new account signs in (`anchorleg accounts login`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SignIn {
    /// The CLI's own sign-in command, which opens the browser; `check` exits 0 once signed in
    /// (Claude: prints `"loggedIn": true`).
    Browser {
        #[serde(skip)]
        args: &'static [&'static str],
        #[serde(skip)]
        check: &'static [&'static str],
    },
    /// The CLI signs in from its own interactive screen, so it needs a terminal.
    Terminal {
        #[serde(skip)]
        args: &'static [&'static str],
        #[serde(skip)]
        check: &'static [&'static str],
    },
    /// Not through anchorleg (yet).
    Manual,
}

impl SignIn {
    pub fn commands(self) -> Option<(&'static [&'static str], &'static [&'static str])> {
        match self {
            Self::Browser { args, check } | Self::Terminal { args, check } => Some((args, check)),
            Self::Manual => None,
        }
    }
}

/// One vendor's row.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Caps {
    pub vendor: Vendor,
    /// The product's name, for people.
    pub label: &'static str,
    /// anchorleg can run tasks on it (it has an adapter).
    pub driven: bool,
    pub quota: QuotaSource,
    /// A session goes on in a later run (a reply, or after a switch).
    pub resume: bool,
    /// A new session from this one's history (`--fork`).
    pub fork: bool,
    /// A read-only side question (`--btw`).
    pub side: bool,
    /// Back to before a run (`--rewind`).
    pub rewind: bool,
    /// Plan first, approve, then carry it out (`--plan`).
    pub plan: bool,
    /// Tool calls can wait for the person's approval.
    pub approvals: bool,
    /// The agent can ask the person questions with options.
    pub questions: bool,
    /// The final answer can follow a JSON Schema (`--json-schema`).
    pub json_schema: bool,
    /// Project instruction files the CLI reads by itself.
    pub instructions: &'static [&'static str],
    /// The env var that points the CLI at an account's own folder, and that folder's prefix in
    /// the home directory (`~/.claude-NAME`).
    pub folder_env: Option<&'static str>,
    pub folder_prefix: Option<&'static str>,
    pub sign_in: SignIn,
}

const CLAUDE: Caps = Caps {
    vendor: Vendor::Claude,
    label: "Claude Code",
    driven: true,
    quota: QuotaSource::Live,
    resume: true,
    fork: true,
    side: true,
    rewind: true,
    plan: true,
    approvals: true,
    questions: true,
    json_schema: true,
    instructions: &["CLAUDE.md"],
    folder_env: Some("CLAUDE_CONFIG_DIR"),
    folder_prefix: Some(".claude-"),
    sign_in: SignIn::Browser {
        args: &["auth", "login"],
        check: &["auth", "status", "--json"],
    },
};

const CODEX: Caps = Caps {
    vendor: Vendor::Codex,
    label: "Codex",
    driven: true,
    quota: QuotaSource::Live,
    resume: true,
    fork: true,
    side: true,
    rewind: true,
    plan: true,
    approvals: true,
    questions: true,
    json_schema: true,
    instructions: &["AGENTS.md"],
    folder_env: Some("CODEX_HOME"),
    folder_prefix: Some(".codex-"),
    sign_in: SignIn::Browser {
        args: &["login"],
        check: &["login", "status"],
    },
};

const ANTIGRAVITY: Caps = Caps {
    vendor: Vendor::Antigravity,
    label: "Antigravity",
    driven: true,
    quota: QuotaSource::None,
    resume: true,
    fork: false,
    side: false,
    rewind: false,
    plan: true,
    approvals: true,
    questions: false,
    json_schema: true,
    instructions: &["GEMINI.md", "AGENTS.md"],
    // No narrower setting than HOME; in a HOME of its own agy can't reach the Keychain and
    // keeps the login as a file there.
    folder_env: Some("HOME"),
    folder_prefix: Some(".agy-"),
    sign_in: SignIn::Terminal {
        args: &[],
        check: &["models"],
    },
};

const KIMI: Caps = Caps {
    vendor: Vendor::Kimi,
    label: "Kimi",
    driven: false,
    quota: QuotaSource::None,
    resume: false,
    fork: false,
    side: false,
    rewind: false,
    plan: false,
    approvals: false,
    questions: false,
    json_schema: false,
    instructions: &[],
    folder_env: None,
    folder_prefix: None,
    sign_in: SignIn::Manual,
};

const CURSOR: Caps = Caps {
    vendor: Vendor::Cursor,
    label: "Cursor",
    driven: false,
    quota: QuotaSource::None,
    resume: false,
    fork: false,
    side: false,
    rewind: false,
    plan: false,
    approvals: false,
    questions: false,
    json_schema: false,
    instructions: &[],
    folder_env: None,
    folder_prefix: None,
    sign_in: SignIn::Manual,
};

impl Vendor {
    pub fn caps(self) -> &'static Caps {
        match self {
            Self::Claude => &CLAUDE,
            Self::Codex => &CODEX,
            Self::Antigravity => &ANTIGRAVITY,
            Self::Kimi => &KIMI,
            Self::Cursor => &CURSOR,
        }
    }

    /// The vendors anchorleg can run tasks on.
    pub fn driven() -> impl Iterator<Item = Self> {
        Self::ALL.into_iter().filter(|v| v.caps().driven)
    }

    /// From a config or command-line name (`agy` too).
    pub fn parse(name: &str) -> Option<Self> {
        let name = if name == "agy" { "antigravity" } else { name };
        Self::ALL.into_iter().find(|v| v.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_vendor_has_its_own_row() {
        for v in Vendor::ALL {
            assert_eq!(v.caps().vendor, v);
        }
    }

    #[test]
    fn rows_are_consistent() {
        for v in Vendor::ALL {
            let c = v.caps();
            // Session features need sessions that go on.
            if c.fork || c.side || c.rewind {
                assert!(c.resume, "{v:?}");
            }
            // Only driven vendors claim run features.
            if !c.driven {
                assert!(
                    !(c.resume || c.plan || c.approvals || c.json_schema),
                    "{v:?}"
                );
            }
            // Signing in makes an account folder.
            if c.sign_in.commands().is_some() {
                assert!(c.folder_env.is_some() && c.folder_prefix.is_some(), "{v:?}");
            }
        }
        assert_eq!(Vendor::parse("agy"), Some(Vendor::Antigravity));
        assert_eq!(Vendor::parse("nope"), None);
    }
}
