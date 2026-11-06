# Theming

Themes control font size/family, colors, and a light/dark/system mode (default: system, follows OS).
In system mode Mouser reads the desktop colour-scheme preference (GNOME/freedesktop
`color-scheme`, macOS `AppleInterfaceStyle`); if the desktop does not explicitly prefer
light, the dark variant is used. `MOUSER_SYSTEM_DARK=1|0` overrides detection.

Built-in themes (dark/light variants):
- Red Star (default off Omarchy)
- Omarchy (Trainyard palette; default when `/etc/os-release` has `ID=omarchy`, or `MOUSER_OMARCHY=1`)
  - Dark: foreground `#E6FBFA`, background `#04192A`
  - Light: foreground `#0B2A3F`, background `#F2FAFB`
  - Unedited theme files from older releases are upgraded on start; edited files are kept.
  - Palette source: `~/Pictures/Wallpapers/redstar-title-starlight/redstar-wallpaper-title-1080x2400-starlight.png`
  - Extracted accents and highlights:
    - Background gradient: top `#04192A` → bottom `#020F1B`
    - Primary accent: `#2FD8E5`
    - Secondary accents: `#2698A7`, `#44B9C1`, `#49B9C8`
    - Highlights: `#94F3F2`, `#E6FBFA`
- Tokyo Night
- Catppuccin
- Gruvbox
- Dracula
- Nord
- One Dark / One Light
- Solarized
- Rose Pine
- Kanagawa
- GitHub

Switching
- Sidebar → **Appearance**, or `:theme`, opens the switcher: built-in themes first, then user themes
  found in `~/.mouser/themes/`, plus dark/light/system mode. Moving the selection previews live;
  Apply saves `theme`/`mode` to `preferences.toml`. `:theme <name>` and `:mode <m>` do the same directly.
- Mode buttons size to their labels (Unicode width). In a narrow window they shrink padding, then
  abbreviate to `d` / `l` / `sys` — they never chop mid-word (`dar`, `ligh`).
- **Match Omarchy** (on the switcher) copies the current Omarchy desktop theme's `colors.toml` into
  Mouser's built-in Omarchy theme and selects it. On Omarchy hosts with no `theme` preference yet,
  Mouser defaults to Omarchy automatically (`MOUSER_OMARCHY=1|0` overrides detection).
- **New** (`n`) / **Duplicate** (`u`) create a user theme under `~/.mouser/themes/` (Duplicate copies both
  variants of the selection). Blank cursor, accent and palette fields on the source are filled so Edit colours never opens empty.
  **Export** writes `dark.toml`/`light.toml` to
  `~/Downloads/mouser-theme-<name>/`. Import with `:import-theme /path/to/dir`; export also via
  `:export-theme [name]`.

Editing
- **Edit colours & font** in the switcher (or `:edit-theme`) edits the active variant: foreground,
  background, cursor, accent and the 16-colour palette as hex (`#rrggbb` or `#rgb`) with swatches,
  plus font family and size. Valid edits preview live (a side Preview pane shows sample text in the pending colours); invalid fields are outlined in red.
- **Dropper** (in the editor, or **F2** when a colour field is focused) samples a screen colour into the focused colour field. On Hyprland /
  Omarchy it uses `hyprpicker`; otherwise `grim`+`slurp`, then ImageMagick `import` on X11.
- **Auto** (button or `a` on the switcher) builds/selects a theme from the desktop when possible (currently
  Omarchy `colors.toml`, same as Match Omarchy; more desktops later).
- Tab completes the focused field (colour values from the theme, font family names,
  font sizes, or the new/duplicate theme name). ↑/↓ move between fields. The
  helper row under Save/Cancel/Dropper lists the keys (`Tab complete`, `F2 samples
  a screen colour`); F2 is the key for Dropper so the wording is not repeated.
- A validation message such as `Font size must be 6-48` has its own row above the
  buttons and never covers them.
- Save writes `~/.mouser/themes/<Theme>/<variant>.toml` and the font to `preferences.toml`.
- Theme files may contain `cursor`, `accent` and `palette = [16 colours]`; all are optional.

Accuracy
- Palettes are sourced from each theme’s canonical definitions (foreground/background primaries).

Storage
- Themes live under `~/.mouser/themes/<ThemeName>/{dark.toml,light.toml}` (override with `MOUSER_HOME`).
- Built-ins are written on first run; editing copies from built-ins.

Mode and resolution
- Preferences select `theme` and `mode` (`light`/`dark`/`system`). For tests, system mode is simulated via `MOUSER_SYSTEM_DARK=1`.


### Font picker

Clicking (or tabbing to) the editor's **Font family** field — or running `:font`
with no argument — opens a list of the font families installed on the system,
in name order. Typing fuzzy-filters it (case-insensitive subsequence; matched
letters are highlighted) while the list stays alphabetical. ↑/↓ or the mouse
wheel move, Enter or a click picks (the font then loads in the background), and
Esc or a click elsewhere closes the list. Families are discovered once, on a
background thread, at startup; typing only filters. Each row is drawn in
that family so you can preview the look before picking. Long names are padded
and end in an ellipsis so they never run into the right edge.
