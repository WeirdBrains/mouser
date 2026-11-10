# Releases

CI
- `.github/workflows/ci.yml` runs on `v*` tag pushes (and manual dispatch) across Ubuntu, macOS and Windows:
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

Engines in packages
- Mouser's renderers are libraries (`mouser_engine_wgpu`, `mouser_engine_skia`); `cargo build --release` builds them next to `mouser-app`.
- `mouser engine bundle --from target/<triple>/release --out DIR` copies them into `DIR/<name>/` with their `engine.toml` (`--from` defaults to the folder of the running mouser). The release workflow runs it once per target and ships the result:
  - `.tar.gz`/`.zip`: `engines/` beside `mouser-app`
  - `.deb`/`.rpm`/Arch: `/usr/lib/mouser/engines/`
  - `Mouser.app`: `Contents/Resources/engines/`; Homebrew formula: `lib/mouser/engines/`
  - Windows installer: `{app}\engines\`
- Mouser looks in `engines/` beside itself, `../Resources/engines` (macOS) and `../lib/mouser/engines` (Unix); `MOUSER_ENGINE_DIRS` replaces those ([Engines](./engines.md)).

Signing
- Every signing step is skipped with a notice when its secrets aren't set, so a release without keys still builds (unsigned). The minisign secrets are set on WeirdBrains/mouser; the macOS and Windows certificates aren't yet.
- macOS: `MACOS_CERT_P12` (base64 Developer ID Application .p12), `MACOS_CERT_PASSWORD`, `MACOS_SIGN_IDENTITY` sign `mouser-app`, the engine dylibs and `Mouser.app` (hardened runtime); with `APPLE_ID`, `APPLE_TEAM_ID` and `APPLE_APP_PASSWORD` the dmg is notarized and stapled.
- Windows: `WINDOWS_CERT_PFX` (base64 .pfx) and `WINDOWS_CERT_PASSWORD` sign `mouser-app.exe`, the engine DLLs and `MouserSetup-<ver>.exe` (Authenticode, timestamped).
- Release files: `MINISIGN_SECRET_KEY` (the key file's text) and `MINISIGN_PASSWORD` add `FILE.minisig` beside every file. The matching public key is the one Mouser pins as its official key (`OFFICIAL_KEY` in `crates/mouser-core/src/install.rs`: `61BA1720747F9146`); see [Downloads](./downloads.md).
- Plugins and engines published from their own repos are signed there.
- `-full` offline bundles (the `plugins.lock` archives in `plugins/` beside Mouser) are built once the lock has entries; it is empty in this tree.

What starts a release
- Only a pushed `v*` tag runs the Release and CI workflows. Only a tag ref reaches the Release `publish` job (`if: startsWith(github.ref, 'refs/tags/v')`). A push to `main` starts neither. The Docs site workflow deploys from GitHub Actions on a `v*` tag, on a `main` push that touches the site or `docs/`, and when started by hand. A manual Release run (`workflow_dispatch`) builds and publishes nothing; CI can also be started by hand.

Local steps (per AGENTS.md)
1. Ensure quality bar passes locally: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `timeout 120 cargo test --workspace`.
2. On a clean, up-to-date `main`, run `scripts/release.py`:

   ```bash
   scripts/release.py "Tab completion in every text field"   # MINOR: 1.4.5 -> 1.5.0
   scripts/release.py --patch "Commander no longer crashes on start"   # 1.4.6
   scripts/release.py --major "New plugin protocol"   # 2.0.0
   scripts/release.py --dry-run "..."   # print every step and the diffs, change nothing
   ```

   It bumps the versions (docs-only: `--patch`; feature: default MINOR), folds `## [Unreleased]` into a new `## [X.Y.Z] - YYYY-MM-DD` section (America/New_York) with the message added, commits `release: vX.Y.Z` with the message as the body, creates the annotated tag `vX.Y.Z` and runs `git push origin main --follow-tags`. Pushing that tag is what starts the Release workflow.
3. The Release workflow then pushes a `packaging:` commit: `Formula/mouser.rb` and `Casks/mouser.rb` from `scripts/packaging.py brew <ver> --assets dist`, and `packaging/arch/mouser-bin/` from `packaging/arch/update-bin.sh <ver>`. If that step fails, run the same by hand from the published release: `python3 scripts/packaging.py brew <ver> --release`, `packaging/arch/update-bin.sh <ver>`, commit `packaging: …`.

Homebrew checksums (`scripts/packaging.py`)
- `brew` hashes both macOS tarballs (formula) and both `.dmg`s (cask), checks each against its `.sha256`, and only then writes the two files. A missing asset, two different copies, or a mismatch exits 1 and writes nothing, so an empty `sha256 ""` can't be committed (1.4.x and 1.5.0 shipped empty ones: the workflow hashed `dist/<file>` while the artifacts sit in `dist/<artifact>/<file>`; fixed for 1.5.0 in a follow-up commit).
- `check` fails on an empty or malformed sha256, a url for another version, or a missing architecture. CI runs it with `scripts/test-packaging.py` and `scripts/test-release.py`, and on macOS installs the committed formula from a local tap: `brew fetch` for both architectures (formula and cask), each download checked against its sha256, then `brew install` and `mouser --version`.

`scripts/release.py`
- Message: one sentence becomes one bullet under `### Added` (`### Fixed` with `--patch`; `--section Changed` etc. to choose); `- ` bullets and `### Heading` blocks are used as written. Keep it summary level, like the rest of `CHANGELOG.md`.
- Bumps what earlier `release:` commits bumped: `[workspace.package] version` in `Cargo.toml`, `crates/mouser-llm/Cargo.toml` (and any member pinned to the workspace version), `Cargo.lock` if present (gitignored, so not committed), the version lines in `man/*.1` (the man tests compare them with the generated pages), whatever version they carry, and `Current release` in `docs/HANDOVER.md`. The Homebrew files and `mouser-bin` PKGBUILD need the release checksums, so they stay a step 3 follow-up.
- Refuses (exit 1, nothing written) when not on `main`, with uncommitted tracked changes, when `main` is behind or diverged from `origin/main`, when the tag exists locally or on `origin`, or when HEAD is not `--expect-head SHA`. If the commit fails, the files are put back.
- `--no-push` commits and tags without pushing (push later with `git push origin main --follow-tags`); `--no-fetch` skips the `origin` checks; `--date YYYY-MM-DD` overrides the date.
- Tests: `python3 scripts/test-release.py` (temporary repos with a local bare `origin`; no network).

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
  - Prebuilt: download `mouser-bin-<ver>-1-x86_64.pkg.tar.zst` and its `.sha256`, `sha256sum -c` it, then `sudo pacman -U ./mouser-bin-<ver>-1-x86_64.pkg.tar.zst`; or, from 1.4.0, trust the package key once and `sudo pacman -U <url>` (see §Arch signatures)
  - From source: `cd packaging/arch/mouser-git && makepkg -si`
  - Planned: `yay -S mouser-git` (or `mouser-bin`) once published to the AUR.
- macOS:
  - Tap this repository once: `brew tap weirdbrains/mouser https://github.com/WeirdBrains/mouser` (Homebrew no longer installs a formula from a URL).
  - CLI: `brew install weirdbrains/mouser/mouser`
  - App: `brew install --cask weirdbrains/mouser/mouser`
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

Upgrading a machine after a release: download `mouser-bin-<ver>-1-x86_64.pkg.tar.zst` from the release with `curl -LO`, then `sudo pacman -U` the local file
(same package name, so a plain upgrade).

### Publishing to the AUR (not done yet)
Needs the maintainer's personal AUR account (registration is closed at the moment):
1. Add an SSH key to the AUR account, then
   `git clone ssh://aur@aur.archlinux.org/mouser-git.git` (and `mouser-bin.git`).
2. Copy `PKGBUILD` and `.SRCINFO` in, set the `# Maintainer:` line, commit, push.
3. The repo's `LICENSE` (BSD-3-Clause) must ship in `/usr/share/licenses/<pkg>/`;
   both PKGBUILDs install it.
4. Per release, push the updated `mouser-bin` PKGBUILD/.SRCINFO to the AUR.


## Linux build host

The Linux release builds on Ubuntu 22.04 (glibc 2.35), not `ubuntu-latest`: a
binary built against a newer glibc fails to start on older distributions
(`GLIBC_2.43 not found`). 22.04 has no `glslc` package and its Vulkan headers are
too old for llama.cpp, so the workflow unpacks the LunarG Vulkan SDK and puts its
`bin/` first on PATH. The plugin and engine repos build on 22.04 for the same reason.

Three checks keep it that way (v1.3.0 shipped built on a newer runner and needed
`GLIBC_2.43`, issue #2; 1.3.1 fixed the build host):

- `scripts/check-glibc.py --max 2.35 PATH...` prints the newest `GLIBC_x.y` any
  ELF file needs (archives are unpacked) and fails above 2.35. The packaging step
  runs it on `mouser`, `mouser-app` and `engines/`, and the `-full` step on the
  bundle's `plugins/` archives and `engines/`, so a plugin or engine from
  `plugins.lock` built on a newer host fails the release too.
- The `.deb` depends on `libc6 (>= x.y)` and the `.rpm` on `glibc >= x.y`, the
  version the check found, so apt and dnf refuse a system that is too old instead
  of installing a binary that cannot start.
- A clean `ubuntu:22.04` container installs the `.deb`, runs `mouser --version`,
  checks every engine `.so` with `ldd`, and starts each bundled plugin with
  `--manifest`.

Check a downloaded build by hand the same way: `python3 scripts/check-glibc.py
mouser_1.4.0_amd64.deb` does not read `.deb` files, so unpack it first
(`dpkg-deb -x mouser_*.deb d && python3 scripts/check-glibc.py --max 2.35 d`).

Run the workflow by hand (Actions → Release → Run workflow) to build every artifact
without publishing: the publish job only runs for `v*` tags, and the packages are
versioned `0.0.0`.

## Arch signatures

`pacman -U https://…/mouser-bin-<ver>-1-x86_64.pkg.tar.zst` downloads the package,
then `<url>.sig`, because pacman's `RemoteFileSigLevel` (default `Required`)
wants an OpenPGP detached signature from a key in pacman's keyring for anything
installed from a URL. Releases up to 1.3.1 have no `.sig`, so that fails with a
404 there. Never turn signature checking off in `pacman.conf` for this.

From 1.4.0 every Arch package has a `.pkg.tar.zst.sig`, made with the WeirdBrains
Mouser packages key:

- fingerprint `3561BE80D8B3E767CF0B0B176318D43051C4DBBC` (ed25519, expires 2029-11-04)
- key ID `6318D43051C4DBBC` (what `pacman -U` prints when it asks to import the key)
- public key: `packaging/arch/weirdbrains-mouser.asc` in this repo,
  `weirdbrains-mouser.asc` and `mouser-release-key.asc` on each release, and
  https://mouser.weirdbrains.com/mouser-release-key.asc
- also on `hkps://keyserver.ubuntu.com` (Arch's default keyserver) and
  `keys.openpgp.org` (the UID stays hidden there until the address is verified)

Trust it once, then install from the URL:

```bash
curl -sL https://mouser.weirdbrains.com/mouser-release-key.asc | sudo pacman-key --add -
# or: sudo pacman-key --keyserver hkps://keyserver.ubuntu.com --recv-keys 3561BE80D8B3E767CF0B0B176318D43051C4DBBC
sudo pacman-key --lsign-key 6318D43051C4DBBC
sudo pacman -U https://github.com/WeirdBrains/mouser/releases/download/v<ver>/mouser-bin-<ver>-1-x86_64.pkg.tar.zst
```

`pacman -U <url>` prints key ID `6318D43051C4DBBC` when it asks to import the
key. That short id is fine for `--lsign-key`, but `pacman-key --recv-keys`
needs the full fingerprint (or the key file from the site). Answering `Y` to
the import prompt is not enough: the key also has to be locally signed, and a
lookup by the short id can fail with "could not be looked up remotely".

To inspect the fingerprint before trusting it:

```bash
curl -LO https://mouser.weirdbrains.com/mouser-release-key.asc
# or: curl -LO https://raw.githubusercontent.com/WeirdBrains/mouser/main/packaging/arch/weirdbrains-mouser.asc
gpg --show-keys mouser-release-key.asc   # check the fingerprint above
sudo pacman-key --add mouser-release-key.asc
sudo pacman-key --lsign-key 3561BE80D8B3E767CF0B0B176318D43051C4DBBC
sudo pacman -U https://github.com/WeirdBrains/mouser/releases/download/v<ver>/mouser-bin-<ver>-1-x86_64.pkg.tar.zst
```

Without trusting the key (or for 1.3.x), download, `sha256sum -c` against the
release's `.sha256`, then `sudo pacman -U ./<file>`: `LocalFileSigLevel`
(default `Optional`) checks a `.sig` beside the file only if there is one. The
`.sha256` files stay on every release.

The two signatures are different things: `.minisig` is WeirdBrains' minisign
signature (key `61BA1720747F9146`, for Mouser's own downloads and `plugins.lock`),
which pacman cannot read; `.sig` is OpenPGP, what pacman checks.

Release side: the `ARCH_GPG_PRIVATE_KEY` and `ARCH_GPG_PASSPHRASE` secrets hold
the signing key. The release workflow signs each `.pkg.tar.zst`, verifies the
`.sig` with `gpgv` against the committed public key (so a mismatched secret
fails the release) and uploads the key with the release. Rotating the key means
a new `weirdbrains-mouser.asc`, new secrets and a CHANGELOG note.
