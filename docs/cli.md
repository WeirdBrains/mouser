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
| `mouser shell-integration zsh\|bash\|fish` | Directory reports and commander's silent `cd` ([Modes](./modes.md)) |
| `mouser engine …` | Rendering engines ([Engines](./engines.md)) |
| `mouser plugin …` | Plugins ([Plugins](./plugins.md)) |
| `mouser model …` | Model providers, including grok ([Models](./models.md)) |
| `mouser mcp …` | MCP server and clients, Let's Encrypt ([MCP](./mcp.md)) |
| `mouser default-terminal …` | Windows default terminal ([Windows](./windows.md)) |
| `mouser help [TOPIC \| search WORDS]` | These guides, built in ([Help](./help.md)) |
| `mouser man [--out DIR]` | Write the man pages (default `man/`) |

`mouser help COMMAND` (for example `mouser help model add`) prints that command's
usage when no guide has that name; `mouser COMMAND --help` always does.

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
| `MOUSER_MODELS_DIR` | Where local model weights live (before `[models.local] dir` and `<home>/models`). |
| `MOUSER_MODEL_URL_OVERRIDE` | Download URL for `mouser model download` instead of the pinned one. |
| `MOUSER_GROKBOT_HOME` | Grok Bot desktop home for `mouser model add grok --desktop`. |
| `MOUSER_GROK_POLL_MS` | Poll interval (ms) of the grok Cursor-agents and desktop providers. |
| `MOUSER_SECRET_STORE` | `file`: keys and tokens in a 0600 file under the home instead of the OS keychain. |
| `MOUSER_SSH_BIN` | ssh binary for remote commander and the connection manager. |
| `MOUSER_SSH_CONFIG` | SSH config Mouser reads and edits (default `~/.ssh/config`). |
| `MOUSER_REMOTE_BACKEND` | `rsync`, `scp` or `cat`: force how remote files are fetched. |
| `MOUSER_PLAYER_LOG` | Write the media player's log to this file. |
| `MOUSER_LLM_TIMINGS` | Print llama.cpp timings for the built-in local model. |
| `RUST_LOG` | Log filter, e.g. `debug`. |

## Exit status

0 on success, 1 on an error (message on stderr), 2 for a usage error.

## Man pages

The pages in `man/` are generated: after changing a flag, command or variable run
`cargo run -p mouser-app -- man --out man` and commit the result. A test fails
when they are stale or when the code reads a `MOUSER_*` variable that is not listed.
