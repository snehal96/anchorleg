//! `anchorleg sessions agents <run>`: a session's subagents in full (what each was asked, its
//! steps, what it reported), read from what Claude Code and Codex keep on disk.

use std::collections::HashSet;

use anchorleg_core::agents::{self, Agent, Status};
use anchorleg_core::registry::{Config, Vendor};
use anchorleg_core::store::Store;

pub fn show(id: i64, json: bool) -> anyhow::Result<()> {
    let config = Config::load(&Config::default_path()?)?;
    let store = Store::open(&Store::default_path()?)?;
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    if store.run(id)?.is_none() {
        anyhow::bail!("no run #{id}; `anchorleg sessions` lists them");
    }
    let now = crate::now();
    let mut out: Vec<(i64, Agent)> = Vec::new();
    // Each CLI session in the chain once (a fork starts a new one), on the account that ran it.
    let mut seen = HashSet::new();
    for rid in store.session_runs(id)?.into_iter().rev() {
        let Some(run) = store.run(rid)? else { continue };
        let (Some(session), Some(account)) = (run.session_id.clone(), run.account.clone()) else {
            continue;
        };
        if !seen.insert(session.clone()) {
            // A follow-up in the same session: its subagents are read with the first run's.
            continue;
        }
        let Some(a) = config.get(&account) else {
            continue;
        };
        let found = match a.vendor {
            Vendor::Claude => a
                .claude_dir(&home)
                .map(|d| agents::claude_agents(&d, &session, now))
                .unwrap_or_default(),
            Vendor::Codex => a
                .codex_home(&home)
                .map(|d| agents::codex_agents(&d, &session, run.started_at, now))
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        out.extend(found.into_iter().map(|g| (rid, g)));
    }
    if json {
        let list: Vec<_> = out
            .iter()
            .map(|(run, a)| {
                let mut v = serde_json::to_value(a).unwrap_or_default();
                v["run"] = (*run).into();
                v
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "schema": 1, "agents": list }))?
        );
        return Ok(());
    }
    if out.is_empty() {
        println!("no subagents in this session (Claude Code and Codex sessions only)");
        return Ok(());
    }
    for (_, a) in &out {
        let mark = match a.status {
            Status::Working => "…",
            Status::Done => "✓",
            Status::Stopped => "·",
        };
        let kind = a
            .kind
            .as_deref()
            .map(|k| format!(" ({k})"))
            .unwrap_or_default();
        let steps = match a.steps {
            1 => "1 step".to_owned(),
            n => format!("{n} steps"),
        };
        println!("{mark} {}{kind}  {steps}", a.name);
        if let Some(r) = &a.result {
            let line: String = r
                .trim()
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(100)
                .collect();
            println!("    → {line}");
        }
    }
    Ok(())
}
