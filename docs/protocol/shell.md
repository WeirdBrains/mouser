# Fuzzball shell actions (`s`): v0.1

> **Status: v0.1, implementable draft; wire version `v=0`.** Implemented in Mouser
> (`crates/mouser-core/src/fuzzball.rs`) with integrations for zsh, bash and fish
> (`crates/mouser-core/shell/`, printed by `mouser shell-integration SHELL`).
> Framing, errors and versioning follow the common [README](README.md).

Some terminal features act on the shell itself. For example, Mouser's commander
changes the shell's directory when you open a folder. Typing `cd -- '/path'` into
the shell works everywhere, but it shows up as a command line and its echo, and it
fights with whatever the user is typing. The `s` subsystem lets a shell integration
offer **silent actions**:

- the shell does the action without printing a command line;
- the line being edited (text and cursor) is kept;
- the prompt is redrawn in place;
- the shell replies so the terminal knows how it went.

Unlike the other subsystems, the shell (an application) advertises here and the
terminal sends the requests. Requests arrive on the shell's **input** (the PTY),
because that is the only channel that also works over ssh, tmux and screen.

## 1. Messages

| direction | message | meaning |
|---|---|---|
| shell → terminal | `ESC P = F z s a ; v=0,shell=<name>,caps=<cap>[+<cap>…] ESC \` | advertise: silent actions are possible now |
| shell → terminal | `ESC P = F z s a ; v=0,caps= ESC \` | withdraw (a command is starting) |
| terminal → shell input | `ESC P = F z s c ; id=<n>,path=<pct> ESC \` | change directory (cap `cwd`) |
| shell → terminal | `ESC P = F z s r ; id=<n>,ok=1 ESC \` | done |
| shell → terminal | `ESC P = F z s r ; id=<n>,ok=0,err=<code>,msg=<pct> ESC \` | failed (`ENOENT`, `EPERM`, …) |

- `shell` is informational (`zsh`, `bash`, `fish`).
- `caps` lists actions joined by `+`. v0 defines only `cwd`. Unknown caps MUST be
  ignored. An empty `caps` (or a missing one) withdraws the advertisement.
- An advertisement with a `v` the terminal does not implement is ignored. With an
  `id`, the terminal replies `ok=0,err=EVERSION`.
- `s r` is itself a reply. The terminal never answers it.
- The terminal capability reply (`c q`) lists `s` in `sub` when the subsystem is
  supported.

## 2. When the terminal may send a request

The integration advertises at **every prompt** and withdraws **before every
command**:

- zsh: `precmd` / `preexec`
- bash: `PROMPT_COMMAND` / `PS0`
- fish: `fish_prompt` / `fish_preexec`

The terminal MUST NOT send `s c` unless:

1. the latest `s a` from this tab advertised `cwd`, and
2. the shell is still the PTY's foreground process (where that can be told).

Otherwise the bytes would reach whatever program is running. A full-screen program
or a nested shell without the integration never advertises, so it never receives a
request.

## 3. Encoding

`path` (and `msg`) are **percent-encoded** bytes:

- every byte other than `A-Z a-z 0-9 - . _ ~ /` is written as `%XX` (uppercase hex);
- the value can therefore never contain `;` `,` `=` space or `ESC`;
- paths are raw bytes, not necessarily UTF-8.

Percent-encoding is used instead of base64 because a shell can decode it with
builtins (`printf %b` in bash, `${(g::)}` in zsh, `string unescape --style=url` in
fish), so the integration runs no external programs.

Example: the directory `/home/me/a dir` with id 7.

```
ESC P = F z s c ; id=7,path=/home/me/a%20dir ESC \
ESC P = F z s r ; id=7,ok=1 ESC \
ESC ] 7 ; file://host/home/me/a%20dir ESC \
```

## 4. What the shell does on `s c`

1. Read the rest of the message up to `ST`. The integration binds the fixed prefix
   `ESC P = F z s c` as a key sequence. Requests longer than 16 KiB are dropped.
2. `cd` to the decoded path, without printing anything, and keep the edit buffer
   and the cursor.
3. Reply `s r` with the same `id`.
4. Report the new directory with OSC 7.
5. Re-run the hooks that build the prompt and redraw the prompt in place:
   - zsh: `precmd_functions` and `zle reset-prompt`;
   - fish: `commandline -f repaint`;
   - bash: readline keeps the prompt it expanded, so the binding is a macro. It
     saves the line, accepts an empty line (bash prints a fresh prompt for the new
     directory, drawn over the old one), then puts the line and cursor back.

## 5. Security

- The request only does what typing `cd` would do, in the user's own shell.
- A program can't trigger it, because requests come from the terminal side of the
  PTY. A program printing `ESC P = F z s c …` to its output sends it to the
  terminal, and the terminal ignores `s c` from the application side.
- A forged `s a` from a program only makes the terminal send `s c` to that program
  (after the `cd` request, where it would have typed the `cd` text anyway). It is
  no worse than the fallback.

## 6. Fallback without the integration

When no `cwd` advertisement is active, Mouser types `cd -- '<path>'`. It keeps the
user's line through the kill ring (Ctrl+E Ctrl+U … Ctrl+Y). When the shell's prompt
is back, it **erases the rows** from the old prompt's start to the new prompt's
start, so one prompt remains:

- Prompt starts come from OSC 133 `A` marks.
- Without marks, the cursor row is used, which is right for one-line prompts.
- Nothing is erased on the alternate screen, or when the rows have already
  scrolled off the screen.

## 7. Changes

- v0.1 (Mouser 1.0.1): first version, cap `cwd`.
