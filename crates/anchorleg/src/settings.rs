//! `anchorleg settings …`: per-CLI settings (`[vendor.<name>]` in `config.toml`): the model and
//! effort every account of that CLI uses, and extra arguments. Applied to that CLI only.

use anchorleg_core::registry::{Config, Vendor, VendorSettings};
use anyhow::bail;

use crate::accounts::parse_vendor;

pub struct Changes {
    pub args: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub clear: bool,
}

pub fn run(vendor: Option<String>, changes: Changes) -> anyhow::Result<()> {
    let path = Config::default_path()?;
    let mut config = Config::load(&path)?;
    let changing = changes.args.is_some()
        || changes.model.is_some()
        || changes.effort.is_some()
        || changes.clear;
    let Some(name) = vendor else {
        if changing {
            bail!("name the CLI, e.g. `anchorleg settings claude --model opus`");
        }
        for v in Vendor::ALL {
            println!("{:<7} {}", v.name(), describe(&config, v));
        }
        return Ok(());
    };
    let v = parse_vendor(&name).map_err(anyhow::Error::msg)?;
    if !changing {
        println!("{}", describe(&config, v));
        return Ok(());
    }
    if changes.clear {
        config.vendors.remove(&v);
    }
    if let Some(line) = changes.args {
        let words =
            VendorSettings::parse_args(&line).map_err(|e| anyhow::anyhow!("--args: {e}"))?;
        update(&mut config, v, |s| s.args = words);
    }
    if let Some(m) = changes.model {
        set_model(&mut config, v, &m);
    }
    if let Some(e) = changes.effort {
        set_effort(&mut config, v, &e).map_err(anyhow::Error::msg)?;
    }
    config.save(&path)?;
    println!("{:<7} {}", v.name(), describe(&config, v));
    Ok(())
}

/// `anchorleg policy [name]`: show or set how runs pick among accounts with room.
pub fn policy(new: Option<anchorleg_core::policy::Policy>) -> anyhow::Result<()> {
    let path = Config::default_path()?;
    let mut config = Config::load(&path)?;
    if let Some(p) = new {
        config.policy = p;
        config.save(&path)?;
    }
    for p in anchorleg_core::policy::Policy::ALL {
        let mark = if p == config.policy { "*" } else { " " };
        println!("{mark} {:<12} {}", p.name(), p.describe());
    }
    Ok(())
}

/// `anchorleg rules`: list, add or remove lasting allow rules.
pub fn rules(add: Vec<String>, rm: Vec<String>) -> anyhow::Result<()> {
    let path = Config::default_path()?;
    let mut config = Config::load(&path)?;
    if !add.is_empty() || !rm.is_empty() {
        for r in &rm {
            let before = config.allow.len();
            config.allow.retain(|a| a != r);
            if config.allow.len() == before {
                anyhow::bail!("no rule `{r}`");
            }
        }
        for r in add {
            let r = r.trim().to_owned();
            let ok = !r.is_empty() && !r.contains(',') && (!r.contains('(') || r.ends_with(')'));
            if !ok {
                anyhow::bail!(
                    "`{r}`: a rule is `Tool` or `Tool(pattern)`, e.g. `Bash(cargo test *)`"
                );
            }
            if !config.allow.contains(&r) {
                config.allow.push(r);
            }
        }
        config.save(&path)?;
    }
    if config.allow.is_empty() {
        println!("no rules: every tool call the CLI asks about comes to you");
    }
    for r in &config.allow {
        println!("{r}");
    }
    Ok(())
}

/// `anchorleg notify`: change what's given, then show the settings.
pub fn notify(
    desktop: Option<bool>,
    command: Option<String>,
    no_command: bool,
    test: bool,
) -> anyhow::Result<()> {
    use anchorleg_core::notify::{self, Kind, Notice};
    let path = Config::default_path()?;
    let mut config = Config::load(&path)?;
    if desktop.is_some() || command.is_some() || no_command {
        if let Some(d) = desktop {
            config.notify.desktop = d;
        }
        if let Some(c) = command {
            config.notify.command = Some(c).filter(|c| !c.trim().is_empty());
        }
        if no_command {
            config.notify.command = None;
        }
        config.save(&path)?;
    }
    let n = &config.notify;
    println!("desktop  {}", if n.desktop { "on" } else { "off" });
    println!("command  {}", n.command.as_deref().unwrap_or("(none)"));
    if test {
        notify::send(
            n,
            &Notice::new(Kind::Test, None, "anchorleg", "Notifications work."),
        );
        // The channels run on their own; give a command a moment before this process exits.
        std::thread::sleep(std::time::Duration::from_millis(500));
        println!("sent a test notice");
    }
    Ok(())
}

/// Change one CLI's settings; a section left empty is removed.
pub fn update(config: &mut Config, v: Vendor, change: impl FnOnce(&mut VendorSettings)) {
    change(config.vendors.entry(v).or_default());
    if config.vendors[&v].is_empty() {
        config.vendors.remove(&v);
    }
}

/// `default` (or nothing) goes back to the CLI's own default.
pub fn set_model(config: &mut Config, v: Vendor, model: &str) {
    let model = model.trim();
    update(config, v, |s| {
        s.model = (!model.is_empty() && model != "default").then(|| model.to_owned());
    });
}

pub fn set_effort(config: &mut Config, v: Vendor, effort: &str) -> Result<(), String> {
    let effort = effort.trim();
    let value = (!effort.is_empty() && effort != "default").then(|| effort.to_owned());
    if let (Some(e), Some(known)) = (&value, v.efforts())
        && !known.contains(&e.as_str())
    {
        return Err(format!(
            "{} effort is one of: {} (or default)",
            v.name(),
            known.join(", ")
        ));
    }
    update(config, v, |s| s.effort = value);
    Ok(())
}

fn describe(config: &Config, v: Vendor) -> String {
    match config.vendors.get(&v) {
        Some(s) => format!(
            "model: {} · effort: {} · args: {}",
            s.model.as_deref().unwrap_or("default"),
            s.effort.as_deref().unwrap_or("default"),
            if s.args.is_empty() {
                "(none)".to_owned()
            } else {
                s.args_line()
            }
        ),
        None => "(defaults)".to_owned(),
    }
}
