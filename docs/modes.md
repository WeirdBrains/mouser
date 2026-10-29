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
- `:theme <name>` (planned)

Menus, keybindings and the command line dispatch the same named actions.

