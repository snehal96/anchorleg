//! `anchorleg accounts …`: manage `config.toml` and Keychain tokens.

use std::io::{BufRead, IsTerminal};
use std::os::fd::AsFd as _;
use std::path::{Path, PathBuf};

use anchorleg_core::policy::CreditsUse;
use anchorleg_core::registry::{
    Account, Config, Keychain, Secret, SecretStore, TokenFrom, Vendor, accounts_from_aliases,
    default_program, parse_command,
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
    /// Sign in a new account: give it its own folder (`~/.claude-NAME`, `~/.codex-NAME`,
    /// `~/.agy-NAME`), run the CLI's own sign-in (`claude auth login`, `codex login`, or `agy`'s
    /// sign-in screen, in a terminal) in your browser, then add it. The CLI keeps the login
    /// itself; anchorleg never sees it.
    Login(LoginArgs),
    /// Remove an account (and its Keychain token, if any).
    Rm { name: String },
    /// Change how an account is used.
    Set(SetArgs),
    /// Turn shell aliases like `claude-personal='CLAUDE_CONFIG_DIR=… claude'` into accounts.
    ImportAliases {
        /// Write the accounts. Without it, only show what would be added.
        #[arg(long)]
        yes: bool,
        /// Read `alias` output from this file instead of running `$SHELL -ic alias`.
        #[arg(long)]
        from_file: Option<PathBuf>,
        /// Print what would be (or was) added and skipped as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
pub struct LoginArgs {
    name: String,
    /// claude, codex or antigravity (agy).
    #[arg(long, value_parser = parse_vendor, default_value = "claude")]
    cli: Vendor,
    /// The CLI binary to sign in with and run the account with. Defaults to the one on PATH
    /// (for Codex, the ChatGPT app's copy when there's none on PATH).
    #[arg(long)]
    bin: Option<String>,
    /// Print `{"name", "cli", "dir"}` when it's added; the sign-in's own output goes to stderr.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct SetArgs {
    name: String,
    /// Lower is used first (with the `drain` policy, and to break ties).
    #[arg(long)]
    priority: Option<u32>,
    /// Keep this fraction of the weekly window for when nothing else is free, e.g. 0.2;
    /// 0 turns it off.
    #[arg(long, value_parser = parse_reserve)]
    reserve_7d: Option<f64>,
    /// This account's own limit, e.g. 0.4: a session on it stops once any quota window reaches
    /// it and moves to another account; past it, the account is used only when no other has
    /// room. 0 turns it off.
    #[arg(long, value_parser = parse_limit)]
    stop_at: Option<f64>,
    /// With --stop-at: never go past it (wait for the window to reset instead of carrying on
    /// when no other account has room).
    #[arg(long)]
    strict: Option<bool>,
    /// Use this account first whenever it has room, whatever the policy.
    #[arg(long)]
    pinned: Option<bool>,
    /// Use the account (`true`) or keep it configured but never pick it (`false`).
    #[arg(long)]
    enabled: Option<bool>,
    /// Paid usage credits past the plan's limits: never (default), last-resort or allow.
    #[arg(long, value_parser = parse_credits)]
    credits: Option<CreditsUse>,
}

fn parse_credits(s: &str) -> Result<CreditsUse, String> {
    s.parse()
}

fn parse_limit(s: &str) -> Result<f64, String> {
    match s.parse::<f64>() {
        Ok(f) if (0.0..=1.0).contains(&f) => Ok(f),
        _ => Err("expected a fraction from 0 to 1, e.g. 0.4 (0 turns it off)".to_owned()),
    }
}

fn parse_reserve(s: &str) -> Result<f64, String> {
    match s.parse::<f64>() {
        Ok(f) if (0.0..1.0).contains(&f) => Ok(f),
        _ => Err("expected a fraction from 0 up to (not including) 1, e.g. 0.2".to_owned()),
    }
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
    /// "CLAUDE_CONFIG_DIR=~/.claude-personal ~/.local/bin/claude".
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
        AccountsCmd::Login(args) => login(&mut config, &path, args),
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
        AccountsCmd::Set(args) => {
            let account = config
                .accounts
                .iter_mut()
                .find(|a| a.name == args.name)
                .with_context(|| format!("no account `{}`", args.name))?;
            if let Some(p) = args.priority {
                account.priority = p;
            }
            if let Some(r) = args.reserve_7d {
                account.reserve_7d = (r > 0.0).then_some(r);
            }
            if let Some(s) = args.stop_at {
                account.stop_at = (s > 0.0).then_some(s);
            }
            if let Some(s) = args.strict {
                account.strict = s;
            }
            if account.stop_at.is_none() {
                account.strict = false;
            }
            if let Some(p) = args.pinned {
                account.pinned = p;
            }
            if let Some(e) = args.enabled {
                account.enabled = e;
            }
            if let Some(c) = args.credits {
                account.credits = c;
            }
            let line = describe_use(account);
            config.save(&path)?;
            println!("{}: {line}", args.name);
            Ok(())
        }
        AccountsCmd::ImportAliases {
            yes,
            from_file,
            json,
        } => import_aliases(&mut config, &path, yes, from_file.as_deref(), json),
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
        let extra = [
            (!a.enabled).then(|| "off".to_owned()),
            a.pinned.then(|| "pinned".to_owned()),
            a.reserve_7d
                .map(|r| format!("weekly reserve {:.0}%", r * 100.0)),
            a.stop_at.map(|s| {
                format!(
                    "stops at {:.0}%{}",
                    s * 100.0,
                    if a.strict { " (strict)" } else { "" }
                )
            }),
            (a.credits != CreditsUse::Never).then(|| format!("credits: {}", a.credits.name())),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let extra = if extra.is_empty() {
            String::new()
        } else {
            format!("  [{}]", extra.join(", "))
        };
        println!(
            "{:>4}  {:<16} {:<7} {how}{extra}",
            a.priority,
            a.name,
            vendor_name(a.vendor)
        );
    }
    Ok(())
}

/// How the policy treats an account, in a few words.
fn describe_use(a: &anchorleg_core::registry::Account) -> String {
    let mut parts = vec![format!("priority {}", a.priority)];
    if !a.enabled {
        parts.push("off".to_owned());
    }
    if a.pinned {
        parts.push("pinned".to_owned());
    }
    if let Some(r) = a.reserve_7d {
        parts.push(format!("weekly reserve {:.0}%", r * 100.0));
    }
    if let Some(s) = a.stop_at {
        parts.push(format!(
            "stops at {:.0}%{}",
            s * 100.0,
            if a.strict { " (strict)" } else { "" }
        ));
    }
    parts.push(format!("credits {}", a.credits.name()));
    parts.join(", ")
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

/// The env var that points a CLI at an account's folder, and the folder's name prefix.
fn login_dir(cli: Vendor) -> anyhow::Result<(&'static str, &'static str)> {
    match cli {
        Vendor::Claude => Ok(("CLAUDE_CONFIG_DIR", ".claude-")),
        Vendor::Codex => Ok(("CODEX_HOME", ".codex-")),
        // agy has no narrower setting than HOME; in a HOME of its own it can't reach the
        // Keychain and keeps the login as a file there.
        Vendor::Antigravity => Ok(("HOME", ".agy-")),
        other => bail!(
            "anchorleg can't sign in to {} for you yet: sign in with its own app, then `anchorleg accounts add`",
            other.name()
        ),
    }
}

fn login(config: &mut Config, path: &Path, args: LoginArgs) -> anyhow::Result<()> {
    if config.get(&args.name).is_some() {
        bail!("account `{}` already exists", args.name);
    }
    if args.name.is_empty()
        || !args
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("an account name is letters, digits, `-` and `_`");
    }
    let (var, prefix) = login_dir(args.cli)?;
    let home = dirs_home()?;
    let folder = format!("~/{prefix}{}", args.name);
    let dir = home.join(format!("{prefix}{}", args.name));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let program = args
        .bin
        .clone()
        .unwrap_or_else(|| default_program(args.cli));

    let (sign_in, check): (&[&str], &[&str]) = match args.cli {
        Vendor::Codex => (&["login"], &["login", "status"]),
        // agy signs in from its own screen, started with no arguments.
        Vendor::Antigravity => (&[], &["models"]),
        _ => (&["auth", "login"], &["auth", "status", "--json"]),
    };
    if args.cli == Vendor::Antigravity {
        if !std::io::stdin().is_terminal() {
            bail!(
                "Antigravity signs in from its own screen: run `anchorleg accounts login {} --cli agy` in a terminal",
                args.name
            );
        }
        eprintln!(
            "agy opens next: sign in there (it opens your browser), then quit agy to finish."
        );
    }
    // With --json, stdout is for the result: the sign-in's own messages (its link, if the
    // browser didn't open) go to stderr.
    let out = if args.json {
        std::process::Stdio::from(std::io::stderr().as_fd().try_clone_to_owned()?)
    } else {
        std::process::Stdio::inherit()
    };
    let status = std::process::Command::new(&program)
        .args(sign_in)
        .env(var, &dir)
        .stdout(out)
        .status()
        .with_context(|| format!("running {program}"))?;
    if !status.success() {
        bail!("sign-in didn't finish ({status}); nothing was added");
    }
    let checked = std::process::Command::new(&program)
        .args(check)
        .env(var, &dir)
        .stderr(std::process::Stdio::null())
        .output()
        .with_context(|| format!("running {program}"))?;
    if !signed_in(args.cli, &checked) {
        bail!("{program} doesn't report a sign-in in {folder}; nothing was added");
    }

    let mut account = Account::new(args.name.clone(), args.cli);
    account.env.insert(var.to_owned(), folder.clone());
    account.bin = args.bin;
    let mut candidate = config.clone();
    candidate.accounts.push(account.clone());
    candidate.validate()?;
    config.accounts.push(account);
    config.save(path)?;
    if args.json {
        println!(
            "{}",
            serde_json::json!({ "name": args.name, "cli": args.cli.name(), "dir": folder })
        );
    } else {
        println!("signed in; added {} ({folder})", args.name);
    }
    Ok(())
}

/// `claude auth status --json` says `"loggedIn": true`; `codex login status` and `agy models`
/// exit 0 (signed out: exit 1).
fn signed_in(cli: Vendor, out: &std::process::Output) -> bool {
    match cli {
        Vendor::Codex | Vendor::Antigravity => out.status.success(),
        _ => serde_json::from_slice::<serde_json::Value>(&out.stdout)
            .is_ok_and(|v| v["loggedIn"] == serde_json::Value::Bool(true)),
    }
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
    json: bool,
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

    if json {
        if yes && !new.is_empty() {
            config.accounts.extend(new.iter().cloned());
            config.save(path)?;
        }
        println!(
            "{}",
            serde_json::json!({
                "schema": 1,
                "add": new,
                "skip": existing.iter().map(|a| &a.name).collect::<Vec<_>>(),
                "saved": yes && !new.is_empty(),
            })
        );
        return Ok(());
    }
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
