# Releasing

`.github/workflows/release.yml` publishes a release when a `v*` tag is pushed.

## What a release contains

| File | For |
|---|---|
| `anchorleg-aarch64-apple-darwin.tar.gz` | macOS, Apple silicon |
| `anchorleg-x86_64-apple-darwin.tar.gz` | macOS, Intel |
| `anchorleg-aarch64-unknown-linux-gnu.tar.gz` | Linux arm64 (glibc 2.35+) |
| `anchorleg-x86_64-unknown-linux-gnu.tar.gz` | Linux x86_64 (glibc 2.35+) |
| `anchorleg_<version>-1_{arm64,amd64}.deb` | Debian / Ubuntu |
| `*.tar.gz.sha256`, `SHA256SUMS` | checksums (`install.sh` checks them) |

Each tarball holds `anchorleg-<target>/{anchorleg, README.md, LICENSE-MIT, LICENSE-APACHE}`.
Asset names carry no version, so `releases/latest/download/<name>` always works (`install.sh`
relies on this). Linux builds run on Ubuntu 22.04 so older distros can run them; libdbus (for
the Secret Service token store) is built in, so the only runtime dependency is glibc.

## One-time setup for Homebrew

1. Create a public repo **snehal96/homebrew-tap** (empty; the workflow adds `Formula/`).
2. Create a fine-grained personal access token with **Contents: read and write** on that repo
   only.
3. In **snehal96/anchorleg → Settings → Secrets and variables → Actions**, add it as
   `HOMEBREW_TAP_TOKEN`.

Without the secret, releases still publish; the Homebrew job just skips. Users then install with
`brew install snehal96/tap/anchorleg`.

## Cutting a release

1. Bump `version` in the root `Cargo.toml` (workspace version), run `cargo build` so
   `Cargo.lock` follows, and run `./scripts/check.sh`.
2. Commit, then tag and push:
   ```bash
   git tag v0.1.0 && git push origin main v0.1.0
   ```
   The workflow refuses a tag that doesn't match `Cargo.toml`.
3. Watch **Actions → release**. When it's green: the GitHub release has the files above and
   the tap has the new formula.

## Checking a build locally

```bash
cargo build --release --locked --target aarch64-apple-darwin -p anchorleg
./scripts/package.sh aarch64-apple-darwin          # → dist/
ANCHORLEG_BASE_URL=file://$PWD/dist ANCHORLEG_INSTALL_DIR=/tmp/al bash install.sh
./scripts/homebrew-formula.sh v0.1.0 dist          # needs all four .sha256 files
```

Linux: the same inside `docker run rust:1-bookworm` (with `cargo install cargo-deb` for the
`.deb`).
