//! Which account to use next. Phase 1 implements `drain`: priority order, skipping accounts that
//! are blocked or nearly out. `balance` and `save-weekly` arrive in Phase 4.
//!
//! Pure: everything it needs is passed in, including the time.

use std::collections::HashMap;

use serde::Serialize;

use crate::adapters::EpochSecs;
use crate::registry::{Account, Vendor};
use crate::store::QuotaRow;

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
}

impl Default for PickOptions {
    fn default() -> Self {
        Self {
            exclude: Vec::new(),
            vendor: None,
            near_limit: 0.95,
            unknown_reset_wait: 3600,
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
    let mut earliest: Option<(&Account, EpochSecs)> = None;

    for &account in accounts {
        if !account.enabled || opts.vendor.is_some_and(|v| v != account.vendor) {
            continue;
        }
        let account_rows = rows
            .get(account.name.as_str())
            .map_or(&[][..], Vec::as_slice);
        if let Some(until) = blocked_until(account_rows, now, opts) {
            if earliest.is_none_or(|(_, t)| until < t) {
                earliest = Some((account, until));
            }
            continue;
        }
        if opts.exclude.contains(&account.name) {
            continue;
        }
        let used = max_used(account_rows);
        if used < opts.near_limit {
            return Pick::Use {
                account,
                reason: PickReason::Available,
            };
        }
        if near.is_none_or(|(_, u)| used < u) {
            near = Some((account, used));
        }
    }

    match (near, earliest) {
        (Some((account, _)), _) => Pick::Use {
            account,
            reason: PickReason::NearLimit,
        },
        (None, Some((account, at))) => Pick::WaitUntil { at, account },
        (None, None) => Pick::NoAccounts,
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
}
