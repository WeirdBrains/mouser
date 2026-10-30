# Burrow keyboard — draft v0

Subsystem letter: `k`. Framing, replies and errors: see [README](README.md).

## 1. One mode

There is exactly one mode: **structured key events**. While it is on, every key event
(press, repeat, release) is reported as one message; nothing is sent in legacy form.
While it is off, the terminal sends legacy (xterm/VT) bytes.

Enable / disable (per screen; the main and alternate screens are independent):

```
ESC P = B w k s ; on=1[,v=0][,id=<n>] ESC \     enable
ESC P = B w k s ; on=0[,id=<n>] ESC \           disable
ESC P = B w k q ; id=<n> ESC \                  query
→ ESC P = B w k r ; id=<n>,ok=1,on=1 ESC \
```

The mode is also turned off by RIS and when the program's screen is left (e.g. leaving
the alternate screen restores the main screen's own state).

## 2. Event message

Sent by the terminal to the application:

```
ESC P = B w k e ; t=<p|r|u>,code=<phys>,key=<logical>[,mods=<n>][,locks=<n>][,text=<hex>][,ime=<phase>] ESC \
```

| field | meaning |
|---|---|
| `t` | `p` press, `r` repeat, `u` release. |
| `code` | physical key, as the USB HID usage name from the W3C UI Events `code` list, e.g. `KeyA`, `Digit1`, `ArrowUp`, `ShiftLeft`, `Numpad7`. Layout independent. |
| `key` | logical key: a W3C UI Events `key` name for non-text keys (`Enter`, `Escape`, `F13`, `MediaPlayPause`…), or `U+XXXX` for the unmodified character the key produces on the current layout (e.g. `U+0061` for `a`, `U+0444` for `ф`). |
| `mods` | bitmask of held modifiers: 1 Shift, 2 Alt, 4 Ctrl, 8 Super, 16 Hyper, 32 Meta, 64 AltGr. Omitted when 0. |
| `locks` | 1 CapsLock, 2 NumLock, 4 ScrollLock. Omitted when 0. |
| `text` | UTF-8 text this event inserts, hex-encoded (so any character is safe on the wire). Omitted when none. Present for presses and repeats only. |
| `ime` | `start`, `update`, `commit`, `cancel` — composition state; see §3. |

Modifier keys themselves produce events (`code=ShiftLeft,key=Shift`), with `mods`
reflecting the state **after** the event.

Examples:

```
a              ESC P = B w k e ; t=p,code=KeyA,key=U+0061,text=61 ESC \
Shift+a        ESC P = B w k e ; t=p,code=KeyA,key=U+0061,mods=1,text=41 ESC \
release a      ESC P = B w k e ; t=u,code=KeyA,key=U+0061 ESC \
Ctrl+c         ESC P = B w k e ; t=p,code=KeyC,key=U+0063,mods=4 ESC \
Ctrl+ф (RU)    ESC P = B w k e ; t=p,code=KeyA,key=U+0444,mods=4 ESC \
Enter          ESC P = B w k e ; t=p,code=Enter,key=Enter,text=0d ESC \
Up (repeat)    ESC P = B w k e ; t=r,code=ArrowUp,key=ArrowUp ESC \
Left Shift     ESC P = B w k e ; t=p,code=ShiftLeft,key=Shift,mods=1 ESC \
```

Applications get both the physical key (for shortcuts that should not move with the
layout, e.g. games, vim's `hjkl` on Dvorak if desired) and the logical key (for shortcuts
by letter), without the terminal guessing which one is meant.

## 3. IME and composed text

Composition is reported separately from key presses so applications don't mistake
pre-edit keystrokes for input:

```
ESC P = B w k e ; t=p,ime=start ESC \
ESC P = B w k e ; t=p,ime=update,text=e3818b ESC \        pre-edit "か", not inserted
ESC P = B w k e ; t=p,ime=commit,text=e6bca2e5ad97 ESC \  insert "漢字"
ESC P = B w k e ; t=p,ime=cancel ESC \
```

Key events consumed by the IME are not reported. Dead keys behave like a one-character
composition (`start` … `commit`). Applications that don't care about pre-edit only act on
`commit`.

## 4. Paste

Pasted text is not keyboard input; it stays on bracketed paste (mode 2004) whether or not
this mode is on.

## 5. Capability keys

In the common capability reply: `k=1` (mode available), `k_ime=1` if IME events are
reported, `k_release=1` if the platform delivers release events (some remote setups
cannot).

## 6. Differences from the Kitty keyboard protocol

- No flag bits: one mode, always full information.
- Physical and logical keys are separate named fields instead of numeric code points
  plus "alternate keys".
- Text is always explicit (hex), so control characters and non-BMP text are unambiguous.
- IME composition is part of the protocol.
