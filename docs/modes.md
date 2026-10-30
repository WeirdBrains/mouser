# Modes

Mouser supports vim-style modes:

- Shell (default): input goes to the PTY.
- Command: `:` command line for actions (e.g., `:open-sidebar`, `:connect <name>`, `:theme <name>`).
- Config: reserved for in-app editing of preferences/themes.

Mode switching
- Enter Command: F10 by default (configurable in future). Esc returns to Shell.
- Mode indicator: the command line is shown at the bottom when in Command mode.

Actions
- `:open-sidebar` / `:close-sidebar`
- `:list-conns`, `:connect <name>`
- `:theme` — open the theme switcher; `:theme <name>` — switch theme (saved)
- `:mode dark|light|system` — switch variant mode (saved)
- `:edit-theme` — open the theme & font editor for the active theme variant
- `:font-size <6-48>`, `:font <family>` (`:font` alone resets to the default stack)

Keybindings
- F10: command line. Alt+Esc: toggle the connections sidebar.
- Ctrl+= / Ctrl++ bigger font, Ctrl+- smaller, Ctrl+0 reset to `config.toml`'s size (saved to
  `preferences.toml`; the grid reflows and the shell is resized).
- Theme switcher: ↑/↓ theme (live preview), ←/→ or d/l/s mode, e editor, Enter apply, Esc cancel.
- Editor: Tab/Shift+Tab or ↑/↓ move between fields, type to edit (live preview when valid),
  Ctrl+S save, Esc cancel. Fields are also clickable.

Menus, keybindings and the command line dispatch the same named actions.

