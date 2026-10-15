//! Accounts, quota, usage, policy and per-CLI settings: each a CLI command (D30). Changes go
//! through the CLI too, so `config.toml` keeps one writer.

use serde::Deserialize;
use serde_json::Value;

use crate::{blocking, cli};

/// `anchorleg status --json`: quota per account and window, credits, what a run would pick.
/// `refresh` asks every account's CLI for its quota first (a few seconds, no model request).
#[tauri::command]
pub async fn status(refresh: bool) -> Result<Value, String> {
    blocking(move || {
        let mut args = vec!["status", "--json"];
        if refresh {
            args.push("--refresh");
        }
        cli::json(&args)
    })
    .await
}

/// Sessions started in the CLIs themselves (`anchorleg sessions import`); `dry_run` only lists.
#[tauri::command]
pub async fn import_sessions(dry_run: bool, since: Option<String>) -> Result<Value, String> {
    blocking(move || {
        let since = since.unwrap_or_else(|| "30d".to_owned());
        let mut args = vec!["sessions", "import", "--json", "--since", &since];
        if dry_run {
            args.push("--dry-run");
        }
        cli::json(&args)
    })
    .await
}

#[tauri::command]
pub async fn accounts() -> Result<Value, String> {
    blocking(|| cli::json(&["accounts", "list", "--json"])).await
}

/// `anchorleg usage --since <24h|7d|30d|all> --json`.
#[tauri::command]
pub async fn usage(since: String) -> Result<Value, String> {
    blocking(move || cli::json(&["usage", "--since", &since, "--json"])).await
}

/// Show, or set and show, the switch policy.
#[tauri::command]
pub async fn policy(set: Option<String>) -> Result<Value, String> {
    blocking(move || {
        let mut args = vec!["policy".to_owned()];
        args.extend(set);
        args.push("--json".to_owned());
        cli::json(&args)
    })
    .await
}

/// One CLI's settings to change; `None` leaves a field as it is, `default` / `""` clears it.
#[derive(Debug, Deserialize)]
pub struct CliSettings {
    vendor: String,
    model: Option<String>,
    effort: Option<String>,
    args: Option<String>,
}

#[tauri::command]
pub async fn settings(change: Option<CliSettings>) -> Result<Value, String> {
    blocking(move || {
        let mut args = vec!["settings".to_owned()];
        if let Some(c) = change {
            args.push(c.vendor);
            for (flag, v) in [
                ("--model", c.model),
                ("--effort", c.effort),
                ("--args", c.args),
            ] {
                if let Some(v) = v {
                    args.extend([flag.to_owned(), v]);
                }
            }
        }
        args.push("--json".to_owned());
        cli::json(&args)
    })
    .await
}

/// `anchorleg accounts set`: only the fields given change.
#[derive(Debug, Deserialize)]
pub struct AccountChange {
    name: String,
    priority: Option<u32>,
    reserve_7d: Option<f64>,
    stop_at: Option<f64>,
    strict: Option<bool>,
    pinned: Option<bool>,
    enabled: Option<bool>,
    credits: Option<String>,
}

impl AccountChange {
    fn args(self) -> Vec<String> {
        let mut a = vec!["accounts".to_owned(), "set".to_owned(), self.name];
        let mut put = |flag: &str, v: Option<String>| {
            if let Some(v) = v {
                a.extend([flag.to_owned(), v]);
            }
        };
        put("--priority", self.priority.map(|v| v.to_string()));
        put("--reserve-7d", self.reserve_7d.map(|v| v.to_string()));
        put("--stop-at", self.stop_at.map(|v| v.to_string()));
        put("--strict", self.strict.map(|v| v.to_string()));
        put("--pinned", self.pinned.map(|v| v.to_string()));
        put("--enabled", self.enabled.map(|v| v.to_string()));
        put("--credits", self.credits);
        a
    }
}

/// Apply several account changes in order (a reorder is one per account).
#[tauri::command]
pub async fn set_accounts(changes: Vec<AccountChange>) -> Result<(), String> {
    blocking(move || {
        for c in changes {
            cli::text(&c.args())?;
        }
        Ok(())
    })
    .await
}

/// `anchorleg accounts add <name> --cmd "<command>"`: the command you'd type to run the account.
/// Tokens are never handled here: `accounts add --token-stdin` stays a terminal step.
#[tauri::command]
pub async fn add_account(name: String, command: String) -> Result<(), String> {
    blocking(move || cli::text(&["accounts", "add", &name, "--cmd", &command]).map(drop)).await
}

/// What each CLI can do with anchorleg (`anchorleg vendors --json`), so the window offers only
/// what an account's CLI supports.
#[tauri::command]
pub async fn vendors() -> Result<Value, String> {
    blocking(|| cli::json(&["vendors", "--json"])).await
}

#[tauri::command]
pub async fn remove_account(name: String) -> Result<(), String> {
    blocking(move || cli::text(&["accounts", "rm", &name]).map(drop)).await
}

/// Accounts from the shell's aliases: a preview, or `yes` to add them.
#[tauri::command]
pub async fn import_aliases(yes: bool) -> Result<Value, String> {
    blocking(move || {
        let mut args = vec!["accounts", "import-aliases", "--json"];
        if yes {
            args.push("--yes");
        }
        cli::json(&args)
    })
    .await
}

/// Lasting allow rules: add and remove, then the list.
#[tauri::command]
pub async fn rules(add: Vec<String>, rm: Vec<String>) -> Result<Value, String> {
    blocking(move || {
        let mut args = vec!["rules".to_owned()];
        for r in add {
            args.extend(["--add".to_owned(), r]);
        }
        for r in rm {
            args.extend(["--rm".to_owned(), r]);
        }
        args.push("--json".to_owned());
        cli::json(&args)
    })
    .await
}

/// Notification settings: change what's given, then show them. `test` sends a test notice.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct NotifyChange {
    desktop: Option<bool>,
    /// `""` stops running a command.
    command: Option<String>,
    test: bool,
}

#[tauri::command]
pub async fn notify(change: Option<NotifyChange>) -> Result<Value, String> {
    blocking(move || {
        let mut args = vec!["notify".to_owned()];
        if let Some(c) = change {
            if let Some(d) = c.desktop {
                args.extend([
                    "--desktop".to_owned(),
                    if d { "on" } else { "off" }.to_owned(),
                ]);
            }
            match c.command {
                Some(cmd) if cmd.trim().is_empty() => args.push("--no-command".to_owned()),
                Some(cmd) => args.extend(["--command".to_owned(), cmd]),
                None => {}
            }
            if c.test {
                args.push("--test".to_owned());
            }
        }
        args.push("--json".to_owned());
        cli::json(&args)
    })
    .await
}

/// The same MCP servers on every account: the plan, or `yes` to add them.
#[tauri::command]
pub async fn mcp_sync(with_env: bool, yes: bool) -> Result<Value, String> {
    blocking(move || {
        let mut args = vec!["sync", "mcp", "--json"];
        if with_env {
            args.push("--with-env");
        }
        if yes {
            args.push("--yes");
        }
        cli::json(&args)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_given_fields_change() {
        let c: AccountChange =
            serde_json::from_str(r#"{"name":"work","stop_at":0.4,"strict":true}"#).unwrap();
        assert_eq!(
            c.args(),
            [
                "accounts",
                "set",
                "work",
                "--stop-at",
                "0.4",
                "--strict",
                "true"
            ]
        );
    }
}
