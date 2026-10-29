# Kitty Protocol Compliance — Mouser

Tracking status of Kitty protocol extensions per https://sw.kovidgoyal.net/kitty/protocol-extensions/

Legend: [x] Implemented, [~] Partial, [ ] Not yet

## Keyboard protocol

- [x] Negotiation: recognize enable/disable and query commands; respond as supported
- [~] Reporting: `CSI u` extended key reports with modifiers for common keys (ASCII, arrows, Home/End, PgUp/PgDn, Function keys)
- [ ] Progressive enhancement flags coverage for all key classes
- [ ] IME pre-edit and compose sequences mapped to extended reports

## Graphics protocol

- [~] Direct transmission: parse Kitty graphics data stream; PNG/JPEG payloads supported; basic placement and deletion
- [~] File-based transmission: parse iTerm2-compatible `OSC 1337` File=... for compatibility
- [ ] Temp file and shared memory transports
- [ ] Z-index, animation, multi-frame images
- [ ] Unicode placeholders and alt-text fallbacks
- [ ] Memory management policies for large image lifetimes

## Other commonly used extensions

- [~] Styled/colored underlines (SGR 4.x variants)
- [~] OSC 8 hyperlinks — parse and render underlined clickable regions (rendering limited)
- [~] OSC 52 clipboard — copy/paste integration (copy implemented; paste gated by security prompt; rendering limited)
- [~] Synchronized output (Mode 2026) — coalesced paint during sync windows
- [ ] OSC 99 desktop notifications

## Notes

- Parser coverage is validated by unit tests in `mouser-core/tests/kitty_*_tests.rs`.
- Rendering behavior for graphics is intentionally conservative to avoid excessive memory use; lifetimes and eviction will improve over time.
- If an application misbehaves, please capture the escape stream (e.g., with `script -f`) and open an issue.

