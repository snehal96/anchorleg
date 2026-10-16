The anchorleg desktop app: your Claude Code and Codex sessions in one window, carrying on across
accounts when one hits its 5-hour or weekly limit. It includes the anchorleg CLI (0.9.0), so
installing the app is the only step. **Preview**: it works day to day, but expect rough edges.

**What's in it**

- Start tasks in any folder, watch the conversation live (Markdown, diagrams, charts and maths
  drawn), and message a session while it works; ⌘⏎ (Ctrl+Enter on Linux) has it read now.
- Approve or refuse what the agent may do, with the diff for an edit, and answer its questions.
- Each account's quota in the header and the menu bar; usage and cost per account and model.
- What a session changed, file by file; go back to or fork from any message; its sub-agents.
- Plans to approve, goals, reviews by another CLI, budgets, extra folders and instructions.
- Sessions you started yourself in Claude Code or Codex, imported and continued.
- Notifications when a session needs you, finishes, fails or switches accounts.

**Install**

- **macOS** (Apple silicon and Intel): open the `.dmg` and drag anchorleg to Applications. The
  app isn't notarized yet, so the first time, right-click it → **Open** (or System Settings →
  Privacy & Security → **Open Anyway**).
- **Linux**: the `.AppImage` (`chmod +x`, then run it) or the `.deb` (`sudo apt install ./anchorleg_*.deb`)
  for your architecture. Needs WebKitGTK 4.1 (Ubuntu 22.04 or newer).

You still need Claude Code or Codex signed in on the accounts you want to use; the app's Accounts
page signs new ones in for you. If you also use `anchorleg` in a terminal, the app offers to
point it at its own copy so both stay on the same version.

`SHA256SUMS` lists the checksums. The CLI on its own: see the
[README](https://github.com/snehal96/anchorleg#install).
