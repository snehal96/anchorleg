@AGENTS.md

## Claude Code specifics

- Mods docs: https://code.claude.com/docs/en/plugins/mods/reference (events, `$` API, limits).
  The TypeScript declarations for your installed version are the most accurate reference.
- Load a mod for one session with `claude --plugin-dir <dir>`; in headless runs use
  `claude -p --plugin-dir <dir>` or `CLAUDE_CODE_PLUGIN_DIRS`.
- Headless output: `claude -p --output-format stream-json --verbose` (`stream-json` requires
  `--verbose` in `-p` mode). Don't pass `--bare`: it ignores `CLAUDE_CODE_OAUTH_TOKEN`.
