# Terminal emulation

Mouser emulates a modern xterm (VT220+ with xterm and ECMA-48 extensions).

## Input

| Key / action | What it does |
|---|---|
| Ctrl+Shift+V | Paste the clipboard (`wl-paste`). Wrapped in `ESC[200~ … ESC[201~` when the app enabled bracketed paste (mode 2004). |
| Ctrl+click | Open an OSC 8 hyperlink under the pointer (`http`, `https`, `mailto`, `file` only, via `xdg-open`). |
| Shift+mouse | Bypass mouse reporting when an app (vim, tmux…) has grabbed the mouse. |
| Mouse wheel | Scrollback; reported to the app in mouse modes; arrow keys on the alternate screen. |

OSC 52 clipboard *writes* are passed to `wl-copy`; clipboard *reads* are never answered.

## Supported sequences

- SGR: 16 / bright / 256 / truecolor (`;` and `:` forms), bold, dim, italic (synthetic slant), underline styles `4:1`–`4:5`, double underline 21, underline colour 58/59, blink (drawn steady), inverse, hidden, strike, overline.
- Wide East Asian and emoji characters take two cells; combining marks and ZWJ attach to their base cell.
- Cursor movement, DECSTBM, ICH/DCH/IL/DL/ECH/REP, IRM, HTS/TBC/CHT/CBT, DECSC/DECRC, DECOM, DECAWM, DECSCUSR, DECSTR, DECALN, DECCKM, DECKPAM.
- Modes 1000/1002/1003/1006/1016 mouse, 1004 focus, 1049/1047/47 alternate screen, 2004 bracketed paste, 2026 synchronized output, 2027 grapheme clustering.
- OSC 0/2 title, 4/10/11/12 colour query and set, 104/110–112 reset, 7 cwd, 8 hyperlinks, 52 clipboard, 133 prompt marks.
- DA1/DA2/DA3, DSR 5/6, DECRQM, XTVERSION, XTGETTCAP, DECRQSS, window reports 14/16/18t, Fuzzball `DCS = Fz c q`.
- The PTY gets the grid's pixel size (`TIOCSWINSZ`).

Not yet: DECSLRM (left/right margins), colour emoji, an italic face from the font family.

## TERM and terminfo

Mouser sets `COLORTERM=truecolor` and picks `TERM` in this order: `xterm-kitty` (if its terminfo is installed), `mouser`, then `xterm-256color`.

To install Mouser's own entry for your user (styled underlines, truecolor, synchronized output, bracketed paste and focus capabilities):

```sh
tic -x -o ~/.terminfo extra/mouser.terminfo
```
