# Security

anchorleg runs coding agents on your machine with your accounts, so security reports matter.

## Reporting a vulnerability

Please **don't open a public issue**. Use GitHub's private reporting instead:
[Report a vulnerability](https://github.com/snehal96/anchorleg/security/advisories/new). You'll get a
reply within a week. Include what you found, how to reproduce it and what an attacker could do.

## What's in scope

- Tokens or credentials ending up anywhere other than the system keychain (config, logs, the run
  database, `.handoff/`, fixtures, process arguments).
- A tool call running without the approval anchorleg says it requires.
- anchorleg pushing, rewriting history, or touching your branch, index or stash.
- The install script or release artifacts not matching their checksums.

Problems in the agent CLIs themselves (Claude Code, Codex, Antigravity) should go to their
vendors.
