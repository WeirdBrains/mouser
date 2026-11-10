## [Unreleased]

### Added

- Commander keyboard help: `?` or F1 (or `ctrl+shift+k` from anywhere) opens **Keyboard help**, every key of the mode, Mouser's mode keys and the global keys with what they do, built from the live key tables (remaps included); it scrolls and closes with `?`, `q` or Esc. See docs/modes.md §Keyboard help.
- Commander has a key for every option: `L` / `G` / `C` list / grid / carousel, `O` open fullscreen, `e` sync to the selected folder, `y` / `p` copy / paste, `c` / `M` copy to / move to, `r` rename, `d` delete, `n` new folder, `Y` / `T` copy path / text, `N` / `D` / `S` sort by name / modified / size, `R` reverse. Mode plugins may bind Shift+letter and F1.
- Commander file changes (copy, paste, move, rename, delete, new folder) work in SSH sessions: they run on the remote host after the approval card.
- Hugging Face models: search, details and install from the chat (`/hf`) or `mouser model hf search|info|install|list|remove`. Only GGUF files whose architecture the bundled llama.cpp supports are offered ("supported architecture, not load-tested"); others say why they won't run. Downloads resume, need and check the Hub's SHA256, and the installed model becomes the current one. Nothing from the repo is executed. See docs/models.md §Hugging Face.
- Chat looks like a terminal agent: rounded frame and bubbles, fullscreen in its pane, and it can run commands (`terminal_run`) after an approval card with Allow / Allow all / Deny; Allow all lasts for the conversation and `/revoke` or `/new` ends it. Commands run in their own process group, time out, and anything they leave running is stopped. See docs/chat.md.
- Chat plays images, video and audio from messages and tool output inline, each with fullscreen (new `mouser-media` crate). mouser-mode-api 0.3: frame and bubble panel styles, Hugging Face and `agent.revoke_all` services.
- Views and media for any mode plugin: `mouser-views` (list / grid / carousel layout, carousel slide, image zoom, scrollbars, labels) and `mouser-media` (player, inline players with fullscreen). Commander now runs on both instead of its own copies. See docs/building-modes.md §Views and media.
- Updating Mouser updates the official plugins it pins: on start, official items installed at an older version than `plugins.lock` are updated (a new permission still asks; `gh:` installs and rollbacks are left alone). This release pins chat 0.1.3 and commander 0.1.5 (mouser-mode-api 0.3). See docs/downloads.md.

### Fixed

- The chat's `terminal_run` on Windows stops what a command started (a Job Object that kills its processes when it closes), as process groups already did on Linux and macOS. It is cleanup, not a sandbox; docs/chat.md says what can still outlive it (`setsid`, job breakaway).
- Commander in an SSH session lists the remote host without any setup there: the ssh shim reports the remote shell's folder and pid, and commander follows its `cd`s. The session's login is shared with commander (the interactive ssh is the ControlMaster), so password, one-time-code and passphrase-key hosts work too. `MOUSER_SSH_SHARE=no` turns the sharing off. See docs/ssh.md.

## [1.4.10] - 2026-11-09

### Changed

- The website now lives on main (Jaspr in site_jaspr/) and deploys with GitHub Actions; scripts/release.py bumps the homepage version in the same release commit.

## [1.4.9] - 2026-11-09

### Changed

- CI and the Docs site workflow start on `v*` tag pushes (and manual dispatch), not on every push to `main`.

### Fixed

- Commander, chat and preferences plugins no longer exit at start on Mouser 1.4.5–1.4.8 (`array had incorrect length, expected 2`): Mouser sends plugins field-named MessagePack, so plugins built before a protocol field was added keep working.

## [1.4.8] - 2026-11-09

### Fixed

- ssh from Mouser: hosts without Mouser's terminfo get `TERM=xterm-256color` (tmux no longer fails with `missing or unsuitable terminal: mouser`); hosts that have it keep `TERM=mouser`. One connection, no install; `MOUSER_SSH_TERM` overrides it. See docs/ssh.md.
- Fallback xterm

## [1.4.5] - 2026-11-06

### Fixed

- Arch: the OpenPGP package key (`6318D43051C4DBBC`) is on keyserver.ubuntu.com and keys.openpgp.org, hosted at https://mouser.weirdbrains.com/mouser-release-key.asc, and attached to releases as `mouser-release-key.asc`. Trust it with `pacman-key --recv-keys` (or `curl` the site key) and `--lsign-key` before `pacman -U <url>`. See docs/release.md §Arch signatures.
- Man page ENVIRONMENT lists `MOUSER_OMARCHY` (it is read to override Omarchy host detection; CI's `no_env_var_is_missing` test failed without it).
- Theme **Duplicate** / editor: blank Cursor, Accent, and palette fields are filled from the source theme (cursor←foreground, accent←ANSI 4, full 16-colour palette) so Edit colours never opens with empty colour fields.
- Chat: opening the mode without `agent.use` already allowed now shows Mouser's approval popup instead of a red `agent.models is not allowed` status. Declared Ask capabilities are prompted on the first service call; Never still hard-denies.
- Theme editor: long font family names in the dropdown keep padding and end in an ellipsis instead of running into the right edge.
- Theme editor: validation messages such as `Font size must be 6-48` have their own row and no longer overlap Save/Cancel/Dropper.
- Theme editor helper row sits below the buttons and no longer repeats the Dropper label (`F2 samples a screen colour`).
- Commander carousel: switching view or sliding to a neighbour no longer flashes the kind-label placeholder when a thumbnail of another size is already decoded.

### Added

- Tab completion in every text field: commander move/copy/rename/new-folder/search/pipe, the `:` command line, theme name and path fields, chat input, and SSH form fields. Tab completes the common prefix; repeated Tab cycles matches, Shift+Tab goes back; directories get a trailing `/`; `~` expands; no match reports `no matches` instead of inserting a tab.
- Theme switcher keys: `a` Auto, `u` Duplicate, `n` New (same as the buttons).
- Theme editor: **F2** runs Dropper on the focused colour field.
- `MOUSER_DROPPER_FAKE=#RRGGBB` returns that colour from Dropper after a short pause (automated recordings).
- Commander fullscreen image/player viewer: opening eases the picture from its thumbnail to the window (~220 ms, cubic ease-in-out, timed from the clock) and closing eases it back, with a crossfading dim backdrop. `appearance.reduce_motion` (Preferences → Appearance → Reduce motion) skips the transition. Closing reuses the decoded thumbnail so the tile does not flash a kind-label placeholder.

## [1.4.4] - 2026-11-06

### Added

- Theme editor **Dropper**: sample a screen colour into the focused field (`hyprpicker`, else `grim`+`slurp`, else ImageMagick `import`).
- Appearance switcher **Auto**: match the Omarchy desktop theme when present (same source as Match Omarchy; more desktops later).
- Font picker rows render in each family’s own face.
- Colour editor side **Preview** pane for pending fg/bg/cursor/accent.
- Appearance switcher: **New**, **Duplicate**, and **Export** for user themes; `:import-theme <dir>` and `:export-theme [name]`.

### Changed

- Site hero: borderless video edge (10px radius, 6px chrome inset); no CSS pad/ring or painted frame.

## [1.4.3] - 2026-11-05

### Added

- Built-in **Omarchy** theme (Trainyard palette). On Omarchy hosts (`/etc/os-release` `ID=omarchy`, or `MOUSER_OMARCHY=1`) it is the default when no `theme` preference is set.
- Appearance switcher: **Match Omarchy** copies the current Omarchy desktop theme's `colors.toml` into Mouser's Omarchy theme and selects it.

### Changed

- Front page hero video cropped to window chrome with matching edge color (seamless rounded corners, no dual border).

## [1.4.2] - 2026-11-05

### Fixed

- MCP: a second Mouser window no longer shows a scary "Address already in use" toast when 7801/7802 are held. It binds an ephemeral loopback port instead and logs a line; clients that need a fixed URL should use the first window or `mouser mcp serve --stdio`. See docs/mcp.md.
- Appearance switcher: dark/light/system mode labels never truncate mid-word in a narrow window. Buttons size to Unicode width; when tight they shrink padding, then abbreviate to `d` / `l` / `sys`.
- Chat: the input line and status sit above the host dock chrome (⤢ fullscreen and the focus hint), so the F-button no longer overlaps the footer.

### Changed

- Front page: "Mouser is beautiful" clip uses a Trainyard wallpaper panel (no black box); hero is H1 → video → Version/CTA → lede.

## [1.4.1] - 2026-11-05

### Changed

- Website docs: maintainer pages (deploy-site, release process, HANDOVER) are `internal` and no longer published on mouser.weirdbrains.com. Every public guide still has a site page; see AGENTS.md and docs/sections.toml.
- Developer Guides: how-tos for building a mode plugin, a custom rendering engine, integrating an app with Fuzzball, and using Mouser's MCP server from a plugin (docs/building-modes.md, custom-engines.md, fuzzball-integration.md, mcp-from-plugins.md).
- Front page: "Mouser is beautiful" hero clip (autoplay muted loop, WebM+MP4, poster; prefers-reduced-motion shows the poster). Re-record after the theme editor ships.

## [1.4.0] - 2026-11-05

### Fixed

- Arch: `pacman -U <url>` failed on a missing `<url>.sig` (pacman wants an OpenPGP signature for URL installs; our `.minisig` is minisign, a different format). Arch packages now ship an OpenPGP `.pkg.tar.zst.sig` from the WeirdBrains Mouser packages key (`3561BE80D8B3E767CF0B0B176318D43051C4DBBC`, `packaging/arch/weirdbrains-mouser.asc`, also attached to the release). Trust it once with `pacman-key --add` and `--lsign-key`, and URL installs work. The `.sha256` files stay, and download, `sha256sum -c`, `pacman -U ./file` still works without the key. The release verifies each `.sig` against the committed key. See docs/release.md §Arch signatures.
- Linux packages: the release fails if any binary, engine or bundled plugin needs a glibc newer than 2.35 (Ubuntu 22.04), using `scripts/check-glibc.py`. The `.deb` and `.rpm` now declare the glibc they need (`libc6 (>= x.y)`, `glibc >= x.y`), and a clean Ubuntu 22.04 container installs the `.deb` and starts Mouser, its engines and plugins. Issue #2: v1.3.0's Linux build needed GLIBC_2.43 and would not start on Ubuntu 24.04 / Pop!_OS; 1.3.1 and later need GLIBC_2.34. See docs/release.md §Linux build host.

### Changed

- No mode is compiled into Mouser any more: commander, chat and preferences are only the plugin programs (`mouser-commander`, `mouser-chat`, `mouser-preferences`), installed from `plugins.lock` on first run (or from the `-full` bundle offline). The built-in `commander`, `chat`, `preferences` entries and their compiled-in manifests are gone (`[overrides built-in]` no longer appears); the official modes keep their names and hotkeys. `mouser_core::commander` keeps only listings, entries, kinds and badges, shell quoting and the default keys. The commander plugin's end-to-end tests check the plugin through Mouser on its own. See docs/modes.md and docs/plugins.md.
- Modes take no key from the shell. Commander (and chat, preferences and mode plugins) binds one key only, the focus toggle **ctrl+alt+k** (free in bash, zsh and fish defaults and on the desktops; `[focus] key` in `modes.toml`). The toggle, a click in the view or `esc :` gives the mode the keyboard (accent ring and a key hint); its keys are then plain (arrows, Enter, Space, `s` sync, `/` search, `v` view, `u` back, `h` dotfiles, `f` fullscreen, `k`/`j`/`l` player ...) and nothing typed reaches the shell. Esc, `q` with nothing open, or the toggle give it back. Replaces the alt+key bindings (alt+t, alt+s, alt+arrows, alt+Enter, alt+Backspace ... shadowed shell bindings); the old `modifier` setting is ignored. A test holds the bash/zsh/fish default tables and fails if the toggle collides. docs/modes.md, docs/chat.md, docs/preferences.md, docs/plugins.md, the manual and `ctrl+shift+k` updated.

### Added

- MCP: mode interactions. `modes_list`, `modes_switch`, `mode_actions` (the current mode's context-menu actions for its selected item and view state, with keys, enabled and destructive) and `mode_action` (run one as its key would; asks on the approval popup with the new `modes.control` capability, and destructive file changes ask again on the mode's card). Same registry as keys and menus. See docs/mcp.md §Mode actions and docs/mode-protocol.md.
- Mode panel: resizable and fullscreen. While a mode has the keyboard, `F` toggles fullscreen (or the ⤢ button at the panel's bottom left), `]` / `[` make it taller / shorter, and dragging its bottom edge resizes it; the size is saved (`[window] dock_height`, `dock_full` in preferences.toml). Modes get it as `dock` in `view.resized` (`DockSize` in mode-api). Remappable in `[host.keys]`. See docs/modes.md.
- Context menus in modes: right-click (or `.`) lists every action for the item and view state, each with its key, built from the mode's action registry (`menu.open` / `Out::Menu` / `MenuEntry` in mode-api). See docs/modes.md and docs/mode-protocol.md. Commander's menus cover a file, media file, folder, the empty area, the image view and player (fullscreen or not) and the text view, with new actions copy / paste, copy to…, move to…, rename…, delete…, new folder…, copy path, copy text and sort; every file change asks for a name or destination in the prompt line, then waits for Mouser's approval card, never overwrites, and is local only (the commander plugin now declares `files.write`). Chat: copy, quote, retry, delete from view on a message; output and copy on a tool card. Preferences: edit, step, reset to default, copy value (never a key) on a setting. Mouser fills each entry's key from `modes.toml`. Mode plugins can put text on the clipboard (`ClipboardSet`). `mouser-mode-api` 0.2.0.
- One-character keys in `modes.toml` are case sensitive (`F` is Shift+F; before, it meant `f`).
- Releases carry `-full` offline bundles (`mouser-<target>-full.tar.gz` / `.zip`): Mouser plus every `plugins.lock` archive for the target in `plugins/`, so first run installs all engines and plugins without a network (`scripts/full-bundle.py` checks each against the lock).
- `plugins.lock` pins commander 0.1.4 (audio host and device, the focus-model keys, silent sync, context menus and file actions), chat 0.1.2 and preferences 0.1.2 (context menus, the panel size).
- Release: the `-full` bundle step sets up Python 3.12 (the bundle script needs `tomllib`; ubuntu-22.04 has 3.10). The pwsh sync test presses Enter again if a slow runner drops the first one.
- `mouser shell-integration pwsh`: PowerShell reports its folder (OSC 7) and takes commander's sync at the next prompt through a per-session sync file (a builtin `Set-Location`; nothing typed or added to the history).
- Preferences → Audio: `audio.host` (auto, jack, alsa on Linux; auto tries JACK first, PipeWire's JACK included, else ALSA), `audio.output_device` (a dropdown of that host's devices, default = the system default; a missing device falls back to the default with a notice) and the active host. JACK is the optional `jack` feature with libjack loaded at run time; the official Linux builds and CI include it. The player's status shows the host playing. See docs/preferences.md §Audio.
- Commander: back works like `popd`. Entering a folder or file pushes where you were (the path as you saw it, symlinks kept, with the selection and scroll); back restores it exactly. With an empty stack back goes to the parent of the logical path, never a symlink target's real parent. Remote folders too. See docs/modes.md.
- `plugins.lock` pins the first signed releases of the official plugins and engines (commander, chat, preferences 0.1.0; engines `mouser` and `skia` 0.1.0) from their own WeirdBrains repos, so `mouser plugin install commander` and first run fetch and verify them. See docs/downloads.md.
- The official WeirdBrains minisign key (`61BA1720747F9146`) is pinned: `plugins.lock` items must verify against it, and Mouser's release files are minisigned with it. See docs/downloads.md.

- `mouser-mode-api`: the protocol mode plugins will use — a persistent child process speaking MessagePack (or JSON) over stdio, version negotiation, shared-memory images (1080p at 60 fps with no dropped frames), host services gated by permissions, and crash restarts that back off and stop after `max_restarts`. Commander, chat and preferences move onto it in 1.4.0. See docs/mode-protocol.md.
- Mode plugin protocol: plugins register event listeners in the handshake, emit their own events (heard by themselves too, with loop and rate limits), and call host files/process APIs that never exceed your rights — elevation only through sudo in your visible terminal, UAC or the macOS prompt. See docs/mode-protocol.md.
- Mode plugin SDK: a `Mode` trait and `serve()` for writing a plugin, panels with the plugin's own click actions, and a host supervisor that starts a plugin, runs the handshake and restarts it after a crash (1 s, 2 s, 4 s), giving up after `plugins.max_restarts` (a new preference, default 3) with a restart action. See docs/mode-protocol.md.
- Plugins: `transport = "mode"` runs a mode plugin as a long-lived process on the mode protocol — views and images in the dock, effects checked against its permissions, its own events delivered back to it, crash restarts with a Restart button once it gives up. See docs/plugins.md and docs/mode-protocol.md.
- Mode plugins can call Mouser: preferences (secrets never exposed), their granted permissions, and read-only local files with `files.read`, never inside `~/.ssh`, `~/.gnupg`, keychains or Mouser's secrets. See docs/mode-protocol.md.
- Plugins are also found beside the Mouser executable (the offline bundle) and in `/usr/lib/mouser/plugins` and `/usr/local/lib/mouser/plugins`. `MOUSER_PLUGIN_DIRS` replaces those folders. Your own copy in `$MOUSER_HOME/plugins` wins. A plugin folder can hold versions (`<name>/<version>/`) with `current` choosing one, and a mode plugin can replace a built-in mode of the same name. See docs/plugins.md.
- Mode plugins can now make web requests through Mouser (`fetch`), read files on the shell's ssh host (`remote.*`), and run agent chats inside Mouser with your models and keys (`agent.*`, streamed as `agent.state` events). They can also ask through Mouser's own popup: their own yes/no questions, a permission from their manifest, or an API key, which goes straight to your keychain so the plugin never sees it. Agent tool-call cards are drawn by Mouser too. New permissions: `remote.read`, `agent.use`, `prefs.write`, `net.fetch`. Typing reaches a mode plugin only while its view has a focused field. See docs/mode-protocol.md.
- The preferences mode is also a separate plugin program now (`plugins/preferences`, `mouser-preferences`), built only on `mouser-mode-api`. Installed, it replaces the built-in mode. API keys are typed into Mouser's popup, never into the plugin. A mode plugin's open dropdown (a `Popup` item) now takes the arrows and Enter, like a focused field. See docs/preferences.md.
- The chat mode is also a separate plugin program (`plugins/chat`, `mouser-chat`) on `mouser-mode-api`. The agent, your keys and tool approvals stay in Mouser; the plugin draws the Markdown transcript and has a model picker. The Markdown and highlighting code moved into `mouser-mode-api` (feature `markdown`) so plugins render it the same way. See docs/chat.md and docs/markdown.md.
- The commander mode is also a separate plugin program (`plugins/commander`, `mouser-commander`) on `mouser-mode-api`: the list, grid and carousel, search, pipe, the file view with Markdown, image previews and zoom, drawn exactly like the built-in mode (tested side by side through Mouser). With `remote.read` it shows remote (SSH) folders and previews their files too; Mouser tells mode plugins the ssh session and listing (`shell.remote_ssh`, `shell.remote_changed`). The player runs in the plugin as well (ffmpeg, audio) and hands its frames to Mouser through shared memory at the clip's frame rate; Mouser answers `frame.presented` so a slot is never overwritten before it is copied. See docs/modes.md and docs/mode-protocol.md.
- Mode plugins can say what they take from the keyboard (`Wants`: typing, extra keys, Escape), so alt-key bindings keep working while a plugin's search field is open. Mouser tells a plugin the shell's folder as soon as it starts and after a restart. Plugin key bindings can use punctuation, `space` and `backspace`. See docs/mode-protocol.md and docs/plugins.md.
- The mode plugin protocol is frozen at api 1: plugins built on `mouser-mode-api` 1.4.0 keep working until the wire format breaks, and Mouser will accept the previous api for one more version when it does. Plugins built on the unreleased api 0 draft are refused with "update the plugin".
- Engine ABI v2: engines export `mouser_engine_v2` (a v1 table plus an ABI range, a capabilities query, DPI changes, cache eviction and vsync control, all optional), Mouser negotiates the newest shared ABI, and v1 engines still load. The `flat` example exports both; `mouser_engine.h` and docs/engines.md describe v2.
- The basic renderer: Mouser's built-in CPU drawing is named and labelled in the status bar ("basic renderer — no GPU engine running yet") whenever no engine starts, switches live to an engine once one can start, and can never be removed (`basic` and `cpu` are reserved engine names). `engine.toml` may declare ABI 1 or 2.
- Tests: with no usable engine the basic renderer draws the terminal, and when an engine becomes usable the same UI switches to it live with its shell and tabs (the headless test UI now re-checks the engine order like a window does).
- Mouser's wgpu renderer is also an engine library on the C ABI (`crates/mouser-engine-wgpu`, engine `mouser`, ABI v2), pixel-identical to the built-in renderer in a new offscreen parity test that CI runs on a software adapter. ABI v2 frames keep translucent colours (top byte `255 - opacity`); v1 engines still get opaque ones.
- Mouser's Skia renderer is also an engine library on the C ABI (`crates/mouser-engine-skia`, engine `skia`, ABI v2), pixel-identical to the built-in skia engine in a new raster parity test.
- Engines can ship with Mouser: `engines/<name>/` beside the executable (or the official libraries loose next to a development build) are found after the user's installs, can't be uninstalled, and `MOUSER_ENGINE_DIRS` overrides where they are looked for.
- The `mouser` (wgpu) and `skia` engines are no longer compiled into Mouser: they ship as engine libraries in `engines/` beside it (a newer one can be installed over them), Mouser no longer links wgpu or Skia, and the `skia` cargo feature is gone. When neither starts, the basic renderer draws; `mouser` can now be skipped after a crash like any engine. The parity tests hold the libraries to golden frames saved from the old built-ins.
- The install core behind plugin and engine downloads (mouser-core `install` and `downloads`): a download manager on its own threads (4 at once, 2 per server, cancel and retry), HTTPS-only fetching with a size limit and sha256, minisign verification (the official key, or an author's key remembered on first use), safe archive unpacking, and a versioned store (`<name>/<version>/` with `current`, two versions kept) that engine discovery now reads. The CLI and `esc :` commands come next. See docs/storage.md.
- `mouser plugin|engine install|update|rollback|remove|list|retry|cancel`, `mouser --install SRC [--wait] [--yes]` and `mouser downloads [--cancel ID|all]`: install from an official name, `gh:Owner/Repo[@tag]`, an `https://` URL, a path or archive, or `crate:NAME`, with every check shown on a card before installing (`--yes` skips the question). Engine libraries are probed in a separate process. See docs/downloads.md.
- `esc :plugin VERB` / `:engine VERB` with the same verbs as the CLI, the short forms (`:pi`, `:ei`, `:pu`, `:eu`, …), `:downloads` and `:cancel`. Installs run on the download manager and ask on a card; progress shows on the status line. See docs/downloads.md.
- `mouser engine bundle [--from DIR] --out DIR`: copies Mouser's engine libraries into `DIR/<name>/` with their engine.toml. Release archives, `.deb`/`.rpm`/Arch packages (`/usr/lib/mouser/engines`), `Mouser.app`, the Homebrew formula and the Windows installer now ship the engines, and Mouser also looks in `../lib/mouser/engines` beside itself on Unix. See docs/release.md.
- Release signing, ready for keys: the release workflow signs the macOS binaries, engines and `Mouser.app` (and notarizes the dmg), Authenticode-signs the Windows binaries and installer, and minisigns every release file when the matching secrets are set; without them it builds unsigned with a notice. See docs/release.md.

### Fixed

- Windows: listing audio devices for preferences no longer crashes when several callers probe at once; probing runs on one thread.
- macOS: dingbats missing from the primary font and Apple Symbols, such as oh-my-zsh's prompt arrow ➜ (U+279C) with Cascadia Mono PL, no longer show as boxes: Menlo, Zapf Dingbats and Arial Unicode MS follow Apple Symbols in the fallback list.
- Installs: archives made with `tar -C dir -czf x.tar.gz .` (a leading `./` entry) unpack instead of failing with "empty archive entry name".
- Mouser's own engines (bundled, staged or beside a dev build) report Mouser's version instead of 0.4.0 (mouser-core's).
- Tests: the headless test UI writes its status file through a per-process temp file and a rename, so a crashed UI and its replacement can't interleave their writes (a flaky engine-fallback test on macOS).
- Tests: the commander folder test no longer depends on the filesystem's timestamp resolution.
- Tests: the commander plugin parity test waits for the grid and carousel thumbnails on both sides instead of comparing two empty grids (failed on a fresh, fast machine).
- CI installs ffmpeg on all three OSes and fails the player test if ffmpeg is missing (`MOUSER_REQUIRE_FFMPEG`), so the commander plugin's 60 fps shared-memory path is really tested instead of possibly being skipped.

### Changed

- Commander keeps its own folder: browsing (open, back, the header path, search jumps) never types into the shell or changes its folder, local or over SSH (remote folders are listed over the ssh side channel). The shell's folder changes only with **sync** (`s` while commander has the keyboard), silently through the shell integration (nothing typed, nothing in the history, the half-typed line kept): at once when the shell is idle at its prompt, at its next prompt while a command runs. Without an integration the status bar says so; the typed-`cd` fallback is gone. A `cd` typed in the shell still moves commander. Rebindable (`sync` in `modes.toml`). Keys can now be ctrl+alt+letter (`ctrl+s` in `modes.toml`). See docs/modes.md §Sync.
- plugins.lock pins commander 0.1.2 (the navigation stack), chat, preferences and the mouser and skia engines at 0.1.1 (Linux builds on Ubuntu 22.04, so they load on glibc 2.35+).
- Releases: Linux builds run on Ubuntu 22.04 (glibc 2.35) with glslc and Vulkan headers from the LunarG SDK, so the binary runs on older distributions. A manual run of the release workflow builds and packages everything as v0.0.0 and publishes nothing.
- CI: the commander player fps test holds the plugin to 20 fps on CI runners (30 locally); keeping up with the built-in player on the same machine is still required.

## [1.3.1] - 2026-11-05

### Fixed

- macOS: dingbats missing from the primary font and Apple Symbols, such as oh-my-zsh's prompt arrow ➜ (U+279C) with Cascadia Mono PL, no longer show as boxes: Menlo, Zapf Dingbats and Arial Unicode MS follow Apple Symbols in the fallback list.
- Linux release builds run on Ubuntu 22.04 (glibc 2.35), so the binary starts on older distributions.

## [1.3.0] - 2026-11-04

### Added

- Commander: Markdown files preview rendered (headings, emphasis, lists, quotes, code blocks, links, tables) in the grid, carousel and file view. See docs/markdown.md.
- Chat: messages and replies render as Markdown, streaming replies included; links are clickable. See docs/chat.md.
- Markdown code blocks are syntax highlighted by their fence language (Rust, Python, JS/TS, shell, JSON, TOML, YAML, Go, C/C++, HTML, CSS, SQL, Markdown, diff) in chat, commander previews and `mouser md`, in the theme's palette colours. See docs/markdown.md.
- `mouser md [FILE…]`: Markdown (files or stdin) formatted into the terminal with SGR styles and OSC 8 links; it stays in the scrollback. `--width`, `--color auto|always|never`. See docs/markdown.md.

## [1.2.0] - 2026-11-04

### Added

- Windows: Mouser can be the default terminal for console programs. The installer registers Mouser's terminal handoff (ITerminalHandoff3) for the current user; `mouser default-terminal set` makes it the default and remembers the previous one, `unset` and uninstalling put it back, and `status` shows what is set. Needs Windows Terminal installed. See docs/windows.md.
- MCP server: Let's Encrypt certificates (`[mcp.server.tls] source = "acme"`). tls-alpn-01 on the HTTPS port, http-01 on an optional port-80 listener, or dns-01 through a hook command or by hand. Renews 30 days before expiry and swaps the certificate in without a restart; failures keep the old one and back off. `mouser mcp cert status|renew`, preferences-mode settings, and `bind` to listen beyond 127.0.0.1 (HTTPS and token only). See docs/mcp.md.
- Grok provider: `mouser model add grok` takes a console.x.ai key (`xai-…`), a grok.com key (`grok_…`, Cursor agents API) or the Grok Bot desktop login (`--desktop`). Without `--model` an interactive picker lists the models. Keys sent to the wrong provider are refused with a hint. See docs/models.md.
- Built-in help: `mouser help [TOPIC | search WORDS]` shows the docs/ guides. See docs/help.md.
- Man pages: `mouser man --out DIR` writes mouser(1) and one page per command, with every flag and environment variable. See docs/cli.md.
- Docs: a guide per feature indexed in docs/README.md, command-line and deploy guides (MCP server, website). Finished plans moved to docs/historical/.

### Changed

- Commander carousel: each item's name floats over its preview in the same place (a see-through rounded tag with a soft shadow; long names end in …), and folders show a folder icon. See docs/modes.md.
- Commander grid and carousel: a type badge (folder, image, video, audio, text, code, Markdown, PDF, archive, binary, config, font, document) on each item's top-right corner. See docs/modes.md.
- Fuzzball encoding and parsing now come from the published [`fuzzball`](https://crates.io/crates/fuzzball) 1.0 crate, and the spec lives at https://weirdbrains.github.io/fuzzball/. The bytes on the wire are unchanged.

### Fixed

- Trackpad scrolling works: small pixel scroll deltas (Wayland, macOS) are accumulated into whole lines instead of being dropped, and are sent to mouse-reporting programs one wheel report per line.

## [1.1.5] - 2026-11-02

### Fixed

- Emoji ZWJ sequences no longer overlap the next cell: leftover zero-width glyphs from Apple Color Emoji are dropped, and family sequences that Segoe UI Emoji cannot merge are drawn side by side inside their 2 cells.
- Skin-tone and profession sequences (👋🏽, 👍🏿, 👩🏽‍💻) stay one combined glyph.

## [1.1.4] - 2026-11-02

### Fixed

- Windows: colour emoji draw again. Segoe UI Emoji on Windows 11 is a COLR v1 font (gradients, transforms, layers), which the old COLR v0-only path could not read, so every emoji fell back to a box. Mouser now paints COLR v0 and v1 glyphs with tiny-skia (linear and radial gradients, clips, composite modes; sweep gradients use a solid stop colour). Segoe UI Emoji has no flag glyphs, so flags stay as letters on Windows.

## [1.1.3] - 2026-11-02

### Fixed

- Windows: Mouser no longer dies when Windows Terminal is the default terminal application. Mouser is now a GUI-subsystem program, so launching it opens no console for Windows to hand off to Windows Terminal (closing that window used to kill Mouser). CLI subcommands (`mouser --version`, `mouser model …`) attach to the console they were started from.
- Windows: helper programs (ssh, ffmpeg/ffprobe, plugins, MCP servers, engine hosts, openers) start with `CREATE_NO_WINDOW`, so none of them pops up a console or Windows Terminal window.

## [1.1.2] - 2026-11-02

### Fixed

- Emoji no longer show as boxes: Mouser falls back to the system colour emoji font (Noto Color Emoji, Twemoji, JoyPixels… on Linux; Apple Color Emoji; Segoe UI Emoji) and draws CBDT/sbix bitmaps and COLR glyphs in colour.
- Emoji clusters take one 2-cell slot: skin-tone modifiers, ZWJ sequences (up to three joined parts) and flags join the base, and VS16 widens text symbols such as `❤️`.

### Added

- `emoji_font` (`[ui]` in config.toml or preferences, `esc :p`) picks the emoji font; `"none"` turns colour emoji off.
- Packages suggest the emoji font: Arch optdepends `noto-fonts-emoji`, deb Recommends `fonts-noto-color-emoji`, rpm Recommends `google-noto-color-emoji-fonts`.

## [1.1.1] - 2026-11-02

### Added

- Chat: `alt+m` or clicking the model name in the header opens a model switcher (configured models plus installable local/hosted ones, current marked). The pick becomes the default and applies from the next message; it is disabled while a turn runs.

## [1.1.0] - 2026-11-02

### Added

- Preferences mode (`esc :p`): every preference by section, dropdowns for choices, validation, reset, write-back that keeps comments, live apply; secrets go to the OS keychain only.

## [1.0.2] - 2026-11-02

### Added

- Scroll position indicators in commander: a slim scrollbar and `x–y of N` in the list, grid and text views; `n of N` and a track in the carousel.

## [1.0.1] - 2026-11-02

### Added

- Chat model picker: with no usable model, `esc :a` sets one up (local Gemma or a hosted provider).
- The local model is built into every package; `mouser model local enable|disable|move`.
- `mouser model default|test` accept a bare model name.
- Fuzzball shell actions (`s`): silent `cd` from commander with the zsh/bash/fish integration (`mouser shell-integration`).
- Commander fullscreen media: `alt+e`, the header button or a double-click.
- Mouser remembers the last mode a window closed in.

### Changed

- `esc :` waits 1200 ms for the `:` (was 500 ms), configurable with `[keys] leader_timeout_ms`.
- Without the shell integration, the typed `cd` is erased so one prompt remains.
- Linux packages depend on the Vulkan loader.

## [1.0.0] - 2026-11-01

Mouser 1.0: everything below since 0.7.2 (modes and engines as plugins, the chat
agent, MCP server and client, model providers, the local model) plus these fixes.

### Fixed

- Scrollback keeps colours and attributes (bold, underline, inverse, …) after lines
  scroll off and through reflow; plain-text readers are unchanged.
- Chat cancel now stops a running MCP call: the server gets `notifications/cancelled`,
  and a stdio server that hasn't stopped within 1 s is killed (its whole process
  group) and reconnected. Before, the call was only abandoned.
- Media player: after a seek, audio waits for the first video frame (at most 3 s), so
  on slow SSH links the picture no longer starts up to ~570 ms behind the sound.
- Commander mouse input (click to select, double-click to open, buttons, wheel and
  Ctrl+wheel zoom) is now covered by tests through the window's own routing.

### Known limitations

- Windows runs single-process: an engine or GPU driver crash closes the window and
  ends its shells, and custom engines run in-process (README, Known limitations).

### Changed: license

- Mouser is now licensed under BSD-3-Clause (was MIT). See `LICENSE`.

### Changed (P6): modes and engines are plugins (docs/plugins.md)

- commander, chat and the mouser/skia engines are compiled-in plugins with their
  own `plugin.toml`. The mode switcher, `mouser plugin list`, `mouser engine list`
  and the sidebar's engine list all come from the plugin registry.
- `mouser engine …` is now an alias of `mouser plugin … --kind engine`. `--kind`
  works on every plugin command, and `mouser engine enable|disable` are new.
- Built-ins can be disabled (`mouser plugin disable commander`, `… skia`),
  except the `mouser` engine. With every mode off, Mouser is a plain terminal.
  Disabled engines are passed over at start and in the sidebar.
- Engines may use `plugin.toml` (`kind = "engine"`, `transport = "library"`,
  `[engine] abi/library`); `engine.toml` still works and is read as the same
  manifest. The engine C ABI is unchanged.
- Mode plugins: `kind = "mode"` over the command transport with `[mode] hotkey`
  and `[[mode.keys]]`, drawn in the dock from the program's JSON replies. They are
  gated by the `run` and `files.read` permissions. Example:
  `examples/plugins/hello-mode`.
- Installing a mode plugin keeps commander as the default mode.

### Added (P5): chat mode, the agent (docs/chat.md)

- New mode plugin `chat` (`esc :a`). It swaps commander out and docks above the prompt with a transcript, an input and Cancel. Keys are in `modes.toml` `[chat.keys]`.
- It sees the active tab's screen and the last 200 scrollback lines, with secrets redacted (best effort). Screen text and tool output are marked untrusted in the prompt.
- Tools come from Mouser itself (in-process), connected MCP servers and MCP plugins.
  - Every call goes through the permission broker, with an approval card in the dock showing the real action.
  - Shell, typing and file-write tools ask on every call. Other tools can be allowed for the chat.
- Cancel (Esc, alt+x, Ctrl+C or the button) always works: it stops the stream, denies an open card and abandons a running tool. There is a limit of 8 tool steps per turn.
- It uses `mouser model default`: gemma-local with `--features local-llm` (default since 1.0.1), or any provider. Hosted providers show a one-time "screen text is sent to" note.
- Transcripts are saved to `$MOUSER_HOME/chats/*.jsonl` (0600); `[chat] transcripts = false` turns this off.
- `examples/chat_live` is a headless live run with a sandboxed shell tool (feature local-llm).

### Added (P4b): the built-in local model (docs/models.md §The local model)

- `gemma-local` runs Gemma 4 E2B (default) or E4B on llama.cpp inside Mouser: crate `mouser-llm`, behind the opt-in cargo feature `local-llm`, with Vulkan (Metal on macOS). The default build and CI are unchanged.
- It can use an OpenAI- or Anthropic-compatible server instead: `mouser model local url URL [--api anthropic] [--model NAME] [--key-stdin]`.
- Downloads always need consent. `mouser model local download [ID]` shows the size and asks; `--yes` skips the question. In the app, `[mouser] model download` opens an Allow once / Deny popup that is never saved. Downloads resume and are checked against a pinned SHA256.
- `mouser model local [status] | delete ID | dir PATH`. The models dir can be set with `MOUSER_MODELS_DIR` or `[models.local] dir`.
- Prompts use the model's own chat template (minijinja) and a `<tool_call>` tag protocol for tools. `mouser model test --tool` now does a full round trip (call, demo result, reply). `MOUSER_LLM_TIMINGS=1` prints speeds.
- Arch: `MOUSER_LOCAL_LLM=1 makepkg` builds the variant; default deps are unchanged.

### Added (P4): model providers as plugins (docs/models.md)

- `mouser_core::models::ModelProvider`: one interface with streaming (text, tool-call deltas, usage, done) and tool calling (tools as JSON Schema, tool choice, tool results). Cancel works through the sink.
- There are three wire adapters on Mouser's std + rustls HTTP stack (no new system deps):
  - OpenAI-compatible Chat Completions, covering OpenAI, xAI, OpenRouter, Moonshot/Kimi and any custom URL such as llama-server, Ollama or vLLM. Kimi K2 gets its sampling and `tool_choice` quirks handled.
  - Anthropic Messages, also for compatible URLs.
  - Google Gemini native. Gemini's OpenAI-compatible endpoint works through the openai adapter.
- Every model is a `model-provider` plugin:
  - openai, xai, openrouter, moonshot, anthropic and google are built in;
  - you can add `[[models.providers]]` entries;
  - installed plugins can declare a `[model]` table with the new `http` transport (see `examples/plugins/ollama-model`).
- `mouser model list | add | remove | test | default`. Keys are read with `--key-stdin` into the OS keychain, and preferences.toml holds only `keychain:` references.
- `gemma-local` is a stub manifest that declares Gemma 4 E2B as the default with an E4B switch. Building and downloading it are P4b and wait for approval.

### Added (P3): MCP client and the sidebar MCP section (docs/mcp.md §Client)

- `[[mcp.servers]]` in preferences.toml takes a stdio `command` (with `env`) or a Streamable HTTP(S) `url`. Secrets are `keychain:NAME` references; tokens go to the OS keychain (or a 0600 file with a warning) and never into preferences.
- `mouser mcp servers add NAME (--url URL [--token-stdin] [--ca PEM] | [--env K=V] -- COMMAND…) | list | remove NAME | test NAME [--call TOOL --args JSON]`.
- Mouser keeps a connection to every server and every enabled MCP plugin. It reconnects with backoff, pings every 30 s, and accepts JSON or SSE replies. Plain http is allowed to loopback only; https checks the Mozilla roots or your `ca`.
- The sidebar has an **MCP** section showing each server's state (connecting, connected, failed with the reason, ask, denied, off) and its tools. Click a server to show its tools or retry it; click a tool to call it.
- `[mouser]` line: `mcp`, `mcp tools ID`, `mcp reconnect ID`, `mcp call ID TOOL [JSON]`.
- Every tool call goes through the permission broker (`mcp-server:<name>` / plugin name, capability `tool.<name>`) with the approval popup. MCP plugins need their `run` permission before they start.

### Added (P2): Mouser's MCP server (docs/mcp.md)

- It is off by default. Turn it on with `mouser mcp enable [--https]`; `mouser mcp status | disable | token --show | --rotate` manage it, and the settings live under `[mcp.server]` in preferences.toml.
- It offers three transports:
  - `mouser mcp serve --stdio`, which bridges to the running Mouser;
  - Streamable HTTP on `127.0.0.1:7801/mcp`;
  - HTTPS on `127.0.0.1:7802/mcp`, with your own cert or a self-signed one kept in `$MOUSER_HOME/mcp/` (pin it by its SHA256).
- Security:
  - It binds loopback only and needs a bearer token.
  - The token is kept in the OS keychain (Keychain, Credential Manager, Secret Service), or in a 0600 file with a warning.
  - Host and Origin are checked against DNS rebinding.
- Tools: `tabs_list`, `screen_read`, `scrollback_read`, `shell_type`, `shell_run`, `files_list`, `files_read`, `files_mkdir`, `files_copy`, `files_move` and `files_trash` (to Mouser's trash).
- Typing, running and file changes ask first in Mouser's approval popup (key `mcp:<client>`).

### Added (t200u): Allow always for plugins over ssh

- The approval popup for a remote (ssh) Fuzzball plugin offers **Allow always** next to Allow once and Deny.
- The grant is saved in `plugins/permissions.toml` under the host identity (`user@host:port` from `ssh -G`, plus the host key's SHA256 fingerprint from known_hosts when there is one), the plugin name, and each capability. A different host or key, or a request for more capabilities, prompts again.
- `mouser plugin permissions` with no name lists every saved grant; `mouser plugin permissions '<key>' revoke` removes one.

### Added (t198u): the plugin system (phase 1 of docs/plans/mcp-agent.md)

- `plugin.toml` manifests in `$MOUSER_HOME/plugins/<name>/`, and a registry by kind: mode, engine, model-provider, tool-provider, command. Commander and the engines are listed as built-ins.
- `mouser plugin list | install | uninstall | enable | disable | priority | move | permissions | probe`. It mirrors `mouser engine`, which stays as is; an engine directory given to `plugin install` goes to the engine installer.
- A permission broker: allow / ask / never per plugin and capability, saved in `plugins/permissions.toml`.
- The approval popup is drawn by Mouser above the terminal. While open it takes all input, ignores answers for 400 ms, and names local vs REMOTE requests. Terminal output can't draw or answer it.
- Three transports:
  - **command** plugins add words to the `[mouser]` line (an executable; JSON on stdin and stdout; 10 s limit; scrubbed environment);
  - **MCP** plugins over stdio (handshake, `tools/list`, `tools/call`; `mouser plugin probe`);
  - **Fuzzball `p`** (`p h` hello, `p q` query, `p b` bye; docs/protocol/plugins.md). Grants last for one tab; over ssh it is untrusted and asks every time. The capability reply now says `sub=gkp`.
- Example plugins for each transport in examples/plugins; the docs are in docs/plugins.md.

### Added (t183u-t186u): remote files like local ones

- Commander in SSH sessions shows thumbnails, opens the viewer (zoom included) and plays video/audio with seek, instead of a notice.
- New `mouser_core::fetch` layer (reusable for a later sync): ranged, parallel reads (3 segments per file, 3 files at once) over the session's ControlMaster side channel into a sparse cache (`$MOUSER_HOME/cache/remote`, key host+path+size+mtime, filled-range map, LRU cap `MOUSER_REMOTE_CACHE_MB`, default 2048). The first read is the 256 KB head; seeks and scrolls prioritise their range; off-screen / closed transfers stop, small ones (`MOUSER_REMOTE_COMPLETE_MB`, default 64) finish into the cache with rsync (both ends) or scp, else ranged reads.
- Remote video plays through a local HTTP range server backed by the fetch layer (ffmpeg seeks become prioritised reads); thumbnails come from the host's ffmpeg when it has one, else a ranged read decoded locally.

### Changed

- The image viewer's fit (default and alt+0) fits the WIDTH; tall images scroll/pan.

### Added (t181u/t182u)

- Commander image view zoom: alt+= / alt+- (same keys as player volume; each view takes its own), alt+0 fit, alt+1 100%; ctrl+wheel zooms about the pointer; pan by drag, alt+arrows or the wheel; zoom % in the header; full-resolution decode past the 2048 px preview; clipped to the view.
- Interactive player: clickable « 10s / play-pause / 10s » buttons, a seek bar (click seeks, drag scrubs, hover shows the time), a volume bar (click, drag or wheel). Video and audio.
- Every box (panels, popups, options/key/mode lists, search field, buttons, tiles, sidebar rows, tabs) has 8 px rounded corners and a soft, theme-aware drop shadow (unified with the old popup shadow). Drawn from plain rects, so all renderers (wgpu, skia, CPU, engine plugins) get it.
- Narrow commander header: buttons that do not fit (after the path shortens) collapse progressively into a ••• menu at the upper right (click or alt+i); rounded, shadowed dropdown; Esc closes it.

Fixed:
- Player sync: smooth audio clock between callbacks and a half-redraw lead; measured mean -0.2 ms, p95 9.5 ms, 0 drops (was +20 ms, 6 % dropped)
- The first key after closing a viewer with a double Esc was eaten (Esc glued to it as meta-key)
- Options window: previews no longer show through it; popups get a drop shadow
- Viewer images and player frames are clipped to the view (never over the prompt)
- Player: alt+p (back) closes the player instead of changing folder under it

Added:
- Mode plugins and the commander mode (MODES.md Phase 1, docs/modes.md): one mode at a time, commander on at start; `esc :c` (Enter) switches modes / turns the current one off (a lone Esc reaches the shell after 500 ms; nothing is taken in full-screen apps); `ctrl+shift+k` key mappings, `ctrl+shift+m` modes; keys remappable in `~/.mouser/modes.toml`. Commander docks a listing of the shell's directory above the prompt as a list (`ls -lah` columns), grid (previews by type, tile sizes) or carousel (focused item plus two partial neighbours), with search, an options window (view, `-a`, `-h`, sort, reverse), a manual, and open (cd into folders, desktop opener for video/audio, full-width image/text view); previews load lazily for what is on screen only. Navigation in every view: alt+arrows, PgUp/PgDn, Home/End, alt+p parent folder, alt+q / Esc closes the file view, click selects, double-click opens, wheel moves; multi-select (alt+space mark, alt+* all) and alt+| pipe (types `printf '%s\n' <quoted paths> | <cmd>` into the shell). Modes implement the `mouser_render::modes::Mode` trait; loading third-party modes is future work
- Commander's media player: video (ffmpeg frames through the image path, fitted to the width) and audio (played in-process through cpal; the audio device's sample clock drives the video frames by PTS, late frames dropped; pause, volume and seek without restarts except a seek re-starting both ffmpeg readers at the target) with play/pause, seek, volume and close keys (alt+k, alt+j/l, alt+-/=, alt+q); stops when the viewer closes or the selection changes; the desktop opener is only the fallback without ffmpeg
- Commander follows SSH sessions: remote folder from OSC 7, listed over a multiplexed BatchMode ssh side channel, `user@host:/path` header, remote badges, clear "no directory info" state
- Commander carousel slides between items (180 ms ease-in-out, retargets mid-slide, redraws only while moving)
- Commander: back control in the header (`‹ back`, alt+p, alt+Backspace; clickable path segments; disabled at /); `.` and `..` are no longer listed; going up selects the folder you came from
- Commander: video thumbnails in grid and carousel (ffmpeg frame at 10 %, cached in `$MOUSER_HOME/cache/thumbs`), quieter placeholders, focused carousel item loads first
- Commander: search shows the query, the match count or "no matches", and its focus
- `[mouser]` command line: a line that is not a Mouser command runs in the tab's shell (`!` forces the shell)
- Commander's cd and pipe keep what you have typed (cut to the kill ring and yanked back when the prompt returns; bash, zsh, fish in emacs mode)
- Image previews read JPEG, GIF, WebP and BMP as well as PNG
- Session server / UI split (Linux, macOS): the window and its engine run in a child `mouser __ui` process; sessions, PTYs, scrollback, tabs and images stay in the process you started. A native crash in any engine (built-in too) restarts the window on the next engine in the priority order with everything kept; the crashed engine is marked skipped with the signal; a crash in `mouser` restarts on the CPU renderer; a UI killed from outside restarts on the same engine. `MOUSER_PROC_FORK=0` keeps one process; Windows is unchanged (docs/engines.md §8)
- Sidebar Engines section: available engines in priority order with active/ok/skipped/failed and the reason; a click switches the window live (same as `mouser engine use`); drag rows to reorder (insertion line and ghost; Escape or a drop outside cancels; saved like `mouser engine priority` and applied live)
- Test hooks `MOUSER_TEST_CRASH_ENGINE=<engine>:<n>` and the headless test UI (`MOUSER_UI=headless`); `key-to-frame` timing in `RUST_LOG=mouser_frame=debug`
- Live engine switching: running windows swap engine in place when the order or a skip mark changes (sessions, PTYs, scrollback, selection and images untouched)
- Crash isolation: custom engines run in an engine host process (`mouser __engine-host`); a crash or hang there marks the engine skipped with the reason and the window continues on the next engine; panics in built-in engines are caught the same way; ABI adds `MOUSER_SURFACE_PIXELS`/`MouserPixels` (additive, still ABI 1); example engine option `MOUSER_FLAT_CRASH_AT`
- Engine priority order: `engines = [...]` in preferences.toml (old `engine` key still read), `mouser engine priority a b ...`, `mouser engine move <name> first|last|up|down|<n>`, `use` moves an engine first; startup and runtime failures walk the order to the next engine; `MOUSER_ENGINE=a,b` overrides it for one run; `mouser` is always in the order, cannot be removed, uninstalled or skipped, and stays the final GPU fallback before the CPU renderer
- `mouser engine list` shows each engine's state (ok, skipped after a crash with when, failed with when and why); `mouser engine unskip <name>` / `--all` (alias `restore`) clears it without selecting; fallbacks print, log and show in the status bar which engine was skipped and the command to restore it; `skia` is crash-guarded like custom engines
- Swappable GPU rendering engines: `mouser` (the wgpu renderer, default), `skia` (Skia over OpenGL; in default builds, `--no-default-features` drops it) and custom engines, shared libraries with a versioned C ABI (`mouser-engine-abi`, `mouser_engine.h`); `mouser engine list | install <dir> | uninstall <name> | use <name>`, `MOUSER_ENGINE` overrides; engines live in `~/.mouser/engines/`; an engine that fails to load, reports another ABI, errors while drawing or crashed the last start falls back to `mouser`; example engine `crates/mouser-engine-example` (`flat`); the CPU fallback is unchanged (docs/engines.md)
- Re-wrap on resize: soft-wrapped lines on the main screen and in the scrollback are joined and split at the new width (hard breaks kept, wide characters never split, attributes, links and combining marks kept); the cursor stays on its character; images stay anchored to their line, and fitted images take or give back the empty rows below them so text after them moves instead of being covered; the alternate screen is cut, not re-wrapped, and the main screen behind it is now kept (and re-wrapped) instead of blanked; a resize clears the selection
- Smooth drag-resize with any scrollback: each resize re-wraps the screen and the newest 2000 scrollback rows (about 1 ms); older lines follow on a worker in small steps; idle scrollback compression does one chunk at a time instead of stalling a frame; frame and resize timings with `RUST_LOG=mouser_frame=debug`
- Tests for several images at once (fitted, fixed, `--original`, Kitty `c`/`r`, in the scrollback): no placement overlaps another or covers text after any resize; shrinking an image never removes a line another image or the cursor is on
- Fuzzball capability reply advertises `g_fit=w` and `g_reflow=1`; `mouser show` sends `fit=w` only when `g_fit` lists `w`
- Mouse selection: drag selects characters, double-click a word (`[selection] word_chars` in config.toml), triple-click a line, Shift+click extends, Alt+drag selects a block; works over the scrollback, auto-scrolls past the window edges, handles wide and combining characters, joins soft-wrapped lines and trims trailing blanks; Shift bypasses an app's mouse reporting; cleared when its text changes or a typed key is sent
- Copy with Ctrl+Shift+C (Cmd+C on macOS); selecting sets PRIMARY on Linux (through the Wayland connection, so it works on Hyprland)
- Paste: Ctrl+Shift+V (Cmd+V on macOS) pastes the clipboard, middle-click and Shift+Insert paste PRIMARY on Linux; pasted control characters (except tab, newline, CR) are dropped and a pasted `ESC[201~` cannot end a bracketed paste early
- Themes can set `selection_foreground` / `selection_background` (inverse colours otherwise)
- Arch packaging: AUR-ready `mouser-git` (source) and `mouser-bin` (release binary) PKGBUILDs with `.SRCINFO` in `packaging/arch/`; both install the desktop entry, hicolor icons, metainfo and the compiled terminfo, and replace each other and older `mouser-bin` installs; the release workflow updates the `mouser-bin` checksums after publishing
- Run a command: `mouser -e CMD ARGS...` and `mouser -- CMD ARGS...`, plus `--working-directory`/`--dir`, `--title`/`-T`, `--class`/`--app-id`, `--hold` and `--version`; the desktop entry declares them for xdg-terminal-exec (`X-TerminalArg*`), so Mouser works as the default terminal on Omarchy
- Fuzzball protocol v0.1 (`docs/protocol/`): image upload/place/delete/query, keyboard mode with structured key events, capability query (`DCS = F z c q`, XTGETTCAP `Fz`)
- Golden-pixel tests for Fuzzball images (CPU and the GPU display list)
- `tools/fuzzball-demo.py` demo client
- `mouser show <file|->...`: show PNG, JPEG, GIF (first frame), WebP and BMP images in a Fuzzball terminal, fitted to the width (`--width`, `--height`, `--original`, `--id`); no video or audio
- Fuzzball placements follow their text through scroll regions, SU/SD, IL/DL, the scrollback and resize; they are removed when their anchor row leaves the scrollback, scrolls off the alternate screen
- Fuzzball images scale to fit the screen width on resize (and when placed) and grow back on a wider screen; downscaled images are area-averaged and cached for both renderers; cell-size changes rescale their pixels
- Abandoned Fuzzball uploads are discarded by a timer after 10 s
- ED 3 (`CSI 3 J`) clears the scrollback (and the placements in it) without touching the screen
- Kitty graphics protocol as a fallback behind Fuzzball (graphics.md §7): direct/chunked, file, temp-file and shared-memory transmission, `f=8/16/24/32/100`, zlib, every action and delete specifier, replies with `q`, feature queries, image numbers, relative placements, Unicode placeholders, animation and frame composition, z layers under/over cell backgrounds; shares the placement lifecycle, renderer, scale cache and memory quota with Fuzzball
- `mouser show` falls back to Kitty graphics when Fuzzball does not answer, and works inside tmux (`allow-passthrough on`) through Kitty Unicode placeholders

Changed:
- A start marker left by another window that is still starting is no longer taken as a crash (two windows starting at once could skip each other's engine)
- Images always fit the terminal width: `mouser show` scales each image up or down to the full width (starting on a new line), and Fuzzball placements made with the new `fit=w` key follow the width on every resize; `--width`, `--height` and `--original` still give fixed sizes
- A placement that would overhang the right edge moves to column 0 at its requested size (scaled down only when wider than the screen) instead of shrinking in place and pinning to the last column at one column wide; Kitty placements with explicit `c`/`r` keep their size whenever it fits
- Kitty keyboard protocol kept as a fallback behind Fuzzball keyboard mode
- Kitty keyboard: Ctrl/Alt/Super keys carry no associated text (flag 16)
- Kitty keyboard compliance fixes: F13+/media/keypad keys, lock bits on text keys, Enter/Tab/Backspace releases, Ctrl+Backspace, Ctrl+Shift/Super text keys in legacy mode; tests per spec section

Removed:
- `OSC 1337` image claims from the docs (never implemented)
- `xterm-kitty` TERM fallback; TERM is `mouser` or `xterm-256color`

## [0.7.2] - 2026-10-30

Fixes:
- Box-drawing lines align identically on the CPU and GPU renderers
- Double-line box characters drawn procedurally to match single lines
- Windows CI: glyph snapshot tests ignore CRLF line endings

Docs:
- Handover notes updated

## [0.7.1] - 2026-10-30

Added:
- Tabs; clicking a connection opens it in a new tab
- GPU renderer (wgpu) with CPU fallback (`MOUSER_RENDERER=cpu`)
- Fuzzy font picker listing installed fonts
- Sidebar remembers its open/closed state; starts collapsed by default
- Kitty keyboard and graphics protocols
- Fuzzball protocol draft spec (`docs/protocol/`)
- Broader xterm/ECMA-48 support: SGR styles, wide chars, mouse, focus, bracketed paste, OSC titles, links and clipboard, window reports
- Public handover notes (`docs/HANDOVER.md`)

Changed:
- Event-driven redraws and glyph cache: near-zero idle CPU

Fixes:
- Window closes when the shell exits (Ctrl+D)
- Font editor no longer freezes while typing
- vttest fixes: DEC line drawing, SI/SO, DECSCNM, LNM, DECREQTPARM, DA1
- Box-drawing lines are continuous (no dashed edges)

## [0.7.0] - 2026-10-29

Feature: UI scaffolding for connections
- Real sidebar/modal layout engine with hit-testing
- Render tests for sidebar, modal, and accordion toggle
- Basic wiring in app to show sidebar and open modals

## [0.6.8] - 2026-10-29

Fixes:
- Release (Linux): move Arch .pkg.tar.zst out of container-mounted work dir using sudo to avoid permission errors.

## [0.6.7] - 2026-10-29

Fixes:
- Arch packaging in Release workflow: build without sudo by preinstalling deps and using `makepkg --nodeps`.
- Icon: small-size SVG uses exact Red Star background gradient; regenerated PNG/ICO/ICNS/hicolor assets.

## [0.4.2] - 2026-10-29

## [0.6.6] - 2026-10-29

- Packaging: Arch build installs runtime deps via makepkg -s.


## [0.6.5] - 2026-10-29

- Packaging: fix hicolor tree path and Windows installer source path.


## [0.6.4] - 2026-10-29

- Release: fix sed token replacement for nfpm under strict shell.


## [0.6.3] - 2026-10-29

- Release: fix nfpm env expansion; correct Windows Inno Setup paths.


## [0.6.2] - 2026-10-29

- Icon: further refine mouse silhouette for clarity at 32–64px.
- CI: remove runtime icon generation; use committed assets.


## [0.6.1] - 2026-10-29

- Release: fix Windows installer icon/output paths; use committed hicolor icons for Arch.


## [0.6.0] - 2026-10-29

- Icon: redesigned Red Star mouse silhouette; integrated >_ prompt; full palette.
- Theme: add Red Star (dark/light) as default; docs updated.
- Packaging: deb/rpm via nfpm, Arch mouser-bin, macOS DMG + Homebrew formula & cask, Windows Inno Setup installer; desktop launcher & icons.


Fix: cross-platform config/home resolution and Windows build
- Remove `xdg` usage in app config loader; use core `resolve_home()` (respects `MOUSER_HOME`)
- Unblock Windows build/clippy by avoiding Linux-only XDG calls
- Quality bar: fmt/clippy/tests green locally; CI expected to pass

## [0.4.3] - 2026-10-29

Fix: Release workflow stability across platforms
- Install Rust targets per matrix to fix macOS cross build (add `targets` to toolchain)
- Windows checksums via PowerShell `Get-FileHash` (no `shasum` on Windows)
- Drop Linux aarch64 artifact for now (needs cross-linker); keep x86_64

## [0.4.4] - 2026-10-29

Fix: Stabilize macOS CI tests
- Serialize theme tests to avoid `MOUSER_HOME` environment races on macOS runners

## [0.4.5] - 2026-10-29

Fix: Windows release checksum
- Correct PowerShell quoting for writing `*.sha256` (previously invalid path)

## [0.4.6] - 2026-10-29

Fix: Windows release checksum path
- Use absolute path (`Join-Path $PWD $Env:ARTIFACT`) for `Get-FileHash` and output

## [0.4.7] - 2026-10-29

Fix: Windows release checksum path (final)
- Use `$Env:GITHUB_WORKSPACE` with `Join-Path` for stable absolute path

## [0.4.8] - 2026-10-29

Fix: Windows checksum command
- Replace Set-Content with `Out-File -NoNewline -Encoding ASCII` to avoid parameter issues

## [0.4.9] - 2026-10-29

Fix: Windows path quoting
- Remove unnecessary escaping so PowerShell sees a valid `D:\...` path

## [0.4.10] - 2026-10-29

Fix: GitHub Release permissions
- Add `permissions: contents: write` so Actions can publish Releases

## [0.5.0] - 2026-10-29

Feature: First-class packaging
- Linux: `.deb` and `.rpm` via nfpm; Arch `mouser-bin` `.pkg.tar.zst` via archlinux container
- Desktop integration: `.desktop`, hicolor icons (PNG + SVG), AppStream metainfo; validated in CI
- macOS: `Mouser.app` bundle and `.dmg`; Homebrew formula (CLI) and Cask (App) generated and pushed
- Windows: Installer `.exe` via Inno Setup; embedded icon/version in exe

### Fixes post-tag
- CI: add `winres` build-dep, remove `docker.io` conflict on Ubuntu runner

## [0.4.1] - 2026-10-29

CI and release automation:
- Add GitHub Actions CI (fmt, clippy, build, tests) on pushes to main across Ubuntu, macOS, Windows
- Add release workflow on `v*` tags: build artifacts for Linux/macOS/Windows and publish GitHub Release with checksums
- Docs: `docs/release.md` and index in `docs/README.md`

## [0.4.0] - 2026-10-29

Feature: Theming
- Add built-in themes (10 popular sets) with dark/light variants
- System/light/dark mode selection; renderer uses theme colors
- Themes written to `~/.mouser/themes` on first run; docs added
- Quality bar clean

## [0.3.0] - 2026-10-29

Feature: Vim-style modes and basic command line
- Add Shell/Command/Config modes (default Shell); command line prompt and actions; Esc returns to Shell
- Toggleable right sidebar via commands; groundwork for menus/keybindings to dispatch actions
- Quality bar clean; docs page added for modes

# Changelog

All notable changes to this project will be documented in this file.

## [0.2.0] - 2026-10-29

Feature: Connection manager foundations and storage path
- Add core storage resolver with `MOUSER_HOME` override and default `~/.mouser`
- Add connection manager data model (SSH, Waypipe, Tunnels, X11) with advanced pass-through options
- Right sidebar listing saved connections (toggle)
- Docs page for connection manager and storage; tests for serialization and arg building

Quality:
- fmt/clippy/test clean; README and AGENTS updated per storage rules

## [0.1.0] - 2026-10-29

Initial release:

- PTY-backed terminal with `vte` parsing
- GPU renderer via `winit` + `wgpu` + `glyphon`
- Chunked, compressed scrollback with Zstd/LZ4 and on-demand decompression
- Scrollback search across compressed history
- Config in TOML for font/colors/scrollback/compression
- Kitty protocol groundwork: keyboard negotiation and basic graphics parsing + placement
- Compliance checklist added under `docs/kitty-compliance.md`

