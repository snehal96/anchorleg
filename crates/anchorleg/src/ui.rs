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

use anchorleg_core::adapters::CreditState;
use anchorleg_core::conversation::{self, Message, Role};
use anchorleg_core::policy::{self, PickOptions};
use anchorleg_core::questions;
use anchorleg_core::registry::{
    Account, Config, Keychain, SecretStore, TokenFrom, Vendor, VendorSettings,
    accounts_from_aliases,
};
use anchorleg_core::store::{
    CreditsRow, Permission, PermissionState, QuotaRow, Run, RunState, Store, UsageTotal,
};
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
    /// Pick option n (from 1) for the waiting question.
    Pick(usize),
    /// Confirm the options picked for a multi-select question.
    ConfirmPicks,
    /// Decline the waiting question.
    SkipQuestion,
    /// Send the message box to the working session, stopping it at its next clean point.
    Interrupt,
    /// Edit the selected account's CLI settings (`[vendor.<name>]`).
    Settings,
    /// Next switch policy (drain → balance → save-weekly), saved as the default.
    CyclePolicy,
    Help,
    /// Show or hide the usage panel: tokens, cost, credits.
    Usage,
    /// Show what the selected session changed in its folder since it started.
    Diff,
    /// Put back the files the selected session changed (press twice).
    Undo,
    Quit,
}

/// One completion offered above the message box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    label: String,
    what: String,
    /// The message box's text once it's picked.
    insert: String,
}

/// The selected session's changes, shown over everything (`v`).
pub struct DiffView {
    title: String,
    lines: Vec<String>,
    scroll: usize,
}

/// A `!` command from the message box, run in the task folder.
struct Shell {
    cmd: String,
    folder: String,
    /// Its output (stdout and stderr), once it's done.
    output: Option<String>,
    done: Option<std::sync::mpsc::Receiver<String>>,
}

/// How long a `!` command may run, and how much of its output is kept.
const SHELL_LIMIT: Duration = Duration::from_secs(60);
const SHELL_CAP: usize = 8 * 1024;

/// Run `cmd` with `sh` in `folder` on a thread of its own; its output (capped) arrives on the
/// receiver. A command still running after [`SHELL_LIMIT`] is killed.
fn spawn_shell(cmd: &str, folder: &str) -> std::sync::mpsc::Receiver<String> {
    use std::io::Read as _;
    use std::process::{Command, Stdio};
    let (tx, rx) = std::sync::mpsc::channel();
    let script = format!("{{ {cmd}\n}} 2>&1");
    let folder = folder.to_owned();
    std::thread::spawn(move || {
        let child = Command::new("sh")
            .arg("-c")
            .arg(&script)
            .current_dir(&folder)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(format!("couldn't run it: {e}"));
                return;
            }
        };
        let (out_tx, out_rx) = std::sync::mpsc::channel();
        if let Some(mut stdout) = child.stdout.take() {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = stdout.read_to_end(&mut buf);
                let _ = out_tx.send(buf);
            });
        }
        let start = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if start.elapsed() < SHELL_LIMIT => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        // A background job may keep the pipe open; don't wait for it long.
        let buf = out_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_default();
        let mut out = String::from_utf8_lossy(&buf).trim_end().to_owned();
        if out.len() > SHELL_CAP {
            let mut cut = SHELL_CAP;
            while !out.is_char_boundary(cut) {
                cut -= 1;
            }
            out.truncate(cut);
            out.push_str("\n… (cut)");
        }
        match status {
            None => out.push_str(&format!("\n(stopped after {}s)", SHELL_LIMIT.as_secs())),
            Some(s) if !s.success() => {
                out.push_str(&format!("\n(exit {})", s.code().unwrap_or(-1)));
            }
            Some(_) => {}
        }
        let _ = tx.send(out.trim_start_matches('\n').to_owned());
    });
    rx
}

pub struct App {
    pub focus: Focus,
    pub help: bool,
    pub usage_panel: bool,
    /// Progress through the waiting question request: its id, the answers so far (one per
    /// question answered), and the options ticked on a multi-select question.
    asking: Option<(i64, Vec<Vec<String>>, Vec<usize>)>,
    /// Files going with the next message (pasted images, already in the task folder).
    attached: Vec<PathBuf>,
    pub config: Config,
    config_path: PathBuf,
    store: Store,
    log_dir: PathBuf,
    launcher: Box<dyn Launcher>,
    /// Where `Import` reads `alias` output from; `None` runs `$SHELL -ic alias`.
    alias_source: Option<PathBuf>,
    quota: Vec<QuotaRow>,
    credits: Vec<CreditsRow>,
    /// Tokens and cost over the last day and week, for the top bar and the usage panel.
    usage_day: Vec<UsageTotal>,
    usage_week: Vec<UsageTotal>,
    /// Tokens per day over the last two weeks.
    usage_days: Vec<(i64, u64, f64)>,
    runs: Vec<Run>,
    /// Tool calls waiting for the person, oldest first.
    pending: Vec<Permission>,
    /// Pids of running sessions that are still alive.
    alive: Vec<i64>,
    /// 0 is "+ New session"; `i` is `runs[i - 1]`.
    pub selected: usize,
    pub agent: usize,
    conversation: Vec<Message>,
    /// The selected session's task list (TodoWrite / Codex's plan), last version.
    steps: Vec<anchorleg_core::handoff::Step>,
    /// The selected session's context at its last model request, and the model's window.
    context: Option<(u64, Option<u64>)>,
    /// The tool call the selected session is in right now, and since when it's been seen.
    running: Option<(conversation::Running, std::time::Instant)>,
    /// The selected session's subagents (its newest run's), shown while it works.
    subagents: Vec<conversation::Subagent>,
    /// `!` commands run from the message box, oldest first. Their output goes with the next
    /// message sent in the same folder.
    shell: Vec<Shell>,
    pub diff_view: Option<DiffView>,
    /// The terminal's height at the last draw (a page for scrolling).
    last_height: u16,
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
    /// The session an undo waits for a second `U` on.
    confirm_undo: Option<i64>,
    /// `/mode` picked with no session selected: the next new task's mode.
    new_mode: Option<String>,
    /// Files of a task folder, for `@` completions (folder, paths), listed once per folder.
    files: std::cell::RefCell<Option<(String, Vec<String>)>>,
    /// Messages sent from this UI, oldest first, for Ctrl+P recall in a new task.
    sent: Vec<String>,
    /// Where Ctrl+P / Ctrl+N are in the recall list.
    recall: Option<usize>,
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
            usage_panel: false,
            asking: None,
            attached: Vec::new(),
            config: Config::default(),
            config_path,
            store,
            log_dir,
            launcher,
            alias_source: None,
            quota: Vec::new(),
            credits: Vec::new(),
            usage_day: Vec::new(),
            usage_week: Vec::new(),
            usage_days: Vec::new(),
            runs: Vec::new(),
            pending: Vec::new(),
            alive: Vec::new(),
            selected: 0,
            agent: 0,
            conversation: Vec::new(),
            steps: Vec::new(),
            context: None,
            running: None,
            shell: Vec::new(),
            subagents: Vec::new(),
            diff_view: None,
            last_height: 24,
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
            confirm_undo: None,
            new_mode: None,
            files: std::cell::RefCell::new(None),
            sent: Vec::new(),
            recall: None,
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
        self.credits = self.store.credits(None)?;
        let t = now();
        self.usage_day = self.store.usage_totals(t - 86_400)?;
        self.usage_week = self.store.usage_totals(t - 7 * 86_400)?;
        self.usage_days = self.store.usage_by_day(t - 14 * 86_400)?;
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
            self.steps.clear();
            self.context = None;
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
        let mut whole = String::new();
        for id in chain.iter().rev() {
            let log = std::fs::read_to_string(self.log_dir.join(format!("run-{id}.jsonl")))
                .unwrap_or_default();
            whole.push_str(&log);
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
        // The call running now, and the subagents: from the newest run's log.
        let newest = std::fs::read_to_string(self.log_dir.join(format!("run-{}.jsonl", chain[0])))
            .unwrap_or_default();
        let now_running = conversation::running(&newest);
        self.subagents = conversation::subagents(&newest);
        conversation::read_transcripts(&mut self.subagents);
        self.running = match (now_running, self.running.take()) {
            (Some(r), Some((old, since))) if old.what == r.what => Some((r, since)),
            (Some(r), _) => Some((r, std::time::Instant::now())),
            (None, _) => None,
        };
        let t = anchorleg_core::handoff::extract(&whole);
        self.context = t.context_tokens.map(|n| (n, t.context_window));
        self.steps = t.steps;
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
        if action != Action::Undo {
            self.confirm_undo = None;
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
            Action::Pick(n) => self.pick(n)?,
            Action::ConfirmPicks => self.confirm_picks()?,
            Action::SkipQuestion => self.answer(PermissionState::Denied)?,
            Action::Interrupt => self.send_to_running(true)?,
            Action::Send if self.settings_for.is_some() => self.save_settings()?,
            Action::Send => self.send()?,
            Action::Settings => self.edit_settings(),
            Action::Stop => self.stop()?,
            Action::MoveUp | Action::MoveDown => self.move_agent(action == Action::MoveUp)?,
            Action::Toggle => self.toggle_agent()?,
            Action::CyclePolicy => self.cycle_policy()?,
            Action::Import => self.import()?,
            Action::Remove => self.remove_agent()?,
            Action::Help => self.help = !self.help,
            Action::Usage => self.usage_panel = !self.usage_panel,
            Action::Diff => self.open_diff(),
            Action::Undo => self.undo()?,
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
    fn suggestions(&self) -> Vec<Suggestion> {
        if self.focus != Focus::Composer || (self.selected == 0 && self.field == Field::Folder) {
            return Vec::new();
        }
        let text = self.message_box.text.as_str();
        if let Some(typed) = text.strip_prefix('/')
            && !typed.contains(' ')
        {
            return self.slash_suggestions(typed);
        }
        // `@path`: a file of the task folder, completed from the last word.
        let start = text.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        if let Some(typed) = text[start..].strip_prefix('@') {
            let typed = typed.to_lowercase();
            let mut hits: Vec<String> = self
                .task_files()
                .into_iter()
                .filter(|f| f.to_lowercase().contains(&typed))
                .collect();
            // Names that start with what's typed first, then shorter paths.
            hits.sort_by_key(|f| {
                let name = f.rsplit('/').next().unwrap_or(f).to_lowercase();
                (!name.starts_with(&typed), f.len())
            });
            return hits
                .into_iter()
                .take(8)
                .map(|f| Suggestion {
                    label: format!("@{f}"),
                    what: String::new(),
                    insert: format!("{}@{f} ", &text[..start]),
                })
                .collect();
        }
        Vec::new()
    }

    fn slash_suggestions(&self, typed: &str) -> Vec<Suggestion> {
        let v = self.provider().name();
        let own = [
            ("model", format!("anchorleg: model for every {v} account")),
            ("effort", format!("anchorleg: effort for every {v} account")),
            (
                "clear",
                "anchorleg: a new session in this folder".to_owned(),
            ),
            (
                "compact",
                "summarise the session to free context".to_owned(),
            ),
            (
                "goal",
                "anchorleg: keep working until a condition holds".to_owned(),
            ),
            (
                "review",
                "anchorleg: review this session's changes on another account".to_owned(),
            ),
        ];
        own.into_iter()
            .map(|(n, d)| (n.to_owned(), d))
            .chain(
                self.slash
                    .iter()
                    .filter(|c| !own_command(c))
                    .map(|c| (c.clone(), String::new())),
            )
            .filter(|(n, _)| n.starts_with(typed))
            .take(8)
            .map(|(n, what)| Suggestion {
                label: format!("/{n}"),
                what,
                insert: format!("/{n} "),
            })
            .collect()
    }

    fn run_shell(&mut self, cmd: &str) -> anyhow::Result<()> {
        if cmd.is_empty() {
            self.message = "type a command after !".to_owned();
            return Ok(());
        }
        let folder = self.task_cwd();
        if !Path::new(&folder).is_dir() {
            self.message = format!("no such folder: {folder}");
            return Ok(());
        }
        self.shell.push(Shell {
            cmd: cmd.to_owned(),
            done: Some(spawn_shell(cmd, &folder)),
            folder,
            output: None,
        });
        self.message_box = TextBox::default();
        self.message = "running; its output goes with your next message".to_owned();
        Ok(())
    }

    /// Pick up the output of `!` commands that finished.
    fn poll_shell(&mut self) {
        for sh in &mut self.shell {
            if let Some(rx) = &sh.done {
                match rx.try_recv() {
                    Ok(out) => {
                        sh.output = Some(out);
                        sh.done = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        sh.output = Some("(no output)".to_owned());
                        sh.done = None;
                    }
                }
            }
        }
    }

    /// The finished `!` commands run in `folder`, as a note for the agent, and forget them.
    fn take_shell_notes(&mut self, folder: &str) -> String {
        self.poll_shell();
        let mut notes = String::new();
        self.shell.retain(|sh| {
            if sh.folder != folder {
                return true;
            }
            let Some(out) = &sh.output else {
                return true;
            };
            notes.push_str(&format!(
                "I ran `{}` in the task folder:\n```\n{}\n```\n\n",
                sh.cmd,
                if out.is_empty() { "(no output)" } else { out }
            ));
            false
        });
        notes
    }

    /// The selected session's folder, or the one the new task will run in (as typed, expanded).
    fn task_cwd(&self) -> String {
        match self.selected_run() {
            Some(r) => r.cwd.clone(),
            None => {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default();
                anchorleg_core::registry::expand_tilde(self.folder.text.trim(), &home)
            }
        }
    }

    /// The files `@` completes: the selected session's folder (or the new task's), as git sees
    /// it (tracked and untracked, not ignored), else a shallow walk.
    fn task_files(&self) -> Vec<String> {
        let folder = self.task_cwd();
        if let Some((f, files)) = &*self.files.borrow()
            && *f == folder
        {
            return files.clone();
        }
        let files = list_files(Path::new(&folder));
        *self.files.borrow_mut() = Some((folder, files.clone()));
        files
    }

    /// Earlier messages to recall: the selected session's, else the ones sent from here.
    fn recall_list(&self) -> Vec<String> {
        if self.selected_run().is_some() {
            self.conversation
                .iter()
                .filter(|m| m.role == Role::User)
                .map(|m| m.text.clone())
                .collect()
        } else {
            self.sent.clone()
        }
    }

    /// Ctrl+P (`back`) / Ctrl+N: put an earlier message in the box.
    fn recall(&mut self, back: bool) {
        let list = self.recall_list();
        if list.is_empty() {
            return;
        }
        let next = match (self.recall, back) {
            (None, true) => Some(list.len() - 1),
            (None, false) => None,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) => (i + 1 < list.len()).then_some(i + 1),
        };
        self.recall = next;
        self.message_box.set(next.map_or("", |i| list[i].as_str()));
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

    /// The folder the next message's task runs in: the session's, or the new task's.
    fn task_folder(&self) -> Option<PathBuf> {
        match self.selected_run() {
            Some(r) => Some(PathBuf::from(&r.cwd)),
            None => {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default();
                let dir = anchorleg_core::registry::expand_tilde(self.folder.text.trim(), &home);
                Some(PathBuf::from(dir)).filter(|d| d.is_dir())
            }
        }
    }

    /// Ctrl+V: the clipboard's image goes with the next message.
    fn paste_image(&mut self) -> anyhow::Result<()> {
        let Some(dir) = self.task_folder() else {
            self.message = "pick an existing folder first".to_owned();
            return Ok(());
        };
        match anchorleg_core::attachments::paste_image(&dir, now())? {
            Some(rel) => {
                self.attached.push(dir.join(rel));
                self.message =
                    format!("image attached ({} with this message)", self.attached.len());
            }
            None => {
                self.message = "no image on the clipboard (drop a file to attach it)".to_owned()
            }
        }
        Ok(())
    }

    /// The message box's text and every file going with it: pasted images and file paths
    /// dropped into the text. Clears the pasted ones.
    fn take_message(&mut self) -> (String, Vec<PathBuf>) {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let (dropped, text) =
            anchorleg_core::attachments::dropped_paths(self.message_box.text.trim(), &home);
        let mut files = std::mem::take(&mut self.attached);
        files.extend(dropped);
        let text = if text.trim().is_empty() && !files.is_empty() {
            "Have a look at the attached files.".to_owned()
        } else {
            text.trim().to_owned()
        };
        let folder = self.task_cwd();
        let notes = self.take_shell_notes(&folder);
        (notes + &text, files)
    }

    /// The message box, sent to the selected session while it works (`anchorleg send`): Codex
    /// takes it into the running turn, Claude when its turn ends; `interrupt` stops the agent at
    /// its next clean point first.
    fn send_to_running(&mut self, interrupt: bool) -> anyhow::Result<()> {
        let Some(run) = self.selected_run().cloned() else {
            return Ok(());
        };
        if !(Self::is_running(&run) && self.is_alive(&run)) {
            if interrupt {
                self.message = format!("#{} isn't working; Enter sends a reply", run.id);
                return Ok(());
            }
            return self.send();
        }
        if self.message_box.text.trim().is_empty() && self.attached.is_empty() {
            self.focus = Focus::Composer;
            self.message = "type a message first".to_owned();
            return Ok(());
        }
        let (text, files) = self.take_message();
        let staged = match anchorleg_core::attachments::stage(Path::new(&run.cwd), &files, now()) {
            Ok(staged) => staged,
            Err(e) => {
                self.message = format!("couldn't attach: {e}");
                return Ok(());
            }
        };
        let text = anchorleg_core::attachments::with_note(&text, &staged);
        self.store.queue_message(run.id, &text, interrupt, now())?;
        self.message_box.set("");
        let codex = self.provider() == Vendor::Codex;
        self.message = match (interrupt, codex) {
            (true, _) => format!(
                "#{}: stopping at the next clean point, then your message",
                run.id
            ),
            (false, true) => format!("#{}: sent into the running turn", run.id),
            (false, false) => format!(
                "#{}: queued; the agent reads it when this turn ends (Ctrl+X to interrupt)",
                run.id
            ),
        };
        self.refresh()
    }

    /// The waiting question request of the selected session, its questions, and which one is
    /// being answered now.
    fn question(&self) -> Option<(&Permission, Vec<questions::Question>, usize)> {
        let p = self.waiting().filter(|p| p.is_question())?;
        let qs = questions::from_input(&p.input);
        let step = match &self.asking {
            Some((id, done, _)) if *id == p.id => done.len(),
            _ => 0,
        };
        Some((p, qs, step))
    }

    /// Option `n` (from 1) of the current question: answers it, or ticks it on a multi-select one.
    fn pick(&mut self, n: usize) -> anyhow::Result<()> {
        let Some((p, qs, step)) = self.question() else {
            return Ok(());
        };
        let id = p.id;
        let Some(q) = qs.get(step) else { return Ok(()) };
        let Some(label) = n
            .checked_sub(1)
            .and_then(|i| q.options.get(i))
            .map(|o| o.label.clone())
        else {
            self.message = format!("no option {n}");
            return Ok(());
        };
        if q.multi_select {
            let ticks = &mut self.asking_for(id).2;
            match ticks.iter().position(|&t| t == n) {
                Some(i) => {
                    ticks.remove(i);
                }
                None => ticks.push(n),
            }
            self.message = "pick more, then Enter".to_owned();
            return Ok(());
        }
        self.answer_step(vec![label])
    }

    fn confirm_picks(&mut self) -> anyhow::Result<()> {
        let Some((p, qs, step)) = self.question() else {
            return Ok(());
        };
        let id = p.id;
        let Some(q) = qs.get(step).cloned() else {
            return Ok(());
        };
        if q.secret {
            return self.answer_step(Vec::new());
        }
        let mut ticks = std::mem::take(&mut self.asking_for(id).2);
        ticks.sort_unstable();
        if ticks.is_empty() {
            self.message = "pick at least one option, or type an answer".to_owned();
            return Ok(());
        }
        let labels = ticks
            .iter()
            .filter_map(|n| q.options.get(n - 1).map(|o| o.label.clone()))
            .collect();
        self.answer_step(labels)
    }

    fn asking_for(&mut self, id: i64) -> &mut (i64, Vec<Vec<String>>, Vec<usize>) {
        if self.asking.as_ref().is_none_or(|a| a.0 != id) {
            self.asking = Some((id, Vec::new(), Vec::new()));
        }
        self.asking.as_mut().expect("just set")
    }

    /// Record the answer to the current question; after the last one, send them all.
    fn answer_step(&mut self, answer: Vec<String>) -> anyhow::Result<()> {
        let Some((p, qs, _)) = self.question() else {
            return Ok(());
        };
        let id = p.id;
        let state = self.asking_for(id);
        state.1.push(answer);
        state.2.clear();
        if state.1.len() < qs.len() {
            return Ok(());
        }
        let answers: questions::Answers = qs
            .iter()
            .zip(std::mem::take(&mut state.1))
            .filter(|(q, a)| !q.secret && !a.is_empty())
            .map(|(q, a)| (q.id.clone(), a))
            .collect();
        self.asking = None;
        self.store.answer_question(id, &answers, now())?;
        self.message = format!("answered: {}", questions::summary(&qs, &answers));
        self.refresh()
    }

    fn answer(&mut self, state: PermissionState) -> anyhow::Result<()> {
        let Some(p) = self.waiting() else {
            self.message = "nothing is waiting for an answer here".to_owned();
            return Ok(());
        };
        if p.is_question() {
            let id = p.id;
            self.asking = None;
            self.store
                .answer_permission(id, PermissionState::Denied, now())?;
            self.message = "skipped the question".to_owned();
            return self.refresh();
        }
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
        // While the agent waits on a question, what you type answers it.
        if let Some((_, qs, step)) = self.question() {
            if text.is_empty() {
                return self.confirm_picks();
            }
            let q = qs[step.min(qs.len() - 1)].clone();
            self.message_box.set("");
            if q.secret {
                self.message = questions::SECRET_NOTE.to_owned();
                return self.answer_step(Vec::new());
            }
            return self.answer_step(questions::parse_answer(&q, &text));
        }
        if text.is_empty() && self.attached.is_empty() {
            self.focus = Focus::Composer;
            self.field = Field::Message;
            self.message = "type a message first".to_owned();
            return Ok(());
        }
        // `!cmd`: run it in the task folder; its output goes with your next message.
        if let Some(cmd) = text.strip_prefix('!') {
            return self.run_shell(cmd.trim());
        }
        if let Some(rest) = text.strip_prefix('/')
            && let Some((cmd, arg)) = Some(rest.split_once(' ').unwrap_or((rest, "")))
            && matches!(cmd, "model" | "effort")
        {
            return self.provider_setting(cmd, arg.trim());
        }
        // `/clear`: what Claude Code's does, a fresh session; here, a new task in the same folder.
        if text == "/clear" {
            let Some(run) = self.selected_run() else {
                self.message_box = TextBox::default();
                return Ok(());
            };
            let cwd = run.cwd.clone();
            self.folder.set(&cwd);
            self.select_session(0);
            self.focus = Focus::Composer;
            self.field = Field::Message;
            self.message_box = TextBox::default();
            self.message = format!("new session in {cwd}");
            return Ok(());
        }
        // Codex runs its slash commands only in its own app; in a headless session the model
        // would just read "/review" as text.
        if let Some(rest) = text.strip_prefix('/')
            && self.provider() == Vendor::Codex
            && !matches!(
                rest.split(' ').next().unwrap_or(""),
                "model"
                    | "effort"
                    | "mode"
                    | "fork"
                    | "plan"
                    | "worktree"
                    | "clear"
                    | "compact"
                    | "goal"
                    | "review"
            )
        {
            let cmd = rest.split(' ').next().unwrap_or("");
            self.message = format!(
                "Codex runs /{cmd} only in its own app; here: /compact, /clear, /plan, /fork, /mode"
            );
            return Ok(());
        }
        if text == "/compact" {
            match self.selected_run() {
                None => {
                    self.message = "select the session to compact first".to_owned();
                    return Ok(());
                }
                Some(r) if Self::is_running(r) && self.is_alive(r) => {
                    self.message = format!("#{} is working; /compact once it's done", r.id);
                    return Ok(());
                }
                Some(_) => {}
            }
        }
        if let Some(arg) = text
            .strip_prefix("/mode")
            .filter(|a| a.is_empty() || a.starts_with(' '))
        {
            return self.set_mode(arg.trim());
        }
        // `/review [fix] [notes]`: the selected session's changes, reviewed read-only on another
        // account (`anchorleg review`); `fix` sends the findings back to it.
        if let Some(rest) = text
            .strip_prefix("/review")
            .filter(|r| r.is_empty() || r.starts_with(' '))
        {
            let Some(run) = self.selected_run().cloned() else {
                self.message = "select the session to review first".to_owned();
                return Ok(());
            };
            if Self::is_running(&run) && self.is_alive(&run) {
                self.message = format!("#{} is working; review it once it's done", run.id);
                return Ok(());
            }
            let rest = rest.trim();
            let (fix, notes) = match rest.strip_prefix("fix") {
                Some(n) if n.is_empty() || n.starts_with(' ') => (true, n.trim()),
                _ => (false, rest),
            };
            let mut args = vec!["review".to_owned(), run.id.to_string(), "--json".to_owned()];
            if fix {
                args.push("--fix".to_owned());
            }
            if !notes.is_empty() {
                args.extend(["--".to_owned(), notes.to_owned()]);
            }
            let pid = self.launcher.start(&args)?;
            self.pending_pid = Some(i64::from(pid));
            self.message_box = TextBox::default();
            self.message = format!("reviewing #{}", run.id);
            return self.refresh();
        }
        // `/fork <message>`: a new session from the selected one's history.
        if let Some(message) = text.strip_prefix("/fork ") {
            let Some(run) = self.selected_run().cloned() else {
                self.message = "select the session to fork first".to_owned();
                return Ok(());
            };
            if run.session_id.is_none() {
                self.message = format!("#{} never started a session", run.id);
                return Ok(());
            }
            let args = vec![
                "run".to_owned(),
                "--json".to_owned(),
                "--fork".to_owned(),
                run.id.to_string(),
                "--".to_owned(),
                message.trim().to_owned(),
            ];
            let pid = self.launcher.start(&args)?;
            self.pending_pid = Some(i64::from(pid));
            self.message_box = TextBox::default();
            self.message = format!("forked #{}", run.id);
            return self.refresh();
        }
        if text.starts_with("/goal ")
            && let Some(r) = self.selected_run()
            && Self::is_running(r)
            && self.is_alive(r)
        {
            self.message = format!("#{} is working; set the goal once it's done", r.id);
            return Ok(());
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
                return self.send_to_running(false);
            }
            Some(run) if run.session_id.is_none() => {
                self.message = format!("#{} never started a session; start a new one (n)", run.id);
                return Ok(());
            }
            Some(run) => args.extend(["--follow-up".to_owned(), run.id.to_string()]),
        }
        let new_task = self.selected_run().is_none();
        let (mut text, files) = self.take_message();
        for f in files {
            args.extend(["--attach".to_owned(), f.display().to_string()]);
        }
        if new_task && let Some(m) = &self.new_mode {
            args.extend(["--mode".to_owned(), m.clone()]);
        }
        // `/goal <condition>`: keep the session working until it holds (`anchorleg run --goal`);
        // the condition is the task too.
        if let Some(cond) = text.strip_prefix("/goal ") {
            args.extend(["--goal".to_owned(), cond.trim().to_owned()]);
            text = String::new();
        }
        // `/plan <task>`: plan first, approve, then carry it out (`anchorleg run --plan`).
        // `/worktree <task>`: in a git worktree of its own. Both may be given.
        loop {
            if new_task && let Some(task) = text.strip_prefix("/plan ") {
                text = task.trim().to_owned();
                args.push("--plan".to_owned());
            } else if new_task && let Some(task) = text.strip_prefix("/worktree ") {
                text = task.trim().to_owned();
                args.push("--worktree".to_owned());
            } else {
                break;
            }
        }
        args.push("--".to_owned());
        self.sent.push(text.clone());
        self.recall = None;
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

    fn cycle_policy(&mut self) -> anyhow::Result<()> {
        let all = policy::Policy::ALL;
        let i = all
            .iter()
            .position(|p| *p == self.config.policy)
            .unwrap_or(0);
        let next = all[(i + 1) % all.len()];
        self.config.policy = next;
        self.config.save(&self.config_path)?;
        self.message = format!("policy {}: {}", next.name(), next.describe());
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
        if let Some(d) = &mut self.diff_view {
            let page = usize::from(self.last_height.saturating_sub(4)).max(1);
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => d.scroll = d.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => d.scroll += 1,
                KeyCode::PageUp => d.scroll = d.scroll.saturating_sub(page),
                KeyCode::PageDown | KeyCode::Char(' ') => d.scroll += page,
                KeyCode::Home => d.scroll = 0,
                KeyCode::End => d.scroll = usize::MAX,
                _ => self.diff_view = None,
            }
            return Ok(());
        }
        if self.help || self.usage_panel {
            self.help = false;
            self.usage_panel = false;
            return Ok(());
        }
        let suggestions = self.suggestions();
        if self.focus == Focus::Composer && self.settings_for.is_none() && !suggestions.is_empty() {
            let pick = self.suggestion.min(suggestions.len() - 1);
            match key.code {
                KeyCode::Tab => {
                    self.message_box.set(&suggestions[pick].insert);
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
                KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return self.apply(Action::Interrupt);
                }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return self.paste_image();
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.recall(true);
                }
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.recall(false);
                }
                KeyCode::Up
                    if self.selected != 0
                        && (self.message_box.text.is_empty() || self.recall.is_some()) =>
                {
                    self.recall(true);
                }
                KeyCode::Down if self.selected != 0 && self.recall.is_some() => {
                    self.recall(false);
                }
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
                    self.recall = None;
                }
            }
            return Ok(());
        }

        // Keys that work in every other pane.
        let action = match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
            KeyCode::Char('?') => Some(Action::Help),
            KeyCode::Char('u') => Some(Action::Usage),
            KeyCode::Char('v') => Some(Action::Diff),
            KeyCode::Char('U') => Some(Action::Undo),
            KeyCode::Char('n') => Some(Action::NewSession),
            KeyCode::Char('/') | KeyCode::Char('m') => Some(Action::Focus(Focus::Composer)),
            KeyCode::Char('s') => Some(Action::Stop),
            KeyCode::Char(c @ '1'..='9') if self.question().is_some() => {
                Some(Action::Pick(usize::from(c as u8 - b'0')))
            }
            KeyCode::Char('0') if self.question().is_some() => Some(Action::SkipQuestion),
            KeyCode::Enter
                if self.question().is_some_and(|(_, qs, step)| {
                    qs.get(step).is_some_and(|q| q.multi_select || q.secret)
                }) =>
            {
                Some(Action::ConfirmPicks)
            }
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
                    KeyCode::Char('p') => Action::CyclePolicy,
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
            MouseEventKind::ScrollUp if self.diff_view.is_some() => {
                if let Some(d) = &mut self.diff_view {
                    d.scroll = d.scroll.saturating_sub(3);
                }
            }
            MouseEventKind::ScrollDown if self.diff_view.is_some() => {
                if let Some(d) = &mut self.diff_view {
                    d.scroll += 3;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.diff_view.take().is_some() {
                    return Ok(());
                }
                if self.help || self.usage_panel {
                    self.help = false;
                    self.usage_panel = false;
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
                Span::styled(
                    usage_note(&self.usage_day),
                    Style::new().fg(Color::DarkGray),
                ),
                Span::styled(paying_note(&self.credits), Style::new().fg(Color::Red)),
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
        if self.usage_panel {
            self.draw_usage(f);
        }
        self.last_height = f.area().height;
        if self.diff_view.is_some() {
            self.draw_diff(f);
        }
    }

    /// `/mode [ask|edits]`: the selected session's permission mode (its follow-ups use it), or
    /// the next new task's.
    fn set_mode(&mut self, value: &str) -> anyhow::Result<()> {
        use anchorleg_core::supervisor::Mode;
        let names: Vec<&str> = Mode::ALL.iter().map(|m| m.name()).collect();
        let selected = self.selected_run().cloned();
        if value.is_empty() {
            let now = match &selected {
                Some(r) => r.mode.clone(),
                None => self.new_mode.clone(),
            };
            self.message = format!(
                "mode: {} (one of {})",
                now.as_deref().unwrap_or("the CLI settings'"),
                names.join(", ")
            );
            return Ok(());
        }
        let Some(mode) = Mode::parse(value) else {
            self.message = format!("{value}: one of {}", names.join(", "));
            return Ok(());
        };
        self.message_box = TextBox::default();
        match selected {
            Some(r) => {
                self.store.set_run_mode(r.id, Some(mode.name()))?;
                self.message = format!(
                    "#{} mode → {} from its next message{}",
                    r.id,
                    mode.name(),
                    if Self::is_running(&r) {
                        " (the turn running now keeps its mode)"
                    } else {
                        ""
                    }
                );
            }
            None => {
                self.new_mode = Some(mode.name().to_owned());
                self.message = format!("new tasks start in {} mode", mode.name());
            }
        }
        self.refresh()
    }

    /// `U` twice: put back the files the selected session changed (`anchorleg undo`).
    fn undo(&mut self) -> anyhow::Result<()> {
        let Some(run) = self.selected_run().cloned() else {
            self.message = "select a session to undo its changes".to_owned();
            return Ok(());
        };
        if Self::is_running(&run) && self.is_alive(&run) {
            self.message = format!("#{} is working; stop it (s) before undoing", run.id);
            return Ok(());
        }
        if self.confirm_undo != Some(run.id) {
            let chain = self.store.session_runs(run.id).unwrap_or_default();
            let files = anchorleg_core::checkpoint::session_diff(Path::new(&run.cwd), &chain, true)
                .ok()
                .flatten()
                .map_or(0, |s| s.lines().count().saturating_sub(1));
            self.message = format!(
                "Put back the {files} file(s) #{} changed? Press U again (a backup is kept).",
                run.id
            );
            self.confirm_undo = Some(run.id);
            return Ok(());
        }
        self.confirm_undo = None;
        self.message = match crate::run::undo_session(&self.store, &self.log_dir, run.id, false) {
            Ok(line) => format!("#{}: {line} (anchorleg undo {} --redo)", run.id, run.id),
            Err(e) => format!("#{}: {e}", run.id),
        };
        self.load_conversation();
        Ok(())
    }

    /// `v`: the selected session's changes since it started (`anchorleg diff`).
    fn open_diff(&mut self) {
        let Some(run) = self.selected_run().cloned() else {
            self.message = "select a session to see its changes".to_owned();
            return;
        };
        let chain = self.store.session_runs(run.id).unwrap_or_default();
        match anchorleg_core::checkpoint::session_diff(Path::new(&run.cwd), &chain, false) {
            Ok(Some(d)) if d.trim().is_empty() => {
                self.message = format!("#{}: no changes yet", run.id);
            }
            Ok(Some(d)) => {
                self.diff_view = Some(DiffView {
                    title: format!(" Changes in #{} since it started ", run.id),
                    lines: d.lines().map(str::to_owned).collect(),
                    scroll: 0,
                });
            }
            Ok(None) => {
                self.message = format!(
                    "#{}: no start to compare with (not a git repository, or an older session)",
                    run.id
                );
            }
            Err(e) => self.message = format!("couldn't diff #{}: {e}", run.id),
        }
    }

    fn draw_diff(&mut self, f: &mut Frame) {
        let area = f.area();
        let rect = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        let Some(d) = &mut self.diff_view else {
            return;
        };
        let height = usize::from(rect.height.saturating_sub(2));
        d.scroll = d.scroll.min(d.lines.len().saturating_sub(height));
        let lines: Vec<Line> = d
            .lines
            .iter()
            .skip(d.scroll)
            .take(height)
            .map(|l| {
                let style = if l.starts_with("diff --git") {
                    Style::new().add_modifier(Modifier::BOLD)
                } else if l.starts_with("+++") || l.starts_with("---") {
                    Style::new().fg(Color::DarkGray)
                } else if l.starts_with('+') {
                    Style::new().fg(Color::Green)
                } else if l.starts_with('-') {
                    Style::new().fg(Color::Red)
                } else if l.starts_with("@@") {
                    Style::new().fg(Color::Cyan)
                } else {
                    Style::new()
                };
                Line::from(Span::styled(l.replace('\t', "    "), style))
            })
            .collect();
        let shown = format!(
            " {}–{} of {} · ↑↓ PgUp PgDn · any other key closes ",
            (d.scroll + 1).min(d.lines.len()),
            (d.scroll + height).min(d.lines.len()),
            d.lines.len()
        );
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(lines).block(
                Block::new()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(Color::Cyan))
                    .title(d.title.clone())
                    .title_bottom(shown),
            ),
            rect,
        );
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
        let title = format!("Agents · {}", self.config.policy.name());
        let block = self.pane(&title, Focus::Agents);
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
            let mut spans = vec![
                Span::raw(format!("{} ", i + 1)),
                Span::raw(format!("{:<20} ", truncate(&a.name, 20))),
                state,
            ];
            if a.pinned {
                spans.push(Span::styled(" pinned", Style::new().fg(Color::DarkGray)));
            }
            match self
                .credits
                .iter()
                .find(|c| c.account == a.name)
                .map(|c| c.state)
            {
                Some(CreditState::InUse) => {
                    spans.push(Span::styled(" paying", Style::new().fg(Color::Red)));
                }
                Some(CreditState::Available | CreditState::Unlimited)
                    if a.credits != policy::CreditsUse::Never =>
                {
                    spans.push(Span::styled(" +credits", Style::new().fg(Color::DarkGray)));
                }
                _ => {}
            }
            let mut line = Line::from(spans);
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
        for (label, action) in [
            ("Import", Action::Import),
            ("Settings", Action::Settings),
            ("Policy", Action::CyclePolicy),
        ] {
            x = self.button(f, buttons2, x, label, action) + 1;
        }
    }

    fn draw_conversation(&mut self, f: &mut Frame, area: Rect) {
        let title = match self.selected_run() {
            None => "New session".to_owned(),
            Some(r) => format!(
                "#{} · {} · {} · {}{}",
                r.id,
                r.account.as_deref().unwrap_or("-"),
                format!("{:?}", r.state).to_lowercase(),
                truncate(&r.cwd, 40),
                r.mode
                    .as_deref()
                    .map_or(String::new(), |m| format!(" · mode {m}"))
                    + &r.goal_status
                        .as_deref()
                        .map_or(String::new(), |g| format!(" · ◎ goal {g}"))
                    + &r.review_of
                        .map_or(String::new(), |id| format!(" · review of #{id}"))
                    + &self.context.map_or(String::new(), |(n, w)| format!(
                        " · {}",
                        context_meter(n, w)
                    )),
            ),
        };
        let block = self.pane(&title, Focus::Conversation);
        let mut inner = block.inner(area);
        f.render_widget(block, area);
        self.conversation_area = Some(area);
        if let Some(p) = self.waiting().cloned()
            && inner.height > 6
        {
            let height = match self.question() {
                Some((_, qs, step)) => {
                    let options = qs.get(step).map_or(0, |q| q.options.len());
                    u16::try_from(options + 4).unwrap_or(u16::MAX)
                }
                None => 4,
            }
            .min(inner.height.saturating_sub(3));
            let [rest, prompt] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(height)]).areas(inner);
            inner = rest;
            if p.is_question() {
                self.draw_question(f, prompt);
            } else {
                self.draw_permission(f, prompt, &p);
            }
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
        // The agent's task list, as Claude Code shows it under the conversation.
        if !self.steps.is_empty() && self.selected_run().is_some() {
            use anchorleg_core::handoff::StepStatus;
            let done = self
                .steps
                .iter()
                .filter(|s| s.status == StepStatus::Done)
                .count();
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                format!("Tasks  {done}/{}", self.steps.len()),
                Style::new().add_modifier(Modifier::BOLD),
            )));
            for step in &self.steps {
                let (mark, style) = match step.status {
                    StepStatus::Done => (
                        "  ✔ ",
                        Style::new()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::CROSSED_OUT),
                    ),
                    StepStatus::InProgress => ("  ▸ ", Style::new().add_modifier(Modifier::BOLD)),
                    StepStatus::Pending => ("  ◻ ", Style::new()),
                };
                let text = truncate(&step.text, width.saturating_sub(6).max(10));
                lines.push(Line::from(vec![
                    Span::styled(mark, style.remove_modifier(Modifier::CROSSED_OUT)),
                    Span::styled(text, style),
                ]));
            }
        }
        if let Some(r) = self.selected_run()
            && Self::is_running(r)
            && self.is_alive(r)
        {
            // Sent while it works, not read by the agent yet.
            for m in self.store.queued_messages(r.id).unwrap_or_default() {
                lines.push(Line::default());
                let note = if m.interrupt {
                    "interrupting"
                } else {
                    "queued"
                };
                for (i, piece) in wrap(&m.text, width.saturating_sub(14).max(10))
                    .into_iter()
                    .enumerate()
                {
                    lines.push(Line::from(vec![
                        Span::styled(
                            if i == 0 {
                                format!("> ({note}) ")
                            } else {
                                " ".repeat(note.len() + 5)
                            },
                            Style::new().fg(Color::DarkGray),
                        ),
                        Span::styled(piece, Style::new().fg(Color::Gray)),
                    ]));
                }
            }
            lines.push(Line::default());
            let dim = Style::new().fg(Color::DarkGray);
            if let Some((r, since)) = &self.running {
                let (name, detail) = r.what.split_once("  ").unwrap_or((r.what.as_str(), ""));
                let secs = since.elapsed().as_secs();
                lines.push(Line::from(vec![
                    Span::styled("⏺ ", dim),
                    Span::styled(name.to_owned(), Style::new().add_modifier(Modifier::BOLD)),
                    Span::raw(if detail.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "({})",
                            truncate(detail, width.saturating_sub(name.len() + 20).max(10))
                        )
                    }),
                    Span::styled(format!("  running {secs}s"), dim),
                ]));
                if !r.output.is_empty() {
                    lines.extend(output_lines(&r.output, width, dim));
                }
            }
            // Subagents, as Claude Code lists them under the Agent call.
            for a in &self.subagents {
                let (mark, style) = if a.done {
                    ("  ✔ ", dim)
                } else {
                    ("  ▸ ", Style::new().add_modifier(Modifier::BOLD))
                };
                let steps = match a.steps {
                    0 => String::new(),
                    1 => " · 1 tool use".to_owned(),
                    n => format!(" · {n} tool uses"),
                };
                lines.push(Line::from(vec![
                    Span::styled(mark, style),
                    Span::styled(truncate(&a.name, width.saturating_sub(24).max(10)), style),
                    Span::styled(steps, dim),
                ]));
                if !a.done
                    && let Some(d) = &a.doing
                {
                    let (name, detail) = d.split_once("  ").unwrap_or((d.as_str(), ""));
                    let call = if detail.is_empty() {
                        name.to_owned()
                    } else {
                        format!("{name}({detail})")
                    };
                    lines.push(Line::from(Span::styled(
                        format!(
                            "      ⎿  {}",
                            truncate(&call, width.saturating_sub(10).max(10))
                        ),
                        dim,
                    )));
                }
            }
            lines.push(Line::from(Span::styled("✻ Working…", dim)));
        }
        // `!` commands, until their output goes with the next message.
        self.poll_shell();
        let folder = self.task_cwd();
        for sh in self.shell.iter().filter(|sh| sh.folder == folder) {
            let dim = Style::new().fg(Color::DarkGray);
            lines.push(Line::default());
            lines.push(Line::from(vec![
                Span::styled("! ", Style::new().fg(Color::Magenta)),
                Span::raw(truncate(&sh.cmd, width.saturating_sub(4).max(10))),
            ]));
            let shown = match &sh.output {
                None => "running…".to_owned(),
                Some(out) => {
                    conversation::output_excerpt(out).unwrap_or_else(|| "(no output)".to_owned())
                }
            };
            lines.extend(output_lines(&shown, width, dim));
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
            .map(|(i, sug)| {
                let line = Line::from(vec![
                    Span::styled(
                        format!("{:<29}", sug.label),
                        Style::new().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(sug.what.clone(), Style::new().fg(Color::DarkGray)),
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

    /// The agent's question: header, text, numbered options; clickable.
    fn draw_question(&mut self, f: &mut Frame, area: Rect) {
        let Some((p, qs, step)) = self.question() else {
            return;
        };
        let id = p.id;
        let Some(q) = qs.get(step).cloned() else {
            return;
        };
        let ticks = match &self.asking {
            Some((a, _, t)) if *a == id => t.clone(),
            _ => Vec::new(),
        };
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let dim = Style::new().fg(Color::DarkGray);
        let width = usize::from(area.width);
        let mut y = area.y;
        let mut row = |f: &mut Frame, line: Line| {
            if y < area.bottom() {
                f.render_widget(Paragraph::new(line), Rect::new(area.x, y, area.width, 1));
            }
            y += 1;
            y - 1
        };
        row(
            f,
            Line::styled("─".repeat(width), Style::new().fg(Color::Yellow)),
        );
        let count = if qs.len() > 1 {
            format!("  ({}/{})", step + 1, qs.len())
        } else {
            String::new()
        };
        let header = q
            .header
            .as_deref()
            .map_or(String::new(), |h| format!("{h} · "));
        row(
            f,
            Line::from(vec![
                Span::styled("? ", Style::new().fg(Color::Yellow)),
                Span::styled(
                    truncate(&format!("{header}{}", q.question), width.saturating_sub(12)),
                    bold,
                ),
                Span::styled(count, dim),
            ]),
        );
        for (i, o) in q.options.iter().enumerate() {
            let n = i + 1;
            let mark = if q.multi_select {
                if ticks.contains(&n) { "[x] " } else { "[ ] " }
            } else {
                ""
            };
            let more = o
                .description
                .as_deref()
                .map_or(String::new(), |d| format!("  {d}"));
            let ry = row(
                f,
                Line::from(vec![
                    Span::styled(format!("  {n} "), Style::new().fg(Color::Cyan)),
                    Span::raw(format!("{mark}{}", o.label)),
                    Span::styled(
                        truncate(&more, width.saturating_sub(o.label.len() + 10)),
                        dim,
                    ),
                ]),
            );
            if ry < area.bottom() {
                self.hits
                    .push((Rect::new(area.x, ry, area.width, 1), Action::Pick(n)));
            }
        }
        let hint = match (q.options.is_empty(), q.multi_select) {
            _ if q.secret => {
                "secret: anchorleg won't pass it; give it to the agent another way · Enter next"
            }
            (true, _) => "type your answer below and press Enter · 0 skip",
            (false, true) => "number keys tick · Enter confirm · or type your own answer · 0 skip",
            (false, false) => "number to pick · or type your own answer below · 0 skip",
        };
        row(f, Line::styled(format!("  {hint}"), dim));
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
        let mut title = match self.selected_run() {
            None => "New task (Enter to start)".to_owned(),
            Some(r) => format!("Reply to #{} (Enter to send)", r.id),
        };
        if !self.attached.is_empty() {
            title.push_str(&format!(" · {} attached", self.attached.len()));
        }
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
            ("Usage (u)", Action::Usage),
            ("Changes (v)", Action::Diff),
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

    /// The usage panel: per account and model over a day and a week, tokens per day, and
    /// each account's usage credits.
    fn draw_usage(&self, f: &mut Frame) {
        let dim = Style::new().fg(Color::DarkGray);
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let mut lines: Vec<Line> = Vec::new();
        for (title, totals) in [
            ("Last 24 hours", &self.usage_day),
            ("Last 7 days", &self.usage_week),
        ] {
            lines.push(Line::from(Span::styled(title, bold)));
            if totals.is_empty() {
                lines.push(Line::from(Span::styled("  no runs", dim)));
            }
            for t in totals {
                lines.push(Line::from(vec![
                    Span::raw(format!(
                        "  {:<14} {:<20} {:>4} runs  {:>7} in  {:>7} out  {:>7} cached  ",
                        truncate(&t.account, 14),
                        truncate(t.model.as_deref().unwrap_or("-"), 20),
                        t.launches,
                        crate::usage::count(t.input_tokens),
                        crate::usage::count(t.output_tokens),
                        crate::usage::count(t.cache_read_tokens + t.cache_write_tokens),
                    )),
                    Span::styled(
                        t.cost_usd.map_or("-".to_owned(), |c| format!("${c:.2}")),
                        Style::new().fg(Color::Green),
                    ),
                    Span::styled(
                        t.credits_cost_usd
                            .filter(|c| *c > 0.0)
                            .map_or(String::new(), |c| format!("  (${c:.2} on credits)")),
                        Style::new().fg(Color::Red),
                    ),
                ]));
            }
            lines.push(Line::raw(""));
        }
        let tokens: Vec<u64> = self.usage_days.iter().map(|d| d.1).collect();
        if !tokens.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("Tokens per day (14d)  ", bold),
                Span::styled(crate::usage::spark(&tokens), Style::new().fg(Color::Cyan)),
            ]));
            lines.push(Line::raw(""));
        }
        lines.push(Line::from(Span::styled("Usage credits", bold)));
        let owned: Vec<&Account> = self.config.by_priority();
        for a in owned {
            let state = self.credits.iter().find(|c| c.account == a.name);
            let (text, style) = match state.map(|c| (c.state, c.reason.as_deref())) {
                None => ("not reported yet".to_owned(), dim),
                Some((CreditState::Off, Some(r))) => (format!("off ({r})"), dim),
                Some((CreditState::Off, None)) => ("off".to_owned(), dim),
                Some((CreditState::Available, _)) => ("available".to_owned(), Style::new()),
                Some((CreditState::Unlimited, _)) => ("unlimited".to_owned(), Style::new()),
                Some((CreditState::InUse, _)) => {
                    ("IN USE, paying".to_owned(), Style::new().fg(Color::Red))
                }
            };
            lines.push(Line::from(vec![
                Span::raw(format!("  {:<16} ", truncate(&a.name, 16))),
                Span::styled(format!("{text:<28}"), style),
                Span::styled(format!("anchorleg may use: {}", a.credits.name()), dim),
            ]));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "$ = API list price of the same tokens (Claude only); a subscription doesn't pay it.",
            dim,
        )));
        lines.push(Line::from(Span::styled(
            "Change with: anchorleg accounts set <name> --credits never|last-resort|allow",
            dim,
        )));

        let area = f.area();
        let w = 104.min(area.width.saturating_sub(4));
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
            Paragraph::new(lines).block(
                Block::new()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(Color::Cyan))
                    .title(" Usage · any key closes "),
            ),
            rect,
        );
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

/// `ctx 42k/200k (21%)`, or `ctx 42k` when the window isn't known.
fn context_meter(tokens: u64, window: Option<u64>) -> String {
    let short = |n: u64| match n {
        n if n >= 1_000_000 && n % 1_000_000 == 0 => format!("{}M", n / 1_000_000),
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{}k", (n + 500) / 1_000),
        n => n.to_string(),
    };
    match window.filter(|w| *w > 0) {
        Some(w) => format!("ctx {}/{} ({}%)", short(tokens), short(w), tokens * 100 / w),
        None => format!("ctx {}", short(tokens)),
    }
}

/// The files of `folder` for `@` completions: `git ls-files` (tracked and untracked, not ignored)
/// in a repository, else a walk three levels deep that skips hidden folders. At most 5000.
fn list_files(folder: &Path) -> Vec<String> {
    const MAX: usize = 5000;
    if let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(folder)
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .stderr(std::process::Stdio::null())
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
            let path = e.path();
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if depth > 0 {
                    walk(base, &path, depth - 1, out);
                }
            } else if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.display().to_string());
            }
        }
    }
    let mut out = Vec::new();
    walk(folder, folder, 2, &mut out);
    out.sort();
    out
}

fn draw_help(f: &mut Frame) {
    let lines = [
        "Everywhere",
        "  Tab / Shift+Tab   next / previous pane",
        "  n                 new session",
        "  / or m            type a message (new task or reply)",
        "  s                 stop the selected session",
        "  u                 usage: tokens, API price, credits",
        "  v                 the selected session's changes since it started (a diff)",
        "  U U               undo them: files go back to how the session found them",
        "Message box (more)  Ctrl+V paste an image · drop a file to attach it",
        "                    while a session works: Enter sends to it · Ctrl+X interrupts first",
        "  r                 refresh      ?  this help      q  quit",
        "",
        "Sessions            ↑↓ select · Enter reply",
        "Conversation        ↑↓ PgUp PgDn scroll · End latest",
        "Message box         Enter send · Esc leave · ↑↓ folder/message (new task)",
        "                    /model x · /effort x  set it for every account of the provider",
        "                    /plan <task>  new task, plan first: approve the plan, then it runs",
        "                    /mode ask|edits  the session's permission mode (no session: new tasks')",
        "                    /worktree <task>  new task in its own git worktree · /fork <msg>  branch the session",
        "                    /clear  new session in this folder · /compact  free the session's context",
        "                    /goal <condition>  keep working until it holds (new task or the session)",
        "                    /review [fix]  review the session's changes on another account (fix: send findings back)",
        "                    /other  the CLI's own commands; Tab completes, ↑↓ choose",
        "                    Ctrl+A/E start/end · Ctrl+U clear",
        "                    @file  complete a file of the task folder (Tab) · Ctrl+P/N or ↑ earlier messages",
        "                    !cmd  run it in the task folder; its output goes with your next message",
        "Agents              ↑↓ select · Shift+↑↓ (K/J) reorder · space on/off",
        "                    i import aliases · d d remove",
        "                    p switch policy: drain → balance → save-weekly",
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

/// "· today 1.2M tokens · $3.40 API price" for the top bar, or nothing.
fn usage_note(day: &[UsageTotal]) -> String {
    let tokens: u64 = day
        .iter()
        .map(|t| t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_write_tokens)
        .sum();
    if tokens == 0 {
        return String::new();
    }
    let cost: f64 = day.iter().filter_map(|t| t.cost_usd).sum();
    let cost = if cost > 0.0 {
        format!(" · ${cost:.2} API price")
    } else {
        String::new()
    };
    format!("  · 24h {} tokens{cost} (u)", crate::usage::count(tokens))
}

/// "· claude-sm paying from credits" for the top bar, or nothing.
fn paying_note(credits: &[CreditsRow]) -> String {
    let paying: Vec<&str> = credits
        .iter()
        .filter(|c| c.state == CreditState::InUse)
        .map(|c| c.account.as_str())
        .collect();
    if paying.is_empty() {
        String::new()
    } else {
        format!("  · {} paying from credits", paying.join(", "))
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
        Role::Output => output_lines(&m.text, width, dim),
        Role::Thinking => {
            // A few lines at most: the gist, not the whole chain of thought.
            let mut lines = bulleted("✻ ", &m.text, width, dim.add_modifier(Modifier::ITALIC));
            if lines.len() > 3 {
                lines.truncate(3);
                lines.push(Line::from(Span::styled("  …", dim)));
            }
            lines
        }
        Role::Anchorleg => bulleted("· ", &format!("anchorleg: {}", m.text), width, dim),
        Role::Error => bulleted("✗ ", &m.text, width, red.add_modifier(Modifier::BOLD)),
    }
}

/// Wrapped text after a marker, continuation lines indented under the text; `**bold**` shown
/// bold.
/// A tool's output under its call: `⎿` before the first line, each line cut to the width.
/// Slash commands anchorleg lists itself, ahead of the CLI's.
fn own_command(name: &str) -> bool {
    matches!(
        name,
        "model" | "effort" | "clear" | "compact" | "goal" | "review"
    )
}

fn output_lines(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    text.lines()
        .enumerate()
        .map(|(i, l)| {
            let head = if i == 0 { "  ⎿  " } else { "     " };
            Line::from(vec![
                Span::styled(head, style),
                Span::styled(truncate(l, width.saturating_sub(6).max(10)), style),
            ])
        })
        .collect()
}

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
        // `/plan <task>`: a plan-first run.
        key(&mut fx.app, KeyCode::Esc);
        key(&mut fx.app, KeyCode::Char('n'));
        typed(&mut fx.app, "/plan add a docs page");
        key(&mut fx.app, KeyCode::Enter);
        let started = fx.calls.borrow().started.clone();
        assert_eq!(started.len(), 2, "{started:?}");
        assert_eq!(started[1][4..], ["--plan", "--", "add a docs page"]);
        key(&mut fx.app, KeyCode::Esc);
        key(&mut fx.app, KeyCode::Char('n'));
        typed(&mut fx.app, "/worktree /plan split it");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(
            fx.calls.borrow().started[2][4..],
            ["--worktree", "--plan", "--", "split it"]
        );
    }

    #[test]
    fn bang_runs_a_command_and_its_output_goes_with_the_next_message() {
        let mut fx = fixture(&[("sm", 1)], &[]);
        std::fs::write(fx.dir.path().join("marker.txt"), "x").unwrap();
        key(&mut fx.app, KeyCode::Char('n'));
        typed(&mut fx.app, "!ls marker.txt; echo oops >&2; false");
        key(&mut fx.app, KeyCode::Enter);
        assert!(fx.calls.borrow().started.is_empty());
        for _ in 0..200 {
            fx.app.poll_shell();
            if fx.app.shell[0].output.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let s = screen(&mut fx.app);
        assert!(s.contains("! ls marker.txt"), "{s}");
        assert!(s.contains("marker.txt") && s.contains("oops"), "{s}");
        typed(&mut fx.app, "why did it fail?");
        key(&mut fx.app, KeyCode::Enter);
        let started = fx.calls.borrow().started.clone();
        assert_eq!(
            started[0].last().unwrap(),
            "I ran `ls marker.txt; echo oops >&2; false` in the task folder:\n```\nmarker.txt\noops\n(exit 1)\n```\n\nwhy did it fail?"
        );
        // Sent once.
        assert!(fx.app.shell.is_empty());
    }

    #[test]
    fn clear_starts_a_new_session_in_the_same_folder() {
        let mut fx = fixture(
            &[("sm", 1)],
            &[("fix the tests", RunState::Done, Some("s1"))],
        );
        let cwd = fx.app.selected_run().unwrap().cwd.clone();
        key(&mut fx.app, KeyCode::Char('/'));
        typed(&mut fx.app, "/clear");
        key(&mut fx.app, KeyCode::Enter);
        assert!(fx.app.selected_run().is_none());
        assert_eq!(fx.app.folder.text, cwd);
        assert_eq!(fx.app.focus, Focus::Composer);
        // The new task goes to that folder (`/repo`, which the test machine doesn't have).
        typed(&mut fx.app, "next thing");
        key(&mut fx.app, KeyCode::Enter);
        assert!(fx.calls.borrow().started.is_empty());
        assert_eq!(fx.app.message, "no such folder: /repo");
    }

    #[test]
    fn slash_goal_starts_a_task_or_sets_the_sessions_goal() {
        let mut fx = fixture(&[("sm", 1)], &[]);
        key(&mut fx.app, KeyCode::Char('n'));
        typed(&mut fx.app, "/goal cargo test passes");
        key(&mut fx.app, KeyCode::Enter);
        let started = fx.calls.borrow().started.clone();
        assert_eq!(started[0][4..], ["--goal", "cargo test passes", "--", ""]);

        let mut fx = fixture(
            &[("sm", 1)],
            &[("fix the tests", RunState::Done, Some("s1"))],
        );
        key(&mut fx.app, KeyCode::Char('/'));
        typed(&mut fx.app, "/goal lint is clean");
        key(&mut fx.app, KeyCode::Enter);
        let started = fx.calls.borrow().started.clone();
        assert_eq!(
            started[0],
            [
                "run",
                "--json",
                "--follow-up",
                "1",
                "--goal",
                "lint is clean",
                "--",
                ""
            ]
        );
    }

    #[test]
    fn slash_review_reviews_the_selected_session() {
        let mut fx = fixture(
            &[("sm", 1)],
            &[("fix the tests", RunState::Done, Some("s1"))],
        );
        key(&mut fx.app, KeyCode::Char('/'));
        typed(&mut fx.app, "/review fix check the error paths");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(
            fx.calls.borrow().started[0],
            [
                "review",
                "1",
                "--json",
                "--fix",
                "--",
                "check the error paths"
            ]
        );
        typed(&mut fx.app, "/review");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.calls.borrow().started[1], ["review", "1", "--json"]);
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
    fn a_working_session_lists_its_subagents() {
        let mut fx = fixture(
            &[("sm", 1)],
            &[("list the files", RunState::Running, Some("s1"))],
        );
        // A real Claude run, cut while its subagent was on its Bash call.
        let log: String =
            include_str!("../../../fixtures/claude-2.1.296-subagent-20261012/subagent.jsonl")
                .lines()
                .take_while(|l| !l.contains("task_updated"))
                .map(|l| format!("{l}\n"))
                .collect();
        std::fs::write(fx.dir.path().join("runs/run-1.jsonl"), log).unwrap();
        fx.app.refresh().unwrap();
        let s = screen(&mut fx.app);
        assert!(s.contains("⏺ Agent(List files in working folder)"), "{s}");
        assert!(
            s.contains("▸ List files in working folder · 1 tool use"),
            "{s}"
        );
        assert!(s.contains("⎿  Bash(ls -la"), "{s}");
        // The subagent's own call isn't shown as the session's.
        assert!(!s.contains("⏺ Bash"), "{s}");
    }

    #[test]
    fn messages_to_a_running_session_are_queued_or_interrupt() {
        let mut fx = fixture(
            &[("sm", 1)],
            &[("long task", RunState::Running, Some("s1"))],
        );
        assert!(screen(&mut fx.app).contains("✻ Working…"));
        key(&mut fx.app, KeyCode::Char('/'));
        typed(&mut fx.app, "also update the docs");
        key(&mut fx.app, KeyCode::Enter);
        // No new run: the message waits for the working one.
        assert!(fx.calls.borrow().started.is_empty());
        assert!(fx.app.message.contains("queued"), "{}", fx.app.message);
        let s = screen(&mut fx.app);
        assert!(s.contains("> (queued) also update the docs"), "{s}");
        typed(&mut fx.app, "stop, wrong folder");
        fx.app
            .on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(fx.app.message.contains("stopping at the next clean point"));
        assert!(fx.app.store.interrupt_requested(1).unwrap());
        assert!(screen(&mut fx.app).contains("> (interrupting) stop, wrong folder"));
        assert_eq!(fx.app.store.queued_messages(1).unwrap().len(), 2);

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

        // p cycles the switch policy and saves it.
        assert!(screen(&mut fx.app).contains("Agents · drain"));
        key(&mut fx.app, KeyCode::Char('p'));
        assert_eq!(
            Config::load(&fx.app.config_path).unwrap().policy,
            policy::Policy::Balance
        );
        assert!(screen(&mut fx.app).contains("Agents · balance"));
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
    fn usage_panel_shows_tokens_cost_and_credits() {
        use anchorleg_core::adapters::{Credits, Usage};
        let mut fx = fixture(&[("sm", 1), ("two", 2)], &[]);
        let usage = Usage {
            model: Some("claude-haiku-5-5".into()),
            input_tokens: 1_000,
            output_tokens: 2_000,
            cache_read_tokens: 50_000,
            cache_write_tokens: 0,
            cost_usd: Some(1.25),
        };
        fx.app
            .store
            .record_usage(None, "sm", &usage, false, now())
            .unwrap();
        let paying = Credits {
            state: CreditState::InUse,
            balance: None,
            reason: None,
        };
        fx.app
            .store
            .record_credits("sm", &paying, anchorleg_core::store::Source::Stream, now())
            .unwrap();
        fx.app.refresh().unwrap();

        let top = screen(&mut fx.app);
        assert!(top.contains("24h 53.0k tokens · $1.25 API price"), "{top}");
        assert!(top.contains("sm paying from credits"), "{top}");
        assert!(top.contains("paying"), "{top}");

        key(&mut fx.app, KeyCode::Char('u'));
        let panel = screen(&mut fx.app);
        assert!(panel.contains("Last 7 days"), "{panel}");
        assert!(panel.contains("claude-haiku-5-5"), "{panel}");
        assert!(panel.contains("$1.25"), "{panel}");
        assert!(panel.contains("IN USE, paying"), "{panel}");
        assert!(panel.contains("not reported yet"), "{panel}");
        key(&mut fx.app, KeyCode::Char('x'));
        assert!(!fx.app.usage_panel);
    }

    #[test]
    fn agent_questions_are_answered_with_numbers_or_typed_text() {
        use anchorleg_core::questions::{Choice, Question, to_input};
        let mut fx = fixture(
            &[("sm", 1)],
            &[("migrate tests", RunState::Running, Some("s1"))],
        );
        let qs = vec![
            Question {
                id: "Which test runner?".into(),
                header: Some("Runner".into()),
                question: "Which test runner?".into(),
                options: vec![
                    Choice {
                        label: "Vitest".into(),
                        description: Some("fast, ESM".into()),
                    },
                    Choice {
                        label: "Jest".into(),
                        description: None,
                    },
                ],
                multi_select: false,
                secret: false,
            },
            Question {
                id: "Folders?".into(),
                header: None,
                question: "Which folders?".into(),
                options: ["src", "lib", "test"]
                    .map(|l| Choice {
                        label: l.into(),
                        description: None,
                    })
                    .to_vec(),
                multi_select: true,
                secret: false,
            },
            Question {
                id: "Anything else?".into(),
                header: None,
                question: "Anything else?".into(),
                options: vec![],
                multi_select: false,
                secret: false,
            },
        ];
        let id = fx
            .app
            .store
            .ask_permission(1, questions::TOOL, &to_input(&qs), None, now())
            .unwrap();
        fx.app.refresh().unwrap();
        fx.app.select_session(1);

        let s = screen(&mut fx.app);
        assert!(s.contains("Runner · Which test runner?"), "{s}");
        assert!(s.contains("(1/3)"), "{s}");
        assert!(s.contains("1 Vitest  fast, ESM"), "{s}");
        key(&mut fx.app, KeyCode::Char('1'));
        // Multi-select: tick two, confirm with Enter.
        assert!(screen(&mut fx.app).contains("[ ] lib"));
        key(&mut fx.app, KeyCode::Char('3'));
        key(&mut fx.app, KeyCode::Char('1'));
        assert!(screen(&mut fx.app).contains("[x] test"));
        key(&mut fx.app, KeyCode::Enter);
        // Free text through the message box.
        assert!(screen(&mut fx.app).contains("type your answer below"));
        fx.app.message_box.set("keep the jest config for now");
        fx.app.apply(Action::Send).unwrap();

        let p = fx.app.store.permission(id).unwrap().unwrap();
        assert_eq!(p.state, PermissionState::Allowed);
        let a = p.answers.unwrap();
        assert_eq!(a["Which test runner?"], ["Vitest"]);
        assert_eq!(a["Folders?"], ["src", "test"]);
        assert_eq!(a["Anything else?"], ["keep the jest config for now"]);
        assert!(
            fx.calls.borrow().started.is_empty(),
            "typing answered; it didn't send a reply"
        );
    }

    #[test]
    fn a_question_can_be_skipped() {
        use anchorleg_core::questions::{Question, to_input};
        let mut fx = fixture(&[("sm", 1)], &[("t", RunState::Running, Some("s1"))]);
        let q = Question {
            id: "q".into(),
            header: None,
            question: "Token?".into(),
            options: vec![],
            multi_select: false,
            secret: true,
        };
        let id = fx
            .app
            .store
            .ask_permission(1, questions::TOOL, &to_input(&[q]), None, now())
            .unwrap();
        fx.app.refresh().unwrap();
        fx.app.select_session(1);
        key(&mut fx.app, KeyCode::Char('0'));
        let p = fx.app.store.permission(id).unwrap().unwrap();
        assert_eq!(p.state, PermissionState::Denied);
    }

    #[test]
    fn task_list_and_changes_of_a_session() {
        let mut fx = fixture(&[("sm", 1)], &[]);
        // A session in a real repository, started (and its start saved) before the agent wrote.
        let repo = fx.dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .env("GIT_AUTHOR_NAME", "t")
                    .env("GIT_AUTHOR_EMAIL", "t@t")
                    .env("GIT_COMMITTER_NAME", "t")
                    .env("GIT_COMMITTER_EMAIL", "t@t")
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-q", "-m", "init"]);
        let id = fx
            .app
            .store
            .start_run("split the module", &repo.display().to_string(), None, 100)
            .unwrap();
        fx.app.store.set_run_account(id, "sm", Some("s1")).unwrap();
        fx.app.store.set_run_state(id, RunState::Done, 100).unwrap();
        anchorleg_core::checkpoint::save_start(&repo, id).unwrap();
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
        let todo = serde_json::json!({ "type": "assistant", "message": { "role": "assistant", "content": [
            { "type": "tool_use", "id": "t1", "name": "TodoWrite", "input": { "todos": [
                { "content": "Move the parser", "status": "completed" },
                { "content": "Update the imports", "status": "in_progress" },
                { "content": "Run the tests", "status": "pending" }
            ]}}
        ]}});
        let usage =
            r#"{"type":"assistant","message":{"usage":{"input_tokens":42000},"content":[]}}"#;
        let result = r#"{"type":"result","modelUsage":{"m":{"contextWindow":200000}}}"#;
        std::fs::write(
            fx.dir.path().join(format!("runs/run-{id}.jsonl")),
            format!("{todo}\n{usage}\n{result}\n"),
        )
        .unwrap();
        fx.app.refresh().unwrap();
        fx.app.select_session(1);

        let s = screen(&mut fx.app);
        assert!(s.contains("Tasks  1/3"), "{s}");
        assert!(s.contains("ctx 42k/200k (21%)"), "{s}");
        assert!(
            s.contains("✔ Move the parser") && s.contains("▸ Update the imports"),
            "{s}"
        );
        assert!(s.contains("◻ Run the tests"), "{s}");

        key(&mut fx.app, KeyCode::Char('v'));
        let s = screen(&mut fx.app);
        assert!(
            s.contains(&format!("Changes in #{id} since it started")),
            "{s}"
        );
        assert!(s.contains("+two"), "{s}");
        key(&mut fx.app, KeyCode::Char('x'));
        assert!(fx.app.diff_view.is_none());

        // Undo asks first, then puts the file back.
        key(&mut fx.app, KeyCode::Char('U'));
        assert!(
            fx.app.message.contains("Press U again"),
            "{}",
            fx.app.message
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );
        key(&mut fx.app, KeyCode::Char('U'));
        assert!(
            fx.app.message.contains("1 file(s) put back"),
            "{}",
            fx.app.message
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "one\n"
        );
        assert!(screen(&mut fx.app).contains("undid the session's changes"));
    }

    #[test]
    fn dropped_files_go_with_the_message() {
        let mut fx = fixture(
            &[("sm", 1)],
            &[
                ("done task", RunState::Done, Some("s1")),
                ("working task", RunState::Running, Some("s2")),
            ],
        );
        let file = fx.dir.path().join("crash report.txt");
        std::fs::write(&file, "trace").unwrap();
        let dropped = file.display().to_string().replace(' ', "\\ ");

        // A reply to a finished session: the file is passed with --attach, the path leaves the text.
        let done = fx
            .app
            .runs
            .iter()
            .position(|r| r.task == "done task")
            .unwrap()
            + 1;
        fx.app.select_session(done);
        fx.app
            .message_box
            .set(&format!("why does this crash {dropped}"));
        fx.app.apply(Action::Send).unwrap();
        let started = fx.calls.borrow().started[0].clone();
        let at = started.iter().position(|a| a == "--attach").unwrap();
        assert_eq!(started[at + 1], file.display().to_string());
        assert_eq!(started.last().unwrap(), "why does this crash");

        // To a working session: copied into its folder and named in the queued message.
        let folder = fx.dir.path().join("work");
        std::fs::create_dir_all(&folder).unwrap();
        let run_id = fx
            .app
            .store
            .start_run("busy", &folder.display().to_string(), None, 100)
            .unwrap();
        fx.app
            .store
            .set_run_account(run_id, "sm", Some("s3"))
            .unwrap();
        fx.app.store.set_run_pid(run_id, Some(4242)).unwrap();
        fx.app.refresh().unwrap();
        let working = fx.app.runs.iter().position(|r| r.id == run_id).unwrap() + 1;
        fx.app.select_session(working);
        fx.app.message_box.set(&dropped);
        fx.app.apply(Action::Send).unwrap();
        let queued = fx.app.store.queued_messages(run_id).unwrap();
        assert!(
            queued[0]
                .text
                .starts_with("Have a look at the attached files."),
            "{}",
            queued[0].text
        );
        assert!(
            queued[0].text.contains(".anchorleg/attachments/"),
            "{}",
            queued[0].text
        );
        assert!(
            folder
                .join(".anchorleg/attachments")
                .read_dir()
                .unwrap()
                .next()
                .is_some()
        );
    }

    #[test]
    fn at_completes_files_and_ctrl_p_recalls() {
        let mut fx = fixture(&[("sm", 1)], &[]);
        std::fs::create_dir_all(fx.dir.path().join("src")).unwrap();
        std::fs::write(fx.dir.path().join("src/parser.rs"), "").unwrap();
        std::fs::write(fx.dir.path().join("src/sparse.rs"), "").unwrap();
        key(&mut fx.app, KeyCode::Char('n'));
        typed(&mut fx.app, "look at @pars");
        let s = screen(&mut fx.app);
        assert!(s.contains("@src/parser.rs"), "{s}");
        // A name that starts with it comes first.
        assert_eq!(fx.app.suggestions()[0].label, "@src/parser.rs");
        key(&mut fx.app, KeyCode::Tab);
        assert_eq!(fx.app.message_box.text, "look at @src/parser.rs ");
        key(&mut fx.app, KeyCode::Enter);

        // Ctrl+P brings back what was sent; Ctrl+N goes forward to an empty box.
        key(&mut fx.app, KeyCode::Esc);
        key(&mut fx.app, KeyCode::Char('n'));
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        fx.app.on_key(ctrl('p')).unwrap();
        assert_eq!(fx.app.message_box.text, "look at @src/parser.rs");
        fx.app.on_key(ctrl('n')).unwrap();
        assert_eq!(fx.app.message_box.text, "");
    }

    #[test]
    fn fork_from_the_composer() {
        let mut fx = fixture(&[("sm", 1)], &[("first task", RunState::Done, Some("s1"))]);
        fx.app.select_session(1);
        key(&mut fx.app, KeyCode::Char('/'));
        typed(&mut fx.app, "/fork try it with a macro instead");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.message, "forked #1");
        assert_eq!(
            fx.calls.borrow().started[0],
            [
                "run",
                "--json",
                "--fork",
                "1",
                "--",
                "try it with a macro instead"
            ]
        );
    }

    #[test]
    fn mode_per_session() {
        let mut fx = fixture(&[("sm", 1)], &[("first task", RunState::Done, Some("s1"))]);
        // No session: the next new task's mode.
        key(&mut fx.app, KeyCode::Char('n'));
        typed(&mut fx.app, "/mode edits");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.message, "new tasks start in edits mode");
        typed(&mut fx.app, "fix it");
        key(&mut fx.app, KeyCode::Enter);
        let started = fx.calls.borrow().started[0].clone();
        assert_eq!(started[4..6], ["--mode", "edits"], "{started:?}");
        // A session: stored on it, shown in its title, inherited by its follow-ups.
        key(&mut fx.app, KeyCode::Esc);
        fx.app.select_session(1);
        key(&mut fx.app, KeyCode::Char('/'));
        typed(&mut fx.app, "/mode ask");
        key(&mut fx.app, KeyCode::Enter);
        assert!(fx.app.message.contains("mode → ask"), "{}", fx.app.message);
        assert_eq!(
            fx.app.store.run(1).unwrap().unwrap().mode.as_deref(),
            Some("ask")
        );
        assert!(screen(&mut fx.app).contains("· mode ask"));
        typed(&mut fx.app, "/mode yolo");
        key(&mut fx.app, KeyCode::Enter);
        assert_eq!(fx.app.message, "yolo: one of ask, edits");
    }

    #[test]
    fn context_meters() {
        assert_eq!(context_meter(42_003, Some(200_000)), "ctx 42k/200k (21%)");
        assert_eq!(context_meter(150_000, Some(1_000_000)), "ctx 150k/1M (15%)");
        assert_eq!(context_meter(900, None), "ctx 900");
        assert_eq!(context_meter(1_260_000, Some(0)), "ctx 1.3M");
    }

    #[test]
    fn wrap_breaks_on_words() {
        assert_eq!(wrap("aaa bbb ccc", 7), ["aaa bbb", "ccc"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("a\nb", 10), ["a", "b"]);
    }
}
