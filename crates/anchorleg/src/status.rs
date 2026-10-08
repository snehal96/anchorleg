//! `anchorleg status` and `anchorleg report`.

use std::io::Read;

use anchorleg_core::adapters::{QuotaReading, QuotaStatus, Window};
use anchorleg_core::output::{self, Next, Report, StatusReport};
use anchorleg_core::policy::PickOptions;
use anchorleg_core::registry::Config;
use anchorleg_core::store::{Source, Store};
use anyhow::Context;

use crate::now;

pub fn status(json: bool, refresh: bool) -> anyhow::Result<()> {
    let config = Config::load(&Config::default_path()?)?;
    let store = Store::open(&Store::default_path()?)?;
    if refresh {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .context("HOME is not set")?;
        let accounts = config.by_priority();
        let failed = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(anchorleg_core::supervisor::refresh_codex_quota(
                &config,
                &store,
                &anchorleg_core::registry::Keychain,
                &home,
                &accounts,
                now(),
            ))?;
        for (name, e) in failed {
            eprintln!("{name}: couldn't read quota ({e})");
        }
    }
    let report = output::status(&config, &store.quota(None)?, now(), &PickOptions::default());
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_status(&report);
    }
    Ok(())
}

fn print_status(r: &StatusReport) {
    if r.accounts.is_empty() {
        println!("no accounts; try `anchorleg accounts import-aliases`");
        return;
    }
    println!(
        "{:>4}  {:<16} {:<14} {:<14} STATE",
        "PRI", "ACCOUNT", "5H", "7D"
    );
    for a in &r.accounts {
        let cell = |name: &str| {
            a.windows.iter().find(|w| w.window == name).map_or_else(
                || "-".to_owned(),
                |w| window_cell(w.used, w.resets_at, r.now),
            )
        };
        let mut state = match a.blocked_until {
            Some(t) => format!("blocked, frees in {}", ago(t - r.now)),
            None => "ok".to_owned(),
        };
        // Windows of other lengths, e.g. Codex's 30-day free-plan window.
        for w in a
            .windows
            .iter()
            .filter(|w| w.window != "five_hour" && w.window != "seven_day")
        {
            state.push_str(&format!(
                "  {} {}",
                w.window,
                window_cell(w.used, w.resets_at, r.now)
            ));
        }
        println!(
            "{:>4}  {:<16} {:<14} {:<14} {state}",
            a.priority,
            a.name,
            cell("five_hour"),
            cell("seven_day")
        );
    }
    let next = match &r.next {
        Next::Use { account, reason } => format!("use {account} ({reason:?})"),
        Next::Wait { until, account } => {
            format!("wait {} for {account}", ago(until - r.now))
        }
        Next::NoAccounts => "no eligible account".to_owned(),
    };
    println!("\nnext run: {next}");
}

fn window_cell(used: Option<f64>, resets_at: Option<i64>, now: i64) -> String {
    let pct = used.map_or_else(|| "?".to_owned(), |u| format!("{:.0}%", u * 100.0));
    match resets_at {
        Some(t) if t > now => format!("{pct} ↻{}", ago(t - now)),
        _ => pct,
    }
}

/// `3h05m`, `12m`, `40s`.
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

/// Read a [`Report`] from stdin and store its readings as exact (`mod`) quota.
pub fn report() -> anyhow::Result<()> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    let report: Report = serde_json::from_str(&text).context("parsing report JSON from stdin")?;
    let config = Config::load(&Config::default_path()?)?;
    if config.get(&report.account).is_none() {
        anyhow::bail!("no account `{}`", report.account);
    }
    let store = Store::open(&Store::default_path()?)?;
    let at = now();
    for r in &report.readings {
        if let Some(u) = r.used
            && !(0.0..=1.0).contains(&u)
        {
            anyhow::bail!("`used` must be a fraction between 0 and 1, got {u}");
        }
        let reading = QuotaReading {
            window: Some(Window::from_name(&r.window)),
            status: r.status.as_deref().map(QuotaStatus::from_name),
            used: r.used,
            resets_at: r.resets_at,
        };
        store.record_quota(&report.account, &reading, Source::Mod, at)?;
    }
    if let Some(reason) = &report.stop_reason {
        let run = report.run_id.context("`stop_reason` needs `run_id`")?;
        if !store.request_stop(run, reason)? {
            anyhow::bail!("no run {run}");
        }
    }
    Ok(())
}

/// `anchorleg sessions`.
pub fn sessions(json: bool, limit: usize) -> anyhow::Result<()> {
    let store = Store::open(&Store::default_path()?)?;
    let runs = store.recent_runs(limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
        return Ok(());
    }
    if runs.is_empty() {
        println!("no sessions yet; start one with `anchorleg run -- <task>` or `anchorleg ui`");
    }
    let t = now();
    for r in runs {
        let state = format!("{:?}", r.state).to_lowercase();
        let follow = r.parent_id.map(|p| format!(" ↳#{p}")).unwrap_or_default();
        println!(
            "#{:<4} {:<9} {:<16} {:>8} ago  {}{follow}",
            r.id,
            state,
            r.account.unwrap_or_default(),
            ago(t - r.started_at),
            r.task.replace('\n', " ")
        );
    }
    Ok(())
}
