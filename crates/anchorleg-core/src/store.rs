//! Local state: the latest quota reading per account and window, runs, and the decision log.
//!
//! SQLite in WAL mode, so several `anchorleg` processes (parallel runs, the mod's `anchorleg report`)
//! can share one file. Holds account names only, never tokens.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::adapters::{EpochSecs, QuotaReading};

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
}

const PERMISSION_COLUMNS: &str = "id, run_id, tool, input, reason, state, asked_at, answered_at";

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
}
