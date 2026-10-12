//! Local state: the latest quota reading per account and window, runs, and the decision log.
//!
//! SQLite in WAL mode, so several `anchorleg` processes (parallel runs, the mod's `anchorleg report`)
//! can share one file. Holds account names only, never tokens.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::adapters::{CreditState, Credits, EpochSecs, QuotaReading, Usage};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("creating {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("no home directory; set ANCHORLEG_HOME")]
    NoHome,
    #[error(
        "the database is from a newer anchorleg (schema {found}, this version knows {known}); \
         update anchorleg, or use the one that upgraded it"
    )]
    Newer { found: i64, known: i64 },
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Where a quota reading came from. `Mod` readings are exact; `Stream` ones are often partial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Stream,
    Mod,
    Manual,
    /// Asked of the CLI between launches (Codex `app-server`).
    Probe,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stream => "stream",
            Self::Mod => "mod",
            Self::Manual => "manual",
            Self::Probe => "probe",
        }
    }
}

/// Window name stored when a reading doesn't say which window it's about.
pub const UNKNOWN_WINDOW: &str = "unknown";

/// The latest known quota for one account and window.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuotaRow {
    pub account: String,
    pub window: String,
    pub status: Option<String>,
    /// Fraction used, 0.0–1.0.
    pub used: Option<f64>,
    pub resets_at: Option<EpochSecs>,
    pub source: String,
    pub updated_at: EpochSecs,
}

/// A message the person sent to a running session, waiting to reach the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueuedMessage {
    pub id: i64,
    pub run_id: RunId,
    pub text: String,
    /// Stop the agent at its next clean point instead of waiting for its turn to end.
    pub interrupt: bool,
    pub created_at: EpochSecs,
}

/// The latest known usage-credit state for one account.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CreditsRow {
    pub account: String,
    pub state: CreditState,
    pub balance: Option<f64>,
    pub reason: Option<String>,
    pub updated_at: EpochSecs,
}

/// Tokens and cost added up over a span of time, for one account and model.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UsageTotal {
    pub account: String,
    /// `None` when the CLI didn't name it.
    pub model: Option<String>,
    /// Launches that reported usage.
    pub launches: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// List-price cost, summed over launches that reported one; `None` when none did.
    pub cost_usd: Option<f64>,
    /// The part of `cost_usd` that ran while the account was paying from credits.
    pub credits_cost_usd: Option<f64>,
}

/// A tool call waiting for the person, and how they answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Pending,
    /// Allowed this once.
    Allowed,
    /// Allowed, and the same tool is allowed for the rest of the session.
    Always,
    Denied,
    /// Nobody answered in time, or the run ended first.
    Expired,
}

impl PermissionState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Allowed => "allowed",
            Self::Always => "always",
            Self::Denied => "denied",
            Self::Expired => "expired",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "allowed" => Self::Allowed,
            "always" => Self::Always,
            "denied" => Self::Denied,
            _ => Self::Expired,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Permission {
    pub id: i64,
    pub run_id: RunId,
    pub tool: String,
    pub input: serde_json::Value,
    /// The CLI's own words for why it asks.
    pub reason: Option<String>,
    pub state: PermissionState,
    pub asked_at: EpochSecs,
    pub answered_at: Option<EpochSecs>,
    /// For a question ([`crate::questions::TOOL`]): the person's answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answers: Option<crate::questions::Answers>,
}

impl Permission {
    /// A question from the agent rather than a tool call to approve.
    pub fn is_question(&self) -> bool {
        self.tool == crate::questions::TOOL
    }
}

const PERMISSION_COLUMNS: &str =
    "id, run_id, tool, input, reason, state, asked_at, answered_at, answer";

fn permission_from_row(r: &rusqlite::Row) -> rusqlite::Result<Permission> {
    let input: String = r.get(3)?;
    let state: String = r.get(5)?;
    Ok(Permission {
        id: r.get(0)?,
        run_id: r.get(1)?,
        tool: r.get(2)?,
        input: serde_json::from_str(&input).unwrap_or(serde_json::Value::Null),
        reason: r.get(4)?,
        state: PermissionState::parse(&state),
        asked_at: r.get(6)?,
        answered_at: r.get(7)?,
        answers: r
            .get::<_, Option<String>>(8)?
            .and_then(|a| serde_json::from_str(&a).ok()),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Switching,
    Waiting,
    Done,
    Failed,
    /// Stopped by the user (`anchorleg stop`, Ctrl-C).
    Stopped,
}

impl RunState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Switching => "switching",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "switching" => Self::Switching,
            "waiting" => Self::Waiting,
            "done" => Self::Done,
            "stopped" => Self::Stopped,
            _ => Self::Failed,
        }
    }
}

pub type RunId = i64;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Run {
    pub id: RunId,
    pub task: String,
    pub cwd: String,
    pub account: Option<String>,
    pub session_id: Option<String>,
    pub state: RunState,
    pub started_at: EpochSecs,
    pub ended_at: Option<EpochSecs>,
    /// The run this one follows up (a message sent to an existing session).
    pub parent_id: Option<RunId>,
    /// The `anchorleg run` process working on it, while it runs.
    pub pid: Option<i64>,
}

const RUN_COLUMNS: &str =
    "id, task, cwd, account, session_id, state, started_at, ended_at, parent_id, pid";

fn run_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: r.get(0)?,
        task: r.get(1)?,
        cwd: r.get(2)?,
        account: r.get(3)?,
        session_id: r.get(4)?,
        state: RunState::parse(&r.get::<_, String>(5)?),
        started_at: r.get(6)?,
        ended_at: r.get(7)?,
        parent_id: r.get(8)?,
        pid: r.get(9)?,
    })
}

/// One entry in the decision log: why anchorleg switched, waited or blocked an account.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Decision {
    pub run_id: Option<RunId>,
    pub at: EpochSecs,
    pub from_account: Option<String>,
    pub to_account: Option<String>,
    /// Which switch rule fired, e.g. `rejected`, `hard_95`, `all_blocked`.
    pub rule: String,
    pub detail: Value,
}

/// Schema changes, applied in order. Index + 1 is the `user_version` after applying it.
/// Never edit an entry that has shipped; append a new one.
const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE quota (
    account    TEXT NOT NULL,
    window     TEXT NOT NULL,
    status     TEXT,
    used       REAL,
    resets_at  INTEGER,
    source     TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (account, window)
);
CREATE TABLE runs (
    id         INTEGER PRIMARY KEY,
    task       TEXT NOT NULL,
    cwd        TEXT NOT NULL,
    account    TEXT,
    session_id TEXT,
    state      TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at   INTEGER
);
CREATE TABLE decisions (
    id           INTEGER PRIMARY KEY,
    run_id       INTEGER REFERENCES runs(id),
    at           INTEGER NOT NULL,
    from_account TEXT,
    to_account   TEXT,
    rule         TEXT NOT NULL,
    detail       TEXT NOT NULL
);
CREATE INDEX decisions_run ON decisions(run_id);
"#,
    r#"
ALTER TABLE runs ADD COLUMN stop_reason TEXT;
"#,
    r#"
ALTER TABLE runs ADD COLUMN parent_id INTEGER REFERENCES runs(id);
ALTER TABLE runs ADD COLUMN pid INTEGER;
"#,
    r#"
CREATE TABLE permissions (
    id          INTEGER PRIMARY KEY,
    run_id      INTEGER NOT NULL REFERENCES runs(id),
    tool        TEXT NOT NULL,
    input       TEXT NOT NULL,
    reason      TEXT,
    state       TEXT NOT NULL,
    asked_at    INTEGER NOT NULL,
    answered_at INTEGER
);
CREATE INDEX permissions_run ON permissions(run_id);
"#,
    r#"
CREATE TABLE credits (
    account    TEXT PRIMARY KEY,
    state      TEXT NOT NULL,
    balance    REAL,
    reason     TEXT,
    source     TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE usage (
    id                 INTEGER PRIMARY KEY,
    run_id             INTEGER REFERENCES runs(id),
    account            TEXT NOT NULL,
    model              TEXT,
    at                 INTEGER NOT NULL,
    input_tokens       INTEGER NOT NULL,
    output_tokens      INTEGER NOT NULL,
    cache_read_tokens  INTEGER NOT NULL,
    cache_write_tokens INTEGER NOT NULL,
    cost_usd           REAL,
    on_credits         INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX usage_at ON usage(at);
"#,
    r#"
ALTER TABLE permissions ADD COLUMN answer TEXT;
"#,
    r#"
CREATE TABLE messages (
    id         INTEGER PRIMARY KEY,
    run_id     INTEGER NOT NULL REFERENCES runs(id),
    text       TEXT NOT NULL,
    interrupt  INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    taken_at   INTEGER
);
CREATE INDEX messages_run ON messages(run_id);
"#,
];

pub struct Store {
    conn: Connection,
}

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

impl Store {
    /// `$ANCHORLEG_HOME/anchorleg.db`, or `~/Library/Application Support/anchorleg/anchorleg.db` on macOS.
    pub fn default_path() -> Result<PathBuf> {
        let dir = match std::env::var_os("ANCHORLEG_HOME") {
            Some(home) => PathBuf::from(home),
            None => {
                let data = dirs::data_dir().ok_or(StoreError::NoHome)?;
                let dir = data.join("anchorleg");
                crate::adopt_legacy_dir(
                    &data.join("relay"),
                    &dir,
                    &[
                        ("relay.db", "anchorleg.db"),
                        ("relay.db-wal", "anchorleg.db-wal"),
                        ("relay.db-shm", "anchorleg.db-shm"),
                    ],
                );
                dir
            }
        };
        Ok(dir.join("anchorleg.db"))
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| StoreError::CreateDir {
                path: dir.to_owned(),
                source,
            })?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // Several anchorleg processes (a run, the UI, the mod's reports) may open a new database at
        // the same moment. Switching to WAL needs an exclusive lock and can fail at once with
        // "database is locked" instead of waiting, so it's retried.
        let mut tries = 0;
        loop {
            match conn.pragma_update(None, "journal_mode", "WAL") {
                Ok(()) => break,
                Err(e) if is_busy(&e) && tries < 50 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) => return Err(e.into()),
            }
        }
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // IMMEDIATE takes the write lock up front (waiting via busy_timeout), and the version
        // is read inside it, so two processes never run the same migration.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        // An older binary (say a CLI next to the desktop app's newer one) must not write rows
        // a newer schema doesn't expect.
        let known = MIGRATIONS.len() as i64;
        if version > known {
            return Err(StoreError::Newer {
                found: version,
                known,
            });
        }
        let mut next = version;
        for sql in MIGRATIONS
            .get(usize::try_from(version).unwrap_or(0)..)
            .unwrap_or_default()
        {
            tx.execute_batch(sql)?;
            next += 1;
            tx.pragma_update(None, "user_version", next)?;
        }
        tx.commit()?;
        Ok(Self { conn })
    }

    /// Merge a reading into the latest state for its account and window.
    ///
    /// Replace everything known about an account's quota with a complete, current snapshot:
    /// windows it no longer lists (e.g. an `unknown` one from a limit hit) are dropped.
    pub fn replace_quota(
        &self,
        account: &str,
        readings: &[QuotaReading],
        source: Source,
        now: EpochSecs,
    ) -> Result<()> {
        self.conn
            .execute("DELETE FROM quota WHERE account = ?1", params![account])?;
        for r in readings {
            self.record_quota(account, r, source, now)?;
        }
        Ok(())
    }

    /// Missing fields keep their previous value, because CLIs often send partial readings. The
    /// exception: when `resets_at` moves, the window rolled over, so the old `used` is dropped.
    pub fn record_quota(
        &self,
        account: &str,
        reading: &QuotaReading,
        source: Source,
        now: EpochSecs,
    ) -> Result<()> {
        let window = reading.window.as_ref().map_or(UNKNOWN_WINDOW, |w| w.name());
        let status = reading.status.as_ref().map(|s| s.name());
        self.conn.execute(
            "INSERT INTO quota (account, window, status, used, resets_at, source, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (account, window) DO UPDATE SET
               used = CASE
                 WHEN excluded.used IS NOT NULL THEN excluded.used
                 WHEN excluded.resets_at IS NOT NULL AND excluded.resets_at IS NOT quota.resets_at
                   THEN NULL
                 ELSE quota.used END,
               status     = COALESCE(excluded.status, quota.status),
               resets_at  = COALESCE(excluded.resets_at, quota.resets_at),
               source     = excluded.source,
               updated_at = excluded.updated_at",
            params![
                account,
                window,
                status,
                reading.used,
                reading.resets_at,
                source.as_str(),
                now
            ],
        )?;
        Ok(())
    }

    /// Latest quota rows, for every account when `account` is `None`.
    pub fn quota(&self, account: Option<&str>) -> Result<Vec<QuotaRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT account, window, status, used, resets_at, source, updated_at FROM quota
             WHERE ?1 IS NULL OR account = ?1 ORDER BY account, window",
        )?;
        let rows = stmt.query_map(params![account], |r| {
            Ok(QuotaRow {
                account: r.get(0)?,
                window: r.get(1)?,
                status: r.get(2)?,
                used: r.get(3)?,
                resets_at: r.get(4)?,
                source: r.get(5)?,
                updated_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Replace what's known about an account's usage credits.
    pub fn record_credits(
        &self,
        account: &str,
        credits: &Credits,
        source: Source,
        now: EpochSecs,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO credits (account, state, balance, reason, source, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                account,
                credits.state.name(),
                credits.balance,
                credits.reason,
                source.as_str(),
                now
            ],
        )?;
        Ok(())
    }

    /// Usage-credit state, for every account when `account` is `None`.
    pub fn credits(&self, account: Option<&str>) -> Result<Vec<CreditsRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT account, state, balance, reason, updated_at FROM credits
             WHERE ?1 IS NULL OR account = ?1 ORDER BY account",
        )?;
        let rows = stmt.query_map(params![account], |r| {
            Ok(CreditsRow {
                account: r.get(0)?,
                state: CreditState::from_name(&r.get::<_, String>(1)?),
                balance: r.get(2)?,
                reason: r.get(3)?,
                updated_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Add one launch's tokens and cost.
    pub fn record_usage(
        &self,
        run_id: Option<RunId>,
        account: &str,
        usage: &Usage,
        on_credits: bool,
        now: EpochSecs,
    ) -> Result<()> {
        let n = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
        self.conn.execute(
            "INSERT INTO usage (run_id, account, model, at, input_tokens, output_tokens,
               cache_read_tokens, cache_write_tokens, cost_usd, on_credits)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                run_id,
                account,
                usage.model,
                now,
                n(usage.input_tokens),
                n(usage.output_tokens),
                n(usage.cache_read_tokens),
                n(usage.cache_write_tokens),
                usage.cost_usd,
                on_credits
            ],
        )?;
        Ok(())
    }

    /// Usage since `since`, per account and model, most expensive (then most tokens) first.
    pub fn usage_totals(&self, since: EpochSecs) -> Result<Vec<UsageTotal>> {
        let mut stmt = self.conn.prepare(
            "SELECT account, model, COUNT(*), SUM(input_tokens), SUM(output_tokens),
                    SUM(cache_read_tokens), SUM(cache_write_tokens), SUM(cost_usd),
                    SUM(CASE WHEN on_credits THEN cost_usd END)
             FROM usage WHERE at >= ?1 GROUP BY account, model
             ORDER BY COALESCE(SUM(cost_usd), 0) DESC,
                      SUM(input_tokens + output_tokens + cache_read_tokens + cache_write_tokens) DESC,
                      account",
        )?;
        let n = |v: i64| u64::try_from(v).unwrap_or(0);
        let rows = stmt.query_map(params![since], |r| {
            Ok(UsageTotal {
                account: r.get(0)?,
                model: r.get(1)?,
                launches: n(r.get(2)?),
                input_tokens: n(r.get(3)?),
                output_tokens: n(r.get(4)?),
                cache_read_tokens: n(r.get(5)?),
                cache_write_tokens: n(r.get(6)?),
                cost_usd: r.get(7)?,
                credits_cost_usd: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Tokens per day since `since` (UTC days, as epoch seconds of the day's start), for a chart.
    pub fn usage_by_day(&self, since: EpochSecs) -> Result<Vec<(EpochSecs, u64, f64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT (at / 86400) * 86400 AS day,
                    SUM(input_tokens + output_tokens + cache_read_tokens + cache_write_tokens),
                    COALESCE(SUM(cost_usd), 0)
             FROM usage WHERE at >= ?1 GROUP BY day ORDER BY day",
        )?;
        let rows = stmt.query_map(params![since], |r| {
            Ok((
                r.get(0)?,
                u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
                r.get(2)?,
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn start_run(
        &self,
        task: &str,
        cwd: &str,
        parent: Option<RunId>,
        now: EpochSecs,
    ) -> Result<RunId> {
        self.conn.execute(
            "INSERT INTO runs (task, cwd, state, started_at, parent_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![task, cwd, RunState::Running.as_str(), now, parent],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Record the process working on a run; `None` once it's gone.
    pub fn set_run_pid(&self, id: RunId, pid: Option<i64>) -> Result<()> {
        self.conn
            .execute("UPDATE runs SET pid = ?2 WHERE id = ?1", params![id, pid])?;
        Ok(())
    }

    /// Point a run at the account and session it's on now.
    pub fn set_run_account(
        &self,
        id: RunId,
        account: &str,
        session_id: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET account = ?2, session_id = COALESCE(?3, session_id) WHERE id = ?1",
            params![id, account, session_id],
        )?;
        Ok(())
    }

    /// Set the run's state; `Done` and `Failed` also set `ended_at`.
    pub fn set_run_state(&self, id: RunId, state: RunState, now: EpochSecs) -> Result<()> {
        let ended =
            matches!(state, RunState::Done | RunState::Failed | RunState::Stopped).then_some(now);
        self.conn.execute(
            "UPDATE runs SET state = ?2, ended_at = COALESCE(?3, ended_at) WHERE id = ?1",
            params![id, state.as_str(), ended],
        )?;
        Ok(())
    }

    pub fn run(&self, id: RunId) -> Result<Option<Run>> {
        let run = self
            .conn
            .query_row(
                &format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1"),
                params![id],
                run_from_row,
            )
            .optional()?;
        Ok(run)
    }

    /// anchorleg-mod asks to stop this run at a clean point (Phase 2). Errors if the run is unknown.
    pub fn request_stop(&self, id: RunId, reason: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE runs SET stop_reason = ?2 WHERE id = ?1",
            params![id, reason],
        )?;
        Ok(n == 1)
    }

    /// The pending stop request for this run, cleared as it's read.
    pub fn take_stop(&self, id: RunId) -> Result<Option<String>> {
        let reason: Option<String> = self
            .conn
            .query_row(
                "SELECT stop_reason FROM runs WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if reason.is_some() {
            self.conn.execute(
                "UPDATE runs SET stop_reason = NULL WHERE id = ?1",
                params![id],
            )?;
        }
        Ok(reason)
    }

    /// Record a tool call that needs the person's answer.
    pub fn ask_permission(
        &self,
        run_id: RunId,
        tool: &str,
        input: &serde_json::Value,
        reason: Option<&str>,
        now: EpochSecs,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO permissions (run_id, tool, input, reason, state, asked_at)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
            params![run_id, tool, input.to_string(), reason, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn permission(&self, id: i64) -> Result<Option<Permission>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {PERMISSION_COLUMNS} FROM permissions WHERE id = ?1"),
                params![id],
                permission_from_row,
            )
            .optional()?)
    }

    /// Every unanswered request, oldest first.
    pub fn pending_permissions(&self) -> Result<Vec<Permission>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PERMISSION_COLUMNS} FROM permissions WHERE state = 'pending' ORDER BY id"
        ))?;
        let rows = stmt.query_map([], permission_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Answer a pending request. `false` if it was already answered (or doesn't exist).
    pub fn answer_permission(
        &self,
        id: i64,
        state: PermissionState,
        now: EpochSecs,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE permissions SET state = ?2, answered_at = ?3 WHERE id = ?1 AND state = 'pending'",
            params![id, state.as_str(), now],
        )?;
        Ok(n == 1)
    }

    /// Answer a pending question. `false` if it was already answered (or doesn't exist).
    pub fn answer_question(
        &self,
        id: i64,
        answers: &crate::questions::Answers,
        now: EpochSecs,
    ) -> Result<bool> {
        let text = serde_json::to_string(answers).unwrap_or_else(|_| "{}".to_owned());
        let n = self.conn.execute(
            "UPDATE permissions SET state = 'allowed', answer = ?2, answered_at = ?3
             WHERE id = ?1 AND state = 'pending'",
            params![id, text, now],
        )?;
        Ok(n == 1)
    }

    /// Queue a message for a running session.
    pub fn queue_message(
        &self,
        run_id: RunId,
        text: &str,
        interrupt: bool,
        now: EpochSecs,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO messages (run_id, text, interrupt, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![run_id, text, interrupt, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Messages for this run that haven't reached the agent yet, oldest first.
    pub fn queued_messages(&self, run_id: RunId) -> Result<Vec<QueuedMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, run_id, text, interrupt, created_at FROM messages
             WHERE run_id = ?1 AND taken_at IS NULL ORDER BY id",
        )?;
        let rows = stmt.query_map(params![run_id], |r| {
            Ok(QueuedMessage {
                id: r.get(0)?,
                run_id: r.get(1)?,
                text: r.get(2)?,
                interrupt: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Mark a message delivered. `false` if it already was.
    pub fn take_message(&self, id: i64, now: EpochSecs) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE messages SET taken_at = ?2 WHERE id = ?1 AND taken_at IS NULL",
            params![id, now],
        )?;
        Ok(n == 1)
    }

    /// Whether the person asked this run to stop at its next clean point for a new message.
    pub fn interrupt_requested(&self, run_id: RunId) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM messages WHERE run_id = ?1 AND taken_at IS NULL AND interrupt = 1",
                params![run_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Whether the person said "always" to `tool` in any of these runs (one session's runs).
    pub fn always_allowed(&self, runs: &[RunId], tool: &str) -> Result<bool> {
        for run in runs {
            let hit: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM permissions WHERE run_id = ?1 AND tool = ?2 AND state = 'always'",
                    params![run, tool],
                    |r| r.get(0),
                )
                .optional()?;
            if hit.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// This run and the runs it follows up, newest first: one session.
    pub fn session_runs(&self, id: RunId) -> Result<Vec<RunId>> {
        let mut out = Vec::new();
        let mut next = Some(id);
        while let Some(id) = next {
            let Some(run) = self.run(id)? else { break };
            out.push(id);
            next = run.parent_id.filter(|p| !out.contains(p));
        }
        Ok(out)
    }

    /// The most recent runs, newest first.
    pub fn recent_runs(&self, limit: usize) -> Result<Vec<Run>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM runs ORDER BY id DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map(
            params![i64::try_from(limit).unwrap_or(i64::MAX)],
            run_from_row,
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn record_decision(&self, d: &Decision) -> Result<()> {
        self.conn.execute(
            "INSERT INTO decisions (run_id, at, from_account, to_account, rule, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                d.run_id,
                d.at,
                d.from_account,
                d.to_account,
                d.rule,
                d.detail.to_string()
            ],
        )?;
        Ok(())
    }

    /// Decisions in the order they were made, for one run or for all runs.
    pub fn decisions(&self, run_id: Option<RunId>) -> Result<Vec<Decision>> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, at, from_account, to_account, rule, detail FROM decisions
             WHERE ?1 IS NULL OR run_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![run_id], |r| {
            let detail: String = r.get(5)?;
            Ok(Decision {
                run_id: r.get(0)?,
                at: r.get(1)?,
                from_account: r.get(2)?,
                to_account: r.get(3)?,
                rule: r.get(4)?,
                detail: serde_json::from_str(&detail).unwrap_or(Value::String(detail)),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{QuotaStatus, Window};

    fn reading(
        window: Window,
        status: Option<QuotaStatus>,
        used: Option<f64>,
        resets_at: Option<EpochSecs>,
    ) -> QuotaReading {
        QuotaReading {
            window: Some(window),
            status,
            used,
            resets_at,
        }
    }

    #[test]
    fn partial_reading_keeps_known_values() {
        let s = Store::open_in_memory().unwrap();
        let full = reading(
            Window::FiveHour,
            Some(QuotaStatus::Warning),
            Some(0.91),
            Some(1000),
        );
        s.record_quota("sm", &full, Source::Mod, 10).unwrap();
        // Claude's usual "allowed" event: status only.
        let partial = reading(Window::FiveHour, Some(QuotaStatus::Allowed), None, None);
        s.record_quota("sm", &partial, Source::Stream, 20).unwrap();

        let row = &s.quota(Some("sm")).unwrap()[0];
        assert_eq!(row.status.as_deref(), Some("allowed"));
        assert_eq!(row.used, Some(0.91));
        assert_eq!(row.resets_at, Some(1000));
        assert_eq!(row.source, "stream");
        assert_eq!(row.updated_at, 20);
    }

    #[test]
    fn new_reset_time_drops_stale_usage() {
        let s = Store::open_in_memory().unwrap();
        let old = reading(Window::FiveHour, None, Some(0.97), Some(1000));
        s.record_quota("sm", &old, Source::Mod, 10).unwrap();
        let rolled = reading(
            Window::FiveHour,
            Some(QuotaStatus::Allowed),
            None,
            Some(19000),
        );
        s.record_quota("sm", &rolled, Source::Stream, 1100).unwrap();

        let row = &s.quota(Some("sm")).unwrap()[0];
        assert_eq!(row.used, None);
        assert_eq!(row.resets_at, Some(19000));
    }

    #[test]
    fn rejected_reading_is_stored() {
        let s = Store::open_in_memory().unwrap();
        let hit = reading(
            Window::FiveHour,
            Some(QuotaStatus::Rejected),
            None,
            Some(1000),
        );
        s.record_quota("sm", &hit, Source::Stream, 10).unwrap();
        let row = &s.quota(Some("sm")).unwrap()[0];
        assert_eq!(row.status.as_deref(), Some("rejected"));
        assert_eq!(row.resets_at, Some(1000));
    }

    #[test]
    fn reading_without_window_is_stored_as_unknown() {
        let s = Store::open_in_memory().unwrap();
        let r = QuotaReading {
            window: None,
            status: Some(QuotaStatus::Rejected),
            used: None,
            resets_at: None,
        };
        s.record_quota("two", &r, Source::Stream, 5).unwrap();
        let rows = s.quota(None).unwrap();
        assert_eq!(rows[0].window, UNKNOWN_WINDOW);
        assert_eq!(rows[0].status.as_deref(), Some("rejected"));
    }

    #[test]
    fn run_lifecycle_and_decisions() {
        let s = Store::open_in_memory().unwrap();
        let id = s.start_run("fix the tests", "/repo", None, 100).unwrap();
        s.set_run_pid(id, Some(4242)).unwrap();
        assert_eq!(s.run(id).unwrap().unwrap().pid, Some(4242));
        let child = s.start_run("now add docs", "/repo", Some(id), 300).unwrap();
        assert_eq!(s.run(child).unwrap().unwrap().parent_id, Some(id));
        s.set_run_pid(id, None).unwrap();
        s.set_run_account(id, "sm", Some("sess-1")).unwrap();
        s.record_decision(&Decision {
            run_id: Some(id),
            at: 150,
            from_account: Some("sm".into()),
            to_account: Some("two".into()),
            rule: "rejected".into(),
            detail: serde_json::json!({ "resets_at": 1000 }),
        })
        .unwrap();
        // Switching keeps the session id when the new launch hasn't reported one yet.
        s.set_run_account(id, "two", None).unwrap();
        s.set_run_state(id, RunState::Done, 200).unwrap();

        let run = s.run(id).unwrap().unwrap();
        assert_eq!(s.recent_runs(5).unwrap()[1], run.clone());
        assert_eq!(run.account.as_deref(), Some("two"));
        assert_eq!(run.session_id.as_deref(), Some("sess-1"));
        assert_eq!(run.state, RunState::Done);
        assert_eq!(run.ended_at, Some(200));

        assert!(s.request_stop(id, "quota at 91%").unwrap());
        assert!(!s.request_stop(9999, "x").unwrap());
        assert_eq!(s.take_stop(id).unwrap().as_deref(), Some("quota at 91%"));
        assert_eq!(s.take_stop(id).unwrap(), None);

        let log = s.decisions(Some(id)).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].detail["resets_at"], 1000);
    }

    #[test]
    fn reopening_a_file_keeps_data_and_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/anchorleg.db");
        {
            let s = Store::open(&path).unwrap();
            let r = reading(Window::SevenDay, None, Some(0.4), None);
            s.record_quota("sm", &r, Source::Mod, 1).unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.quota(None).unwrap().len(), 1);
        let mode: String = s
            .conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("anchorleg.db");
        drop(Store::open(&path).unwrap());
        let newer = MIGRATIONS.len() as i64 + 1;
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", newer)
            .unwrap();
        let err = Store::open(&path).err().expect("refused");
        assert!(
            matches!(err, StoreError::Newer { found, .. } if found == newer),
            "{err}"
        );
    }

    #[test]
    fn many_processes_can_open_a_new_database_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("anchorleg.db");
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let store = Store::open(&path).unwrap();
                    store.recent_runs(5).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn permissions_wait_for_an_answer_and_always_covers_the_session() {
        let s = Store::open_in_memory().unwrap();
        let first = s.start_run("task", "/r", None, 1).unwrap();
        let reply = s.start_run("more", "/r", Some(first), 2).unwrap();
        let other = s.start_run("other", "/r", None, 3).unwrap();
        assert_eq!(s.session_runs(reply).unwrap(), [reply, first]);

        let input = serde_json::json!({ "file_path": "/r/a.txt" });
        let id = s
            .ask_permission(first, "Write", &input, Some("why"), 10)
            .unwrap();
        assert_eq!(s.pending_permissions().unwrap().len(), 1);
        assert!(
            s.answer_permission(id, PermissionState::Always, 11)
                .unwrap()
        );
        assert!(
            !s.answer_permission(id, PermissionState::Denied, 12)
                .unwrap()
        );
        let p = s.permission(id).unwrap().unwrap();
        assert_eq!(
            (p.state, p.answered_at, p.input),
            (PermissionState::Always, Some(11), input)
        );
        assert!(s.pending_permissions().unwrap().is_empty());

        assert!(
            s.always_allowed(&s.session_runs(reply).unwrap(), "Write")
                .unwrap()
        );
        assert!(
            !s.always_allowed(&s.session_runs(reply).unwrap(), "Bash")
                .unwrap()
        );
        assert!(
            !s.always_allowed(&s.session_runs(other).unwrap(), "Write")
                .unwrap()
        );
    }

    #[test]
    fn credits_and_usage_are_kept() {
        let s = Store::open_in_memory().unwrap();
        let c = Credits {
            state: CreditState::Available,
            balance: Some(12.5),
            reason: None,
        };
        s.record_credits("a", &c, Source::Probe, 10).unwrap();
        let off = Credits {
            state: CreditState::Off,
            balance: None,
            reason: Some("out_of_credits".into()),
        };
        s.record_credits("a", &off, Source::Stream, 20).unwrap();
        let rows = s.credits(None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, CreditState::Off);
        assert_eq!(rows[0].reason.as_deref(), Some("out_of_credits"));

        let run = s.start_run("t", "/w", None, 100).unwrap();
        let u = |model: &str, cost: Option<f64>| Usage {
            model: Some(model.into()),
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 100,
            cache_write_tokens: 1,
            cost_usd: cost,
        };
        s.record_usage(Some(run), "a", &u("haiku", Some(0.5)), false, 100)
            .unwrap();
        s.record_usage(Some(run), "a", &u("haiku", Some(0.25)), true, 200)
            .unwrap();
        s.record_usage(Some(run), "b", &u("gpt", None), false, 300)
            .unwrap();
        s.record_usage(Some(run), "a", &u("haiku", Some(9.0)), false, 10)
            .unwrap(); // too old

        let t = s.usage_totals(50).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].account.as_str(), t[0].launches), ("a", 2));
        assert_eq!(t[0].input_tokens, 20);
        assert_eq!(t[0].cost_usd, Some(0.75));
        assert_eq!(t[0].credits_cost_usd, Some(0.25));
        assert_eq!((t[1].account.as_str(), t[1].cost_usd), ("b", None));
        assert_eq!(s.usage_by_day(0).unwrap(), [(0, 4 * 116, 9.75)]);
    }
}
