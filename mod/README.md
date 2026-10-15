# anchorleg-mod

A Claude Code mod for [anchorleg](https://github.com/snehal96/anchorleg), which keeps headless
coding tasks running when a subscription reaches its 5-hour or weekly limit. Inside each Claude
Code session that anchorleg starts, the mod reports the session's exact quota and stops the task
at a clean point between model requests, so anchorleg can continue it on another account or CLI.

**It needs the anchorleg binary.** anchorleg loads the mod into every run it starts with
`--plugin-dir`, so you don't install it yourself. In a session anchorleg didn't start (no
`ANCHORLEG_ACCOUNT` or `ANCHORLEG_RUN_ID` in the environment) it does nothing.

## What it does

- **Reports quota.** When Claude Code measures the session, the mod sends each window's percent
  used and reset time to `anchorleg report --json`.
- **Stops between requests.** Before each model request after the first, it checks the quota. If
  a window is at or above the run's threshold (`ANCHORLEG_STOP_AT`), it records the stop with
  anchorleg and ends the turn without making the request. It does the same when the task's token
  budget (`ANCHORLEG_BUDGET_LEFT`) is used up, or when you sent the task a message that should
  interrupt it.
- **Forwards questions.** In `claude -p` a permission prompt becomes a refusal and
  `AskUserQuestion` isn't offered. The mod sends the permission prompt to
  `anchorleg permission ask` and waits for your answer in `anchorleg ui` or
  `anchorleg permission answer`. It adds an `ask_user` tool that sends the model's questions the same
  way. Nothing is allowed without your answer. If anchorleg can't be reached, Claude Code's own
  decision stands. In auto mode (`ANCHORLEG_PERMISSION_MODE=auto`) it leaves the decision to
  Claude Code.
- **Forwards MCP input requests** (the `Elicitation` hook) to `anchorleg permission elicit`.

## What it runs and sends

- It runs only the anchorleg binary named in `ANCHORLEG_BIN`, with the subcommands `report`,
  `interrupt-check`, `permission ask` and `permission elicit`. These are local processes, and the
  mod makes no network requests.
- It sends them the account name anchorleg gave the run, the quota readings, the run id, the stop
  reason, and, for a prompt, the tool's name and input or the question being asked.
- It reads these environment variables: `ANCHORLEG_ACCOUNT`, `ANCHORLEG_BIN`, `ANCHORLEG_RUN_ID`,
  `ANCHORLEG_STOP_AT`, `ANCHORLEG_BUDGET_LEFT` and `ANCHORLEG_PERMISSION_MODE`.
- It never reads, changes or sends credentials and never bypasses a limit. Switching accounts is
  done by anchorleg, which starts the official CLI again.

## Develop

```
claude plugin validate mod
claude plugin test mod
```

## License

Dual-licensed under MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache-2.0
([LICENSE-APACHE](LICENSE-APACHE)), at your option, like anchorleg.
