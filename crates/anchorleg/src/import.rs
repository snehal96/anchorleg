//! `anchorleg sessions import`: sessions started in the CLIs themselves, brought into anchorleg.

use std::path::{Path, PathBuf};

use anchorleg_core::import::{self, Found};
use anchorleg_core::registry::{Config, Vendor};
use anchorleg_core::store::Store;

pub struct Options {
    pub account: Option<String>,
    /// Only sessions active since this time (epoch seconds).
    pub since: i64,
    /// Only sessions in this folder or below it.
    pub cwd: Option<PathBuf>,
    pub dry_run: bool,
    pub json: bool,
    /// Also sessions in temporary folders (scratch runs, mostly).
    pub include_temp: bool,
}

/// Reads one session file of an account.
type Parse = fn(&str, &str, &Path) -> Option<Found>;

/// A session changed this recently may still be open in its CLI.
const IN_USE: i64 = 5 * 60;

/// Temporary folders: `/tmp`, `/private/tmp`, `/var/folders`, `$TMPDIR`.
fn is_temp(dir: &str) -> bool {
    let tmp = std::env::temp_dir();
    let tmp = std::fs::canonicalize(&tmp).unwrap_or(tmp);
    [
        "/tmp/",
        "/private/tmp/",
        "/var/folders/",
        "/private/var/folders/",
    ]
    .iter()
    .any(|p| dir.starts_with(p))
        || Path::new(dir).starts_with(&tmp)
}

fn tilde(p: &str) -> String {
    match std::env::var("HOME")
        .ok()
        .and_then(|h| p.strip_prefix(&h).map(str::to_owned))
    {
        Some(rest) => format!("~{rest}"),
        None => p.to_owned(),
    }
}

pub fn run(o: Options) -> anyhow::Result<()> {
    let config = Config::load(&Config::default_path()?)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    if let Some(a) = &o.account
        && config.get(a).is_none()
    {
        anyhow::bail!("no account `{a}`");
    }
    let within = match &o.cwd {
        Some(d) => {
            Some(std::fs::canonicalize(d).map_err(|e| anyhow::anyhow!("{}: {e}", d.display()))?)
        }
        None => None,
    };
    let mut found = Vec::new();
    let mut skipped_cli = Vec::new();
    for a in &config.accounts {
        if o.account.as_ref().is_some_and(|n| *n != a.name) {
            continue;
        }
        let (files, parse): (Vec<PathBuf>, Parse) = match a.vendor {
            Vendor::Claude => match a.claude_dir(&home) {
                Some(d) => (import::claude_files(&d, o.since), import::claude),
                None => continue,
            },
            Vendor::Codex => match a.codex_home(&home) {
                Some(d) => (import::codex_files(&d, o.since), import::codex),
                None => continue,
            },
            _ => {
                skipped_cli.push(a.name.clone());
                continue;
            }
        };
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            if let Some(s) = parse(&text, &a.name, &f) {
                found.push(s);
            }
        }
    }
    let found = import::dedup(found);

    let db = Store::default_path()?;
    let store = Store::open(&db)?;
    let mut new = Vec::new();
    let (mut known, mut gone, mut elsewhere, mut temp, mut open) = (0, 0, 0, 0, 0);
    // Imported before and not gone on with in anchorleg: its log is written again, so it shows
    // what was said since in the CLI and has the points to go back to (`at`).
    let mut refresh = Vec::new();
    let now = crate::now();
    for f in found {
        if store.has_session(&f.session_id)? {
            known += 1;
            if let Some(id) = store.lone_run(&f.session_id)?
                && log_of(&db, id).starts_with(IMPORT_NOTE_START)
            {
                refresh.push((id, f));
            }
        } else if !Path::new(&f.cwd).is_dir() {
            gone += 1;
        } else if !o.include_temp && is_temp(&f.cwd) {
            temp += 1;
        } else if now - f.ended_at < IN_USE {
            open += 1;
        } else if within
            .as_ref()
            .is_some_and(|w| !std::fs::canonicalize(&f.cwd).is_ok_and(|c| c.starts_with(w)))
        {
            elsewhere += 1;
        } else {
            new.push(f);
        }
    }

    let mut ids = Vec::new();
    if !o.dry_run {
        let logs = db.parent().map(|d| d.join("runs")).unwrap_or_default();
        std::fs::create_dir_all(&logs)?;
        for f in &new {
            let id = store.import_run(f)?;
            std::fs::write(logs.join(format!("run-{id}.jsonl")), import_log(f))?;
            ids.push(id);
        }
        for (id, f) in &refresh {
            std::fs::write(logs.join(format!("run-{id}.jsonl")), import_log(f))?;
        }
    }

    if o.json {
        let sessions: Vec<_> = new
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let mut v = serde_json::to_value(f).unwrap_or_default();
                v["run_id"] = ids.get(i).copied().into();
                v
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema": 1,
                "dry_run": o.dry_run,
                "sessions": sessions,
                "skipped": {
                    "known": known,
                    "folder_gone": gone,
                    "other_folder": elsewhere,
                    "temporary_folder": temp,
                    "in_use": open,
                },
                "not_importable": skipped_cli,
                "updated": refresh.len(),
            }))?
        );
        return Ok(());
    }
    for (i, f) in new.iter().enumerate() {
        let when = format!("{:>9}", crate::status::ago(crate::now() - f.ended_at));
        let task: String = f
            .task
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(60)
            .collect();
        let folder = Path::new(&f.cwd)
            .file_name()
            .map_or_else(|| tilde(&f.cwd), |n| n.to_string_lossy().into_owned());
        println!(
            "{}{when} ago  {:<14} {:<18} {}",
            ids.get(i).map_or(String::new(), |id| format!("#{id:<4} ")),
            f.account,
            folder,
            f.name.as_deref().unwrap_or(&task)
        );
    }
    let verb = if o.dry_run {
        "would import"
    } else {
        "imported"
    };
    let mut also = Vec::new();
    if known > 0 {
        let updated = match (refresh.len(), o.dry_run) {
            (0, _) => String::new(),
            (n, true) => format!(" ({n} would be brought up to date)"),
            (n, false) => format!(" ({n} brought up to date)"),
        };
        also.push(format!("{known} already in anchorleg{updated}"));
    }
    if gone > 0 {
        also.push(format!("{gone} whose folder is gone"));
    }
    if elsewhere > 0 {
        also.push(format!("{elsewhere} in other folders"));
    }
    if temp > 0 {
        also.push(format!("{temp} in temporary folders (--include-temp)"));
    }
    if open > 0 {
        also.push(format!(
            "{open} used in the last 5 minutes (may still be open)"
        ));
    }
    println!(
        "{verb} {} session{}{}",
        new.len(),
        if new.len() == 1 { "" } else { "s" },
        if also.is_empty() {
            String::new()
        } else {
            format!("; skipped {}", also.join(", "))
        }
    );
    if !skipped_cli.is_empty() {
        println!(
            "not imported: {} (Antigravity keeps no readable transcript)",
            skipped_cli.join(", ")
        );
    }
    if o.dry_run && !new.is_empty() {
        println!("run it without --dry-run to import them");
    }
    Ok(())
}

/// How an imported session's log starts (the note [`import_log`] writes).
const IMPORT_NOTE_START: &str = r#"{"text":"imported from "#;

fn log_of(db: &Path, id: i64) -> String {
    let path = db
        .parent()
        .map(|d| d.join("runs").join(format!("run-{id}.jsonl")))
        .unwrap_or_default();
    std::fs::read_to_string(path).unwrap_or_default()
}

/// An imported session's run log: a note saying where it came from, then its conversation.
fn import_log(f: &import::Found) -> String {
    let note = serde_json::json!({
        "text": format!(
            "imported from {} on {}: started outside anchorleg; a follow-up continues it",
            f.vendor.caps().label,
            f.account
        ),
        "type": "anchorleg",
    });
    let mut body = note.to_string() + "\n";
    for l in &f.log {
        body.push_str(l);
        body.push('\n');
    }
    body
}
