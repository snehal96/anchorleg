//! Claude quota read between launches with `claude -p /usage`: a built-in command answered
//! locally, with no model request (`num_turns` 0, no tokens). It sees use from anywhere — an
//! interactive session, another machine — which the mod's readings during anchorleg's own runs
//! can't.
//!
//! The answer is text meant for people:
//!
//! ```text
//! Current session: 12% used · resets Oct 13 at 2:59pm (Europe/Berlin)
//! Current week (all models): 100% used · resets Oct 13 at 2:29pm (Europe/Berlin)
//! ```
//!
//! so unknown lines are skipped, and a reset time that doesn't parse leaves `resets_at` empty.

use std::time::Duration;

use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;

use super::{EpochSecs, QuotaReading, QuotaStatus, Window};

/// The windows in `/usage`'s text, read at `now`. `Err` when it has none (e.g. an API-key login).
pub fn parse_usage(text: &str, now: EpochSecs) -> Result<Vec<QuotaReading>, String> {
    let readings: Vec<QuotaReading> = text
        .lines()
        .filter_map(|line| {
            let (label, rest) = line.trim().split_once(": ")?;
            let window = match label {
                "Current session" => Window::FiveHour,
                "Current week (all models)" => Window::SevenDay,
                _ => return None,
            };
            let (pct, rest) = rest.split_once("% used")?;
            let used = pct.trim().parse::<f64>().ok()? / 100.0;
            let resets_at = rest
                .split_once("resets ")
                .and_then(|(_, when)| reset_time(when, now));
            Some(QuotaReading {
                window: Some(window),
                status: Some(if used >= 1.0 {
                    QuotaStatus::Rejected
                } else {
                    QuotaStatus::Allowed
                }),
                used: Some(used),
                resets_at,
            })
        })
        .collect();
    if readings.is_empty() {
        return Err("`/usage` listed no plan limits".to_owned());
    }
    Ok(readings)
}

/// `Oct 13 at 2:29pm (Europe/Berlin)` or `3pm (Europe/Berlin)` → Unix seconds. No year is given:
/// the one that puts it nearest `now` (a reset is at most a week away).
fn reset_time(when: &str, now: EpochSecs) -> Option<EpochSecs> {
    let (when, tz) = match when
        .trim()
        .strip_suffix(')')
        .and_then(|w| w.rsplit_once(" ("))
    {
        Some((w, zone)) => (w, TimeZone::get(zone).ok()?),
        None => (when.trim(), TimeZone::system()),
    };
    let (day, time) = match when.split_once(" at ") {
        Some((day, time)) => (Some(day), time),
        None => (None, when),
    };
    let time = clock(time)?;
    let now_ts = jiff::Timestamp::from_second(now).ok()?;
    let today = now_ts.to_zoned(tz.clone()).date();
    let candidates: Vec<Date> = match day {
        Some(day) => {
            let (month, d) = day.split_once(' ')?;
            let month = MONTHS.iter().position(|m| month.starts_with(m))? as i8 + 1;
            let d: i8 = d.trim_end_matches(',').parse().ok()?;
            (-1..=1)
                .filter_map(|y| Date::new(today.year() + y, month, d).ok())
                .collect()
        }
        None => vec![today, today.tomorrow().ok()?],
    };
    candidates
        .into_iter()
        .filter_map(|date| date.to_datetime(time).to_zoned(tz.clone()).ok())
        .map(|z| z.timestamp().as_second())
        .min_by_key(|t| match day {
            Some(_) => (t - now).abs(),
            // A bare time is the next one to come.
            None => {
                if *t >= now {
                    t - now
                } else {
                    i64::MAX
                }
            }
        })
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `2:29pm`, `3pm`, `14:30`.
fn clock(s: &str) -> Option<Time> {
    let s = s.trim().to_ascii_lowercase();
    let (s, pm) = match (s.strip_suffix("pm"), s.strip_suffix("am")) {
        (Some(t), _) => (t.to_owned(), Some(true)),
        (_, Some(t)) => (t.to_owned(), Some(false)),
        _ => (s, None),
    };
    let (h, m) = s.split_once(':').unwrap_or((&s, "0"));
    let (mut h, m): (i8, i8) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    match pm {
        Some(true) if h < 12 => h += 12,
        Some(false) if h == 12 => h = 0,
        _ => {}
    }
    Time::new(h, m, 0, 0).ok()
}

/// Run `claude -p /usage` (`cmd`: the account's binary and env) and read its windows.
pub async fn read_usage(
    mut cmd: tokio::process::Command,
    timeout: Duration,
    now: EpochSecs,
) -> Result<Vec<QuotaReading>, String> {
    // No session file, no project settings from wherever anchorleg was started.
    cmd.args([
        "-p",
        "--output-format",
        "json",
        "--no-session-persistence",
        "/usage",
    ])
    .current_dir(std::env::temp_dir())
    .stdin(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .kill_on_drop(true);
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| format!("`/usage` gave no answer in {}s", timeout.as_secs()))?
        .map_err(|e| format!("starting claude: {e}"))?;
    if !out.status.success() {
        return Err(format!("`claude -p /usage` failed ({})", out.status));
    }
    let answer: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("reading `/usage`: {e}"))?;
    if answer.get("num_turns").and_then(serde_json::Value::as_u64) != Some(0) {
        // Not answered locally: this CLI sent `/usage` to the model. Don't trust the text.
        return Err("`/usage` isn't a built-in command in this Claude Code".to_owned());
    }
    let text = answer
        .get("result")
        .and_then(serde_json::Value::as_str)
        .ok_or("`/usage` printed no result")?;
    parse_usage(text, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(name: &str) -> String {
        let path = format!(
            "{}/../../fixtures/claude-2.1.296-usage-20261013/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        v["result"].as_str().unwrap().to_owned()
    }

    /// 2026-10-13 12:10 Europe/Berlin, when the fixtures were captured (in that zone).
    fn captured() -> EpochSecs {
        jiff::civil::date(2026, 10, 13)
            .at(12, 10, 0, 0)
            .in_tz("Europe/Berlin")
            .unwrap()
            .timestamp()
            .as_second()
    }

    fn at(y: i16, mo: i8, d: i8, h: i8, mi: i8) -> EpochSecs {
        jiff::civil::date(y, mo, d)
            .at(h, mi, 0, 0)
            .in_tz("Europe/Berlin")
            .unwrap()
            .timestamp()
            .as_second()
    }

    #[test]
    fn a_used_up_week_is_a_rejected_window() {
        let r = parse_usage(&result("limit-reached.json"), captured()).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].window, Some(Window::FiveHour));
        assert_eq!(r[0].used, Some(0.0));
        assert_eq!(r[0].resets_at, None);
        assert_eq!(r[1].window, Some(Window::SevenDay));
        assert_eq!(r[1].used, Some(1.0));
        assert_eq!(r[1].status, Some(QuotaStatus::Rejected));
        assert_eq!(r[1].resets_at, Some(at(2026, 10, 13, 14, 29)));
    }

    #[test]
    fn both_windows_with_their_resets() {
        let r = parse_usage(&result("partly-used.json"), captured()).unwrap();
        assert_eq!(r[0].used, Some(0.12));
        assert_eq!(r[0].status, Some(QuotaStatus::Allowed));
        assert_eq!(r[0].resets_at, Some(at(2026, 10, 13, 14, 59)));
        assert_eq!(r[1].used, Some(0.38));
        assert_eq!(r[1].resets_at, Some(at(2026, 10, 19, 4, 29)));
    }

    #[test]
    fn odd_text_is_skipped_not_misread() {
        let now = at(2026, 12, 30, 22, 0);
        // A reset in the new year, a bare time, an unknown window and an unknown zone.
        let text = "Current session: 40% used · resets 3am (Europe/Berlin)\n\
                    Current week (Sonnet only): 9% used\n\
                    Current week (all models): 50% used · resets Jan 2 at 12pm (Nowhere/Land)";
        let r = parse_usage(text, now).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].resets_at, Some(at(2026, 12, 31, 3, 0)));
        assert_eq!(r[1].used, Some(0.5));
        assert_eq!(r[1].resets_at, None);
        let r = parse_usage(
            "Current week (all models): 1% used · resets Jan 2 at 12pm (Europe/Berlin)",
            now,
        )
        .unwrap();
        assert_eq!(r[0].resets_at, Some(at(2027, 1, 2, 12, 0)));
        assert!(parse_usage("You are using an API key", now).is_err());
    }
}
