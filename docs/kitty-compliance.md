# Kitty keyboard protocol compliance — Mouser

Mouser supports **only** the Kitty keyboard protocol, as a fallback for applications that do
not use [Fuzzball keyboard mode](protocol/keyboard.md). Fuzzball wins when both are active.
Kitty graphics, iTerm2 `OSC 1337` images and the `xterm-kitty` TERM fallback are not supported.

Spec: <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>. Tests, one per spec section:
`crates/mouser-core/tests/kitty_keyboard_spec_tests.rs` (plus `kitty_keyboard_tests.rs`).

Legend: ✅ compliant · 🟡 partial · ➖ not applicable

| Spec section | Status | Notes |
|---|---|---|
| Progressive enhancement: `CSI > flags u` push, `CSI < n u` pop, `CSI = flags ; mode u` set/or/clear, `CSI ? u` query | ✅ | Stack per screen (main/alt), bounded; pop beyond empty resets to 0; RIS clears both. |
| Flag 1 — disambiguate escape codes | ✅ | Esc, Alt/Ctrl+keys, F13+, media, keypad nav keys as `CSI u`; Enter/Tab/Backspace stay legacy. |
| Flag 2 — report event types | ✅ | `:2` repeat, `:3` release; Enter/Tab/Backspace releases only with flag 8. |
| Flag 4 — report alternate keys | ✅ | Shifted key and base-layout key fields. |
| Flag 8 — report all keys as escape codes | ✅ | Text keys, Enter/Tab/Backspace, modifier keys (57441…) reported; lock bits kept. |
| Flag 16 — report associated text | ✅ | Text codepoints as third field; omitted on release. |
| Key event encoding `CSI code;mods:event;text u` | ✅ | Unicode codepoint of the unshifted key; trailing defaults omitted. |
| Modifiers (1 + bits: shift 1, alt 2, ctrl 4, super 8) | ✅ | |
| Modifiers hyper 16, meta 32 | 🟡 | winit does not expose Hyper/Meta; never set. |
| Lock modifiers caps 64, num 128 | 🟡 | Encoding supported; winit does not report lock state, so not set from the window. |
| Functional key definitions (PUA 57344+, legacy `CSI ~` / `SS3` forms) | ✅ | Full table incl. keypad (57399…), media (57428…), modifiers (57441…). |
| Legacy mode (flags 0) keys | ✅ | xterm encoding; Ctrl+Backspace 0x08, Ctrl+Shift/Super+text as `CSI u`; F13+ not sent (optional). |
| Disambiguation of Alt+Esc, Ctrl+Tab etc. | 🟡 | Some combinations are reserved by Mouser's own shortcuts and are not delivered. |
| Detection (`CSI ? u` answered before DA1) | ✅ | |
| IME / compose | ➖ | Committed text is sent as text; the spec defines no pre-edit encoding. |
