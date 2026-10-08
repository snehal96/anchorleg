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
