# Connection Manager

Mouser manages SSH sessions, Waypipe sessions, SSH tunnels (local/remote/dynamic), and X11-forwarded SSH connections.

UI
- A collapsible right sidebar lists categories: SSH, Waypipe, Tunnels, X11. Toggle it with Alt+Esc
  (plain Esc goes to the shell). While open, the terminal grid shrinks so nothing is hidden.
- Each category has an Add action (command-mode: `:add-ssh`, `:add-waypipe`, `:add-tunnel`, `:add-x11`).
- Saved connections can be launched (`:connect <name>`), edited (`:edit-conn <name>`) and deleted (`:del-conn <name>`).

Security
- No plaintext secrets. Prefer ssh-agent and key paths. If a secret is ever needed, use the system keyring.

Advanced options
- SSH: common fields are first-class; all other options are passed via `advanced_options` map as `-o key=value`.
- Waypipe: all flags supported via `args` (free-form tokens) plus selected common fields.
- Tunnels: local (`-L`), remote (`-R`), dynamic (`-D`) forwarding supported; multiple forwards per connection are allowed.

Storage
- Lives under `~/.mouser` (override with `MOUSER_HOME`).
- Connections serialize to TOML files under `~/.mouser/connections/`.

Commands
- `:open-sidebar` — toggle the right sidebar
- `:add-ssh name key=val ... adv.key=val` — create/update SSH entry
- `:connect name` — launch connection
- `:list-conns` — list all connections in the log

