# anchorleg desktop (in development)

A desktop window over the `anchorleg` CLI, built with Tauri 2, React and Vite. The app includes
its own copy of the CLI (`anchorleg.app/Contents/MacOS/anchorleg`), built from the same commit,
and does everything through the CLI's `--json` commands. Installing the app is the only step. To
get the `anchorleg` command in a terminal as well, use **Use in Terminal** in the app: it links
`~/.local/bin/anchorleg` to the app's copy, and replaces only an older CLI. To use a different
binary, set `ANCHORLEG_BIN=/path/to/anchorleg`.

This crate is not part of the root Cargo workspace, so the CLI's `cargo test` and releases don't
build Tauri.

## Develop

```bash
cd apps/desktop
npm install
npm run sidecar                           # builds the CLI into src-tauri/binaries/ (Tauri needs it)
# ANCHORLEG_CLI_DIR=../../../anchorleg-cli npm run sidecar   # the CLI from a cli/<topic> worktree instead
npm run tauri dev                         # hot-reloading window
(cd src-tauri && cargo test --lib && cargo clippy --all-targets -- -D warnings)
```

## Keeping the terminal CLI in step

The app and a terminal `anchorleg` share one store, and an older CLI refuses a store a newer one
has upgraded. At start the app checks which `anchorleg` a terminal runs (`VersionCheck.tsx`):
older than the app's copy → a dialog offers to point `~/.local/bin/anchorleg` at the app's copy
(or says what to do when Homebrew or another copy comes first on PATH); "Not now" is remembered
for that pair of versions. When the app's own copy is the older one (the CLI answers "the
database is from a newer anchorleg"), a dialog sends you to the desktop downloads instead.

## Notifications

While the app is open it shows the CLI's notices itself (a run waiting for an answer, done,
failed, switched, blocked), so you don't get them twice: it keeps its pid in
`<store dir>/desktop.pid` and reads `<store dir>/desktop-notices.jsonl`. Coming back to the
window right after a notice opens that session. When the app quits, runs notify on their own again.

## Menu bar

The menu bar item shows the fullest quota window of the account the next run would use (and
`! N` while something waits for you), one line per account, and Open / Quit. Closing the window
only hides it: sessions, notices and the menu bar item go on until **Quit anchorleg**; the Dock
icon brings the window back.

## Release bundles

```bash
./scripts/release.sh                      # macOS: one universal .dmg; Linux: AppImage + .deb
./scripts/release.sh aarch64-apple-darwin # one architecture only (for testing)
```

They land in `release/` with `SHA256SUMS`. Each bundle carries the CLI built from the same
checkout (universal on macOS: native on Apple silicon and Intel). CI does the same for macOS and both Linux architectures when a `desktop-v*`
tag matching `version` in `src-tauri/tauri.conf.json` is pushed
(`.github/workflows/desktop-release.yml`), and leaves a draft release, with `RELEASE_NOTES.md`
as its notes, to check and publish (`gh release edit desktop-vX.Y.Z --draft=false --latest=false`:
the CLI's release stays "latest").

The app is ad-hoc signed and not notarized, so macOS blocks it the first time it opens. To allow
it, open System Settings → Privacy & Security and click **Open Anyway**, or run
`xattr -dr com.apple.quarantine /Applications/anchorleg.app`.

Linux (AppImage, `.deb`) needs the WebKitGTK development packages; see the Tauri prerequisites.
