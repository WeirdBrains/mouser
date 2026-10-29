# Storage

Mouser stores preferences, themes and connections under `~/.mouser` (override with `MOUSER_HOME`):

- `~/.mouser/preferences.toml`
- `~/.mouser/themes/<ThemeName>/{dark.toml,light.toml}`
- `~/.mouser/connections/*.toml`

Tests and manual runs must use a temporary `MOUSER_HOME` and never touch the real `~/.mouser`.

