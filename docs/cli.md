# Command line

`mouser` with no command opens a window running your login shell. Every flag,
command and environment variable is also in the man pages (`man mouser`,
`man mouser-model-add`, …), generated from the same definitions.

## Window flags

| Flag | What it does |
| --- | --- |
| `-e, --command CMD…` | Run a command instead of the login shell; everything after it is its arguments. |
| `-- CMD…` | Same as `-e`. |
| `--working-directory DIR` (`--dir`) | Start in this directory instead of `$HOME`. |
| `-T, --title TITLE` | Window title until the program sets its own. |
| `--class CLASS` (`--app-id`) | Wayland app_id / X11 WM_CLASS (default `Mouser`). |
| `--hold` | Keep the window open after the command exits. |
| `--config PATH` | TOML config file to use. |
| `--config-dir DIR` | Config directory to use (tests, manual runs). |

## Commands

| Command | Guide |
| --- | --- |
| `mouser md [FILE…]` | Markdown formatted into the terminal, stdin when no file ([Markdown rendering](./markdown.md)) |
| `mouser show FILE…` | Images in any Fuzzball or Kitty-graphics terminal ([Terminal](./terminal.md)) |
| `mouser ssh [SSH_ARGS…]` | ssh with `TERM=mouser` where the host knows it, else `xterm-256color` ([SSH and TERM](./ssh.md)) |
| `mouser shell-integration zsh\|bash\|fish\|pwsh [--install \| --uninstall]` | Directory reports and commander's silent `cd` ([Modes](./modes.md)); [installing it](#shell-integration) |
| `mouser engine …` | Rendering engines ([Engines](./engines.md)) |
| `mouser plugin …` | Plugins ([Plugins](./plugins.md)) |
| `mouser model …` | Model providers, including grok ([Models](./models.md)) |
| `mouser mcp …` | MCP server and clients, Let's Encrypt ([MCP](./mcp.md)) |
| `mouser default-terminal …` | Windows default terminal ([Windows](./windows.md)) |
| `mouser help [TOPIC \| search WORDS]` | These guides, built in ([Help](./help.md)) |
| `mouser man [--out DIR]` | Write the man pages (default `man/`) |

`mouser help COMMAND` (for example `mouser help model add`) prints that command's
usage when no guide has that name; `mouser COMMAND --help` always does.

## Shell integration

`mouser shell-integration SHELL` prints the integration script (zsh, bash, fish or
pwsh) for your startup file to load. To have Mouser add the load line for you:

```sh
mouser shell-integration --install            # the shell in $SHELL (PowerShell on Windows)
mouser shell-integration fish --install       # name the shell
mouser shell-integration --install --dry-run  # show the change, write nothing
mouser shell-integration --install --rc FILE  # another startup file
mouser shell-integration --uninstall          # take it out again
```

| Shell | File edited |
| --- | --- |
| zsh | `$ZDOTDIR/.zshrc` (else `~/.zshrc`) |
| bash | `~/.bashrc` (on macOS, make sure `~/.bash_profile` sources it) |
| fish | `$XDG_CONFIG_HOME/fish/conf.d/mouser.fish` (else `~/.config/…`), a file of its own |
| pwsh | PowerShell's `$PROFILE` (asked from `pwsh`; else the usual profile path) |

`--install` adds a block between `# >>> mouser shell integration >>>` and
`# <<< mouser shell integration <<<` that loads the integration only when `mouser`
is on `PATH`. It prints the block, the file and the backup it made first
(`FILE.mouser-bak`, then `.mouser-bak.2`, … so an older backup is kept). Running it
again changes nothing; a block from an older Mouser is updated in place. A symlinked
startup file is edited through the link, and its permissions and line endings are
kept. When the file already loads `mouser shell-integration` on a line you wrote,
it is left alone and the line is named. A broken block (one marker without the
other) is refused with nothing written. `--uninstall` removes only the block (with
the same backup), never your own lines.

## Questions

A yes/no question on the terminal ends in `[Y/n]`: Enter, `y` or `yes` is yes; `n`,
`no` or anything else is no. No answer is never yes: end of input, a read error or
no terminal at all cancels (pass `--yes` to say yes from a script). Approvals that
offer more than yes and no (allow once, always, deny) keep their own choices.

## Environment

| Variable | What it does |
| --- | --- |
| `MOUSER_HOME` | Root directory instead of `~/.mouser` (config, themes, plugins, models, logs). |
| `MOUSER_ENGINE` | Rendering engine for this run (`mouser`, `skia` or a custom engine's name), ahead of the saved order. |
| `MOUSER_RENDERER` | `cpu`: the CPU renderer instead of a GPU engine. |
| `MOUSER_PROC_FORK` | `0`: sessions and window in one process (no session server / UI split; Unix). |
| `MOUSER_UI` | `headless`: the session server without a window (tests). |
| `MOUSER_SYSTEM_DARK` | `1` or `0`: whether the desktop prefers dark, overriding detection. |
| `MOUSER_OPEN_SIDEBAR` | Open the sidebar for this run. |
| `MOUSER_OMARCHY` | `1` or `0`: treat this host as Omarchy (default Omarchy theme) or not, overriding `/etc/os-release` detection. |
| `MOUSER_DROPPER_FAKE` | Theme Dropper returns this `#RRGGBB` after a short pause (recordings). |
| `MOUSER_WINDOW_SIZE` | Initial window size `WxH` (e.g. `1268x1056`) when the compositor allows. |
| `MOUSER_MODELS_DIR` | Where local model weights live (before `[models.local] dir` and `<home>/models`). |
| `MOUSER_MODEL_URL_OVERRIDE` | Download URL for `mouser model download` instead of the pinned one. |
| `MOUSER_GROKBOT_HOME` | Grok Bot desktop home for `mouser model add grok --desktop`. |
| `MOUSER_GROK_POLL_MS` | Poll interval (ms) of the grok Cursor-agents and desktop providers. |
| `MOUSER_SECRET_STORE` | `file`: keys and tokens in a 0600 file under the home instead of the OS keychain. |
| `MOUSER_SSH_BIN` | ssh binary for remote commander and the connection manager. |
| `MOUSER_SSH_CONFIG` | SSH config Mouser reads and edits (default `~/.ssh/config`). |
| `MOUSER_SSH_TERM` | `TERM` for ssh from a Mouser tab: `auto` (default: `mouser` where the host has its terminfo, else `xterm-256color`), `mouser`, or `xterm-256color`; per host with `SetEnv MOUSER_SSH_TERM=…` in ssh_config ([SSH and TERM](./ssh.md)). |
| `MOUSER_SSH_SHARE` | `no`: don't make ssh sessions from a Mouser tab a ControlMaster that commander shares (default yes); per host with `SetEnv MOUSER_SSH_SHARE=…` ([SSH and TERM](./ssh.md#commander-on-the-remote-host)). |
| `MOUSER_SSH_WRAPPER` | Set in Mouser's tabs: the `ssh` shim the shell integration's `ssh` function runs ([SSH and TERM](./ssh.md)). |
| `MOUSER_SSH_WRAPPED` | Set on the real ssh the shim runs, so an ssh wrapper of your own that calls `ssh` again is passed through unchanged. |
| `MOUSER_REMOTE_BACKEND` | `rsync`, `scp` or `cat`: force how remote files are fetched. |
| `MOUSER_PLAYER_LOG` | Write the media player's log to this file. |
| `MOUSER_HF_URL` | Hugging Face Hub base URL for `mouser model hf` and the chat's model browser (default https://huggingface.co). |
| `MOUSER_LLM_TIMINGS` | Print llama.cpp timings for the built-in local model. |
| `MOUSER_PLUGIN_DIRS` | Plugin folders (a path list) replacing the offline bundle and system folders; empty: only `$MOUSER_HOME/plugins` ([Plugins](./plugins.md)). |
| `MOUSER_ENGINE_DIRS` | Folders of engines shipped with Mouser (a path list) replacing `engines/` beside the executable; empty: none ([Engines](./engines.md)). |
| `MOUSER_OFFLINE_BUNDLE` | Folder with the offline bundle's `plugins.lock` archives, instead of `plugins/` beside the executable ([Storage](./storage.md)). |
| `MOUSER_CARGO` | cargo used for `install crate:NAME` (default `cargo`; needs cargo-binstall). |
| `MOUSER_MODE_WIRE` | `json`: mode plugins speak JSON instead of MessagePack on stdio, for debugging ([Mode plugin protocol](./mode-protocol.md)). |
| `RUST_LOG` | Log filter, e.g. `debug`. |

## Exit status

0 on success, 1 on an error (message on stderr), 2 for a usage error.

## Man pages

The pages in `man/` are generated: after changing a flag, command or variable run
`cargo run -p mouser-app -- man --out man` and commit the result. A test fails
when they are stale or when the code reads a `MOUSER_*` variable that is not listed.
