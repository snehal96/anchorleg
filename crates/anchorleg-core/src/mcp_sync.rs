//! `anchorleg sync mcp`: the same MCP servers on every account, whatever its CLI.
//!
//! Each account's servers are read from its own config, read-only (Claude Code's
//! `.claude.json`, `codex mcp list --json`, Antigravity's `mcp_config.json`); missing ones are
//! added through the CLI's own `mcp add` command, never by writing its config. The person's own
//! Antigravity `HOME` is left alone (D26), and anchorleg's own question server isn't copied.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::registry::{Account, Vendor, expand_tilde};

/// anchorleg's own server in an Antigravity account (`agy::install_mcp`): not the person's.
const OWN: &str = "anchorleg";

#[derive(Debug, Clone, PartialEq)]
pub enum Transport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    pub name: String,
    pub transport: Transport,
}

impl Server {
    /// Env values or headers, which often hold tokens.
    pub fn has_secrets(&self) -> bool {
        match &self.transport {
            Transport::Stdio { env, .. } => !env.is_empty(),
            Transport::Http { headers, .. } => !headers.is_empty(),
        }
    }
}

/// One `mcp add` to run with an account's CLI.
#[derive(Debug, Clone, PartialEq)]
pub struct Add {
    pub account: String,
    pub server: String,
    /// The CLI's arguments; `None` with `skip` when it can't be added.
    pub args: Option<Vec<String>>,
    pub skip: Option<String>,
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
        .collect()
}

/// Servers in a Claude Code `.claude.json` or an Antigravity `mcp_config.json` (`mcpServers`).
pub fn from_json_config(config: &Value) -> Vec<Server> {
    let Some(servers) = config.get("mcpServers").and_then(Value::as_object) else {
        return Vec::new();
    };
    servers
        .iter()
        .filter(|(name, s)| *name != OWN && s.get("disabled") != Some(&Value::Bool(true)))
        .filter_map(|(name, s)| {
            let url = s
                .get("url")
                .or_else(|| s.get("serverUrl"))
                .and_then(Value::as_str);
            let transport = match (s.get("command").and_then(Value::as_str), url) {
                (Some(command), _) => Transport::Stdio {
                    command: command.to_owned(),
                    args: strings(s.get("args")),
                    env: map(s.get("env")),
                },
                (None, Some(url)) => Transport::Http {
                    url: url.to_owned(),
                    headers: map(s.get("headers")),
                },
                (None, None) => return None,
            };
            Some(Server {
                name: name.clone(),
                transport,
            })
        })
        .collect()
}

/// Servers in `codex mcp list --json`.
pub fn from_codex_list(list: &Value) -> Vec<Server> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter(|s| s.get("enabled") != Some(&Value::Bool(false)))
        .filter_map(|s| {
            let name = s.get("name")?.as_str()?.to_owned();
            let t = s.get("transport")?;
            let transport = match t.get("type").and_then(Value::as_str) {
                Some("stdio") => Transport::Stdio {
                    command: t.get("command")?.as_str()?.to_owned(),
                    args: strings(t.get("args")),
                    env: map(t.get("env")),
                },
                _ => Transport::Http {
                    url: t.get("url")?.as_str()?.to_owned(),
                    headers: map(t.get("http_headers")),
                },
            };
            Some(Server { name, transport })
        })
        .collect()
}

/// Where an account keeps its servers, for the vendors read from a file.
pub fn config_file(account: &Account, home: &Path) -> Option<PathBuf> {
    match account.vendor {
        Vendor::Claude => Some(match account.env.get("CLAUDE_CONFIG_DIR") {
            Some(dir) => PathBuf::from(expand_tilde(dir, home)).join(".claude.json"),
            None => home.join(".claude.json"),
        }),
        Vendor::Antigravity => Some(
            account
                .agy_home(home)?
                .join(".gemini/config/mcp_config.json"),
        ),
        _ => None,
    }
}

/// Whether anchorleg may add servers to this account: not the person's own Antigravity `HOME`.
pub fn writable(account: &Account, home: &Path) -> bool {
    match account.vendor {
        Vendor::Claude | Vendor::Codex => true,
        Vendor::Antigravity => account.agy_home(home).is_some_and(|h| h != home),
        _ => false,
    }
}

/// The CLI arguments that add `server` to an account of `vendor`. `Err`: why it can't.
pub fn add_args(vendor: Vendor, server: &Server) -> Result<Vec<String>, String> {
    let name = server.name.clone();
    let mut args: Vec<String> = vec!["mcp".into(), "add".into()];
    match (vendor, &server.transport) {
        (Vendor::Claude, t) => {
            let config = match t {
                Transport::Stdio { command, args, env } => {
                    json!({ "type": "stdio", "command": command, "args": args, "env": env })
                }
                Transport::Http { url, headers } => {
                    json!({ "type": "http", "url": url, "headers": headers })
                }
            };
            return Ok(vec![
                "mcp".into(),
                "add-json".into(),
                "--scope".into(),
                "user".into(),
                name,
                config.to_string(),
            ]);
        }
        (
            Vendor::Codex,
            Transport::Stdio {
                command,
                args: a,
                env,
            },
        ) => {
            args.push(name);
            for (k, v) in env {
                args.extend(["--env".into(), format!("{k}={v}")]);
            }
            args.push("--".into());
            args.push(command.clone());
            args.extend(a.iter().cloned());
        }
        (Vendor::Codex, Transport::Http { url, headers }) => {
            if !headers.is_empty() {
                return Err("Codex's `mcp add` takes no headers for a URL server".into());
            }
            args.extend([name, "--url".into(), url.clone()]);
        }
        (
            Vendor::Antigravity,
            Transport::Stdio {
                command,
                args: a,
                env,
            },
        ) => {
            // Flags before the name; `--` so the command's own flags pass through.
            for (k, v) in env {
                args.extend(["--env".into(), format!("{k}={v}")]);
            }
            args.extend([name, "--".into(), command.clone()]);
            args.extend(a.iter().cloned());
        }
        (Vendor::Antigravity, Transport::Http { url, headers }) => {
            for (k, v) in headers {
                args.extend(["--header".into(), format!("{k}: {v}")]);
            }
            args.extend([name, url.clone()]);
        }
        (v, _) => {
            return Err(format!(
                "anchorleg doesn't add MCP servers to {v:?} accounts"
            ));
        }
    }
    Ok(args)
}

/// What syncing does: every server any account has, added to each writable account that lacks
/// one by that name. With `with_secrets` false, servers with env values or headers are skipped
/// (they would copy tokens into more configs).
pub fn plan(accounts: &[(&Account, Vec<Server>)], home: &Path, with_secrets: bool) -> Vec<Add> {
    let mut all: Vec<&Server> = Vec::new();
    for (_, servers) in accounts {
        for s in servers {
            if !all.iter().any(|a| a.name == s.name) {
                all.push(s);
            }
        }
    }
    let mut out = Vec::new();
    for (account, have) in accounts {
        for s in &all {
            if have.iter().any(|h| h.name == s.name) {
                continue;
            }
            let skip = if !writable(account, home) {
                Some("the person's own Antigravity setup; anchorleg leaves it alone".to_owned())
            } else if s.has_secrets() && !with_secrets {
                Some("has env values or headers (may be tokens); add with --with-env".to_owned())
            } else {
                None
            };
            let (args, skip) = match skip {
                Some(why) => (None, Some(why)),
                None => match add_args(account.vendor, s) {
                    Ok(a) => (Some(a), None),
                    Err(why) => (None, Some(why)),
                },
            };
            out.push(Add {
                account: account.name.clone(),
                server: s.name.clone(),
                args,
                skip,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(name: &str, vendor: Vendor, env: &[(&str, &str)]) -> Account {
        let mut a: Account = toml::from_str(&format!(
            "name = \"{name}\"\nvendor = \"{}\"",
            match vendor {
                Vendor::Codex => "codex",
                Vendor::Antigravity => "antigravity",
                _ => "claude",
            }
        ))
        .unwrap();
        a.env = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        a
    }

    #[test]
    fn servers_are_read_from_each_clis_config() {
        let claude = json!({ "mcpServers": {
            "docs": { "command": "npx", "args": ["-y", "docs-mcp"] },
            "api": { "type": "http", "url": "https://api.example/mcp",
                     "headers": { "Authorization": "Bearer x" } },
            "off": { "command": "x", "disabled": true },
            "anchorleg": { "command": "/bin/anchorleg" } } });
        let servers = from_json_config(&claude);
        assert_eq!(servers.len(), 2, "{servers:?}");
        assert!(!servers[1].has_secrets());
        assert!(servers[0].has_secrets());
        let codex = json!([{ "name": "docs", "enabled": true, "transport": {
            "type": "stdio", "command": "npx", "args": ["-y", "docs-mcp"], "env": null } }]);
        assert_eq!(from_codex_list(&codex), vec![servers[1].clone()]);
    }

    #[test]
    fn each_cli_gets_its_own_add_command() {
        let s = Server {
            name: "docs".into(),
            transport: Transport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "docs-mcp".into()],
                env: BTreeMap::from([("K".into(), "v".into())]),
            },
        };
        assert_eq!(
            add_args(Vendor::Codex, &s).unwrap().join(" "),
            "mcp add docs --env K=v -- npx -y docs-mcp"
        );
        assert_eq!(
            add_args(Vendor::Antigravity, &s).unwrap().join(" "),
            "mcp add --env K=v docs -- npx -y docs-mcp"
        );
        let claude = add_args(Vendor::Claude, &s).unwrap();
        assert_eq!(claude[..5].join(" "), "mcp add-json --scope user docs");
        let config: Value = serde_json::from_str(&claude[5]).unwrap();
        assert_eq!(config["args"], json!(["-y", "docs-mcp"]));
    }

    #[test]
    fn the_plan_fills_gaps_and_leaves_the_persons_agy_and_secrets_alone() {
        let home = Path::new("/home/me");
        let c = account("c", Vendor::Claude, &[]);
        let x = account("x", Vendor::Codex, &[]);
        let own = account("g", Vendor::Antigravity, &[]);
        let docs = Server {
            name: "docs".into(),
            transport: Transport::Stdio {
                command: "npx".into(),
                args: vec![],
                env: BTreeMap::new(),
            },
        };
        let secret = Server {
            name: "api".into(),
            transport: Transport::Http {
                url: "https://a".into(),
                headers: BTreeMap::from([("Authorization".into(), "t".into())]),
            },
        };
        let plan = plan(
            &[
                (&c, vec![docs.clone(), secret]),
                (&x, vec![docs]),
                (&own, vec![]),
            ],
            home,
            false,
        );
        let line = |a: &Add| format!("{} {} {}", a.account, a.server, a.args.is_some());
        assert_eq!(
            plan.iter().map(line).collect::<Vec<_>>(),
            ["x api false", "g docs false", "g api false"]
        );
        assert!(plan[1].skip.as_deref().unwrap().contains("own Antigravity"));
    }
}
