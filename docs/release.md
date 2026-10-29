# Releases

CI
- `.github/workflows/ci.yml` runs on pushes to `main` across Ubuntu, macOS and Windows:
  - `cargo fmt --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo build --release`
  - `cargo test --workspace`
  - Linux installs X11/Wayland/fontconfig dev packages.

Release workflow
- `.github/workflows/release.yml` runs on `v*` tag pushes:
  - Builds release binaries for Linux (x86_64 + aarch64), macOS (x86_64 + aarch64) and Windows (x86_64).
  - Packages: tar.gz on Linux/macOS, zip on Windows. Checksums included.
  - Publishes a GitHub Release with the CHANGELOG section for that version as notes and uploads artifacts.
  - Installing does not require Rust.

Local steps (per AGENTS.md)
1. Ensure quality bar passes locally: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `timeout 120 cargo test --workspace`.
2. Bump versions (docs-only: PATCH; feature: MINOR), update `CHANGELOG.md`.
3. Commit as `release: vX.Y.Z` and create an annotated tag `vX.Y.Z`.
4. Push: `git push origin main --follow-tags`.

Notes
- Waypipe and X11 are Linux features; on macOS and Windows they are disabled in UI with an explanation (XQuartz/VcXsrv hints).
- Mode `system` follows OS theme preference (Linux via dark-light where available; macOS/Windows supported).
- Windows PTY uses ConPTY via `portable-pty`; default shell is PowerShell/cmd there, `$SHELL` on Unix.

