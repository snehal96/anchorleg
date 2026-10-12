//! A goal: a completion condition the session keeps working toward, across turns, accounts and
//! CLIs (D48).
//!
//! Claude Code's `/goal` and Codex's `thread/goal/*` loop inside one CLI, and Claude's evaluator
//! verdicts don't show in `-p` output (seen on 2.1.296), so anchorleg keeps the loop itself and
//! the same for every CLI: each turn is told the goal and ends its reply with a `GOAL:` line;
//! anchorleg reads it and, while the goal isn't met, continues the session (on whichever
//! account it's on by then). The goal is stored with the run and written into every handoff.

/// How many turns a goal gets unless `--goal-turns` says otherwise.
pub const DEFAULT_TURNS: u32 = 10;

/// The goal's state, as stored on the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Being worked toward.
    Active,
    Met,
    /// The agent says it can't be met.
    Impossible,
    /// The turn limit came first.
    Unmet,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Met => "met",
            Status::Impossible => "impossible",
            Status::Unmet => "unmet",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "active" => Status::Active,
            "met" => Status::Met,
            "impossible" => Status::Impossible,
            "unmet" => Status::Unmet,
            _ => return None,
        })
    }

    /// Whether a follow-up should keep working toward it.
    pub fn open(self) -> bool {
        matches!(self, Status::Active | Status::Unmet)
    }
}

/// The agent's verdict on its own work, from the `GOAL:` line that ends its reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Met(String),
    NotMet(String),
    Impossible(String),
}

/// What every goal turn is told, after its prompt.
pub fn note(goal: &str) -> String {
    format!(
        "Goal (anchorleg keeps this session going until it holds): {goal}\n\
         Work toward it now. Prove it with evidence in this conversation (run the check, show the \
         result). End your reply with exactly one of these lines:\n\
         GOAL: MET — <the evidence>\n\
         GOAL: NOT MET — <what is left>\n\
         GOAL: IMPOSSIBLE — <why it can't be met>"
    )
}

/// The message that starts the next turn while the goal isn't met.
pub fn continue_text(left: &str) -> String {
    if left.is_empty() {
        "The goal isn't met yet. Keep working toward it.".to_owned()
    } else {
        format!("The goal isn't met yet ({left}). Keep working toward it.")
    }
}

/// The last `GOAL:` line in a reply. `None` when there is none (the agent forgot: treated as
/// not met).
pub fn verdict(reply: &str) -> Option<Verdict> {
    let line = reply
        .lines()
        .rev()
        .map(|l| l.trim().trim_matches(|c| c == '*' || c == '`').trim())
        .find(|l| l.to_ascii_uppercase().starts_with("GOAL:"))?;
    let rest = line[5..].trim();
    let upper = rest.to_ascii_uppercase();
    let reason = |word: &str| {
        rest[word.len()..]
            .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '—' | '–' | '-' | ':'))
            .trim()
            .to_owned()
    };
    if upper.starts_with("NOT MET") {
        Some(Verdict::NotMet(reason("NOT MET")))
    } else if upper.starts_with("MET") {
        Some(Verdict::Met(reason("MET")))
    } else if upper.starts_with("IMPOSSIBLE") {
        Some(Verdict::Impossible(reason("IMPOSSIBLE")))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_come_from_the_last_goal_line() {
        assert_eq!(
            verdict("Tests run.\n\nGOAL: MET — 14 passed, 0 failed"),
            Some(Verdict::Met("14 passed, 0 failed".into()))
        );
        assert_eq!(
            verdict("GOAL: MET — x\nmore work\n**GOAL: NOT MET - 2 tests fail**"),
            Some(Verdict::NotMet("2 tests fail".into()))
        );
        assert_eq!(
            verdict("goal: impossible: the API was removed"),
            Some(Verdict::Impossible("the API was removed".into()))
        );
        assert_eq!(verdict("`GOAL: MET`"), Some(Verdict::Met(String::new())));
        assert_eq!(verdict("all done"), None);
        assert_eq!(Status::parse("unmet"), Some(Status::Unmet));
        assert!(Status::Unmet.open() && !Status::Met.open());
    }
}
