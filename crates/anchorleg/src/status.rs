//! `anchorleg status` and `anchorleg report`.

use std::io::Read;

use anchorleg_core::adapters::{QuotaReading, QuotaStatus, Window};
use anchorleg_core::output::{self, Next, Report, StatusReport};
use anchorleg_core::policy::{CreditsUse, PickOptions};
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
            .block_on(anchorleg_core::supervisor::refresh_quota(
                &config,
                &store,
                &anchorleg_core::registry::Keychain,
                &home,
                &accounts,
                now(),
                0,
            ))?;
        for (name, e) in failed {
            eprintln!("{name}: couldn't read quota ({e})");
        }
    }
    let opts = PickOptions {
        policy: config.policy,
        ..PickOptions::default()
    };
    let report = output::status(
        &config,
        &store.quota(None)?,
        &store.credits(None)?,
        now(),
        &opts,
    );
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
        if let Some(c) = &a.credits {
            state.push_str(&format!("  credits {}", credits_cell(c, a.credits_use)));
        }
        if let Some(note) = stale_note(a.windows.iter().map(|w| w.updated_at), r.now) {
            state.push_str(&format!("  {note}"));
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
    println!("\nnext run: {next}  (policy {})", r.policy.name());
}

/// `available (last-resort)`, `off: out_of_credits`, `in use, $12.50 left`.
pub fn credits_cell(c: &output::CreditsStatus, setting: CreditsUse) -> String {
    use anchorleg_core::adapters::CreditState;
    let mut s = match c.state {
        CreditState::Off => match &c.reason {
            Some(r) => format!("off: {r}"),
            None => "off".to_owned(),
        },
        CreditState::Available => "available".to_owned(),
        CreditState::InUse => "IN USE".to_owned(),
        CreditState::Unlimited => "unlimited".to_owned(),
    };
    if let Some(b) = c.balance {
        s.push_str(&format!(", {b} left"));
    }
    if c.state.spendable() {
        s.push_str(&format!(" (use: {})", setting.name()));
    }
    s
}

fn window_cell(used: Option<f64>, resets_at: Option<i64>, now: i64) -> String {
    let pct = used.map_or_else(|| "?".to_owned(), |u| format!("{:.0}%", u * 100.0));
    match resets_at {
        Some(t) if t > now => format!("{pct} ↻{}", ago(t - now)),
        _ => pct,
    }
}

/// How old the account's quota reading is, when it's old enough to be wrong: anchorleg only sees
/// Claude's quota during its own runs, so use elsewhere (an interactive session) doesn't show.
pub fn stale_note(updated: impl Iterator<Item = i64>, now: i64) -> Option<String> {
    let newest = updated.max()?;
    (now - newest >= STALE_AFTER).then(|| format!("read {} ago", ago(now - newest)))
}

/// A quota reading older than this gets its age shown.
const STALE_AFTER: i64 = 3600;

/// `3h05m`, `12m`, `40s`.
pub fn ago(secs: i64) -> String {
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

/// Version of `anchorleg sessions show --json`'s object; bumped when a field changes meaning.
const SESSION_SCHEMA: u32 = 1;

/// The `anchorleg run` process is still there.
fn pid_alive(pid: i64) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `anchorleg sessions show RUN`: the session up to RUN, as `anchorleg ui` shows it.
pub fn show_session(id: i64, since: usize, json: bool) -> anyhow::Result<()> {
    use anchorleg_core::conversation::{self, Role};
    use anchorleg_core::store::RunState;

    let db = Store::default_path()?;
    let store = Store::open(&db)?;
    let log_dir = db.parent().map(|d| d.join("runs")).unwrap_or_default();
    let Some(run) = store.run(id)? else {
        anyhow::bail!("no run #{id}; `anchorleg sessions` lists them");
    };
    let mut runs = Vec::new();
    for rid in store.session_runs(id)?.into_iter().rev() {
        runs.extend(store.run(rid)?);
    }
    let refs: Vec<_> = runs.iter().collect();
    let view = conversation::session_view(&refs, &log_dir);
    let working = matches!(run.state, RunState::Running | RunState::Waiting)
        && run.pid.is_some_and(pid_alive);
    let next = view.messages.len();
    let since = since.min(next);
    if json {
        let slash = std::fs::read_to_string(log_dir.join(format!("run-{id}.jsonl")))
            .ok()
            .and_then(|log| conversation::slash_commands(&log));
        let out = serde_json::json!({
            "schema": SESSION_SCHEMA,
            "run": run,
            "runs": runs.iter().map(|r| r.id).collect::<Vec<_>>(),
            "name": store.session_name(id)?,
            "working": working,
            "since": since,
            "next": next,
            "messages": &view.messages[since..],
            "running": view.running,
            "subagents": view.subagents,
            "steps": view.steps,
            "context": { "tokens": view.context_tokens, "window": view.context_window },
            "slash_commands": slash,
            "suggestion": view.suggestion,
            "changes": anchorleg_core::changes::session(
                &log_dir,
                &runs.iter().map(|r| r.id).collect::<Vec<_>>()
            ),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    for m in &view.messages[since..] {
        let who = match m.role {
            Role::User => "you",
            Role::Agent => "agent",
            Role::Tool => "tool",
            Role::ToolError => "tool error",
            Role::Output => "output",
            Role::Thinking => "thinking",
            Role::Anchorleg => "anchorleg",
            Role::Error => "error",
        };
        println!("[{who}] {}", m.text);
    }
    if let Some(r) = view.running.filter(|_| working) {
        println!("[working] {}", r.what);
    }
    Ok(())
}

/// `anchorleg sessions`.
pub fn sessions(json: bool, limit: usize) -> anyhow::Result<()> {
    let store = Store::open(&Store::default_path()?)?;
    let runs = store.recent_runs(limit)?;
    if json {
        // `side`: a side question's run (`btw`), not the session going on.
        let out: Vec<serde_json::Value> = runs
            .iter()
            .map(|r| {
                let mut v = serde_json::to_value(r)?;
                v["side"] = crate::run::is_side(&r.task).into();
                Ok(v)
            })
            .collect::<anyhow::Result<_>>()?;
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if runs.is_empty() {
        println!("no sessions yet; start one with `anchorleg run -- <task>` or `anchorleg ui`");
    }
    let t = now();
    for r in runs {
        let state = format!("{:?}", r.state).to_lowercase();
        let follow = r.parent_id.map(|p| format!(" ↳#{p}")).unwrap_or_default();
        let name = store
            .session_name(r.id)?
            .map_or(String::new(), |n| format!("[{n}] "));
        println!(
            "#{:<4} {:<9} {:<16} {:>8} ago  {name}{}{follow}",
            r.id,
            state,
            r.account.unwrap_or_default(),
            ago(t - r.started_at),
            r.task.replace('\n', " ")
        );
    }
    Ok(())
}
