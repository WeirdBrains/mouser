# SSH and TERM

Mouser's tabs run with `TERM=mouser` when its terminfo is installed
([Terminal](./terminal.md#term-and-terminfo)). ssh sends that `TERM` to the server, and a
server without Mouser's terminfo cannot start tmux or other curses programs:

```
$ ssh web1
web1$ tmux new
missing or unsuitable terminal: mouser
```

Mouser handles this for you: on a host that knows `mouser` you keep `TERM=mouser` (styled
underlines, truecolor, synchronized output); on a host that does not, the session gets
`TERM=xterm-256color`, so `ssh web1` and then `tmux` just work. Nothing is installed on the
server.

## How it works

Mouser puts an `ssh` shim first on the `PATH` of every tab it starts (a link to Mouser in
`$XDG_RUNTIME_DIR/mouser/bin`, or `~/.mouser/bin` where there is no runtime folder) and
sets `MOUSER_SSH_WRAPPER` to it. Typing `ssh …` in a Mouser shell, a saved connection, or
`mouser -e ssh …` all go through it. `mouser ssh …` does the same by hand.

For an **interactive session** (a local terminal, no `-T`, no `-N`), the shim adds a short
POSIX `sh` check to the same connection, so there is no second login or password prompt.
On the server it:

1. keeps `TERM=mouser` if `infocmp mouser` succeeds (or, without `infocmp`, if
   `~/.terminfo`, `$TERMINFO` or a system terminfo folder has `mouser`), otherwise sets
   `TERM=xterm-256color`;
2. prints `/run/motd.dynamic` and `/etc/motd` unless `~/.hushlogin` exists (sshd skips the
   message of the day when a command is given);
3. `exec`s your login shell (`$SHELL -l`).

`ssh -t host CMD` (for example `ssh -t web1 tmux new -A`) gets the same check, then runs
`CMD` through your login shell exactly as sshd would (`$SHELL -c CMD`).

**Everything else runs the real ssh unchanged**, with the same arguments and environment:
no local terminal (scp, sftp, rsync, git, scripts, pipes), `-N`, `-W`, `-O`, `-G`, `-V`,
`-Q`, `-T`, `-s`, `-f`, `-n`, a command without `-t`, `RequestTTY no`, `SessionType none`,
and `git@` hosts. ProxyJump, ControlMaster/ControlPersist, forwarding and every other
option work as before. The shim finds the real ssh further along `PATH`, skipping itself,
and marks it with `MOUSER_SSH_WRAPPED` so an ssh wrapper of your own that calls `ssh`
again is not wrapped twice.

When a session cannot be wrapped safely, it falls back to `TERM=xterm-256color` without a
check: a `RemoteCommand` in ssh_config (ssh refuses a second command), or a `-t` command
containing a backslash or a newline.

The shim is on `PATH` first, but a login rc file that rebuilds `PATH` (macOS
`path_helper` in `/etc/zprofile`, for example) can move it behind `/usr/bin/ssh`. The
shell integration (`mouser shell-integration zsh|bash|fish`, [Modes](./modes.md)) defines an
`ssh` function that calls `$MOUSER_SSH_WRAPPER`, so it works wherever `PATH` ends up. An
`ssh` function you defined yourself is left alone; aliases such as `alias ssh='ssh -A'`
keep working.

## Choosing

| Setting | Effect |
| --- | --- |
| `MOUSER_SSH_TERM=auto` (default) | `mouser` where the host has its terminfo, else `xterm-256color` |
| `MOUSER_SSH_TERM=mouser` | always send `TERM=mouser`; the shim does nothing |
| `MOUSER_SSH_TERM=xterm-256color` | always send `xterm-256color` for terminal sessions (no check, so the login is untouched) |
| `TERM=… ssh host` | a `TERM` other than `mouser` is sent as is |
| `SetEnv MOUSER_SSH_TERM=…` in a `Host` block of `~/.ssh/config` | the same choices for one host (read with `ssh -G`) |

Set `MOUSER_SSH_TERM` in your shell's rc file to make it permanent.

## Getting `TERM=mouser` on a host

Install Mouser's terminfo there once; the next session finds it and keeps `TERM=mouser`:

```sh
infocmp -x mouser | ssh web1 'tic -x -'        # for your user (~/.terminfo)
infocmp -x mouser | ssh web1 'sudo tic -x -'   # for everyone on the host
```

## Limits

- The check needs a POSIX `sh` on the server. For a Windows OpenSSH server (cmd.exe or
  PowerShell) set `SetEnv MOUSER_SSH_TERM=xterm-256color` for that host.
- An interactive session runs as a command, so sshd's "Last login" line is not shown, and
  a server `ForceCommand` runs instead of the check (`TERM=mouser` is sent as before).
  Restricted shells that refuse `sh -c` need `MOUSER_SSH_TERM=xterm-256color` too.
- mosh needs nothing: `mosh-server` sets `TERM` itself (`xterm-256color`).
- On Windows Mouser's tabs use `TERM=xterm-256color` already, so there is no shim.
- ssh started on the server (a second hop from there) sends the server's `TERM`; use
  ProxyJump (`ssh -J web1 db1`) so the hop starts from Mouser.
