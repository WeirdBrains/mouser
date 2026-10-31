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
- Arch (see "Arch packages" below):
  - Prebuilt: `sudo pacman -U https://github.com/WeirdBrains/mouser/releases/download/v<ver>/mouser-bin-<ver>-1-x86_64.pkg.tar.zst`
  - From source: `cd packaging/arch/mouser-git && makepkg -si`
  - Planned: `yay -S mouser-git` (or `mouser-bin`) once published to the AUR.
- macOS:
  - CLI: `brew install --formula https://raw.githubusercontent.com/WeirdBrains/mouser/main/Formula/mouser.rb`
  - App: `brew install --cask https://raw.githubusercontent.com/WeirdBrains/mouser/main/Casks/mouser.rb`
  - Or download the `.dmg` and drag `Mouser.app` to Applications.
- Windows:
  - Download and run `MouserSetup-<ver>.exe` (Start Menu shortcut installed), or unzip the portable `.zip`.

## Arch packages

`packaging/arch/` holds two AUR-ready packages:

- `mouser-git/PKGBUILD`: builds `main` from source (`cargo`, `tic`), version from
  `git describe` (e.g. `0.7.2.r33.g90b79b6`).
- `mouser-bin/PKGBUILD`: the release binary (`mouser-x86_64-unknown-linux-gnu.tar.gz`)
  plus the tag's source tree for the desktop entry, icons, metainfo and terminfo.

Both install `/usr/bin/mouser`, `mouser.desktop` (xdg-terminal-exec keys),
`io.github.m3talsmith.mouser.metainfo.xml`, hicolor icons (PNG sizes + SVG), the
compiled `mouser` terminfo in `/usr/share/terminfo` and the source in
`/usr/share/mouser/mouser.terminfo`. They `provides=mouser` and conflict with each other,
so installing one replaces the other, including the old `mouser-bin` 0.7.x packages
(pacman asks `Remove mouser-bin? [y/N]`; answer `y`).

Build locally (no install):

```bash
cd packaging/arch/mouser-git && makepkg -f      # or: makepkg -si to install
namcap PKGBUILD *.pkg.tar.zst
```

Release workflow: the Arch job builds `mouser-bin` in an `archlinux:base-devel`
container from this PKGBUILD with the freshly built binary and `git archive` of the tag.
After the release is published, `packaging/arch/update-bin.sh <ver>` sets `pkgver` and
real `sha256sums` (downloaded from the release) and rewrites `.SRCINFO`, and the bot
commits it with the Homebrew update. Regenerate both `.SRCINFO` files on Arch with
`packaging/arch/srcinfo.sh` after editing a PKGBUILD.

Upgrading a machine after a release: `sudo pacman -U <release URL of mouser-bin-<ver>-1-x86_64.pkg.tar.zst>`
(same package name, so a plain upgrade).

### Publishing to the AUR (not done yet)
Needs the maintainer's personal AUR account (registration is closed at the moment):
1. Add an SSH key to the AUR account, then
   `git clone ssh://aur@aur.archlinux.org/mouser-git.git` (and `mouser-bin.git`).
2. Copy `PKGBUILD` and `.SRCINFO` in, set the `# Maintainer:` line, commit, push.
3. Before publishing, add a `LICENSE` file to the repo: MIT requires the license text
   in `/usr/share/licenses/<pkg>/` (the PKGBUILDs install it when present; namcap reports
   an error until then).
4. Per release, push the updated `mouser-bin` PKGBUILD/.SRCINFO to the AUR.

