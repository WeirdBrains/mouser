# Releases

CI
- `.github/workflows/ci.yml` runs on pushes to `main` across Ubuntu, macOS and Windows:
  - `cargo fmt --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo build --release`
  - `cargo test --workspace`
  - Linux installs X11/Wayland/fontconfig dev packages.

Release workflow
- `.github/workflows/release.yml` runs on `v*` tag pushes and:
  - Builds release binaries for Linux (x86_64), macOS (x86_64 + arm64) and Windows (x86_64)
  - Produces installable packages from the prebuilt binaries (no Rust toolchain required to install):
    - Debian/Ubuntu: `.deb` via `nfpm` (maps `/usr/bin/mouser`, `.desktop`, icons, AppStream)
    - Fedora: `.rpm` via `nfpm` (same contents as `.deb`)
    - Arch: `mouser-bin` PKGBUILD generated and built in an `archlinux` container to a `.pkg.tar.zst`
    - macOS: `Mouser.app` bundle and a `.dmg`; Homebrew formula (CLI) and Cask (App)
    - Windows: portable `.zip` and an installer built with Inno Setup (`MouserSetup-<ver>.exe`)
  - Validates desktop integration on Linux: `desktop-file-validate` and `appstreamcli validate`
  - Publishes a GitHub Release with the CHANGELOG section for that version as notes and uploads all artifacts with checksums

Local steps (per AGENTS.md)
1. Ensure quality bar passes locally: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `timeout 120 cargo test --workspace`.
2. Bump versions (docs-only: PATCH; feature: MINOR), update `CHANGELOG.md`.
3. Commit as `release: vX.Y.Z` and create an annotated tag `vX.Y.Z`.
4. Push: `git push origin main --follow-tags`.

Notes
- Waypipe and X11 are Linux features; on macOS and Windows they are disabled in UI with an explanation (XQuartz/VcXsrv hints).
- Mode `system` follows OS theme preference (Linux via dark-light where available; macOS/Windows supported).
- Windows PTY uses ConPTY via `portable-pty`; default shell is PowerShell/cmd there, `$SHELL` on Unix.

## Install

- Debian/Ubuntu:
  - Download `mouser_<ver>_amd64.deb` from the Release and install:
    - `sudo apt install ./mouser_<ver>_amd64.deb`
- Fedora:
  - Download `mouser-<ver>-1.x86_64.rpm` and install:
    - `sudo rpm -Uvh mouser-<ver>-1.x86_64.rpm`
- Arch:
  - Download `mouser-bin-<ver>-1-x86_64.pkg.tar.zst` and install:
    - `sudo pacman -U mouser-bin-<ver>-1-x86_64.pkg.tar.zst`
- macOS:
  - CLI: `brew install --formula https://raw.githubusercontent.com/m3talsmith/mouser/main/Formula/mouser.rb`
  - App: `brew install --cask https://raw.githubusercontent.com/m3talsmith/mouser/main/Casks/mouser.rb`
  - Or download the `.dmg` and drag `Mouser.app` to Applications.
- Windows:
  - Download and run `MouserSetup-<ver>.exe` (Start Menu shortcut installed), or unzip the portable `.zip`.


