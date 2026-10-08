//! `anchorleg ui`: the session manager (D13). Keyboard first; the mouse works too.
//!
//! ```text
//! ┌ Sessions ─────────┐┌ Conversation ────────────────────┐
//! │ + New session     ││ you: …                            │
//! │ ● #12 claude-sm … ││ agent: …                          │
//! ├ Agents ───────────┤├ Message ─────────────────────────┤
//! │ claude-sm  ██ 29% ││ > …                               │
//! └───────────────────┘└───────────────────────────────────┘
//! ```
//!
//! Each session is a `anchorleg run` process started in its own process group, so it keeps
//! running when the UI closes. The UI reads everything else from the store and run logs.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anchorleg_core::conversation::{self, Message, Role};
use anchorleg_core::policy::{self, PickOptions};
use anchorleg_core::registry::{
    Account, Config, Keychain, SecretStore, TokenFrom, Vendor, VendorSettings,
    accounts_from_aliases,
};
use anchorleg_core::store::{Permission, PermissionState, QuotaRow, Run, RunState, Store};
use anyhow::Context;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::now;

const TICK: Duration = Duration::from_millis(700);

/// Agent CLIs anchorleg knows about, shown in the Agents pane when not configured as accounts.
const KNOWN_CLIS: &[(&str, &str)] = &[
    ("codex", "Codex"),
    ("agy", "Antigravity"),
    ("cursor-agent", "Cursor"),
    ("kimi", "Kimi"),
];

/// Starts and stops `anchorleg run` processes. A trait so tests can see what would run.
pub trait Launcher {
    /// Start `anchorleg <args>` in the background; returns its pid.
    fn start(&self, args: &[String]) -> anyhow::Result<u32>;
    fn stop(&self, run: i64) -> anyhow::Result<()>;
    fn is_alive(&self, pid: i64) -> bool;
    /// Where an agent CLI is installed, if it is.
    fn find_cli(&self, name: &str) -> Option<PathBuf>;
}

/// The real thing: spawns this same `anchorleg` binary.
pub struct Processes {
    exe: PathBuf,
}

impl Launcher for Processes {
    fn start(&self, args: &[String]) -> anyhow::Result<u32> {
        use std::os::unix::process::CommandExt as _;
        let child = std::process::Command::new(&self.exe)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // Own process group: survives the UI closing, and `anchorleg stop` can end it whole.
            .process_group(0)
            .spawn()
            .with_context(|| format!("starting {}", self.exe.display()))?;
        Ok(child.id())
    }

    fn stop(&self, run: i64) -> anyhow::Result<()> {
        crate::run::stop(run)
    }

    fn is_alive(&self, pid: i64) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    fn find_cli(&self, name: &str) -> Option<PathBuf> {
        let on_path = std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .find(|p| p.is_file())
        });
        // The ChatGPT desktop app ships `codex` without putting it on PATH.
        let bundled = PathBuf::from(anchorleg_core::adapters::codex::BUNDLED_BIN);
        on_path.or_else(|| (name == "codex" && bundled.is_file()).then_some(bundled))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sessions,
    Conversation,
    Composer,
    Agents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Folder,
    Message,
}

/// A one-line text box with a cursor.
#[derive(Debug, Default, Clone)]
struct TextBox {
    text: String,
    /// In chars.
    cursor: usize,
}

impl TextBox {
    fn set(&mut self, text: &str) {
        self.text = text.to_owned();
        self.cursor = text.chars().count();
    }

    fn byte(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map_or(self.text.len(), |(i, _)| i)
    }

    /// Apply an editing key.
    fn edit(&mut self, key: &KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = self.text.chars().count();
        match key.code {
            KeyCode::Char('u') if ctrl => {
                let at = self.byte(self.cursor);
                self.text.replace_range(..at, "");
                self.cursor = 0;
            }
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = len,
            KeyCode::Char(c) if !ctrl => {
                let at = self.byte(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let (a, b) = (self.byte(self.cursor - 1), self.byte(self.cursor));
                self.text.replace_range(a..b, "");
                self.cursor -= 1;
            }
            KeyCode::Delete if self.cursor < len => {
                let (a, b) = (self.byte(self.cursor), self.byte(self.cursor + 1));
                self.text.replace_range(a..b, "");
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            _ => {}
        }
    }
}

/// Everything a click or a key can do outside text editing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Focus(Focus),
    SelectSession(usize),
    SelectAgent(usize),
    NewSession,
    Send,
    Stop,
    MoveUp,
    MoveDown,
    Toggle,
    Import,
    Remove,
    /// Answer the selected session's waiting tool call.
    Answer(PermissionState),
    /// Edit the selected account's CLI settings (`[vendor.<name>]`).
    Settings,
    Help,
    Quit,
}

pub struct App {
    pub focus: Focus,
    pub help: bool,
    pub config: Config,
    config_path: PathBuf,
    store: Store,
    log_dir: PathBuf,
    launcher: Box<dyn Launcher>,
    /// Where `Import` reads `alias` output from; `None` runs `$SHELL -ic alias`.
    alias_source: Option<PathBuf>,
    quota: Vec<QuotaRow>,
    runs: Vec<Run>,
    /// Tool calls waiting for the person, oldest first.
    pending: Vec<Permission>,
    /// Pids of running sessions that are still alive.
    alive: Vec<i64>,
    /// 0 is "+ New session"; `i` is `runs[i - 1]`.
    pub selected: usize,
    pub agent: usize,
    conversation: Vec<Message>,
    /// Lines scrolled up from the bottom of the conversation.
    scroll_up: usize,
    folder: TextBox,
    message_box: TextBox,
    /// Slash commands the selected session's CLI offers (from its last `init`).
    slash: Vec<String>,
    /// Highlighted completion while typing a slash command.
    suggestion: usize,
    /// While set, the message box edits this CLI's settings instead.
    settings_for: Option<Vendor>,
    settings_box: TextBox,
    field: Field,
    /// Status line text.
    pub message: String,
    pub confirm_remove: Option<String>,
    /// After starting a session, select it once it shows up.
    pending_pid: Option<i64>,
    hits: Vec<(Rect, Action)>,
    session_rows: Option<Rect>,
    agent_rows: Option<Rect>,
    conversation_area: Option<Rect>,
    composer_area: Option<Rect>,
    pub quit: bool,
}

impl App {
    pub fn new(
        config_path: PathBuf,
        store: Store,
        log_dir: PathBuf,
        launcher: Box<dyn Launcher>,
        cwd: &Path,
    ) -> anyhow::Result<Self> {
        let mut folder = TextBox::default();
        folder.set(&cwd.display().to_string());
        let mut app = Self {
            focus: Focus::Sessions,
            help: false,
            config: Config::default(),
            config_path,
            store,
            log_dir,
            launcher,
            alias_source: None,
            quota: Vec::new(),
            runs: Vec::new(),
            pending: Vec::new(),
            alive: Vec::new(),
            selected: 0,
            agent: 0,
            conversation: Vec::new(),
            scroll_up: 0,
            folder,
            message_box: TextBox::default(),
            slash: Vec::new(),
            suggestion: 0,
            settings_for: None,
            settings_box: TextBox::default(),
            field: Field::Message,
            message: String::new(),
            confirm_remove: None,
            pending_pid: None,
            hits: Vec::new(),
            session_rows: None,
            agent_rows: None,
            conversation_area: None,
            composer_area: None,
            quit: false,
        };
        app.refresh()?;
        if !app.runs.is_empty() {
            app.select_session(1);
        }
        Ok(app)
    }

    #[cfg(test)]
    fn with_alias_source(mut self, path: PathBuf) -> Self {
        self.alias_source = Some(path);
        self
    }

    fn selected_run(&self) -> Option<&Run> {
        self.selected.checked_sub(1).and_then(|i| self.runs.get(i))
    }

    fn is_running(run: &Run) -> bool {
        matches!(run.state, RunState::Running | RunState::Waiting)
    }

    fn is_alive(&self, run: &Run) -> bool {
        run.pid.is_some_and(|p| self.alive.contains(&p))
    }

    /// Reload config, quota, sessions and the selected conversation.
    pub fn refresh(&mut self) -> anyhow::Result<()> {
        self.config = Config::load(&self.config_path)?;
        self.quota = self.store.quota(None)?;
        let selected_id = self.selected_run().map(|r| r.id);
        self.runs = self.store.recent_runs(200)?;
        self.pending = self.store.pending_permissions()?;
        self.alive = self
            .runs
            .iter()
            .filter(|r| Self::is_running(r))
            .filter_map(|r| r.pid)
            .filter(|&p| self.launcher.is_alive(p))
            .collect();
        // Keep the same session selected as new ones appear above it.
        if let Some(pid) = self.pending_pid
            && let Some(i) = self.runs.iter().position(|r| r.pid == Some(pid))
        {
            self.pending_pid = None;
            self.selected = i + 1;
            self.scroll_up = 0;
        } else if let Some(id) = selected_id {
            self.selected = self
                .runs
                .iter()
                .position(|r| r.id == id)
                .map_or(0, |i| i + 1);
        }
        self.selected = self.selected.min(self.runs.len());
        self.agent = self.agent.min(self.agent_count().saturating_sub(1));
        self.load_conversation();
        Ok(())
    }

    fn load_conversation(&mut self) {
        // Completions come from the selected session, else the newest one with a log.
        let source = self.selected_run().or(self.runs.first()).map(|r| r.id);
        if let Some(id) = source
            && let Ok(log) = std::fs::read_to_string(self.log_dir.join(format!("run-{id}.jsonl")))
            && let Some(cmds) = conversation::slash_commands(&log)
        {
            self.slash = cmds;
        }
        let Some(run) = self.selected_run() else {
            self.conversation.clear();
            return;
        };
        // A follow-up shows the whole session: its parents' messages first.
        let mut chain = vec![run.id];
        let mut parent = run.parent_id;
        while let Some(p) = parent {
            chain.push(p);
            parent = self
                .runs
                .iter()
                .find(|r| r.id == p)
                .and_then(|r| r.parent_id);
            if chain.len() > 50 {
                break;
            }
        }
        let mut messages = Vec::new();
        for id in chain.iter().rev() {
            let log = std::fs::read_to_string(self.log_dir.join(format!("run-{id}.jsonl")))
                .unwrap_or_default();
            let mut part = conversation::from_log(&log);
            // Paths inside the session's folder read better relative to it.
            if let Some(r) = self.runs.iter().find(|r| r.id == *id) {
                let prefix = format!("{}/", r.cwd.trim_end_matches('/'));
                for m in part.iter_mut().filter(|m| m.role == Role::Tool) {
                    m.text = m.text.replace(&prefix, "");
                }
            }
            // Logs from before anchorleg recorded the prompt: show the task as the user's message.
            if !part.iter().any(|m| m.role == Role::User)
                && let Some(r) = self.runs.iter().find(|r| r.id == *id)
            {
                part.insert(
                    0,
                    Message {
                        role: Role::User,
                        text: r.task.clone(),
                    },
                );
            }
            messages.extend(part);
        }
        self.conversation = messages;
    }

    fn agent_count(&self) -> usize {
        self.config.accounts.len()
    }

    fn ordered_names(&self) -> Vec<String> {
        self.config
            .by_priority()
            .iter()
            .map(|a| a.name.clone())
            .collect()
    }

    fn select_session(&mut self, i: usize) {
        self.selected = i.min(self.runs.len());
        self.scroll_up = 0;
        self.load_conversation();
    }

    pub fn apply(&mut self, action: Action) -> anyhow::Result<()> {
        if action != Action::Remove {
            self.confirm_remove = None;
        }
        if matches!(action, Action::SelectSession(_) | Action::NewSession) {
            self.settings_for = None;
        }
        match action {
            Action::Focus(f) => self.focus = f,
            Action::SelectSession(i) => {
                self.focus = Focus::Sessions;
                self.select_session(i);
            }
            Action::SelectAgent(i) => {
                self.focus = Focus::Agents;
                self.agent = i.min(self.agent_count().saturating_sub(1));
            }
            Action::NewSession => {
                self.select_session(0);
                self.focus = Focus::Composer;
                self.field = Field::Message;
            }
            Action::Answer(state) => self.answer(state)?,
            Action::Send if self.settings_for.is_some() => self.save_settings()?,
            Action::Send => self.send()?,
            Action::Settings => self.edit_settings(),
            Action::Stop => self.stop()?,
            Action::MoveUp | Action::MoveDown => self.move_agent(action == Action::MoveUp)?,
            Action::Toggle => self.toggle_agent()?,
            Action::Import => self.import()?,
            Action::Remove => self.remove_agent()?,
            Action::Help => self.help = !self.help,
            Action::Quit => self.quit = true,
        }
        Ok(())
    }

    /// The provider of the selected session (Claude for a new one).
    fn provider(&self) -> Vendor {
        self.selected_run()
            .and_then(|r| r.account.as_deref())
            .and_then(|a| self.config.get(a))
            .map_or(Vendor::Claude, |a| a.vendor)
    }

    /// `/model x` and `/effort x`: anchorleg's own commands. They set the provider's model or
    /// effort for every one of its accounts (D20), from the next launch on.
    fn provider_setting(&mut self, cmd: &str, value: &str) -> anyhow::Result<()> {
        let v = self.provider();
        let current = self.config.vendors.get(&v).cloned().unwrap_or_default();
        if value.is_empty() {
            let now = if cmd == "model" {
                current.model
            } else {
                current.effort
            };
            let options = match (cmd, v.efforts()) {
                ("effort", Some(e)) => format!(" (one of {}, default)", e.join(", ")),
                _ => String::new(),
            };
            self.message = format!(
                "{} {cmd}: {}{options}",
                v.name(),
                now.as_deref().unwrap_or("default")
            );
            return Ok(());
        }
        if cmd == "model" {
            crate::settings::set_model(&mut self.config, v, value);
        } else if let Err(e) = crate::settings::set_effort(&mut self.config, v, value) {
            self.message = e;
            return Ok(());
        }
        self.config.save(&self.config_path)?;
        self.message_box = TextBox::default();
        self.message = format!(
            "{} {cmd} → {value} for every {} account, from the next message",
            v.name(),
            v.name()
        );
        self.refresh()
    }

    /// Completions for a slash command being typed: (name, what it does).
    fn suggestions(&self) -> Vec<(String, String)> {
        if self.focus != Focus::Composer || (self.selected == 0 && self.field == Field::Folder) {
            return Vec::new();
        }
        let Some(typed) = self.message_box.text.strip_prefix('/') else {
            return Vec::new();
        };
        if typed.contains(' ') {
            return Vec::new();
        }
        let v = self.provider().name();
        let own = [
            ("model", format!("anchorleg: model for every {v} account")),
            ("effort", format!("anchorleg: effort for every {v} account")),
        ];
        own.into_iter()
            .map(|(n, d)| (n.to_owned(), d))
            .chain(
                self.slash
                    .iter()
                    .filter(|c| *c != "model" && *c != "effort")
                    .map(|c| (c.clone(), String::new())),
            )
            .filter(|(n, _)| n.starts_with(typed))
            .take(8)
            .collect()
    }

    /// The selected session's oldest waiting tool call.
    fn waiting(&self) -> Option<&Permission> {
        let run = self.selected_run()?;
        self.pending.iter().find(|p| p.run_id == run.id)
    }

    /// The call in one line, paths relative to its session's folder.
    fn tool_label(&self, p: &Permission) -> String {
        let cwd = self
            .runs
            .iter()
            .find(|r| r.id == p.run_id)
            .map_or("", |r| r.cwd.as_str());
        tool_label(&p.tool, &conversation::relative_input(&p.input, cwd))
    }

    fn answer(&mut self, state: PermissionState) -> anyhow::Result<()> {
        let Some(p) = self.waiting() else {
            self.message = "nothing is waiting for an answer here".to_owned();
            return Ok(());
        };
        let (id, what) = (p.id, self.tool_label(p));
        self.store.answer_permission(id, state, now())?;
        self.message = match state {
            PermissionState::Allowed => format!("allowed {what}"),
            PermissionState::Always => format!("allowed {what} for this session"),
            _ => format!("refused {what}"),
        };
        self.refresh()
    }

    fn edit_settings(&mut self) {
        let vendor = self
            .config
            .by_priority()
            .get(self.agent)
            .map_or(Vendor::Claude, |a| a.vendor);
        let line = self
            .config
            .vendors
            .get(&vendor)
            .map(VendorSettings::args_line)
            .unwrap_or_default();
        self.settings_box.set(&line);
        self.settings_for = Some(vendor);
        self.focus = Focus::Composer;
        self.message = format!("arguments added to every {} launch", vendor.name());
    }

    fn save_settings(&mut self) -> anyhow::Result<()> {
        let Some(vendor) = self.settings_for else {
            return Ok(());
        };
        match VendorSettings::parse_args(&self.settings_box.text) {
            Ok(args) => {
                crate::settings::update(&mut self.config, vendor, |s| s.args = args);
                self.config.save(&self.config_path)?;
                self.settings_for = None;
                self.focus = Focus::Agents;
                self.message = format!("{} settings saved", vendor.name());
                self.refresh()
            }
            Err(e) => {
                self.message = format!("not saved: {e}");
                Ok(())
            }
        }
    }

    fn send(&mut self) -> anyhow::Result<()> {
        let text = self.message_box.text.trim().to_owned();
        if text.is_empty() {
            self.focus = Focus::Composer;
            self.field = Field::Message;
            self.message = "type a message first".to_owned();
            return Ok(());
        }
        if let Some(rest) = text.strip_prefix('/')
            && let Some((cmd, arg)) = Some(rest.split_once(' ').unwrap_or((rest, "")))
            && matches!(cmd, "model" | "effort")
        {
            return self.provider_setting(cmd, arg.trim());
        }
        let mut args = vec!["run".to_owned(), "--json".to_owned()];
        match self.selected_run() {
            None => {
                let folder = self.folder.text.trim();
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default();
                let expanded = anchorleg_core::registry::expand_tilde(folder, &home);
                if !Path::new(&expanded).is_dir() {
                    self.focus = Focus::Composer;
                    self.field = Field::Folder;
                    self.message = format!("no such folder: {folder}");
                    return Ok(());
                }
                args.extend(["--cwd".to_owned(), expanded]);
            }
            Some(run) if Self::is_running(run) && self.is_alive(run) => {
                self.message = format!(
                    "#{} is still working; wait for it to finish or stop it (s)",
                    run.id
                );
                return Ok(());
            }
            Some(run) if run.session_id.is_none() => {
                self.message = format!("#{} never started a session; start a new one (n)", run.id);
                return Ok(());
            }
            Some(run) => args.extend(["--follow-up".to_owned(), run.id.to_string()]),
        }
        args.push("--".to_owned());
        args.push(text);
        let pid = self.launcher.start(&args)?;
        self.pending_pid = Some(i64::from(pid));
        self.message_box = TextBox::default();
        self.message = "sent".to_owned();
        self.refresh()
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        match self.selected_run() {
            Some(run) if Self::is_running(run) => {
                let id = run.id;
                self.launcher.stop(id)?;
                self.message = format!("stopping #{id}");
            }
            Some(run) => self.message = format!("#{} isn't running", run.id),
            None => self.message = "select a running session to stop".to_owned(),
        }
        Ok(())
    }

    /// Swap the selected account with its neighbour and renumber priorities 10, 20, 30…
    fn move_agent(&mut self, up: bool) -> anyhow::Result<()> {
        let mut names = self.ordered_names();
        let i = self.agent;
        let j = if up {
            i.checked_sub(1)
        } else {
            Some(i + 1).filter(|&j| j < names.len())
        };
        let Some(j) = j else { return Ok(()) };
        names.swap(i, j);
        for (rank, name) in names.iter().enumerate() {
            if let Some(a) = self.config.accounts.iter_mut().find(|a| &a.name == name) {
                a.priority = u32::try_from(rank + 1).unwrap_or(u32::MAX) * 10;
            }
        }
        self.config.save(&self.config_path)?;
        self.agent = j;
        self.message = format!("{} moved {}", names[j], if up { "up" } else { "down" });
        Ok(())
    }

    fn toggle_agent(&mut self) -> anyhow::Result<()> {
        let Some(name) = self.ordered_names().get(self.agent).cloned() else {
            return Ok(());
        };
        let a = self
            .config
            .accounts
            .iter_mut()
            .find(|a| a.name == name)
            .context("account vanished")?;
        a.enabled = !a.enabled;
        self.message = format!("{name} {}", if a.enabled { "enabled" } else { "disabled" });
        self.config.save(&self.config_path)?;
        Ok(())
    }

    fn remove_agent(&mut self) -> anyhow::Result<()> {
        let Some(name) = self.ordered_names().get(self.agent).cloned() else {
            return Ok(());
        };
        if self.confirm_remove.as_deref() != Some(name.as_str()) {
            self.message = format!("Remove {name}? Press d (or click Remove) again to confirm.");
            self.confirm_remove = Some(name);
            return Ok(());
        }
        self.confirm_remove = None;
        let had_token = self
            .config
            .get(&name)
            .is_some_and(|a| a.token_from == Some(TokenFrom::Keychain));
        self.config.accounts.retain(|a| a.name != name);
        self.config.save(&self.config_path)?;
        if had_token {
            Keychain.delete(&name)?;
        }
        self.message = format!("removed {name}");
        self.refresh()
    }

    fn import(&mut self) -> anyhow::Result<()> {
        let output = match &self.alias_source {
            Some(f) => std::fs::read_to_string(f)?,
            None => crate::accounts::shell_aliases()?,
        };
        let new: Vec<_> = accounts_from_aliases(&output)
            .into_iter()
            .filter(|a| self.config.get(&a.name).is_none())
            .collect();
        if new.is_empty() {
            self.message = "no new aliases that run claude, codex, kimi or cursor-agent".to_owned();
            return Ok(());
        }
        let names: Vec<_> = new.iter().map(|a| a.name.clone()).collect();
        self.config.accounts.extend(new);
        self.config.save(&self.config_path)?;
        self.message = format!("imported {}", names.join(", "));
        Ok(())
    }

    fn next_focus(&self, back: bool) -> Focus {
        let order = [
            Focus::Sessions,
            Focus::Conversation,
            Focus::Composer,
            Focus::Agents,
        ];
        let i = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let n = order.len();
        order[if back { (i + n - 1) % n } else { (i + 1) % n }]
    }

    pub fn on_key(&mut self, key: KeyEvent) -> anyhow::Result<()> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        if ctrl && key.code == KeyCode::Char('c') {
            return self.apply(Action::Quit);
        }
        if self.help {
            self.help = false;
            return Ok(());
        }
        let suggestions = self.suggestions();
        if self.focus == Focus::Composer && self.settings_for.is_none() && !suggestions.is_empty() {
            let pick = self.suggestion.min(suggestions.len() - 1);
            match key.code {
                KeyCode::Tab => {
                    self.message_box.set(&format!("/{} ", suggestions[pick].0));
                    self.suggestion = 0;
                    return Ok(());
                }
                KeyCode::Up => {
                    self.suggestion = pick.saturating_sub(1);
                    return Ok(());
                }
                KeyCode::Down => {
                    self.suggestion = (pick + 1).min(suggestions.len() - 1);
                    return Ok(());
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Tab => {
                self.focus = self.next_focus(false);
                return Ok(());
            }
            KeyCode::BackTab => {
                self.focus = self.next_focus(true);
                return Ok(());
            }
            _ => {}
        }

        if self.focus == Focus::Composer && self.settings_for.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.settings_for = None;
                    self.focus = Focus::Agents;
                    self.message = "settings not changed".to_owned();
                }
                KeyCode::Enter => return self.save_settings(),
                _ => self.settings_box.edit(&key),
            }
            return Ok(());
        }
        if self.focus == Focus::Composer {
            match key.code {
                KeyCode::Esc => self.focus = Focus::Sessions,
                KeyCode::Enter => return self.apply(Action::Send),
                KeyCode::Up | KeyCode::Down if self.selected == 0 => {
                    self.field = match self.field {
                        Field::Folder => Field::Message,
                        Field::Message => Field::Folder,
                    };
                }
                _ => {
                    let target = match (self.selected, self.field) {
                        (0, Field::Folder) => &mut self.folder,
                        _ => &mut self.message_box,
                    };
                    target.edit(&key);
                    self.suggestion = 0;
                }
            }
            return Ok(());
        }

        // Keys that work in every other pane.
        let action = match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
            KeyCode::Char('?') => Some(Action::Help),
            KeyCode::Char('n') => Some(Action::NewSession),
            KeyCode::Char('/') | KeyCode::Char('m') => Some(Action::Focus(Focus::Composer)),
            KeyCode::Char('s') => Some(Action::Stop),
            KeyCode::Char('1') if self.waiting().is_some() => {
                Some(Action::Answer(PermissionState::Allowed))
            }
            KeyCode::Char('2') if self.waiting().is_some() => {
                Some(Action::Answer(PermissionState::Always))
            }
            KeyCode::Char('3') if self.waiting().is_some() => {
                Some(Action::Answer(PermissionState::Denied))
            }
            KeyCode::Char('r') => {
                self.refresh()?;
                self.message = "refreshed".to_owned();
                return Ok(());
            }
            _ => None,
        };
        if let Some(a) = action {
            return self.apply(a);
        }

        match self.focus {
            Focus::Sessions => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.select_session(self.selected.saturating_sub(1));
                }
                KeyCode::Down | KeyCode::Char('j') => self.select_session(self.selected + 1),
                KeyCode::Home => self.select_session(0),
                KeyCode::End => self.select_session(self.runs.len()),
                KeyCode::Enter | KeyCode::Right => self.focus = Focus::Composer,
                _ => {}
            },
            Focus::Conversation => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.scroll_up += 1,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.scroll_up = self.scroll_up.saturating_sub(1);
                }
                KeyCode::PageUp => self.scroll_up += 10,
                KeyCode::PageDown => self.scroll_up = self.scroll_up.saturating_sub(10),
                KeyCode::End | KeyCode::Char('G') => self.scroll_up = 0,
                _ => {}
            },
            Focus::Agents => {
                let action = match key.code {
                    KeyCode::Up if shift => Action::MoveUp,
                    KeyCode::Down if shift => Action::MoveDown,
                    KeyCode::Char('K') => Action::MoveUp,
                    KeyCode::Char('J') => Action::MoveDown,
                    KeyCode::Up | KeyCode::Char('k') => {
                        Action::SelectAgent(self.agent.saturating_sub(1))
                    }
                    KeyCode::Down | KeyCode::Char('j') => Action::SelectAgent(self.agent + 1),
                    KeyCode::Char(' ') | KeyCode::Char('e') => Action::Toggle,
                    KeyCode::Char('i') => Action::Import,
                    KeyCode::Char('o') => Action::Settings,
                    KeyCode::Char('d') | KeyCode::Delete => Action::Remove,
                    _ => return Ok(()),
                };
                self.apply(action)?;
            }
            Focus::Composer => {}
        }
        Ok(())
    }

    pub fn on_mouse(&mut self, m: MouseEvent) -> anyhow::Result<()> {
        let pos = Position::new(m.column, m.row);
        let over = |r: Option<Rect>| r.is_some_and(|r| r.contains(pos));
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.help {
                    self.help = false;
                    return Ok(());
                }
                if let Some(action) = self.action_at(pos) {
                    return self.apply(action);
                }
            }
            MouseEventKind::ScrollUp if over(self.conversation_area) => self.scroll_up += 3,
            MouseEventKind::ScrollDown if over(self.conversation_area) => {
                self.scroll_up = self.scroll_up.saturating_sub(3);
            }
            MouseEventKind::ScrollUp if over(self.session_rows) => {
                self.select_session(self.selected.saturating_sub(1));
            }
            MouseEventKind::ScrollDown if over(self.session_rows) => {
                self.select_session(self.selected + 1);
            }
            _ => {}
        }
        Ok(())
    }

    /// What a click at `pos` does.
    pub fn action_at(&self, pos: Position) -> Option<Action> {
        if let Some((_, a)) = self.hits.iter().find(|(r, _)| r.contains(pos)) {
            return Some(a.clone());
        }
        if let Some(rows) = self.session_rows.filter(|r| r.contains(pos)) {
            let i = usize::from(pos.y - rows.y) + self.session_offset(rows.height);
            return (i <= self.runs.len()).then_some(Action::SelectSession(i));
        }
        if let Some(rows) = self.agent_rows.filter(|r| r.contains(pos)) {
            let i = usize::from(pos.y - rows.y);
            return (i < self.agent_count()).then_some(Action::SelectAgent(i));
        }
        if self.composer_area.is_some_and(|r| r.contains(pos)) {
            return Some(Action::Focus(Focus::Composer));
        }
        if self.conversation_area.is_some_and(|r| r.contains(pos)) {
            return Some(Action::Focus(Focus::Conversation));
        }
        None
    }

    /// First visible session row, so the selection stays on screen.
    fn session_offset(&self, height: u16) -> usize {
        let h = usize::from(height.max(1));
        self.selected.saturating_sub(h - 1)
    }

    pub fn draw(&mut self, f: &mut Frame) {
        self.hits.clear();
        let [top, main, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .areas(f.area());
        let [left, right] =
            Layout::horizontal([Constraint::Length(40), Constraint::Min(30)]).areas(main);
        let agent_lines =
            self.agent_count().max(1) + self.config.vendors.len() + self.detected_clis().len();
        let agents_height = u16::try_from(agent_lines + 4)
            .unwrap_or(u16::MAX)
            .clamp(6, 16);
        let [sessions, agents] =
            Layout::vertical([Constraint::Min(5), Constraint::Length(agents_height)]).areas(left);
        let composer_height = if self.selected == 0 { 4 } else { 3 };
        let [convo, composer] =
            Layout::vertical([Constraint::Min(4), Constraint::Length(composer_height)])
                .areas(right);

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" anchorleg ", Style::new().add_modifier(Modifier::BOLD)),
                Span::styled(
                    "sessions across your accounts · ? for keys",
                    Style::new().fg(Color::DarkGray),
                ),
                Span::styled(waiting_note(&self.pending), Style::new().fg(Color::Yellow)),
            ])),
            top,
        );
        self.draw_sessions(f, sessions);
        self.draw_agents(f, agents);
        self.draw_conversation(f, convo);
        self.draw_composer(f, composer);
        self.draw_suggestions(f, convo);
        self.draw_footer(f, footer);
        if self.help {
            draw_help(f);
        }
    }

    fn pane(&self, title: &str, focus: Focus) -> Block<'static> {
        let style = if self.focus == focus {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        Block::new()
            .borders(Borders::ALL)
            .border_style(style)
            .title(format!(" {title} "))
    }

    fn draw_sessions(&mut self, f: &mut Frame, area: Rect) {
        let block = self.pane("Sessions", Focus::Sessions);
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.session_rows = Some(inner);
        let offset = self.session_offset(inner.height);
        let width = usize::from(inner.width);
        let mut lines = vec![Line::from(Span::styled(
            "+ New session",
            Style::new().fg(Color::Green),
        ))];
        for r in &self.runs {
            let asks = self.pending.iter().any(|p| p.run_id == r.id);
            let (glyph, color) = match r.state {
                _ if asks => ("!", Color::Yellow),
                RunState::Running | RunState::Waiting if self.is_alive(r) => ("●", Color::Yellow),
                RunState::Running | RunState::Waiting => ("?", Color::DarkGray),
                RunState::Done => ("✓", Color::Green),
                RunState::Failed => ("✗", Color::Red),
                RunState::Stopped => ("■", Color::DarkGray),
                RunState::Switching => ("↻", Color::Yellow),
            };
            let head = format!(
                " #{:<3} {:<12} ",
                r.id,
                truncate(r.account.as_deref().unwrap_or("-"), 12)
            );
            let follow = if r.parent_id.is_some() { "↳ " } else { "" };
            let task = truncate(
                &format!("{follow}{}", r.task.replace('\n', " ")),
                width.saturating_sub(head.chars().count() + 1),
            );
            lines.push(Line::from(vec![
                Span::styled(glyph, Style::new().fg(color)),
                Span::raw(head),
                Span::raw(task),
            ]));
        }
        let lines: Vec<Line> = lines
            .into_iter()
            .enumerate()
            .skip(offset)
            .map(|(i, l)| {
                if i == self.selected {
                    l.style(Style::new().add_modifier(Modifier::REVERSED))
                } else {
                    l
                }
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn detected_clis(&self) -> Vec<(&'static str, &'static str, bool)> {
        KNOWN_CLIS
            .iter()
            .filter(|(bin, _)| {
                !self
                    .config
                    .accounts
                    .iter()
                    .any(|a| a.vendor.default_bin() == *bin)
            })
            .map(|(bin, label)| (*bin, *label, self.launcher.find_cli(bin).is_some()))
            .collect()
    }

    fn draw_agents(&mut self, f: &mut Frame, area: Rect) {
        let block = self.pane("Agents", Focus::Agents);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [rows, buttons, buttons2] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner);
        self.agent_rows = Some(rows);

        let t = now();
        let opts = PickOptions::default();
        let owned: Vec<Account> = self.config.by_priority().into_iter().cloned().collect();
        let mut lines: Vec<Line> = Vec::new();
        for (i, a) in owned.iter().enumerate() {
            let mine: Vec<&QuotaRow> = self.quota.iter().filter(|r| r.account == a.name).collect();
            let used = mine
                .iter()
                .filter_map(|r| r.used)
                .fold(None, |m: Option<f64>, u| Some(m.map_or(u, |m| m.max(u))));
            let state = if !a.enabled {
                Span::styled("off", Style::new().fg(Color::DarkGray))
            } else if let Some(until) = policy::blocked_until(&mine, t, &opts) {
                Span::styled(
                    format!("blocked {}", ago(until - t)),
                    Style::new().fg(Color::Red),
                )
            } else {
                meter(used)
            };
            let mut line = Line::from(vec![
                Span::raw(format!("{} ", i + 1)),
                Span::raw(format!("{:<20} ", truncate(&a.name, 20))),
                state,
            ]);
            if self.focus == Focus::Agents && i == self.agent {
                line = line.style(Style::new().add_modifier(Modifier::REVERSED));
            }
            lines.push(line);
        }
        if owned.is_empty() {
            lines.push(Line::from(Span::styled(
                "no accounts: press i (Import)",
                Style::new().fg(Color::Yellow),
            )));
        }
        for (vendor, settings) in &self.config.vendors {
            lines.push(Line::from(Span::styled(
                truncate(
                    &format!("  {}: {}", vendor.name(), settings.summary()),
                    usize::from(rows.width),
                ),
                Style::new().fg(Color::DarkGray),
            )));
        }
        for (bin, label, installed) in self.detected_clis() {
            let driven = Vendor::from_bin(bin)
                .is_some_and(|v| anchorleg_core::supervisor::DRIVEN.contains(&v));
            let note = match (installed, driven) {
                (true, true) => "installed, add an account",
                (true, false) => "installed, unsupported",
                (false, _) => "not installed",
            };
            lines.push(Line::from(Span::styled(
                format!("  {label:<12} {note}"),
                Style::new().fg(Color::DarkGray),
            )));
        }
        f.render_widget(Paragraph::new(lines), rows);

        let mut x = buttons.x;
        for (label, action) in [
            ("▲", Action::MoveUp),
            ("▼", Action::MoveDown),
            ("On/Off", Action::Toggle),
            ("Remove", Action::Remove),
        ] {
            x = self.button(f, buttons, x, label, action) + 1;
        }
        let mut x = buttons2.x;
        for (label, action) in [("Import", Action::Import), ("Settings", Action::Settings)] {
            x = self.button(f, buttons2, x, label, action) + 1;
        }
    }

    fn draw_conversation(&mut self, f: &mut Frame, area: Rect) {
        let title = match self.selected_run() {
            None => "New session".to_owned(),
            Some(r) => format!(
                "#{} · {} · {} · {}",
                r.id,
                r.account.as_deref().unwrap_or("-"),
                format!("{:?}", r.state).to_lowercase(),
                truncate(&r.cwd, 40)
            ),
        };
        let block = self.pane(&title, Focus::Conversation);
        let mut inner = block.inner(area);
        f.render_widget(block, area);
        self.conversation_area = Some(area);
        if let Some(p) = self.waiting().cloned()
            && inner.height > 6
        {
            let [rest, prompt] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(4)]).areas(inner);
            inner = rest;
            self.draw_permission(f, prompt, &p);
        }

        let width = usize::from(inner.width.max(10));
        let mut lines: Vec<Line> = Vec::new();
        if self.selected_run().is_none() {
            for t in [
                "Start a new task: pick the folder it runs in, type what you want done,",
                "and press Enter. anchorleg picks the account and switches if it runs low.",
                "",
                "Select a session on the left to read it and send it a follow-up.",
            ] {
                lines.push(Line::from(Span::styled(
                    t,
                    Style::new().fg(Color::DarkGray),
                )));
            }
        }
        // Claude Code's look: your messages on a grey band, everything else in the terminal's
        // own colour, `⏺` before the agent's text and tool calls.
        for m in &self.conversation {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            lines.extend(message_lines(m, width));
        }
        if let Some(r) = self.selected_run()
            && Self::is_running(r)
            && self.is_alive(r)
        {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                "✻ Working…",
                Style::new().fg(Color::DarkGray),
            )));
        }
        let height = usize::from(inner.height);
        let max_up = lines.len().saturating_sub(height);
        self.scroll_up = self.scroll_up.min(max_up);
        let start = lines.len().saturating_sub(height + self.scroll_up);
        let visible: Vec<Line> = lines.into_iter().skip(start).take(height).collect();
        f.render_widget(Paragraph::new(visible), inner);
    }

    /// Slash-command completions, just above the message box.
    fn draw_suggestions(&mut self, f: &mut Frame, above: Rect) {
        let items = self.suggestions();
        if items.is_empty() || above.height < 4 {
            return;
        }
        let h = u16::try_from(items.len())
            .unwrap_or(8)
            .min(above.height - 2);
        let area = Rect::new(above.x + 1, above.bottom() - 1 - h, above.width - 2, h);
        let pick = self.suggestion.min(items.len() - 1);
        let lines: Vec<Line> = items
            .iter()
            .enumerate()
            .map(|(i, (name, what))| {
                let line = Line::from(vec![
                    Span::styled(
                        format!("/{name:<28}"),
                        Style::new().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(what.clone(), Style::new().fg(Color::DarkGray)),
                ]);
                if i == pick {
                    line.style(Style::new().add_modifier(Modifier::REVERSED))
                } else {
                    line
                }
            })
            .collect();
        f.render_widget(Clear, area);
        f.render_widget(Paragraph::new(lines), area);
    }

    /// Claude Code's question, with its three answers: keys 1–3 or a click.
    fn draw_permission(&mut self, f: &mut Frame, area: Rect, p: &Permission) {
        let [rule, ask, why, buttons] = Layout::vertical([Constraint::Length(1); 4]).areas(area);
        f.render_widget(
            Paragraph::new("─".repeat(usize::from(area.width)))
                .style(Style::new().fg(Color::Yellow)),
            rule,
        );
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Allow ", Style::new().add_modifier(Modifier::BOLD)),
                Span::styled(
                    truncate(
                        &self.tool_label(p),
                        usize::from(area.width).saturating_sub(8),
                    ),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::styled("?", Style::new().add_modifier(Modifier::BOLD)),
            ])),
            ask,
        );
        f.render_widget(
            Paragraph::new(truncate(
                p.reason
                    .as_deref()
                    .unwrap_or("the agent is waiting for your answer"),
                usize::from(area.width),
            ))
            .style(Style::new().fg(Color::DarkGray)),
            why,
        );
        let mut x = buttons.x;
        for (label, state) in [
            ("1 Yes", PermissionState::Allowed),
            ("2 Yes, for this session", PermissionState::Always),
            ("3 No", PermissionState::Denied),
        ] {
            x = self.button(f, buttons, x, label, Action::Answer(state)) + 1;
        }
    }

    fn draw_composer(&mut self, f: &mut Frame, area: Rect) {
        if let Some(vendor) = self.settings_for {
            let title = format!(
                "{} settings: args for every {} launch (Enter save · Esc cancel)",
                vendor.name(),
                vendor.name()
            );
            let block = self.pane(&title, Focus::Composer);
            let inner = block.inner(area);
            f.render_widget(block, area);
            self.composer_area = Some(area);
            let label = "args ";
            let room = usize::from(inner.width).saturating_sub(label.len() + 1);
            let text = &self.settings_box;
            let skip = text.cursor.saturating_sub(room);
            let shown: String = text.text.chars().skip(skip).take(room).collect();
            let row = Rect::new(inner.x, inner.y, inner.width, 1);
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(label, Style::new().fg(Color::Cyan)),
                    Span::raw(shown),
                ])),
                row,
            );
            if self.focus == Focus::Composer {
                let col = u16::try_from(label.len() + text.cursor - skip).unwrap_or(0);
                let x = (inner.x + col).min(inner.right().saturating_sub(1));
                f.set_cursor_position(Position::new(x, inner.y));
            }
            return;
        }
        let title = match self.selected_run() {
            None => "New task (Enter to start)".to_owned(),
            Some(r) => format!("Reply to #{} (Enter to send)", r.id),
        };
        let block = self.pane(&title, Focus::Composer);
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.composer_area = Some(area);
        let typing = self.focus == Focus::Composer;

        let mut rows = vec![];
        if self.selected == 0 {
            rows.push(("folder ", Field::Folder, self.folder.clone()));
        }
        rows.push(("> ", Field::Message, self.message_box.clone()));
        for (i, (label, field, text)) in rows.into_iter().enumerate() {
            let y = inner.y + u16::try_from(i).unwrap_or(0);
            if y >= inner.bottom() {
                break;
            }
            let active = typing && (self.selected != 0 || self.field == field);
            let label_style = if active {
                Style::new().fg(Color::Cyan)
            } else {
                Style::new().fg(Color::DarkGray)
            };
            let room = usize::from(inner.width).saturating_sub(label.len() + 1);
            // Keep the cursor in view on long input.
            let skip = text.cursor.saturating_sub(room);
            let shown: String = text.text.chars().skip(skip).take(room).collect();
            let placeholder = shown.is_empty() && field == Field::Message && !active;
            let body = if placeholder {
                Span::styled(
                    "press / or click here to type",
                    Style::new().fg(Color::DarkGray),
                )
            } else {
                Span::raw(shown)
            };
            let row = Rect::new(inner.x, y, inner.width, 1);
            f.render_widget(
                Paragraph::new(Line::from(vec![Span::styled(label, label_style), body])),
                row,
            );
            if active {
                let col = u16::try_from(label.len() + text.cursor - skip).unwrap_or(0);
                let x = (inner.x + col).min(inner.right().saturating_sub(1));
                f.set_cursor_position(Position::new(x, y));
            }
        }
    }

    fn draw_footer(&mut self, f: &mut Frame, area: Rect) {
        let mut x = area.x;
        for (label, action) in [
            ("New (n)", Action::NewSession),
            ("Send (⏎)", Action::Send),
            ("Stop (s)", Action::Stop),
            ("Help (?)", Action::Help),
            ("Quit (q)", Action::Quit),
        ] {
            x = self.button(f, area, x, label, action) + 1;
        }
        if x + 2 < area.right() {
            let msg = Rect::new(x + 1, area.y, area.right() - x - 1, 1);
            f.render_widget(
                Paragraph::new(self.message.as_str()).style(Style::new().fg(Color::DarkGray)),
                msg,
            );
        }
    }

    /// Draw a clickable label at `x` in row `area`; returns the x after it.
    fn button(&mut self, f: &mut Frame, area: Rect, x: u16, label: &str, action: Action) -> u16 {
        let text = format!(" {label} ");
        let width = u16::try_from(text.chars().count())
            .unwrap_or(u16::MAX)
            .min(area.right().saturating_sub(x));
        if width == 0 {
            return x;
        }
        let rect = Rect::new(x, area.y, width, 1);
        f.render_widget(
            Paragraph::new(text).style(Style::new().fg(Color::Black).bg(Color::Gray)),
            rect,
        );
        self.hits.push((rect, action));
        x + width
    }
}

fn draw_help(f: &mut Frame) {
    let lines = [
        "Everywhere",
        "  Tab / Shift+Tab   next / previous pane",
        "  n                 new session",
        "  / or m            type a message (new task or reply)",
        "  s                 stop the selected session",
        "  r                 refresh      ?  this help      q  quit",
        "",
        "Sessions            ↑↓ select · Enter reply",
        "Conversation        ↑↓ PgUp PgDn scroll · End latest",
        "Message box         Enter send · Esc leave · ↑↓ folder/message (new task)",
        "                    /model x · /effort x  set it for every account of the provider",
        "                    /other  the CLI's own commands; Tab completes, ↑↓ choose",
        "                    Ctrl+A/E start/end · Ctrl+U clear",
        "Agents              ↑↓ select · Shift+↑↓ (K/J) reorder · space on/off",
        "                    i import aliases · d d remove",
        "Permission asked    1 yes · 2 yes for this session · 3 no",
        "                    o settings for the selected account's CLI (e.g. claude:",
        "                      --permission-mode acceptEdits), used on every launch of it",
        "",
        "Mouse: click panes, rows and buttons; scroll the lists and the conversation.",
        "Press any key to close.",
    ];
    let area = f.area();
    let w = 80.min(area.width.saturating_sub(4));
    let h = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(2));
    let rect = Rect::new(
        area.x + (area.width.saturating_sub(w)) / 2,
        area.y + (area.height.saturating_sub(h)) / 2,
        w,
        h,
    );
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines.iter().map(|l| Line::from(*l)).collect::<Vec<_>>()).block(
            Block::new()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(Color::Cyan))
                .title(" Keys "),
        ),
        rect,
    );
}

/// `Write(src/a.rs)`, `Bash(cargo test)`: a tool call in one line.
fn tool_label(tool: &str, input: &serde_json::Value) -> String {
    let text = conversation::describe_tool(tool, input);
    match text.split_once("  ") {
        Some((name, detail)) => format!("{name}({detail})"),
        None => text,
    }
}

/// "· 2 waiting for your answer (#4, #7)" for the top bar, or nothing.
fn waiting_note(pending: &[Permission]) -> String {
    if pending.is_empty() {
        return String::new();
    }
    let mut runs: Vec<String> = pending.iter().map(|p| format!("#{}", p.run_id)).collect();
    runs.dedup();
    format!(
        "  · {} waiting for your answer ({})",
        pending.len(),
        runs.join(", ")
    )
}

/// The grey band behind your messages (256-colour grey, readable on dark and light themes).
const USER_BG: Color = Color::Indexed(237);

/// One conversation message as screen lines, `width` columns wide.
fn message_lines(m: &Message, width: usize) -> Vec<Line<'static>> {
    let dim = Style::new().fg(Color::DarkGray);
    let red = Style::new().fg(Color::Red);
    match m.role {
        Role::User => {
            let band = Style::new().bg(USER_BG).fg(Color::White);
            wrap(&m.text, width.saturating_sub(3).max(10))
                .into_iter()
                .enumerate()
                .map(|(i, piece)| {
                    let head = if i == 0 { "> " } else { "  " };
                    let pad = width.saturating_sub(piece.chars().count() + 2);
                    Line::from(vec![
                        Span::styled(head, band.fg(Color::Gray)),
                        Span::styled(format!("{piece}{}", " ".repeat(pad)), band),
                    ])
                })
                .collect()
        }
        Role::Agent => bulleted("⏺ ", &m.text, width, Style::new()),
        Role::Tool => {
            // "Read  docs/STATUS.md" → "⏺ Read(docs/STATUS.md)", the name in bold.
            let (name, detail) = m.text.split_once("  ").unwrap_or((m.text.as_str(), ""));
            let text = if detail.is_empty() {
                format!("**{name}**")
            } else {
                format!("**{name}**({detail})")
            };
            bulleted("⏺ ", &text, width, Style::new())
        }
        Role::ToolError => bulleted("  ⎿ ", &m.text, width, red),
        Role::Anchorleg => bulleted("· ", &format!("anchorleg: {}", m.text), width, dim),
        Role::Error => bulleted("✗ ", &m.text, width, red.add_modifier(Modifier::BOLD)),
    }
}

/// Wrapped text after a marker, continuation lines indented under the text; `**bold**` shown
/// bold.
fn bulleted(marker: &str, text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let indent = marker.chars().count();
    let mut bold = false;
    wrap(text, width.saturating_sub(indent).max(10))
        .into_iter()
        .enumerate()
        .map(|(i, piece)| {
            let head = if i == 0 {
                marker.to_owned()
            } else {
                " ".repeat(indent)
            };
            let mut spans = vec![Span::styled(head, style)];
            for (k, part) in piece.split("**").enumerate() {
                if k > 0 {
                    bold = !bold;
                }
                if !part.is_empty() {
                    let st = if bold {
                        style.add_modifier(Modifier::BOLD)
                    } else {
                        style
                    };
                    spans.push(Span::styled(part.to_owned(), st));
                }
            }
            Line::from(spans)
        })
        .collect()
}

/// `██░░░  62%` for the fullest window, coloured by how full it is.
fn meter(used: Option<f64>) -> Span<'static> {
    let Some(u) = used else {
        return Span::styled("no data", Style::new().fg(Color::DarkGray));
    };
    let filled = (u.clamp(0.0, 1.0) * 5.0).round() as usize;
    let color = if u >= 0.95 {
        Color::Red
    } else if u >= 0.8 {
        Color::Yellow
    } else {
        Color::Green
    };
    Span::styled(
        format!(
            "{}{} {:>3.0}%",
            "█".repeat(filled),
            "░".repeat(5 - filled),
            u * 100.0
        ),
        Style::new().fg(color),
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Word-wrap to `width` chars; long words are cut.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines() {
        let mut line = String::new();
        for word in para.split(' ') {
            let mut word = word.to_owned();
            while word.chars().count() > width {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                let head: String = word.chars().take(width).collect();
                word = word.chars().skip(width).collect();
                out.push(head);
            }
            let need = line.chars().count() + usize::from(!line.is_empty()) + word.chars().count();
            if need > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// `3h05m`, `12m`, `40s`, `2d4h`.
fn ago(secs: i64) -> String {
    let secs = secs.max(0);
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    match (d, h, m) {
        (0, 0, 0) => format!("{secs}s"),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h{m:02}m"),
        (d, h, _) => format!("{d}d{h}h"),
    }
}

pub fn run() -> anyhow::Result<()> {
    let db = Store::default_path()?;
    let store = Store::open(&db)?;
    let log_dir = db.parent().map(|d| d.join("runs")).unwrap_or_default();
    let launcher = Box::new(Processes {
        exe: std::env::current_exe()?,
    });
    let cwd = std::env::current_dir()?;
    let mut app = App::new(Config::default_path()?, store, log_dir, launcher, &cwd)?;
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let result = event_loop(&mut terminal, &mut app);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> anyhow::Result<()> {
    while !app.quit {
        terminal.draw(|f| app.draw(f))?;
        if event::poll(TICK)? {
            let result = match event::read()? {
                Event::Key(k) => app.on_key(k),
                Event::Mouse(m) => app.on_mouse(m),
                _ => Ok(()),
            };
            if let Err(e) = result {
                app.message = format!("error: {e:#}");
            }
        } else if let Err(e) = app.refresh() {
            app.message = format!("error: {e:#}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    #[derive(Default)]
    struct Calls {
        started: Vec<Vec<String>>,
        stopped: Vec<i64>,
    }

    struct FakeLauncher(Rc<RefCell<Calls>>);

    impl Launcher for FakeLauncher {
        fn start(&self, args: &[String]) -> anyhow::Result<u32> {
            self.0.borrow_mut().started.push(args.to_vec());
            Ok(999)
        }
        fn stop(&self, run: i64) -> anyhow::Result<()> {
            self.0.borrow_mut().stopped.push(run);
            Ok(())
        }
        fn is_alive(&self, _: i64) -> bool {
            true
        }
        fn find_cli(&self, name: &str) -> Option<PathBuf> {
            (name == "cursor-agent").then(|| PathBuf::from("/bin/cursor-agent"))
        }
    }

    struct Fixture {
        dir: tempfile::TempDir,
        app: App,
        calls: Rc<RefCell<Calls>>,
    }

    /// Accounts by (name, priority); `runs` are (task, state, session) rows created first.
    fn fixture(accounts: &[(&str, u32)], runs: &[(&str, RunState, Option<&str>)]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut text = String::new();
        for (name, prio) in accounts {
            text += &format!(
                "[[account]]\nname = \"{name}\"\nvendor = \"claude\"\npriority = {prio}\nbin = \"claude\"\n\n"
            );
        }
        std::fs::write(&path, text).unwrap();
        std::fs::write(
            dir.path().join("aliases.txt"),
            "claude-new='CLAUDE_CONFIG_DIR=~/.claude-new /x/claude'\n",
        )
        .unwrap();
        let logs = dir.path().join("runs");
        std::fs::create_dir_all(&logs).unwrap();

        let store = Store::open_in_memory().unwrap();
        for (task, state, session) in runs {
            let id = store.start_run(task, "/repo", None, 100).unwrap();
            store.set_run_account(id, "sm", *session).unwrap();
            store.set_run_state(id, *state, 100).unwrap();
            if *state == RunState::Running {
                store.set_run_pid(id, Some(4242)).unwrap();
            }
        }
        let calls = Rc::new(RefCell::new(Calls::default()));
        let app = App::new(
            path,
            store,
            logs,
            Box::new(FakeLauncher(calls.clone())),
            dir.path(),
        )
        .unwrap()
        .with_alias_source(dir.path().join("aliases.txt"));
        Fixture { dir, app, calls }
    }

    fn screen(app: &mut App) -> String {
        let mut term = Terminal::new(TestBackend::new(130, 30)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn key(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            key(app, KeyCode::Char(c));
        }
    }

    /// Click the last on-screen occurrence of `label`.
    fn click(app: &mut App, label: &str) {
        let s = screen(app);
        let (y, line) = s
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains(label))
            .last()
            .unwrap_or_else(|| panic!("no `{label}` on screen:\n{s}"));
        let col = u16::try_from(line[..line.find(label).unwrap()].chars().count()).unwrap();
        let action = app
            .action_at(Position::new(col, u16::try_from(y).unwrap()))
            .unwrap_or_else(|| panic!("`{label}` is not clickable"));
        app.apply(action).unwrap();
    }

    #[test]
    fn new_task_from_the_keyboard() {
        let mut fx = fixture(&[("sm", 1)], &[]);
        let folder = fx.dir.path().display().to_string();
        key(&mut fx.app, KeyCode::Char('n'));
        assert_eq!(fx.app.focus, Focus::Composer);
        typed(&mut fx.app, "fix the tests");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(
            fx.calls.borrow().started,
            vec![vec![
                "run".to_owned(),
                "--json".into(),
                "--cwd".into(),
                folder,
                "--".into(),
                "fix the tests".into()
            ]]
        );
    }

    #[test]
    fn new_task_checks_the_folder() {
        let mut fx = fixture(&[("sm", 1)], &[]);
        key(&mut fx.app, KeyCode::Char('n'));
        key(&mut fx.app, KeyCode::Up); // to the folder field
        fx.app
            .on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .unwrap();
        typed(&mut fx.app, "/no/such/dir");
        key(&mut fx.app, KeyCode::Down);
        typed(&mut fx.app, "hello");
        key(&mut fx.app, KeyCode::Enter);
        assert!(fx.calls.borrow().started.is_empty());
        assert!(fx.app.message.contains("no such folder"));
    }

    #[test]
    fn reply_to_a_finished_session() {
        let mut fx = fixture(
            &[("sm", 1)],
            &[("fix the tests", RunState::Done, Some("s1"))],
        );
        std::fs::write(
            fx.dir.path().join("runs/run-1.jsonl"),
            [
                r#"{"type":"anchorleg_user","text":"fix the tests"}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"All 14 tests pass."}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        fx.app.refresh().unwrap();
        let s = screen(&mut fx.app);
        assert!(s.contains("All 14 tests pass."), "{s}");
        assert!(s.contains("Reply to #1"));

        key(&mut fx.app, KeyCode::Char('/'));
        typed(&mut fx.app, "now add docs");
        key(&mut fx.app, KeyCode::Enter);
        let started = &fx.calls.borrow().started;
        assert_eq!(started[0][..4], ["run", "--json", "--follow-up", "1"]);
        assert_eq!(started[0].last().unwrap(), "now add docs");
    }

    #[test]
    fn a_running_session_can_be_stopped_but_not_replied_to() {
        let mut fx = fixture(
            &[("sm", 1)],
            &[("long task", RunState::Running, Some("s1"))],
        );
        assert!(screen(&mut fx.app).contains("✻ Working…"));
        key(&mut fx.app, KeyCode::Char('/'));
        typed(&mut fx.app, "more");
        key(&mut fx.app, KeyCode::Enter);
        assert!(fx.calls.borrow().started.is_empty());
        assert!(fx.app.message.contains("still working"));

        key(&mut fx.app, KeyCode::Esc);
        key(&mut fx.app, KeyCode::Char('s'));
        assert_eq!(fx.calls.borrow().stopped, vec![1]);
    }

    #[test]
    fn sessions_list_and_selection_by_mouse() {
        let mut fx = fixture(
            &[("sm", 1)],
            &[
                ("first task", RunState::Done, Some("s1")),
                ("second task", RunState::Failed, Some("s2")),
            ],
        );
        let s = screen(&mut fx.app);
        assert!(s.contains("+ New session"));
        assert!(
            s.find("second task").unwrap() < s.find("first task").unwrap(),
            "newest first"
        );
        click(&mut fx.app, "first task");
        assert_eq!(fx.app.selected, 2);
        click(&mut fx.app, "+ New session");
        assert_eq!(fx.app.selected, 0);
        assert!(screen(&mut fx.app).contains("folder "));
    }

    #[test]
    fn agents_pane_shows_accounts_and_other_clis() {
        let mut fx = fixture(&[("two", 2), ("sm", 1)], &[]);
        let s = screen(&mut fx.app);
        assert!(s.find("1 sm").unwrap() < s.find("2 two").unwrap(), "{s}");
        assert!(s.contains("Codex        not installed"), "{s}");
        assert!(s.contains("Antigravity  not installed"));
        assert!(s.contains("Cursor       installed, unsupported"));
    }

    #[test]
    fn agents_reorder_toggle_remove_by_keyboard() {
        let mut fx = fixture(&[("sm", 1), ("two", 2), ("three", 3)], &[]);
        fx.app.apply(Action::Focus(Focus::Agents)).unwrap();
        fx.app
            .on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT))
            .unwrap();
        let order = |app: &App| -> Vec<String> {
            Config::load(&app.config_path)
                .unwrap()
                .by_priority()
                .iter()
                .map(|a| a.name.clone())
                .collect()
        };
        assert_eq!(order(&fx.app), ["two", "sm", "three"]);
        key(&mut fx.app, KeyCode::Char(' '));
        let config = Config::load(&fx.app.config_path).unwrap();
        assert!(!config.get("sm").unwrap().enabled);
        key(&mut fx.app, KeyCode::Char('d'));
        assert_eq!(order(&fx.app).len(), 3, "first d only asks");
        key(&mut fx.app, KeyCode::Char('d'));
        assert_eq!(order(&fx.app), ["two", "three"]);
    }

    #[test]
    fn import_by_mouse() {
        let mut fx = fixture(&[], &[]);
        assert!(screen(&mut fx.app).contains("no accounts"));
        click(&mut fx.app, "Import");
        assert!(fx.app.message.contains("imported claude-new"));
    }

    #[test]
    fn cli_settings_by_keyboard_and_mouse() {
        let mut fx = fixture(&[("sm", 1)], &[]);
        fx.app.apply(Action::Focus(Focus::Agents)).unwrap();
        key(&mut fx.app, KeyCode::Char('o'));
        assert!(screen(&mut fx.app).contains("claude settings: args for every claude launch"));
        typed(&mut fx.app, "--permission-mode acceptEdits");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.message, "claude settings saved");
        let saved = Config::load(&fx.dir.path().join("config.toml")).unwrap();
        assert_eq!(
            saved.vendors[&Vendor::Claude].args,
            ["--permission-mode", "acceptEdits"]
        );
        let s = screen(&mut fx.app);
        assert!(s.contains("claude: --permission-mode"), "{s}");

        // Anchorleg's own flags are refused; Esc leaves the settings as they were.
        click(&mut fx.app, "Settings");
        assert_eq!(fx.app.settings_box.text, "--permission-mode acceptEdits");
        typed(&mut fx.app, " -p");
        key(&mut fx.app, KeyCode::Enter);
        assert!(
            fx.app.message.contains("`-p` is set by anchorleg itself"),
            "{}",
            fx.app.message
        );
        key(&mut fx.app, KeyCode::Esc);
        assert_eq!(fx.app.focus, Focus::Agents);
        assert!(fx.app.settings_for.is_none());

        // Emptying the box removes the section.
        key(&mut fx.app, KeyCode::Char('o'));
        fx.app
            .on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .unwrap();
        key(&mut fx.app, KeyCode::Enter);
        assert!(
            Config::load(&fx.dir.path().join("config.toml"))
                .unwrap()
                .vendors
                .is_empty()
        );
    }

    #[test]
    fn conversation_looks_like_claude_code() {
        let mut fx = fixture(&[("sm", 1)], &[("hi", RunState::Done, Some("s1"))]);
        std::fs::write(
            fx.dir.path().join("runs/run-1.jsonl"),
            [
                r#"{"type":"anchorleg_user","text":"hi"}"#,
                r#"{"type":"anchorleg","text":"running on sm"}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/repo/docs/STATUS.md"}}]}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"**Phases 0-3** are done."}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        fx.app.refresh().unwrap();
        let mut term = Terminal::new(TestBackend::new(130, 30)).unwrap();
        term.draw(|f| fx.app.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();
        let row = |text: &str| {
            (0..buf.area.height)
                .find(|&y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol())
                        .collect::<String>()
                        .contains(text)
                })
                .unwrap_or_else(|| panic!("no `{text}` on screen"))
        };
        let x0 = 41; // the conversation pane's first column inside its border
        let you = row("> hi");
        assert_eq!(buf[(x0, you)].bg, USER_BG);
        assert_eq!(buf[(x0 + 60, you)].bg, USER_BG, "the band spans the pane");
        let read = row("⏺ Read(docs/STATUS.md)");
        assert_ne!(buf[(x0, read)].bg, USER_BG);
        let agent = row("⏺ Phases 0-3 are done.");
        assert!(buf[(x0 + 2, agent)].modifier.contains(Modifier::BOLD));
        assert!(!buf[(x0 + 15, agent)].modifier.contains(Modifier::BOLD));
        assert!(row("· anchorleg: running on sm") > you);
    }

    #[test]
    fn a_waiting_tool_call_is_answered_with_1_2_3_or_a_click() {
        let mut fx = fixture(&[("sm", 1)], &[("edit it", RunState::Running, Some("s1"))]);
        let input = serde_json::json!({ "file_path": "/repo/a.txt", "content": "x" });
        let ask = |app: &App| {
            app.store
                .ask_permission(1, "Write", &input, Some("Claude requested permissions"), 5)
                .unwrap()
        };
        let first = ask(&fx.app);
        fx.app.refresh().unwrap();
        let s = screen(&mut fx.app);
        assert!(s.contains("Allow Write(a.txt)?"), "{s}");
        assert!(s.contains("1 Yes") && s.contains("2 Yes, for this session") && s.contains("3 No"));
        assert!(s.contains("1 waiting for your answer (#1)"));
        assert!(s.contains("! #1"), "{s}");

        key(&mut fx.app, KeyCode::Char('2'));
        let p = fx.app.store.permission(first).unwrap().unwrap();
        assert_eq!(p.state, PermissionState::Always);
        assert!(!screen(&mut fx.app).contains("Allow Write"));

        let second = ask(&fx.app);
        fx.app.refresh().unwrap();
        click(&mut fx.app, "3 No");
        assert_eq!(
            fx.app.store.permission(second).unwrap().unwrap().state,
            PermissionState::Denied
        );
        assert!(fx.app.message.starts_with("refused Write"));
        // With nothing waiting, 1 does nothing.
        key(&mut fx.app, KeyCode::Char('1'));
        assert!(fx.calls.borrow().started.is_empty());
    }

    #[test]
    fn slash_model_and_effort_set_the_provider_and_others_complete() {
        let mut fx = fixture(&[("sm", 1)], &[("hi", RunState::Done, Some("s1"))]);
        std::fs::write(
            fx.dir.path().join("runs/run-1.jsonl"),
            r#"{"type":"system","subtype":"init","session_id":"s1","slash_commands":["compact","code-review","doctor"],"terminal_slash_commands":["doctor"]}"#,
        )
        .unwrap();
        fx.app.refresh().unwrap();
        key(&mut fx.app, KeyCode::Char('/'));
        typed(&mut fx.app, "/co");
        let s = screen(&mut fx.app);
        assert!(s.contains("/compact") && s.contains("/code-review"), "{s}");
        assert!(!s.contains("/doctor"));
        key(&mut fx.app, KeyCode::Down);
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.message_box.text, "/code-review ");
        assert_eq!(
            fx.app.focus,
            Focus::Composer,
            "Tab completed instead of moving on"
        );

        // Anchorleg's own: no run is started, the provider's setting changes.
        fx.app.message_box = TextBox::default();
        typed(&mut fx.app, "/model opus");
        key(&mut fx.app, KeyCode::Enter);
        typed(&mut fx.app, "/effort huge");
        key(&mut fx.app, KeyCode::Enter);
        assert!(
            fx.app
                .message
                .contains("one of: low, medium, high, xhigh, max")
        );
        fx.app.message_box = TextBox::default();
        typed(&mut fx.app, "/effort high");
        key(&mut fx.app, KeyCode::Enter);
        assert!(fx.calls.borrow().started.is_empty());
        let saved = Config::load(&fx.dir.path().join("config.toml")).unwrap();
        let claude = &saved.vendors[&Vendor::Claude];
        assert_eq!(
            (claude.model.as_deref(), claude.effort.as_deref()),
            (Some("opus"), Some("high"))
        );
        assert!(screen(&mut fx.app).contains("claude: opus · high"));

        // Any other slash command goes to the CLI as the message.
        typed(&mut fx.app, "/code-review src");
        key(&mut fx.app, KeyCode::Enter);
        let started = fx.calls.borrow().started.clone();
        assert_eq!(started[0].last().unwrap(), "/code-review src");
    }

    #[test]
    fn help_and_quit() {
        let mut fx = fixture(&[("sm", 1)], &[]);
        key(&mut fx.app, KeyCode::Char('?'));
        assert!(screen(&mut fx.app).contains("Tab / Shift+Tab"));
        key(&mut fx.app, KeyCode::Char('x'));
        assert!(!fx.app.help);
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.focus, Focus::Conversation);
        click(&mut fx.app, "Quit (q)");
        assert!(fx.app.quit);
    }

    #[test]
    fn wrap_breaks_on_words() {
        assert_eq!(wrap("aaa bbb ccc", 7), ["aaa bbb", "ccc"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("a\nb", 10), ["a", "b"]);
    }
}
