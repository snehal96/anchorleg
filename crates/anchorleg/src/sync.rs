//! `anchorleg sync mcp`: the same MCP servers on every account (see `anchorleg_core::mcp_sync`).

use std::path::Path;

use anchorleg_core::mcp_sync::{self, Server};
use anchorleg_core::registry::{Account, Config, Keychain, Vendor};
use anyhow::Context;
use serde_json::Value;

/// How to start an account's CLI for a config command: its binary and env, no token (`mcp`
/// commands don't need one) and none of the args meant for runs.
fn command(account: &Account, home: &Path) -> anyhow::Result<std::process::Command> {
    let mut account = account.clone();
    account.token_from = None;
    account.args.clear();
    Ok(account.launch(&Keychain, home)?.command())
}

fn servers(account: &Account, home: &Path) -> anyhow::Result<Vec<Server>> {
    if let Some(path) = mcp_sync::config_file(account, home) {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let config: Value =
            serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
        return Ok(mcp_sync::from_json_config(&config));
    }
    let out = command(account, home)?
        .args(["mcp", "list", "--json"])
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("listing {}'s MCP servers", account.name))?;
    let list: Value = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("reading {}'s MCP server list", account.name))?;
    Ok(mcp_sync::from_codex_list(&list))
}

pub fn mcp(with_env: bool, yes: bool, json: bool) -> anyhow::Result<()> {
    let config = Config::load(&Config::default_path()?)?;
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .context("HOME is not set")?;
    let mut read = Vec::new();
    for a in &config.accounts {
        if !matches!(
            a.vendor,
            Vendor::Claude | Vendor::Codex | Vendor::Antigravity
        ) || a.shell.is_some()
        {
            continue;
        }
        match servers(a, &home) {
            Ok(s) => read.push((a, s)),
            Err(e) => eprintln!("anchorleg: skipping {}: {e:#}", a.name),
        }
    }
    if json {
        return mcp_json(&read, &home, with_env, yes);
    }
    for (a, s) in &read {
        let names: Vec<&str> = s.iter().map(|s| s.name.as_str()).collect();
        println!(
            "{:<16} {}",
            a.name,
            if names.is_empty() {
                "(none)".to_owned()
            } else {
                names.join(", ")
            }
        );
    }
    let plan = mcp_sync::plan(&read, &home, with_env);
    if plan.is_empty() {
        println!("\nevery account has the same MCP servers");
        return Ok(());
    }
    println!();
    for add in &plan {
        match &add.skip {
            Some(why) => println!("skip  {} → {}: {why}", add.server, add.account),
            None if yes => {
                let account = read
                    .iter()
                    .find(|(a, _)| a.name == add.account)
                    .map(|(a, _)| *a)
                    .context("account vanished")?;
                let out = command(account, &home)?
                    .args(add.args.iter().flatten())
                    .stdin(std::process::Stdio::null())
                    .output()
                    .with_context(|| format!("adding {} to {}", add.server, add.account))?;
                if out.status.success() {
                    println!("added {} → {}", add.server, add.account);
                } else {
                    println!(
                        "failed {} → {}: {}",
                        add.server,
                        add.account,
                        String::from_utf8_lossy(&out.stderr).trim()
                    );
                }
            }
            None => println!("add   {} → {}", add.server, add.account),
        }
    }
    if !yes && plan.iter().any(|a| a.skip.is_none()) {
        println!(
            "\nNothing changed yet; run again with --yes to add them (through each CLI's own `mcp add`)."
        );
    }
    Ok(())
}

/// `sync mcp --json`: `{schema, accounts: [{name, servers}], plan: [{server, account, skip}],
/// results: [{server, account, ok, error}]}` (`results` only with `--yes`). Server settings, which
/// can hold tokens, are never printed: only names.
fn mcp_json(
    read: &[(&anchorleg_core::registry::Account, Vec<mcp_sync::Server>)],
    home: &std::path::Path,
    with_env: bool,
    yes: bool,
) -> anyhow::Result<()> {
    let plan = mcp_sync::plan(read, home, with_env);
    let mut results = Vec::new();
    if yes {
        for add in plan.iter().filter(|a| a.skip.is_none()) {
            let Some((account, _)) = read.iter().find(|(a, _)| a.name == add.account) else {
                continue;
            };
            let out = command(account, home)?
                .args(add.args.iter().flatten())
                .stdin(std::process::Stdio::null())
                .output();
            let (ok, error) = match out {
                Ok(o) if o.status.success() => (true, None),
                Ok(o) => (
                    false,
                    Some(String::from_utf8_lossy(&o.stderr).trim().to_owned()),
                ),
                Err(e) => (false, Some(e.to_string())),
            };
            results.push(serde_json::json!({
                "server": add.server, "account": add.account, "ok": ok, "error": error,
            }));
        }
    }
    let out = serde_json::json!({
        "schema": 1,
        "accounts": read.iter().map(|(a, s)| serde_json::json!({
            "name": a.name,
            "servers": s.iter().map(|s| &s.name).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "plan": plan.iter().map(|a| serde_json::json!({
            "server": a.server, "account": a.account, "skip": a.skip,
        })).collect::<Vec<_>>(),
        "results": yes.then_some(results),
    });
    println!("{out}");
    Ok(())
}
