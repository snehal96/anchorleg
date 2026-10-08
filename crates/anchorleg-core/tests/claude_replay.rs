//! Replays every Claude stream-json fixture and snapshots what anchorleg makes of each line.

use anchorleg_core::adapters::claude::{parse_line, signals};

#[test]
fn replay_claude_fixtures() {
    insta::glob!(
        "../../../fixtures",
        "{claude-*,synthetic}/**/*.jsonl",
        |path| {
            let name = path.file_name().unwrap().to_string_lossy();
            // Phase 0 also saves mod and Codex output next to Claude's; those aren't stream-json.
            if name.starts_with("probe-mod") || name.starts_with("codex") {
                return;
            }
            let text = std::fs::read_to_string(path).unwrap();
            let replay: Vec<_> = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|line| {
                    let event = parse_line(line);
                    let signals = signals(&event);
                    serde_json::json!({ "event": event, "signals": signals })
                })
                .collect();
            insta::assert_json_snapshot!(replay);
        }
    );
}
