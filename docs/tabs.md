# Tabs

The `tabs` decoration puts a tab bar in the window chrome and gives each tab
its own PTY. It is built in and off until you enable it, so a window with the
plugin disabled keeps today's behaviour: one shell, and the built-in tab bar
only after a second tab (`Ctrl+Shift+T`).

```sh
mouser plugin enable tabs
```

`mouser plugin disable tabs` turns it off again. The next window picks it up.

## The bar

The strip sits at the top, including when there is one tab. Each tab shows its
number and the shell or command it is running. `+` opens a tab. The `x` on a
tab closes it. A click focuses; a middle click closes.

| Key | Action |
| --- | --- |
| Ctrl+Shift+T | new tab |
| Ctrl+Shift+W | close the tab in front |
| Ctrl+Tab | next tab |
| Ctrl+Shift+Tab | previous tab |
| Alt+1 … Alt+9 | jump to that tab |

Closing the last tab closes the window. Only the tab in front is resized when
the window changes size; a tab you switch to is resized then.

## When a program exits

A shell that exits closes its tab and focuses a neighbour. A command that
exits with a failure stays, with `[exited N]` on the tab, until you press a
key or close it. A command that exits 0 closes like a shell.

## Which program a tab runs

`tabs.shell` in `preferences.toml` is the default for a new tab (and for the
first tab, when you did not pass `-e` or a command on the command line). The
preferences mode edits it; the next new tab uses the new value.

```toml
[tabs]
shell = "/bin/zsh"

[[tabs.tab]]
shell = "/bin/bash"

[[tabs.tab]]
command = "top"
```

`[[tabs.tab]]` entries are read from the file. When the window is a plain
login shell, the first entry replaces that shell and the rest open as extra
tabs once the grid size is known. When you passed a command, every entry is an
extra tab and the command stays in front.

A new tab uses the `shell` on the key or click when one is given, otherwise
`tabs.shell`, otherwise `$SHELL`.
