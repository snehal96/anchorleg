//! `anchorleg usage`: tokens and cost of anchorleg's runs, with quota and credits beside them.

use anchorleg_core::output::{self, SCHEMA, UsageReport};
use anchorleg_core::policy::PickOptions;
use anchorleg_core::registry::Config;
use anchorleg_core::store::{Store, UsageTotal};

use crate::now;
use crate::status::{ago, credits_cell};

/// `24h`, `7d`, `30d`, `all` → the earliest time to count from (Unix seconds).
pub fn parse_since(s: &str) -> Result<i64, String> {
    if s == "all" {
        return Ok(0);
    }
    let (n, unit) = s.split_at(s.len().saturating_sub(1));
    let secs = match unit {
        "h" => 3600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        _ => return Err("expected e.g. 24h, 7d, 4w or all".to_owned()),
    };
    match n.parse::<i64>() {
        Ok(n) if n > 0 => Ok(now() - n * secs),
        _ => Err("expected e.g. 24h, 7d, 4w or all".to_owned()),
    }
}

pub fn run(since: i64, json: bool) -> anyhow::Result<()> {
    let store = Store::open(&Store::default_path()?)?;
    let report = UsageReport {
        schema: SCHEMA,
        now: now(),
        since,
        totals: store.usage_totals(since)?,
        by_day: store.usage_by_day(since)?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let config = Config::load(&Config::default_path()?)?;
    let opts = PickOptions {
        policy: config.policy,
        ..PickOptions::default()
    };
    let status = output::status(
        &config,
        &store.quota(None)?,
        &store.credits(None)?,
        report.now,
        &opts,
    );
    print(&report, &status);
    Ok(())
}

fn print(r: &UsageReport, status: &output::StatusReport) {
    let span = if r.since == 0 {
        "all time".to_owned()
    } else {
        let secs = r.now - r.since + 30;
        match (secs / 86_400, secs % 86_400 < 120, secs / 3600) {
            (1, true, _) => "last 24 hours".to_owned(),
            (d, true, _) if d > 1 => format!("last {d} days"),
            (_, _, h) if secs % 3600 < 120 => format!("last {h} hours"),
            _ => format!("last {}", ago(r.now - r.since)),
        }
    };
    println!("anchorleg usage, {span}\n");

    if r.totals.is_empty() {
        println!("  no usage recorded yet (it is kept for runs from this version on)\n");
    } else {
        println!(
            "  {:<14} {:<22} {:>5} {:>8} {:>8} {:>9} {:>9} {:>10}",
            "ACCOUNT", "MODEL", "RUNS", "INPUT", "OUTPUT", "CACHE RD", "CACHE WR", "API PRICE"
        );
        let mut sum = UsageTotal::default();
        for t in &r.totals {
            row(&t.account, t.model.as_deref().unwrap_or("-"), t);
            sum.launches += t.launches;
            sum.input_tokens += t.input_tokens;
            sum.output_tokens += t.output_tokens;
            sum.cache_read_tokens += t.cache_read_tokens;
            sum.cache_write_tokens += t.cache_write_tokens;
            sum.cost_usd = add(sum.cost_usd, t.cost_usd);
            sum.credits_cost_usd = add(sum.credits_cost_usd, t.credits_cost_usd);
        }
        if r.totals.len() > 1 {
            row("total", "", &sum);
        }
        if let Some(c) = sum.credits_cost_usd.filter(|c| *c > 0.0) {
            println!("\n  ran on paid credits: {}", money(Some(c)));
        }
        println!(
            "\n  API PRICE: what the same tokens would cost at API list prices (Claude reports it;\n  Codex and Antigravity don't). On a subscription you don't pay it."
        );
        if !r.by_day.is_empty() {
            let tokens: Vec<u64> = r.by_day.iter().map(|d| d.1).collect();
            println!(
                "\n  tokens per day  {}  (peak {})",
                spark(&tokens),
                count(tokens.iter().copied().max().unwrap_or(0))
            );
        }
        println!();
    }

    println!("  quota now");
    for a in &status.accounts {
        let window = |name: &str| {
            a.windows
                .iter()
                .find(|w| w.window == name)
                .map_or("  -".to_owned(), |w| {
                    let pct = w
                        .used
                        .map_or("?".to_owned(), |u| format!("{:.0}%", u * 100.0));
                    format!("{} {pct:>4}", bar(w.used.unwrap_or(0.0)))
                })
        };
        let mut line = format!(
            "  {:<14} 5h {}   7d {}",
            a.name,
            window("five_hour"),
            window("seven_day")
        );
        if let Some(t) = a.blocked_until {
            line.push_str(&format!("   blocked {}", ago(t - status.now)));
        }
        if let Some(c) = &a.credits {
            line.push_str(&format!("   credits {}", credits_cell(c, a.credits_use)));
        }
        println!("{line}");
    }
}

fn row(account: &str, model: &str, t: &UsageTotal) {
    println!(
        "  {:<14} {:<22} {:>5} {:>8} {:>8} {:>9} {:>9} {:>10}",
        account,
        model,
        t.launches,
        count(t.input_tokens),
        count(t.output_tokens),
        count(t.cache_read_tokens),
        count(t.cache_write_tokens),
        money(t.cost_usd)
    );
}

fn add(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    }
}

fn money(c: Option<f64>) -> String {
    match c {
        None => "-".to_owned(),
        Some(c) if c < 0.01 && c > 0.0 => "<$0.01".to_owned(),
        Some(c) => format!("${c:.2}"),
    }
}

/// `950`, `12.3k`, `4.1M`.
pub fn count(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let f = n as f64;
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", f / 1e3),
        _ => format!("{:.1}M", f / 1e6),
    }
}

/// Ten-cell bar for a 0–1 fraction.
fn bar(used: f64) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let full = (used.clamp(0.0, 1.0) * 10.0).round() as usize;
    format!("{}{}", "█".repeat(full), "░".repeat(10 - full))
}

/// One block character per value, scaled to the largest.
pub fn spark(values: &[u64]) -> String {
    const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = values.iter().copied().max().unwrap_or(0).max(1);
    values
        .iter()
        .map(|&v| {
            #[allow(clippy::cast_possible_truncation)]
            let i = (v.saturating_mul(7) / max) as usize;
            LEVELS[i.min(7)]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(count(950), "950");
        assert_eq!(count(12_345), "12.3k");
        assert_eq!(count(4_100_000), "4.1M");
        assert_eq!(money(Some(0.004)), "<$0.01");
        assert_eq!(money(Some(1.5)), "$1.50");
        assert_eq!(bar(0.5), "█████░░░░░");
        assert_eq!(spark(&[0, 7, 14]), "▁▄█");
        assert!(parse_since("7d").is_ok());
        assert_eq!(parse_since("all"), Ok(0));
        assert!(parse_since("7x").is_err());
    }
}
