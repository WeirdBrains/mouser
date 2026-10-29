# Theming

Themes control font size/family, colors, and a light/dark/system mode (default: system, follows OS).

Built-in themes (dark/light variants):
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

Accuracy
- Palettes are sourced from each theme’s canonical definitions (foreground/background primaries).

Storage
- Themes live under `~/.mouser/themes/<ThemeName>/{dark.toml,light.toml}` (override with `MOUSER_HOME`).
- Built-ins are written on first run; editing copies from built-ins.

Mode and resolution
- Preferences select `theme` and `mode` (`light`/`dark`/`system`). For tests, system mode is simulated via `MOUSER_SYSTEM_DARK=1`.

