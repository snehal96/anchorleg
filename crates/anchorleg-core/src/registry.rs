//! Accounts: `~/.config/anchorleg/config.toml`, how to launch each one, and their secrets.
//!
//! An account is a user-defined launch command (D9): a binary plus env vars and args (the
//! shape of the owner's `claude-sm` style aliases), an OAuth token kept in Keychain, or, as an
//! escape hatch, a command run through the user's interactive shell.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("reading {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("writing {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    #[error("serializing config: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("account `{account}`: {problem}")]
    Invalid { account: String, problem: String },
    #[error("can't parse command `{0}`")]
    BadCommand(String),
    #[error(
        "account `{0}` has token_from = \"keychain\" but no token is stored; run `anchorleg accounts add {0} --token-stdin`"
    )]
    MissingToken(String),
    #[error("keychain: {0}")]
    Keychain(#[from] keyring::Error),
    #[error("no home directory")]
    NoHome,
}

pub type Result<T> = std::result::Result<T, RegistryError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Claude,
    Codex,
    /// Google Antigravity's CLI, `agy`.
    Antigravity,
    Kimi,
    Cursor,
}

impl Vendor {
    pub fn default_bin(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "agy",
            Self::Kimi => "kimi",
            Self::Cursor => "cursor-agent",
        }
    }

    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Antigravity,
        Self::Kimi,
        Self::Cursor,
    ];

    /// The name used in config and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
            Self::Kimi => "kimi",
            Self::Cursor => "cursor",
        }
    }

    /// Effort levels the CLI accepts, when anchorleg knows them.
    pub fn efforts(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Claude => Some(&["low", "medium", "high", "xhigh", "max"]),
            // `ultra` only on some models (models_cache.json); the CLI rejects it elsewhere.
            Self::Codex => Some(&["low", "medium", "high", "xhigh", "max", "ultra"]),
            Self::Antigravity => Some(&["low", "medium", "high", "xhigh", "max"]),
            Self::Kimi | Self::Cursor => None,
        }
    }

    /// The CLI's own flags for a model and an effort. Empty where anchorleg doesn't drive that CLI
    /// yet (Phase 5 adds them with each adapter).
    pub fn model_args(self, model: Option<&str>, effort: Option<&str>) -> Vec<String> {
        let mut out = Vec::new();
        match self {
            Self::Claude | Self::Antigravity => {
                if let Some(m) = model {
                    out.extend(["--model".to_owned(), m.to_owned()]);
                }
                if let Some(e) = effort {
                    out.extend(["--effort".to_owned(), e.to_owned()]);
                }
            }
            Self::Codex => {
                if let Some(m) = model {
                    out.extend(["-m".to_owned(), m.to_owned()]);
                }
                if let Some(e) = effort {
                    out.extend(["-c".to_owned(), format!("model_reasoning_effort={e}")]);
                }
            }
            Self::Kimi | Self::Cursor => {}
        }
        out
    }

    /// The vendor whose official CLI this binary path points at.
    pub fn from_bin(bin: &str) -> Option<Self> {
        let name = Path::new(bin).file_name()?.to_str()?;
        Self::ALL.into_iter().find(|v| v.default_bin() == name)
    }

    /// Env var that carries a stored token into the CLI, if the vendor supports one.
    pub fn token_env(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("CLAUDE_CODE_OAUTH_TOKEN"),
            Self::Codex | Self::Antigravity | Self::Kimi | Self::Cursor => None,
        }
    }

    /// Inherited env vars that would pick the wrong account. Always removed before an account's
    /// own env is applied, so e.g. anchorleg started from inside a `claude-sm` session doesn't leak
    /// that session's `CLAUDE_CONFIG_DIR` into every launch.
    pub fn scrub_env(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &[
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CLAUDE_CONFIG_DIR",
            ],
            Self::Codex => &["CODEX_HOME", "OPENAI_API_KEY"],
            // Its account is `HOME`, which is never scrubbed: unset means the person's own.
            Self::Antigravity | Self::Kimi | Self::Cursor => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenFrom {
    Keychain,
}

pub const DEFAULT_PRIORITY: u32 = 100;

fn default_priority() -> u32 {
    DEFAULT_PRIORITY
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_true(b: &bool) -> bool {
    *b
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub name: String,
    pub vendor: Vendor,
    /// Lower is used first.
    #[serde(default = "default_priority")]
    pub priority: u32,
    /// `false` keeps the account configured but never picks it.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// CLI binary; defaults to the vendor's (`claude`, `codex`, …). `~` is expanded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<String>,
    /// Env vars set for this account, e.g. `CLAUDE_CONFIG_DIR`. `~` is expanded.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Args placed before anchorleg's own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_from: Option<TokenFrom>,
    /// Run this command through `$SHELL -ic` instead of `bin`. Slower, and shell startup output
    /// can corrupt the event stream; prefer `bin` + `env`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    // Phase 4 policy fields, parsed now so configs written today keep working.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve_7d: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
}

impl Account {
    pub fn new(name: impl Into<String>, vendor: Vendor) -> Self {
        Self {
            name: name.into(),
            vendor,
            priority: DEFAULT_PRIORITY,
            enabled: true,
            bin: None,
            env: BTreeMap::new(),
            args: Vec::new(),
            token_from: None,
            shell: None,
            reserve_7d: None,
            roles: Vec::new(),
            pinned: false,
        }
    }

    fn validate(&self) -> Result<()> {
        let invalid = |problem: &str| {
            Err(RegistryError::Invalid {
                account: self.name.clone(),
                problem: problem.to_owned(),
            })
        };
        let name_ok = !self.name.is_empty()
            && self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !name_ok {
            return invalid("name must be non-empty and use only letters, digits, `-` and `_`");
        }
        if self.shell.is_some() && (self.bin.is_some() || !self.args.is_empty()) {
            return invalid("`shell` can't be combined with `bin` or `args`");
        }
        if self.token_from.is_some() && self.vendor.token_env().is_none() {
            return invalid("this vendor has no token env var; use `env` or `bin` instead");
        }
        if let Some(r) = self.reserve_7d
            && !(0.0..=1.0).contains(&r)
        {
            return invalid("reserve_7d must be between 0 and 1");
        }
        Ok(())
    }

    /// Where this Claude account keeps its sessions, when anchorleg can tell: the account's
    /// `CLAUDE_CONFIG_DIR`, else `~/.claude`. `None` for `shell` accounts and other vendors.
    pub fn claude_config_dir(&self, home: &Path) -> Option<PathBuf> {
        if self.vendor != Vendor::Claude || self.shell.is_some() {
            return None;
        }
        Some(match self.env.get("CLAUDE_CONFIG_DIR") {
            Some(dir) => PathBuf::from(expand_tilde(dir, home)),
            None => home.join(".claude"),
        })
    }

    /// Where this Codex account keeps its login and sessions: the account's `CODEX_HOME`, else
    /// `~/.codex`. `None` for `shell` accounts and other vendors.
    pub fn codex_home(&self, home: &Path) -> Option<PathBuf> {
        if self.vendor != Vendor::Codex || self.shell.is_some() {
            return None;
        }
        Some(match self.env.get("CODEX_HOME") {
            Some(dir) => PathBuf::from(expand_tilde(dir, home)),
            None => home.join(".codex"),
        })
    }

    /// The `HOME` this Antigravity account runs with (agy keeps its login under it): the
    /// account's `HOME`, else the person's. `None` for `shell` accounts and other vendors.
    pub fn agy_home(&self, home: &Path) -> Option<PathBuf> {
        if self.vendor != Vendor::Antigravity || self.shell.is_some() {
            return None;
        }
        Some(match self.env.get("HOME") {
            Some(dir) => PathBuf::from(expand_tilde(dir, home)),
            None => home.to_owned(),
        })
    }

    /// Resolve how to start this account's CLI. Fetches the token when `token_from` is set.
    pub fn launch(&self, secrets: &dyn SecretStore, home: &Path) -> Result<Launch> {
        let mut env_remove: Vec<String> = self
            .vendor
            .scrub_env()
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let env: Vec<(String, String)> = self
            .env
            .iter()
            .map(|(k, v)| (k.clone(), expand_tilde(v, home)))
            .collect();
        env_remove.retain(|k| !self.env.contains_key(k));

        let secret_env = match (self.token_from, self.vendor.token_env()) {
            (Some(TokenFrom::Keychain), Some(var)) => {
                let token = secrets
                    .get(&self.name)?
                    .ok_or_else(|| RegistryError::MissingToken(self.name.clone()))?;
                env_remove.retain(|k| k != var);
                Some((var.to_owned(), token))
            }
            _ => None,
        };

        let (program, args) = match &self.shell {
            Some(cmd) => {
                let sh = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
                // `$0` is "anchorleg"; anchorleg's own args follow and land in "$@".
                let script = format!("{cmd} \"$@\"");
                (sh, vec!["-ic".to_owned(), script, "anchorleg".to_owned()])
            }
            None => {
                let bin = match &self.bin {
                    Some(bin) if bin != self.vendor.default_bin() => expand_tilde(bin, home),
                    _ => default_program(self.vendor),
                };
                (bin, self.args.clone())
            }
        };

        Ok(Launch {
            program,
            args,
            env_remove,
            env,
            secret_env,
        })
    }
}

/// The vendor's binary from `PATH`; for Codex, the copy inside the ChatGPT app when `codex`
/// isn't on `PATH` (the desktop app doesn't install one there).
fn default_program(vendor: Vendor) -> String {
    let name = vendor.default_bin();
    if vendor == Vendor::Codex
        && !on_path(name)
        && Path::new(crate::adapters::codex::BUNDLED_BIN).is_file()
    {
        return crate::adapters::codex::BUNDLED_BIN.to_owned();
    }
    name.to_owned()
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

/// A resolved way to start a CLI. Anchorleg appends its own args after `args`.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    /// Removed from the inherited environment before `env` is applied.
    pub env_remove: Vec<String>,
    pub env: Vec<(String, String)>,
    pub secret_env: Option<(String, Secret)>,
}

impl Launch {
    pub fn command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.program);
        cmd.args(&self.args);
        for key in &self.env_remove {
            cmd.env_remove(key);
        }
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
        if let Some((key, secret)) = &self.secret_env {
            cmd.env(key, secret.expose());
        }
        cmd
    }

    /// A shell-style line for display. Secrets are shown as `***`.
    pub fn display(&self) -> String {
        let mut words: Vec<String> = self.env_remove.iter().map(|k| format!("-u {k}")).collect();
        if !words.is_empty() {
            words.insert(0, "env".to_owned());
        }
        words.extend(self.env.iter().map(|(k, v)| format!("{k}={}", quote(v))));
        if let Some((key, _)) = &self.secret_env {
            words.push(format!("{key}=***"));
        }
        words.push(quote(&self.program));
        words.extend(self.args.iter().map(|a| quote(a)));
        words.join(" ")
    }
}

fn quote(word: &str) -> String {
    shlex::try_quote(word).map_or_else(|_| word.to_owned(), |q| q.into_owned())
}

/// `~` or `~/x` → under `home`; anything else unchanged.
pub fn expand_tilde(s: &str, home: &Path) -> String {
    match s.strip_prefix('~') {
        Some("") => home.display().to_string(),
        Some(rest) if rest.starts_with('/') => format!("{}{rest}", home.display()),
        _ => s.to_owned(),
    }
}

/// A token. `Debug` never prints it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

pub trait SecretStore {
    fn get(&self, account: &str) -> Result<Option<Secret>>;
    fn set(&self, account: &str, secret: &Secret) -> Result<()>;
    fn delete(&self, account: &str) -> Result<()>;
}

/// macOS Keychain, one generic password per account under service `anchorleg`.
pub struct Keychain;

const KEYCHAIN_SERVICE: &str = "anchorleg";

impl SecretStore for Keychain {
    fn get(&self, account: &str) -> Result<Option<Secret>> {
        match keyring::Entry::new(KEYCHAIN_SERVICE, account)?.get_password() {
            Ok(p) => Ok(Some(Secret(p))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set(&self, account: &str, secret: &Secret) -> Result<()> {
        keyring::Entry::new(KEYCHAIN_SERVICE, account)?.set_password(secret.expose())?;
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<()> {
        match keyring::Entry::new(KEYCHAIN_SERVICE, account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// In-memory secrets, for tests.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, Secret>>);

impl SecretStore for MemorySecrets {
    fn get(&self, account: &str) -> Result<Option<Secret>> {
        Ok(self.0.lock().unwrap().get(account).cloned())
    }

    fn set(&self, account: &str, secret: &Secret) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(account.to_owned(), secret.clone());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<()> {
        self.0.lock().unwrap().remove(account);
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, rename = "account")]
    pub accounts: Vec<Account>,
    /// Settings for every account of one CLI (`[vendor.claude]`), applied only when that CLI
    /// is launched.
    #[serde(default, rename = "vendor", skip_serializing_if = "BTreeMap::is_empty")]
    pub vendors: BTreeMap<Vendor, VendorSettings>,
}

/// Flags anchorleg sets itself on every launch; a `[vendor.*]` setting can't take them over.
const ANCHORLEG_OWNED: [&str; 9] = [
    "-p",
    "--print",
    "--output-format",
    "--input-format",
    "--resume",
    "--plugin-dir",
    "--model",
    "--effort",
    "--conversation",
];

impl VendorSettings {
    /// Shell-style words, as typed by a person: `--permission-mode acceptEdits`.
    pub fn parse_args(line: &str) -> std::result::Result<Vec<String>, String> {
        let words = shlex::split(line).ok_or("unbalanced quotes")?;
        if let Some(w) = words.iter().find(|w| {
            ANCHORLEG_OWNED
                .iter()
                .any(|o| *w == o || w.starts_with(&format!("{o}=")))
        }) {
            return Err(match w.split('=').next() {
                Some("--model" | "--effort") => {
                    format!("`{w}` has its own setting: /model or /effort")
                }
                _ => format!("`{w}` is set by anchorleg itself"),
            });
        }
        Ok(words)
    }

    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.effort.is_none() && self.args.is_empty()
    }

    /// "opus · high · --permission-mode acceptEdits", or "" when nothing is set.
    pub fn summary(&self) -> String {
        let args = self.args_line();
        [
            self.model.as_deref(),
            self.effort.as_deref(),
            Some(args.as_str()),
        ]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
    }

    /// The args as one shell-style line.
    pub fn args_line(&self) -> String {
        shlex::try_join(self.args.iter().map(String::as_str)).unwrap_or_default()
    }
}

/// `[vendor.<name>]`: what anchorleg adds to every launch of that CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VendorSettings {
    /// The model every account of this CLI uses, so a task keeps the same strength when it
    /// moves between accounts. `anchorleg run --model` overrides it for one run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// How hard the model thinks (Claude: low, medium, high, xhigh, max).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Placed before the account's own `args`, so an account can override them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

impl Config {
    /// `$ANCHORLEG_CONFIG`, or `~/.config/anchorleg/config.toml`.
    pub fn default_path() -> Result<PathBuf> {
        if let Some(p) = std::env::var_os("ANCHORLEG_CONFIG") {
            return Ok(PathBuf::from(p));
        }
        let config = dirs::home_dir()
            .ok_or(RegistryError::NoHome)?
            .join(".config");
        crate::adopt_legacy_dir(&config.join("relay"), &config.join("anchorleg"), &[]);
        Ok(config.join("anchorleg/config.toml"))
    }

    /// Load and validate. A missing file is an empty config.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(RegistryError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let config: Self = toml::from_str(&text).map_err(|e| RegistryError::Parse {
            path: path.to_owned(),
            source: Box::new(e),
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Validate, then write atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let text = toml::to_string_pretty(self)?;
        let write_err = |source| RegistryError::Write {
            path: path.to_owned(),
            source,
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(write_err)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(write_err)?;
        std::fs::rename(&tmp, path).map_err(write_err)
    }

    pub fn validate(&self) -> Result<()> {
        let mut seen = HashSet::new();
        for a in &self.accounts {
            a.validate()?;
            if !seen.insert(a.name.as_str()) {
                return Err(RegistryError::Invalid {
                    account: a.name.clone(),
                    problem: "duplicate account name".to_owned(),
                });
            }
        }
        Ok(())
    }

    /// How to start `account`: its own launch plus its CLI's `[vendor.<name>]` args.
    pub fn launch(
        &self,
        account: &Account,
        secrets: &dyn SecretStore,
        home: &Path,
    ) -> Result<Launch> {
        let mut launch = account.launch(secrets, home)?;
        if let Some(extra) = self.vendors.get(&account.vendor).map(|v| &v.args) {
            if account.shell.is_some() {
                // `$SHELL -ic "<cmd> \"$@\"" anchorleg`: these land in "$@".
                launch.args.extend(extra.iter().cloned());
            } else {
                launch.args.splice(0..0, extra.iter().cloned());
            }
        }
        Ok(launch)
    }

    pub fn get(&self, name: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.name == name)
    }

    /// Accounts in the order they should be tried: priority, then name.
    pub fn by_priority(&self) -> Vec<&Account> {
        let mut v: Vec<&Account> = self.accounts.iter().collect();
        v.sort_by(|a, b| (a.priority, &a.name).cmp(&(b.priority, &b.name)));
        v
    }
}

/// A command line split into leading `VAR=value` assignments, the binary, and its args.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCommand {
    pub env: BTreeMap<String, String>,
    pub bin: String,
    pub args: Vec<String>,
}

/// Parse `CLAUDE_CONFIG_DIR=~/.claude-sm /path/to/claude --model x` into its parts.
pub fn parse_command(cmd: &str) -> Result<ParsedCommand> {
    let bad = || RegistryError::BadCommand(cmd.to_owned());
    let words = shlex::split(cmd).ok_or_else(bad)?;
    let mut env = BTreeMap::new();
    let mut rest = words.into_iter().peekable();
    while let Some((k, v)) = rest.peek().and_then(|w| env_assignment(w)) {
        env.insert(k, v);
        rest.next();
    }
    let bin = rest.next().ok_or_else(bad)?;
    Ok(ParsedCommand {
        env,
        bin,
        args: rest.collect(),
    })
}

fn env_assignment(word: &str) -> Option<(String, String)> {
    let (k, v) = word.split_once('=')?;
    let valid = k.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then(|| (k.to_owned(), v.to_owned()))
}

/// Accounts found in `alias` output (zsh `name='value'` or bash `alias name='value'`).
///
/// Keeps aliases whose command runs a known CLI, following aliases that call other aliases.
/// Lines that don't parse (shell startup noise) are skipped.
pub fn accounts_from_aliases(alias_output: &str) -> Vec<Account> {
    let aliases: Vec<(String, String)> = alias_output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix("alias ").unwrap_or(line);
            let (name, value) = line.split_once('=')?;
            if name.is_empty() || name.contains(char::is_whitespace) {
                return None;
            }
            let value = shlex::split(value)?.join(" ");
            Some((name.to_owned(), value))
        })
        .collect();
    let lookup: HashMap<&str, &str> = aliases
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();

    aliases
        .iter()
        .filter_map(|(name, value)| {
            let parsed = resolve_alias(value, &lookup)?;
            let vendor = Vendor::from_bin(&parsed.bin)?;
            let mut account = Account::new(name.clone(), vendor);
            account.bin = Some(parsed.bin);
            account.env = parsed.env;
            account.args = parsed.args;
            account.validate().ok()?;
            Some(account)
        })
        .collect()
}

fn resolve_alias(value: &str, lookup: &HashMap<&str, &str>) -> Option<ParsedCommand> {
    let mut parsed = parse_command(value).ok()?;
    for _ in 0..5 {
        let Some(inner) = lookup.get(parsed.bin.as_str()) else {
            return Some(parsed);
        };
        let mut next = parse_command(inner).ok()?;
        next.env.extend(parsed.env);
        next.args.extend(parsed.args);
        parsed = next;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/me";

    // `zsh -ic alias` on the owner's laptop, plus noise.
    const ZSH_ALIASES: &str = "\
Welcome back!
claude='echo use specific commands'
claude-three='CLAUDE_CONFIG_DIR=~/.claude-blilbi /Users/me/.local/bin/claude'
claude-sm='CLAUDE_CONFIG_DIR=~/.claude-sm /Users/me/.local/bin/claude'
cs='claude-sm --model haiku'
ll='ls -la'
";

    #[test]
    fn imports_owner_style_aliases() {
        let accounts = accounts_from_aliases(ZSH_ALIASES);
        let names: Vec<_> = accounts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["claude-three", "claude-sm", "cs"]);

        let sm = &accounts[1];
        assert_eq!(sm.vendor, Vendor::Claude);
        assert_eq!(sm.bin.as_deref(), Some("/Users/me/.local/bin/claude"));
        assert_eq!(sm.env["CLAUDE_CONFIG_DIR"], "~/.claude-sm");

        // `cs` calls the `claude-sm` alias with extra args.
        let cs = &accounts[2];
        assert_eq!(cs.env["CLAUDE_CONFIG_DIR"], "~/.claude-sm");
        assert_eq!(cs.args, ["--model", "haiku"]);
    }

    #[test]
    fn imports_bash_aliases() {
        let out = "alias work='CODEX_HOME=/x/codex-work codex'\n";
        let a = &accounts_from_aliases(out)[0];
        assert_eq!(a.vendor, Vendor::Codex);
        assert_eq!(a.env["CODEX_HOME"], "/x/codex-work");
    }

    #[test]
    fn alias_loops_are_dropped() {
        assert!(accounts_from_aliases("a='b'\nb='a'\n").is_empty());
    }

    #[test]
    fn parse_command_with_quotes() {
        let p = parse_command("A=1 B='two words' claude -p 'hello there'").unwrap();
        assert_eq!(p.env["B"], "two words");
        assert_eq!(p.bin, "claude");
        assert_eq!(p.args, ["-p", "hello there"]);
        assert!(parse_command("A=1").is_err());
    }

    #[test]
    fn config_dir_account_launch() {
        let mut a = Account::new("sm", Vendor::Claude);
        a.bin = Some("~/.local/bin/claude".into());
        a.env
            .insert("CLAUDE_CONFIG_DIR".into(), "~/.claude-sm".into());
        let l = a
            .launch(&MemorySecrets::default(), Path::new(HOME))
            .unwrap();
        assert_eq!(l.program, "/Users/me/.local/bin/claude");
        assert_eq!(
            l.env,
            [(
                "CLAUDE_CONFIG_DIR".to_owned(),
                "/Users/me/.claude-sm".to_owned()
            )]
        );
        assert!(l.env_remove.contains(&"ANTHROPIC_API_KEY".to_owned()));
        assert!(l.env_remove.contains(&"CLAUDE_CODE_OAUTH_TOKEN".to_owned()));
        assert!(!l.env_remove.contains(&"CLAUDE_CONFIG_DIR".to_owned()));
        assert!(l.secret_env.is_none());
    }

    #[test]
    fn token_account_launch_hides_token() {
        let secrets = MemorySecrets::default();
        let mut a = Account::new("two", Vendor::Claude);
        a.token_from = Some(TokenFrom::Keychain);
        assert!(matches!(
            a.launch(&secrets, Path::new(HOME)),
            Err(RegistryError::MissingToken(_))
        ));

        secrets
            .set("two", &Secret::new("sk-ant-oat01-SECRET"))
            .unwrap();
        let l = a.launch(&secrets, Path::new(HOME)).unwrap();
        assert_eq!(l.program, "claude");
        assert!(l.env_remove.contains(&"CLAUDE_CONFIG_DIR".to_owned()));
        assert!(!l.env_remove.contains(&"CLAUDE_CODE_OAUTH_TOKEN".to_owned()));
        for shown in [format!("{l:?}"), l.display()] {
            assert!(!shown.contains("SECRET"), "token leaked: {shown}");
        }
    }

    #[test]
    fn vendor_args_apply_to_that_cli_only() {
        let config: Config = toml::from_str(
            r#"
            [[account]]
            name = "a"
            vendor = "claude"
            args = ["--model", "opus"]
            [[account]]
            name = "s"
            vendor = "claude"
            shell = "claude-s"
            [[account]]
            name = "x"
            vendor = "codex"
            [vendor.claude]
            args = ["--permission-mode", "acceptEdits"]
            "#,
        )
        .unwrap();
        let launch = |n: &str| {
            config
                .launch(
                    config.get(n).unwrap(),
                    &MemorySecrets::default(),
                    Path::new(HOME),
                )
                .unwrap()
                .args
        };
        assert_eq!(
            launch("a"),
            ["--permission-mode", "acceptEdits", "--model", "opus"]
        );
        assert_eq!(
            launch("s")[2..],
            ["anchorleg", "--permission-mode", "acceptEdits"]
        );
        assert!(launch("x").is_empty());
        let saved = toml::to_string_pretty(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&saved).unwrap(), config);
    }

    #[test]
    fn codex_model_flags_and_home() {
        assert_eq!(
            Vendor::Codex.model_args(Some("gpt-5.6-luna"), Some("low")),
            ["-m", "gpt-5.6-luna", "-c", "model_reasoning_effort=low"]
        );
        let home = Path::new("/h");
        let mut a = Account::new("c", Vendor::Codex);
        assert_eq!(a.codex_home(home), Some(PathBuf::from("/h/.codex")));
        a.env.insert("CODEX_HOME".into(), "~/.codex-2".into());
        assert_eq!(a.codex_home(home), Some(PathBuf::from("/h/.codex-2")));
        assert_eq!(Account::new("x", Vendor::Claude).codex_home(home), None);
    }

    #[test]
    fn shell_account_launch() {
        let mut a = Account::new("odd", Vendor::Claude);
        a.shell = Some("claude-sm".into());
        let l = a
            .launch(&MemorySecrets::default(), Path::new(HOME))
            .unwrap();
        assert_eq!(l.args, ["-ic", "claude-sm \"$@\"", "anchorleg"]);
    }

    #[test]
    fn config_round_trip_and_order() {
        let text = r#"
[[account]]
name = "two"
vendor = "claude"
priority = 2
token_from = "keychain"

[[account]]
name = "sm"
vendor = "claude"
priority = 1
bin = "~/.local/bin/claude"
env = { CLAUDE_CONFIG_DIR = "~/.claude-sm" }
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("anchorleg/config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();

        let config = Config::load(&path).unwrap();
        let order: Vec<_> = config.by_priority().iter().map(|a| &a.name).collect();
        assert_eq!(order, ["sm", "two"]);

        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
    }

    #[test]
    fn missing_config_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(&dir.path().join("nope.toml")).unwrap();
        assert!(c.accounts.is_empty());
    }

    #[test]
    fn invalid_configs_are_rejected() {
        let cases = [
            "[[account]]\nname = \"a\"\nvendor = \"claude\"\nbni = \"typo\"\n",
            "[[account]]\nname = \"a b\"\nvendor = \"claude\"\n",
            "[[account]]\nname = \"a\"\nvendor = \"claude\"\n[[account]]\nname = \"a\"\nvendor = \"codex\"\n",
            "[[account]]\nname = \"a\"\nvendor = \"claude\"\nshell = \"x\"\nbin = \"y\"\n",
            "[[account]]\nname = \"a\"\nvendor = \"codex\"\ntoken_from = \"keychain\"\n",
        ];
        let dir = tempfile::tempdir().unwrap();
        for (i, text) in cases.iter().enumerate() {
            let path = dir.path().join(format!("{i}.toml"));
            std::fs::write(&path, text).unwrap();
            assert!(Config::load(&path).is_err(), "case {i} should fail");
        }
    }
}
