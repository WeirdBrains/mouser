# Terminal emulation

Mouser emulates a modern xterm (VT220+ with xterm and ECMA-48 extensions).

## Input

| Key / action | What it does |
|---|---|
| Drag | Select characters (screen and scrollback; dragging past the top/bottom edge scrolls). |
| Double-click / triple-click | Select a word / a line. Word characters are letters, digits and `[selection] word_chars` (default `@-./_~?&=%+#:`). |
| Shift+click | Extend the selection. |
| Alt+drag | Block (rectangular) selection. |
| Ctrl+Shift+C (Cmd+C on macOS) | Copy the selection to the clipboard. Selecting also sets PRIMARY on Linux. |
| Ctrl+Shift+V (Cmd+V on macOS) | Paste the clipboard. Wrapped in `ESC[200~ … ESC[201~` when the app enabled bracketed paste (mode 2004). Control characters other than tab, LF and CR are dropped. |
| Middle-click / Shift+Insert | Paste PRIMARY (Linux; Shift+Insert pastes the clipboard elsewhere). |
| Ctrl+click | Open an OSC 8 hyperlink under the pointer (`http`, `https`, `mailto`, `file` only, via `xdg-open`). |
| Shift+mouse | Bypass mouse reporting when an app (vim, tmux…) has grabbed the mouse. |
| Mouse wheel | Scrollback; reported to the app in mouse modes; arrow keys on the alternate screen. |

Copied text joins soft-wrapped lines, ends hard lines with a newline and trims trailing blanks. The selection clears when its text changes or a typed key is sent to the app. It is drawn with the theme's `selection_foreground`/`selection_background`, or inverse colours. Clipboard access uses the Wayland connection, then `wl-copy`/`wl-paste`, `xclip`/`xsel` on X11, `pbcopy`/`pbpaste` on macOS.

OSC 52 clipboard *writes* are passed to `wl-copy`; clipboard *reads* are never answered.

## Supported sequences

- SGR: 16 / bright / 256 / truecolor (`;` and `:` forms), bold, dim, italic (synthetic slant), underline styles `4:1`–`4:5`, double underline 21, underline colour 58/59, blink (drawn steady), inverse, hidden, strike, overline.
- Wide East Asian and emoji characters take two cells; combining marks and ZWJ attach to their base cell.
- Cursor movement, DECSTBM, ICH/DCH/IL/DL/ECH/REP, IRM, HTS/TBC/CHT/CBT, DECSC/DECRC, DECOM, DECAWM, DECSCUSR, DECSTR, DECALN, DECCKM, DECKPAM.
- Modes 1000/1002/1003/1006/1016 mouse, 1004 focus, 1049/1047/47 alternate screen, 2004 bracketed paste, 2026 synchronized output, 2027 grapheme clustering.
- OSC 0/2 title, 4/10/11/12 colour query and set, 104/110–112 reset, 7 cwd, 8 hyperlinks, 52 clipboard, 133 prompt marks.
- DEC Special Graphics and British sets in G0/G1 (`ESC ( 0`, `ESC ) 0`, SI/SO), DECSCNM reverse video, LNM (mode 20).
- DA1/DA2/DA3, DSR 5/6, DECRQM, DECREQTPARM, XTVERSION, XTGETTCAP, DECRQSS, window reports 11/13/14/16/18/20/21t, Fuzzball `DCS = Fz c q`.
- The PTY gets the grid's pixel size (`TIOCSWINSZ`).

Not yet: DECSLRM (left/right margins), DECCOLM 132-column switching, double-size lines (DECDHL/DECDWL), G2/G3 and NRC sets, protected areas (DECSCA), user-defined keys, colour emoji, an italic face from the font family, visible blink.

## TERM and terminfo

Mouser sets `COLORTERM=truecolor` and picks `TERM` in this order: `mouser` (if its terminfo is installed), then `xterm-256color`.

To install Mouser's own entry for your user (styled underlines, truecolor, synchronized output, bracketed paste and focus capabilities):

```sh
tic -x -o ~/.terminfo extra/mouser.terminfo
```

## Commands

`mouser [OPTIONS] [-e CMD ARGS... | -- CMD ARGS...]` opens a window. Options:

- `-e, --command CMD ARGS...` runs `CMD` (no shell in between) instead of the login
  shell; everything after `-e` is passed to it. `mouser -- CMD ARGS...` is the same.
  The window closes when the command exits.
- `--hold` keeps it open with `[process exited N]` instead.
- `--working-directory DIR` (alias `--dir`) starts the first tab in `DIR` instead of
  `$HOME`; a missing directory is reported on stderr and `$HOME` used.
- `-T, --title TITLE` sets the window title until the program sets one.
- `--class CLASS` (alias `--app-id`) sets the Wayland app_id / X11 WM_CLASS
  (default `Mouser`), for window rules.
- `-V, --version` prints the version.

The desktop entry declares these for
[xdg-terminal-exec](https://github.com/Vladimir-csp/xdg-terminal-exec)
(`X-TerminalArgExec=-e`, `X-TerminalArgDir=--working-directory`,
`X-TerminalArgTitle=--title`, `X-TerminalArgAppId=--class`, `X-TerminalArgHold=--hold`),
so with `mouser.desktop` first in `~/.config/xdg-terminals.list` launchers such as
Omarchy's run `xdg-terminal-exec btop` as `mouser -e btop`.

`mouser show [--width N] [--height N] [--fit|--original] [--id N] <file|->...` shows
images. It sends `DCS = F z c q`, the Kitty query `ESC _ G i=31,s=1,v=1,a=q,t=d,f=24;AAAA ESC \`,
`CSI 16 t`, `CSI 18 t`, `CSI 6 n` and DA1, and waits up to 1.5 s for the replies (DA1
ends the wait outside tmux). With a Fuzzball reply it uploads each image in base64
chunks of at most `max_chunk`, places it at the cursor fitted to the columns right of
it (aspect kept) and moves the cursor below it. With only a Kitty reply it sends
`a=T` in 4096-byte chunks with `q=1` (errors only) and the same fit. PNG within the
published limits is sent as-is; JPEG, GIF (first frame), WebP and BMP are decoded to
RGBA and scaled down to `g_max_dim` / `g_mem` if needed. Video and audio are out of
scope. Inside tmux every message is wrapped for passthrough. tmux answers DA1 itself and
forwards the outer terminal's replies afterwards, so there `mouser show` waits for the
graphics replies instead. If the outer terminal answers the Kitty query, the image is
transmitted quietly (`q=2`), given a `U=1` virtual placement and printed as rows of
`U+10EEEE` placeholders (id low byte in a 256-colour foreground, high byte in the
third diacritic; at most 297x297 cells), so tmux moves and redraws it; otherwise
Fuzzball is passed through. Without any reply it points at `allow-passthrough`.
Code: `crates/mouser-app/src/show.rs`.
