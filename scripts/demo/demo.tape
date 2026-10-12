# The README/site demo: a task pauses at 92% on Claude and finishes on Codex.
# Fake CLIs only (scripts/demo/setup.sh): no quota spent, no real accounts.
#
#   cargo build --release && vhs scripts/demo/demo.tape

Output docs/images/demo.gif

Set Shell "bash"
Set FontSize 18
Set Width 1280
Set Height 720
Set Padding 24
Set Theme "Catppuccin Mocha"
Set TypingSpeed 35ms
Set WindowBar Colorful

Hide
Type "export DEMO_DIR=$(mktemp -d)/anchorleg-demo && scripts/demo/setup.sh $DEMO_DIR && source $DEMO_DIR/env.sh && clear"
Enter
Sleep 2s
Show

Type "# Two subscriptions. Claude is nearly out."
Sleep 600ms
Enter
Type "anchorleg status --refresh"
Sleep 400ms
Enter
Sleep 4s

Type "anchorleg run -- 'migrate the auth tests to vitest'"
Sleep 600ms
Enter
Sleep 16s

Type "# What the next agent was handed:"
Enter
Type "sed -n '7,12p;20,30p' .handoff/TASK.md"
Enter
Sleep 5s

Type "clear && anchorleg usage --since 24h"
Enter
Sleep 7s
