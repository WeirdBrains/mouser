# Storage

Mouser stores preferences, themes and connections under `~/.mouser` (override with `MOUSER_HOME`):

- `~/.mouser/preferences.toml`
- `~/.mouser/themes/<ThemeName>/{dark.toml,light.toml}`
- `~/.mouser/connections/*.toml`
- `~/.mouser/plugins/<name>/<version>/` and `~/.mouser/engines/<name>/<version>/`: installed plugins and engines, one folder per version (the newest two are kept); `<name>/current` names the active version and `<name>/source.toml` where it came from. `plugins/keys.toml` holds the signing keys remembered per source.
- `~/.mouser/downloads/jobs.toml`: the download manager's jobs while Mouser runs.
- `~/.mouser/logs/plugins-audit.log`: every install, update and rollback.

Tests and manual runs must use a temporary `MOUSER_HOME` and never touch the real `~/.mouser`.

