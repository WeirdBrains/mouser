# Connection Manager

Mouser manages SSH sessions, Waypipe sessions, SSH tunnels (local/remote/dynamic), and X11-forwarded SSH connections.

UI
- A collapsible right sidebar lists categories: SSH, Waypipe, Tunnels, X11. Toggle it with Alt+Esc
  (plain Esc goes to the shell), or with `:open-sidebar` / `:close-sidebar` (F10 opens the
  command line). While open, the terminal grid shrinks so nothing is hidden.
- Resize: drag the sidebar's left edge (the pointer turns into a resize cursor). Width is clamped
  between ~22 cells (min 200 px) and 60% of the window, leaving the terminal at least 20 columns.
- Collapse: click `»` in the sidebar header to shrink it to a thin strip; click the strip (`«`) to
  expand it again.
- Width is saved in `preferences.toml`. At startup the sidebar is collapsed to the strip unless
  `MOUSER_OPEN_SIDEBAR=1` is set, which opens it expanded.
- Each category has an Add action (command-mode: `:add-ssh`, `:add-waypipe`, `:add-tunnel`, `:add-x11`).
- Saved connections can be launched (`:connect <name>`), edited (`:edit-conn <name>`) and deleted (`:del-conn <name>`).

SSH hosts come from `~/.ssh/config`
- The SSH category lists every concrete `Host` alias in `~/.ssh/config` and the files it `Include`s
  (globs and relative paths supported). Wildcard-only patterns such as `Host *` are skipped.
  The list reloads automatically within ~2 s when the config or an included file changes.
- Click a host's name to run `ssh <alias>` in the terminal, so ssh applies the config itself.
- Add, edit (HostName, User, Port, IdentityFile, ProxyJump; alias renames the `Host` line) and
  delete write to `~/.ssh/config`. A host defined in an included file is edited in that file.
  Only the affected lines change: comments, blank lines, ordering, indentation and other options are
  kept exactly. Unchanged saves do not write.
- Safe writes: temp file + atomic rename in the same directory, original permissions kept (new files
  0600), and the previous contents saved to `~/.ssh/config.mouser.bak` (0600) before every write.
  No other files in `~/.ssh` are touched.
- Read-only: a `Host` line with several patterns (e.g. `Host web1 web2`) shows **info** instead of
  edit/delete and explains why. `Match` blocks are not listed.
- Older SSH entries saved in `~/.mouser/connections` are offered for migration ("Move N saved to
  ~/.ssh/config…"); they move only after you confirm. Waypipe, tunnels and X11 stay in `~/.mouser`.
- `MOUSER_SSH_CONFIG=/path/to/config` points Mouser at another file (and connects with `ssh -F`);
  use it with a temp directory for testing.

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

