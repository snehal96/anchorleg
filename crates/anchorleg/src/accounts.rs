//! `anchorleg accounts …`: manage `config.toml` and Keychain tokens.

use std::io::{BufRead, IsTerminal};
use std::path::{Path, PathBuf};

use anchorleg_core::registry::{
    Account, Config, Keychain, Secret, SecretStore, TokenFrom, Vendor, accounts_from_aliases,
    parse_command,
};
use anyhow::{Context, bail};
use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum AccountsCmd {
    /// List accounts in the order they're tried.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show exactly how anchorleg would launch an account (tokens hidden).
    Show { name: String },
    /// Add an account.
    Add(AddArgs),
    /// Remove an account (and its Keychain token, if any).
    Rm { name: String },
    /// Turn shell aliases like `claude-sm='CLAUDE_CONFIG_DIR=… claude'` into accounts.
    ImportAliases {
        /// Write the accounts. Without it, only show what would be added.
        #[arg(long)]
        yes: bool,
        /// Read `alias` output from this file instead of running `$SHELL -ic alias`.
        #[arg(long)]
        from_file: Option<PathBuf>,
    },
}

#[derive(Args)]
pub struct AddArgs {
    name: String,
    /// Defaults to the vendor of the `--cmd` binary, else claude.
    #[arg(long, value_parser = parse_vendor)]
    vendor: Option<Vendor>,
    /// Lower is used first.
    #[arg(long)]
    priority: Option<u32>,
    /// The command you'd type to run this account, e.g.
    /// "CLAUDE_CONFIG_DIR=~/.claude-sm ~/.local/bin/claude".
    #[arg(long, conflicts_with = "shell")]
    cmd: Option<String>,
    /// Run this through your interactive shell instead (e.g. an alias name). Slower.
    #[arg(long)]
    shell: Option<String>,
    /// Read a token (from `claude setup-token`) and store it in Keychain.
    #[arg(long)]
    token_stdin: bool,
}

pub(crate) fn parse_vendor(s: &str) -> Result<Vendor, String> {
    let s = if s == "agy" { "antigravity" } else { s };
    Vendor::ALL
        .into_iter()
        .find(|v| v.name() == s)
        .ok_or_else(|| "expected claude, codex, antigravity (agy), kimi or cursor".to_owned())
}

pub(crate) fn vendor_name(v: Vendor) -> &'static str {
    v.name()
}

pub fn run(cmd: AccountsCmd) -> anyhow::Result<()> {
    let path = Config::default_path()?;
    let mut config = Config::load(&path)?;
    match cmd {
        AccountsCmd::List { json } => list(&config, json),
        AccountsCmd::Show { name } => show(&config, &name),
        AccountsCmd::Add(args) => {
            add(&mut config, args)?;
            config.save(&path)?;
            Ok(())
        }
        AccountsCmd::Rm { name } => {
            let before = config.accounts.len();
            let had_token = config
                .get(&name)
                .is_some_and(|a| a.token_from == Some(TokenFrom::Keychain));
            config.accounts.retain(|a| a.name != name);
            if config.accounts.len() == before {
                bail!("no account `{name}`");
            }
            config.save(&path)?;
            if had_token {
                Keychain.delete(&name)?;
            }
            println!("removed {name}");
            Ok(())
        }
        AccountsCmd::ImportAliases { yes, from_file } => {
            import_aliases(&mut config, &path, yes, from_file.as_deref())
        }
    }
}

fn list(config: &Config, json: bool) -> anyhow::Result<()> {
    let accounts = config.by_priority();
    if json {
        println!("{}", serde_json::to_string_pretty(&accounts)?);
        return Ok(());
    }
    if accounts.is_empty() {
        println!(
            "no accounts; try `anchorleg accounts import-aliases` or `anchorleg accounts add`"
        );
    }
    for a in accounts {
        let how = match (&a.shell, a.token_from) {
            (Some(sh), _) => format!("shell: {sh}"),
            (None, Some(TokenFrom::Keychain)) => "token in Keychain".to_owned(),
            (None, None) => {
                let env: Vec<_> = a.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
                let bin = a.bin.as_deref().unwrap_or(a.vendor.default_bin());
                format!("{} {bin}", env.join(" ")).trim().to_owned()
            }
        };
        println!(
            "{:>4}  {:<16} {:<7} {how}",
            a.priority,
            a.name,
            vendor_name(a.vendor)
        );
    }
    Ok(())
}

fn show(config: &Config, name: &str) -> anyhow::Result<()> {
    let account = config
        .get(name)
        .with_context(|| format!("no account `{name}`"))?;
    let home = dirs_home()?;
    let launch = config.launch(account, &Keychain, &home)?;
    println!("{}", launch.display());
    Ok(())
}

fn dirs_home() -> anyhow::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn add(config: &mut Config, args: AddArgs) -> anyhow::Result<()> {
    if config.get(&args.name).is_some() {
        bail!(
            "account `{}` already exists; `anchorleg accounts rm` it first",
            args.name
        );
    }
    let parsed = args.cmd.as_deref().map(parse_command).transpose()?;
    let vendor = args
        .vendor
        .or_else(|| parsed.as_ref().and_then(|p| Vendor::from_bin(&p.bin)))
        .unwrap_or(Vendor::Claude);

    let mut account = Account::new(args.name.clone(), vendor);
    if let Some(p) = args.priority {
        account.priority = p;
    }
    if let Some(p) = parsed {
        account.bin = Some(p.bin);
        account.env = p.env;
        account.args = p.args;
    }
    account.shell = args.shell;
    if args.token_stdin {
        account.token_from = Some(TokenFrom::Keychain);
    }

    // Validate before touching Keychain, so a bad account doesn't leave a stray token.
    let mut candidate = config.clone();
    candidate.accounts.push(account.clone());
    candidate.validate()?;

    if args.token_stdin {
        let token = read_token()?;
        Keychain.set(&account.name, &token)?;
    }
    config.accounts.push(account);
    println!("added {}", args.name);
    Ok(())
}

fn read_token() -> anyhow::Result<Secret> {
    let token = if std::io::stdin().is_terminal() {
        rpassword::prompt_password("token (input hidden): ")?
    } else {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        line
    };
    let token = token.trim();
    if token.is_empty() {
        bail!("empty token");
    }
    Ok(Secret::new(token))
}

fn import_aliases(
    config: &mut Config,
    path: &Path,
    yes: bool,
    from_file: Option<&Path>,
) -> anyhow::Result<()> {
    let output = match from_file {
        Some(f) => {
            std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?
        }
        None => shell_aliases()?,
    };
    let found = accounts_from_aliases(&output);
    let (new, existing): (Vec<_>, Vec<_>) = found
        .into_iter()
        .partition(|a| config.get(&a.name).is_none());

    for a in &existing {
        println!("skip  {} (already configured)", a.name);
    }
    if new.is_empty() {
        println!("no new aliases that run claude, codex, kimi or cursor-agent");
        return Ok(());
    }
    for a in &new {
        let env: Vec<_> = a.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        println!(
            "{}  {} → {} {} {}",
            if yes { "add " } else { "would add" },
            a.name,
            env.join(" "),
            a.bin.as_deref().unwrap_or_default(),
            a.args.join(" ")
        );
    }
    if !yes {
        println!("\nrun again with --yes to write {}", path.display());
        return Ok(());
    }
    config.accounts.extend(new);
    config.save(path)?;
    println!("saved {}", path.display());
    Ok(())
}

/// `alias` output from the user's interactive shell, where aliases are defined.
pub fn shell_aliases() -> anyhow::Result<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
    let out = std::process::Command::new(&shell)
        .args(["-ic", "alias"])
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("running {shell} -ic alias"))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
