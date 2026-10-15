//! What every driven CLI's adapter must do, checked against its real captures in `fixtures/`.
//! A new adapter (Cursor, …) passes these before its row in `vendors.rs` says `driven`.
//!
//! Lines go through [`Decoder`], the same entry point the supervisor uses. Codex app-server
//! captures are translated to `codex exec --json` lines first, the way the driver does.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{AgentSignal, Decoder};
use crate::registry::Vendor;
use crate::vendors::QuotaSource;

/// Files in the capture folders that aren't the CLI's stdout stream (mod logs, rollout files,
/// hook probes, raw app-server replies).
const NOT_STREAMS: &[&str] = &[
    "probe-mod.jsonl",
    "probe-tool-check.jsonl",
    "rollout-token-count.jsonl",
    "subagent-transcript.jsonl",
    "rate-limits-read.jsonl",
    "requests.jsonl",
    // Session files kept on disk (`anchorleg sessions import`), not the stream.
    "transcript.jsonl",
    "rollout.jsonl",
];

/// The capture folders of a vendor, by name prefix. A driven vendor must have one.
fn prefixes(vendor: Vendor) -> &'static [&'static str] {
    match vendor {
        Vendor::Claude => &["claude-", "synthetic"],
        Vendor::Codex => &["codex-"],
        Vendor::Antigravity => &["agy-"],
        _ => &[],
    }
}

struct Capture {
    name: String,
    lines: Vec<String>,
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn captures(vendor: Vendor) -> Vec<Capture> {
    let mut out = Vec::new();
    let mut dirs: Vec<_> = std::fs::read_dir(fixtures())
        .unwrap()
        .map(|d| d.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            prefixes(vendor).iter().any(|pre| name.starts_with(pre))
        })
        .collect();
    dirs.sort();
    for dir in dirs {
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|f| f.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .collect();
        files.sort();
        for file in files {
            let base = file.file_name().unwrap().to_string_lossy().into_owned();
            // Phase 0 kept Codex output next to Claude's.
            if NOT_STREAMS.contains(&base.as_str())
                || (vendor == Vendor::Claude && base.starts_with("codex"))
            {
                continue;
            }
            let text = std::fs::read_to_string(&file).unwrap();
            let app_server = text
                .lines()
                .find(|l| !l.trim().is_empty())
                .is_some_and(|l| l.starts_with(r#"{"dir":"#));
            let lines = if app_server {
                super::codex_server::tests::translate(&text)
                    .iter()
                    .map(Value::to_string)
                    .collect()
            } else {
                text.lines().map(str::to_owned).collect()
            };
            let folder = dir.file_name().unwrap().to_string_lossy();
            out.push(Capture {
                name: format!("{folder}/{base}"),
                lines,
            });
        }
    }
    out
}

fn decode(vendor: Vendor, capture: &Capture) -> Vec<AgentSignal> {
    let mut d = Decoder::new(vendor);
    capture
        .lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .flat_map(|l| d.feed(l))
        .collect()
}

fn driven() -> impl Iterator<Item = Vendor> {
    Vendor::ALL.into_iter().filter(|v| v.caps().driven)
}

#[test]
fn every_driven_cli_has_captures() {
    for v in driven() {
        assert!(
            !captures(v).is_empty(),
            "{v:?} is driven but has no captures: add fixtures/<cli>-<version>-<what>-<date>/ and its prefix here"
        );
    }
}

/// Across its captures, an adapter reports what the supervisor relies on: the session id, a
/// good and a failed end, token use, and whatever its row in `vendors.rs` promises.
#[test]
fn every_driven_cli_reports_the_basics() {
    for v in driven() {
        let caps = v.caps();
        let all: Vec<_> = captures(v).iter().flat_map(|c| decode(v, c)).collect();
        let has = |what: &str, f: &dyn Fn(&AgentSignal) -> bool| {
            assert!(all.iter().any(f), "{v:?}: no capture gives {what}");
        };
        has(
            "Started",
            &|s| matches!(s, AgentSignal::Started { session_id, .. } if !session_id.is_empty()),
        );
        has("Finished ok", &|s| {
            matches!(s, AgentSignal::Finished { ok: true, .. })
        });
        has("Finished failed", &|s| {
            matches!(s, AgentSignal::Finished { ok: false, .. })
        });
        has(
            "Usage",
            &|s| matches!(s, AgentSignal::Usage(u) if !u.is_empty()),
        );
        // Codex's final message is the JSON itself; the driver parses it, not the stream.
        if caps.json_schema && v != Vendor::Codex {
            has("Structured", &|s| matches!(s, AgentSignal::Structured(_)));
        }
        if caps.quota == QuotaSource::Live && v == Vendor::Claude {
            // Codex reads its quota beside the stream (`account/rateLimits/read`), not in it.
            has("Quota", &|s| matches!(s, AgentSignal::Quota(_)));
            has("LimitHit", &|s| matches!(s, AgentSignal::LimitHit { .. }));
        }
    }
}

/// Within one launch: the session starts before it finishes, and the end names the same
/// session (a resume continues under its id; a mismatch would hand off the wrong session).
#[test]
fn every_capture_keeps_one_session() {
    for v in driven() {
        for c in captures(v) {
            let signals = decode(v, &c);
            let started: Vec<_> = signals
                .iter()
                .enumerate()
                .filter_map(|(i, s)| match s {
                    AgentSignal::Started { session_id, .. } => Some((i, session_id.clone())),
                    _ => None,
                })
                .collect();
            for (i, s) in signals.iter().enumerate() {
                let AgentSignal::Finished {
                    session_id: Some(id),
                    ..
                } = s
                else {
                    continue;
                };
                if id.is_empty() {
                    continue;
                }
                if let Some((at, first)) = started.first() {
                    assert!(*at < i, "{}: Finished before Started", c.name);
                    assert!(
                        started.iter().any(|(_, s)| s == id),
                        "{}: finished as {id}, started as {first}",
                        c.name
                    );
                }
            }
        }
    }
}

/// Event parsing never crashes a run (AGENTS.md): every field of every captured line, set to
/// each JSON type, plus cut-off lines and junk, decodes without a panic.
#[test]
fn no_line_crashes_an_adapter() {
    const JUNK: &[&str] = &[
        "",
        " ",
        "not json",
        "null",
        "0",
        "\"x\"",
        "[]",
        "{}",
        "[{}]",
        "{\"type\":null}",
        "{\"event\":null}",
        "{\"type\":\"result\"}",
        "{\"event\":\"result\"}",
        "{\"type\":\"turn.completed\"}",
        "{\"type\":\"thread.started\"}",
        "{\"event\":\"init\"}",
        "\u{0}",
        "{\"type\":\"system\",\"subtype\":\"init\"}",
    ];
    let odd = odd();
    for v in driven() {
        let mut d = Decoder::new(v);
        for line in JUNK {
            d.feed(line);
        }
        for c in captures(v) {
            let mut d = Decoder::new(v);
            for line in &c.lines {
                for cut in line.char_indices().map(|(i, _)| i).step_by(17) {
                    d.feed(&line[..cut]);
                }
                if let Ok(value) = serde_json::from_str::<Value>(line) {
                    let mut paths = Vec::new();
                    leaves(&value, String::new(), &mut paths);
                    for path in paths {
                        for o in &odd {
                            let mut m = value.clone();
                            *m.pointer_mut(&path).unwrap() = o.clone();
                            d.feed(&m.to_string());
                        }
                    }
                }
                d.feed(line);
            }
        }
    }
}

/// JSON pointers to every value in `v`, containers included (the first three items of a list:
/// the rest are alike, and init lines list hundreds of tools).
fn leaves(v: &Value, at: String, out: &mut Vec<String>) {
    if !at.is_empty() {
        out.push(at.clone());
    }
    let esc = |k: &str| k.replace('~', "~0").replace('/', "~1");
    match v {
        Value::Object(map) => map
            .iter()
            .for_each(|(k, x)| leaves(x, format!("{at}/{}", esc(k)), out)),
        Value::Array(xs) => xs
            .iter()
            .take(3)
            .enumerate()
            .for_each(|(i, x)| leaves(x, format!("{at}/{i}"), out)),
        _ => {}
    }
}

/// Values of every JSON type, edge cases included, to put where a field was.
fn odd() -> Vec<Value> {
    vec![
        Value::Null,
        Value::Bool(true),
        serde_json::json!(-1),
        serde_json::json!(1e300),
        serde_json::json!(u64::MAX),
        serde_json::json!(""),
        serde_json::json!("é".repeat(300)),
        serde_json::json!([]),
        serde_json::json!({}),
        serde_json::json!([null, 1, "x"]),
    ]
}

/// The same for the Codex app-server driver, which translates the server's messages before the
/// decoder sees them.
#[test]
fn no_app_server_message_crashes_the_codex_driver() {
    let mut dirs: Vec<_> = std::fs::read_dir(fixtures())
        .unwrap()
        .map(|d| d.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("codex-")
        })
        .collect();
    dirs.sort();
    let odd = odd();
    for dir in dirs {
        for file in std::fs::read_dir(&dir).unwrap() {
            let text = std::fs::read_to_string(file.unwrap().path()).unwrap();
            let mut t = super::codex_server::Translator::default();
            let mut d = Decoder::new(Vendor::Codex);
            let mut feed = |t: &mut super::codex_server::Translator, m: &Value| {
                let Some(method) = m.get("method").and_then(Value::as_str) else {
                    let _ = t.thread_started(&m["result"]);
                    return;
                };
                if let Some(thread) = t.other_thread(m) {
                    for e in t.subagent_notification(&thread, method, &m["params"]) {
                        d.feed(&e.to_string());
                    }
                }
                for e in t.notification(method, &m["params"]) {
                    d.feed(&e.to_string());
                }
                d.feed(&t.turn_completed(&m["params"]).to_string());
            };
            for line in text.lines() {
                let Ok(v) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                if v["dir"] != "in" {
                    continue;
                }
                let msg = &v["msg"];
                let mut paths = Vec::new();
                leaves(msg, String::new(), &mut paths);
                for path in paths.iter().filter(|p| !p.starts_with("/method")) {
                    for o in &odd {
                        let mut m = msg.clone();
                        *m.pointer_mut(path).unwrap() = o.clone();
                        feed(&mut t, &m);
                    }
                }
                feed(&mut t, msg);
            }
        }
    }
}
