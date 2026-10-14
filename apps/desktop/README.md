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

## Build a .dmg (macOS)

```bash
npm run tauri build -- --bundles dmg      # → src-tauri/target/release/bundle/dmg/
```

The app is ad-hoc signed and not notarized, so macOS blocks it the first time it opens. To allow
it, open System Settings → Privacy & Security and click **Open Anyway**, or run
`xattr -dr com.apple.quarantine /Applications/anchorleg.app`.

Linux (AppImage, `.deb`) needs the WebKitGTK development packages; see the Tauri prerequisites.
