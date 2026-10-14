# `/usage` in `claude -p` (Claude Code 2.1.296, 2026-10-13)

`claude -p --output-format json --no-session-persistence /usage` on two subscription logins. A
built-in command answered locally: `num_turns` 0, no tokens, no model request.

- `limit-reached.json`: the weekly window used up, nothing used in the current session.
- `partly-used.json`: both windows partly used, each with its reset time.

Scrubbed with `scripts/scrub-capture.py`; then `session_id`/`uuid` were dropped, the time zone
was changed to `Europe/Berlin`, and the "What's contributing to your limits usage?" breakdown
after the limits (counts from the person's own sessions) was cut.
