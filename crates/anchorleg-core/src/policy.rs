//! Which account to use next (D29). Every policy skips accounts that are blocked, and uses an
//! account that is nearly out (or into its weekly reserve) only when nothing healthier is free.
//! Among the healthy ones:
//!
//! - `drain`: priority order. Spends one account before touching the next.
//! - `balance`: the least used (fullest window), so accounts wear evenly.
//! - `save-weekly`: the most weekly (7-day) room left, so the scarce weekly windows last; the
//!   5-hour windows reset soon anyway.
//!
//! A preferred account (`--prefer`, or `pinned` in the config) goes first whenever it's healthy.
//! Ties keep priority order. Pure: everything it needs is passed in, including the time.

use std::collections::HashMap;

use serde::Serialize;

use crate::adapters::EpochSecs;
use crate::registry::{Account, Vendor};
use crate::store::QuotaRow;

/// How to choose among healthy accounts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Policy {
    #[default]
    Drain,
    Balance,
    SaveWeekly,
}

impl Policy {
    pub const ALL: [Policy; 3] = [Self::Drain, Self::Balance, Self::SaveWeekly];

    pub fn name(self) -> &'static str {
        match self {
            Self::Drain => "drain",
            Self::Balance => "balance",
            Self::SaveWeekly => "save-weekly",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Drain => "priority order: use one account up before the next",
            Self::Balance => "least used first, so accounts wear evenly",
            Self::SaveWeekly => "most weekly room first, so weekly limits last",
        }
    }
}

impl std::str::FromStr for Policy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "drain" | "priority" => Ok(Self::Drain),
            "balance" => Ok(Self::Balance),
            "save-weekly" | "save_weekly" => Ok(Self::SaveWeekly),
            other => Err(format!(
                "unknown policy {other:?} (drain, balance or save-weekly)"
            )),
        }
    }
}

/// Whether an account may keep working on paid usage credits (Claude "extra usage", Codex
/// credits) once its plan's windows run out. Spending money is never the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CreditsUse {
    /// Stop as soon as the CLI starts paying from credits, and switch or wait instead.
    #[default]
    Never,
    /// Only when every other account is out: better to pay than to wait for a reset.
    LastResort,
    /// Keep going on credits instead of switching mid-task.
    Allow,
}

impl CreditsUse {
    pub const ALL: [CreditsUse; 3] = [Self::Never, Self::LastResort, Self::Allow];

    pub fn name(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::LastResort => "last-resort",
            Self::Allow => "allow",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Never => "never spend credits; switch or wait instead",
            Self::LastResort => "spend credits only when every other account is out",
            Self::Allow => "keep going on credits instead of switching",
        }
    }
}

impl std::str::FromStr for CreditsUse {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "never" | "off" => Ok(Self::Never),
            "last-resort" | "last_resort" => Ok(Self::LastResort),
            "allow" | "on" => Ok(Self::Allow),
            other => Err(format!(
                "unknown credits setting {other:?} (never, last-resort or allow)"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PickOptions {
    /// Not eligible right now (e.g. the account that just hit its limit).
    pub exclude: Vec<String>,
    /// Only consider this vendor (Phase 1 is Claude-only).
    pub vendor: Option<Vendor>,
    /// At or above this fraction used in any window, an account is only used when nothing
    /// healthier is available. Matches the hard 95% switch rule.
    pub near_limit: f64,
    /// How long to treat a rejected account as blocked when the CLI gave no reset time.
    pub unknown_reset_wait: i64,
    pub policy: Policy,
    /// Use this account first whenever it's healthy (`--prefer`).
    pub prefer: Option<String>,
    /// Only ever use this account (`--account`).
    pub only: Option<String>,
    /// Accounts the CLI last reported as able to pay from usage credits. A blocked one among them
    /// is used (if its `credits` setting allows) before waiting for a reset.
    pub has_credits: Vec<String>,
}

impl Default for PickOptions {
    fn default() -> Self {
        Self {
            exclude: Vec::new(),
            vendor: None,
            near_limit: 0.95,
            unknown_reset_wait: 3600,
            policy: Policy::Drain,
            prefer: None,
            only: None,
            has_credits: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PickReason {
    /// Highest-priority account with room left.
    Available,
    /// Every unblocked account is near its limit; this is the best of them.
    NearLimit,
    /// Every account is out; this one can go on paying from usage credits.
    Credits,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pick<'a> {
    Use {
        account: &'a Account,
        reason: PickReason,
    },
    /// Every eligible account is blocked; the earliest one frees up at this time.
    WaitUntil { at: EpochSecs, account: &'a Account },
    /// No account matches (none configured, or all excluded / other vendors).
    NoAccounts,
}

/// When the account stops being blocked, or `None` if it isn't blocked at `now`.
pub fn blocked_until(rows: &[&QuotaRow], now: EpochSecs, opts: &PickOptions) -> Option<EpochSecs> {
    rows.iter()
        .filter(|r| r.status.as_deref() == Some("rejected"))
        .map(|r| {
            r.resets_at
                .unwrap_or(r.updated_at + opts.unknown_reset_wait)
        })
        .filter(|&until| until > now)
        .max()
}

fn max_used(rows: &[&QuotaRow]) -> f64 {
    rows.iter().filter_map(|r| r.used).fold(0.0, f64::max)
}

/// The weekly (7-day) window's use; 0 when unknown.
fn weekly_used(rows: &[&QuotaRow]) -> f64 {
    rows.iter()
        .filter(|r| r.window == "seven_day")
        .filter_map(|r| r.used)
        .fold(0.0, f64::max)
}

/// Pick the account to run on. `accounts` must already be in priority order
/// (see [`crate::registry::Config::by_priority`]).
pub fn pick<'a>(
    accounts: &[&'a Account],
    quota: &[QuotaRow],
    now: EpochSecs,
    opts: &PickOptions,
) -> Pick<'a> {
    let mut rows: HashMap<&str, Vec<&QuotaRow>> = HashMap::new();
    for r in quota {
        rows.entry(r.account.as_str()).or_default().push(r);
    }

    let mut near: Option<(&Account, f64)> = None;
    // First blocked account (priority order) that may pay from credits.
    let mut on_credits: Option<&Account> = None;
    let mut earliest: Option<(&Account, EpochSecs)> = None;
    // Healthy accounts, in priority order, with (max used, weekly used).
    let mut healthy: Vec<(&Account, f64, f64)> = Vec::new();

    for &account in accounts {
        if !account.enabled
            || opts.vendor.is_some_and(|v| v != account.vendor)
            || opts.only.as_ref().is_some_and(|only| *only != account.name)
        {
            continue;
        }
        let account_rows = rows
            .get(account.name.as_str())
            .map_or(&[][..], Vec::as_slice);
        if let Some(until) = blocked_until(account_rows, now, opts) {
            if on_credits.is_none()
                && account.credits != CreditsUse::Never
                && opts.has_credits.contains(&account.name)
            {
                on_credits = Some(account);
            }
            if earliest.is_none_or(|(_, t)| until < t) {
                earliest = Some((account, until));
            }
            continue;
        }
        if opts.exclude.contains(&account.name) {
            continue;
        }
        let used = max_used(account_rows);
        let weekly = weekly_used(account_rows);
        // `reserve_7d = 0.2` keeps the last 20% of the weekly window for when nothing else is free.
        let into_reserve = account
            .reserve_7d
            .is_some_and(|reserve| weekly >= 1.0 - reserve);
        // Its own `stop_at` (e.g. 0.4) counts like the near-limit line, for this account only.
        let limit = account
            .stop_at
            .map_or(opts.near_limit, |own| own.min(opts.near_limit));
        if used < limit && !into_reserve {
            healthy.push((account, used, weekly));
            continue;
        }
        if near.is_none_or(|(_, u)| used < u) {
            near = Some((account, used));
        }
    }

    let preferred = healthy.iter().find(|(a, ..)| {
        opts.prefer.as_ref() == Some(&a.name) || (opts.prefer.is_none() && a.pinned)
    });
    // `min_by` keeps the first of equals, so ties stay in priority order.
    let chosen = preferred.or_else(|| match opts.policy {
        Policy::Drain => healthy.first(),
        Policy::Balance => healthy.iter().min_by(|x, y| x.1.total_cmp(&y.1)),
        Policy::SaveWeekly => healthy
            .iter()
            .min_by(|x, y| x.2.total_cmp(&y.2).then(x.1.total_cmp(&y.1))),
    });
    if let Some((account, ..)) = chosen {
        return Pick::Use {
            account,
            reason: PickReason::Available,
        };
    }

    match (near, on_credits, earliest) {
        (Some((account, _)), ..) => Pick::Use {
            account,
            reason: PickReason::NearLimit,
        },
        (None, Some(account), _) => Pick::Use {
            account,
            reason: PickReason::Credits,
        },
        (None, None, Some((account, at))) => Pick::WaitUntil { at, account },
        (None, None, None) => Pick::NoAccounts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: EpochSecs = 10_000;

    fn acct(name: &str, vendor: Vendor) -> Account {
        Account::new(name, vendor)
    }

    fn row(
        account: &str,
        window: &str,
        status: Option<&str>,
        used: Option<f64>,
        resets_at: Option<EpochSecs>,
    ) -> QuotaRow {
        QuotaRow {
            account: account.into(),
            window: window.into(),
            status: status.map(Into::into),
            used,
            resets_at,
            source: "stream".into(),
            updated_at: NOW - 100,
        }
    }

    fn name(p: &Pick) -> String {
        match p {
            Pick::Use { account, reason } => format!("use {} ({reason:?})", account.name),
            Pick::WaitUntil { at, account } => format!("wait {} until {at}", account.name),
            Pick::NoAccounts => "none".into(),
        }
    }

    #[test]
    fn table() {
        let a = acct("a", Vendor::Claude);
        let b = acct("b", Vendor::Claude);
        let c = acct("c", Vendor::Codex);
        let all = [&a, &b, &c];

        let cases: Vec<(&str, Vec<QuotaRow>, PickOptions, &str)> = vec![
            (
                "no data: first by priority",
                vec![],
                PickOptions::default(),
                "use a (Available)",
            ),
            (
                "a rejected until later: skip to b",
                vec![row(
                    "a",
                    "five_hour",
                    Some("rejected"),
                    None,
                    Some(NOW + 50),
                )],
                PickOptions::default(),
                "use b (Available)",
            ),
            (
                "a's reset time passed: a again",
                vec![row("a", "five_hour", Some("rejected"), None, Some(NOW))],
                PickOptions::default(),
                "use a (Available)",
            ),
            (
                "rejected with no reset: blocked for the fallback period",
                vec![row("a", "unknown", Some("rejected"), None, None)],
                PickOptions::default(),
                "use b (Available)",
            ),
            (
                "rejected with no reset, fallback over",
                vec![row("a", "unknown", Some("rejected"), None, None)],
                PickOptions {
                    unknown_reset_wait: 50,
                    ..PickOptions::default()
                },
                "use a (Available)",
            ),
            (
                "a weekly at 96%: prefer b",
                vec![row("a", "seven_day", Some("allowed"), Some(0.96), None)],
                PickOptions::default(),
                "use b (Available)",
            ),
            (
                "everyone near the limit: least used",
                vec![
                    row("a", "five_hour", None, Some(0.99), None),
                    row("b", "five_hour", None, Some(0.97), None),
                    row("c", "five_hour", None, Some(0.98), None),
                ],
                PickOptions::default(),
                "use b (NearLimit)",
            ),
            (
                "near the limit beats waiting",
                vec![
                    row("a", "five_hour", Some("rejected"), None, Some(NOW + 50)),
                    row("b", "five_hour", None, Some(0.99), None),
                ],
                PickOptions {
                    vendor: Some(Vendor::Claude),
                    ..PickOptions::default()
                },
                "use b (NearLimit)",
            ),
            (
                "all blocked: wait for the earliest reset",
                vec![
                    row("a", "five_hour", Some("rejected"), None, Some(NOW + 500)),
                    row("b", "five_hour", Some("rejected"), None, Some(NOW + 200)),
                    row(
                        "b",
                        "seven_day",
                        Some("allowed"),
                        Some(0.5),
                        Some(NOW + 9000),
                    ),
                ],
                PickOptions {
                    vendor: Some(Vendor::Claude),
                    ..PickOptions::default()
                },
                "wait b until 10200",
            ),
            (
                "account blocked in two windows waits for the later one",
                vec![
                    row("a", "five_hour", Some("rejected"), None, Some(NOW + 100)),
                    row("a", "seven_day", Some("rejected"), None, Some(NOW + 9000)),
                    row("b", "five_hour", Some("rejected"), None, Some(NOW + 500)),
                ],
                PickOptions {
                    vendor: Some(Vendor::Claude),
                    ..PickOptions::default()
                },
                "wait b until 10500",
            ),
            (
                "exclude the account that just failed",
                vec![],
                PickOptions {
                    exclude: vec!["a".into()],
                    ..PickOptions::default()
                },
                "use b (Available)",
            ),
            (
                "vendor filter",
                vec![],
                PickOptions {
                    vendor: Some(Vendor::Codex),
                    ..PickOptions::default()
                },
                "use c (Available)",
            ),
            (
                "everything excluded",
                vec![],
                PickOptions {
                    exclude: vec!["a".into(), "b".into()],
                    vendor: Some(Vendor::Claude),
                    ..PickOptions::default()
                },
                "none",
            ),
            (
                "rows for unknown accounts are ignored",
                vec![row(
                    "ghost",
                    "five_hour",
                    Some("rejected"),
                    None,
                    Some(NOW + 5),
                )],
                PickOptions::default(),
                "use a (Available)",
            ),
        ];

        for (label, quota, opts, want) in cases {
            assert_eq!(name(&pick(&all, &quota, NOW, &opts)), want, "{label}");
        }
    }

    #[test]
    fn policies_choose_among_healthy_accounts() {
        let a = acct("a", Vendor::Claude);
        let b = acct("b", Vendor::Claude);
        let c = acct("c", Vendor::Claude);
        let all = [&a, &b, &c];
        // a: 5h 60%, weekly 70%. b: 5h 20%, weekly 80%. c: 5h 40%, weekly 30%.
        let quota = vec![
            row("a", "five_hour", None, Some(0.6), None),
            row("a", "seven_day", None, Some(0.7), None),
            row("b", "five_hour", None, Some(0.2), None),
            row("b", "seven_day", None, Some(0.8), None),
            row("c", "five_hour", None, Some(0.4), None),
            row("c", "seven_day", None, Some(0.3), None),
        ];
        let with = |policy| PickOptions {
            policy,
            ..PickOptions::default()
        };
        let pick_name = |o: &PickOptions| name(&pick(&all, &quota, NOW, o));
        assert_eq!(pick_name(&with(Policy::Drain)), "use a (Available)");
        // Fullest window: a 70%, b 80%, c 40%.
        assert_eq!(pick_name(&with(Policy::Balance)), "use c (Available)");
        assert_eq!(pick_name(&with(Policy::SaveWeekly)), "use c (Available)");

        // No data counts as unused; ties keep priority order.
        assert_eq!(
            name(&pick(&all, &[], NOW, &with(Policy::Balance))),
            "use a (Available)"
        );

        // Preferred wins while healthy, under any policy.
        let prefer_b = PickOptions {
            prefer: Some("b".into()),
            ..with(Policy::Balance)
        };
        assert_eq!(pick_name(&prefer_b), "use b (Available)");
        // A preferred account that's nearly out doesn't.
        let mut tight = quota.clone();
        tight.push(row("b", "five_hour", None, Some(0.97), None));
        assert_eq!(
            name(&pick(&all, &tight, NOW, &prefer_b)),
            "use c (Available)"
        );

        // `--account`: only that one, even when it's the busiest.
        let only_a = PickOptions {
            only: Some("a".into()),
            ..with(Policy::Balance)
        };
        assert_eq!(pick_name(&only_a), "use a (Available)");
    }

    #[test]
    fn weekly_reserve_and_pinned() {
        let mut a = acct("a", Vendor::Claude);
        a.reserve_7d = Some(0.25);
        let mut b = acct("b", Vendor::Claude);
        let quota = vec![
            row("a", "seven_day", None, Some(0.8), None),
            row("b", "five_hour", None, Some(0.9), None),
        ];
        // a is into its last 25% of the week: b goes first, though b's 5h is fuller.
        assert_eq!(
            name(&pick(&[&a, &b], &quota, NOW, &PickOptions::default())),
            "use b (Available)"
        );
        // Nothing else free: the reserve is used after all.
        b.enabled = false;
        assert_eq!(
            name(&pick(&[&a, &b], &quota, NOW, &PickOptions::default())),
            "use a (NearLimit)"
        );
        b.enabled = true;
        b.pinned = true;
        assert_eq!(
            name(&pick(&[&a, &b], &[], NOW, &PickOptions::default())),
            "use b (Available)"
        );
        assert_eq!("save-weekly".parse::<Policy>(), Ok(Policy::SaveWeekly));

        // An account past its own limit goes after the others, and is still used when nothing
        // else is free.
        let mut a = acct("a", Vendor::Claude);
        a.stop_at = Some(0.4);
        let b = acct("b", Vendor::Claude);
        let quota = vec![
            row("a", "five_hour", None, Some(0.45), None),
            row("b", "five_hour", None, Some(0.7), None),
        ];
        let drain = PickOptions::default();
        assert_eq!(
            name(&pick(&[&a, &b], &quota, NOW, &drain)),
            "use b (Available)"
        );
        assert_eq!(name(&pick(&[&a], &quota, NOW, &drain)), "use a (NearLimit)");
        assert_eq!(a.stop_at(0.9), 0.4);
        assert_eq!(a.stop_at(0.3), 0.3);
        assert_eq!(a.stop_at(0.0), 0.4);
        assert_eq!(b.stop_at(0.9), 0.9);
        assert!("fastest".parse::<Policy>().is_err());
    }

    #[test]
    fn disabled_accounts_are_skipped() {
        let mut a = acct("a", Vendor::Claude);
        a.enabled = false;
        let b = acct("b", Vendor::Claude);
        let p = pick(&[&a, &b], &[], NOW, &PickOptions::default());
        assert_eq!(name(&p), "use b (Available)");
        assert_eq!(
            pick(&[&a], &[], NOW, &PickOptions::default()),
            Pick::NoAccounts
        );
    }

    #[test]
    fn no_accounts() {
        assert_eq!(
            pick(&[], &[], NOW, &PickOptions::default()),
            Pick::NoAccounts
        );
    }

    #[test]
    fn credits_are_a_fallback_before_waiting() {
        let mut a = acct("a", Vendor::Claude);
        let b = acct("b", Vendor::Claude);
        let quota = vec![
            row("a", "five_hour", Some("rejected"), None, Some(NOW + 50)),
            row("b", "five_hour", Some("rejected"), None, Some(NOW + 20)),
        ];
        let opts = PickOptions {
            has_credits: vec!["a".into()],
            ..PickOptions::default()
        };
        // Default `never`: credits aren't touched, wait for b.
        assert_eq!(
            name(&pick(&[&a, &b], &quota, NOW, &opts)),
            format!("wait b until {}", NOW + 20)
        );
        a.credits = CreditsUse::LastResort;
        assert_eq!(
            name(&pick(&[&a, &b], &quota, NOW, &opts)),
            "use a (Credits)"
        );
        // An account with room still comes first.
        let room = vec![quota[0].clone()];
        assert_eq!(
            name(&pick(&[&a, &b], &room, NOW, &opts)),
            "use b (Available)"
        );
        // Credits the CLI hasn't reported as available don't count.
        let none = PickOptions::default();
        assert_eq!(
            name(&pick(&[&a, &b], &quota, NOW, &none)),
            format!("wait b until {}", NOW + 20)
        );
    }
}
