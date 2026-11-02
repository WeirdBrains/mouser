# Fuzzball terminal protocol — v0.1 (implementable draft)

> **Status: v0.1, implementable draft.** Implemented in Mouser (`crates/mouser-core/src/fuzzball.rs`).
> The wire version is still `v=0`: formats may change incompatibly before v1, but v0.1 is
> complete enough to build clients against. Decisions are recorded in §10.
> Name: **Fuzzball**. On the wire it appears only as the two-byte family tag `Fz`.

Fuzzball is a small family of terminal extensions, implemented first in Mouser, meant to
replace the Kitty keyboard and graphics protocols with simpler, multiplexer-friendly
designs:

- [`graphics.md`](graphics.md) — images: upload, place, delete, query.
- [`keyboard.md`](keyboard.md) — one opt-in mode for structured key events.
- [`shell.md`](shell.md) — silent shell actions (change directory without typing `cd`).

This document defines what both share: framing, capability negotiation, versioning,
error replies, security rules, conformance and migration.

## 1. Goals

1. **One way to do each thing.** No layered flag bits, no alternative spellings.
2. **Survives multiplexers.** Every sequence can travel through tmux and GNU screen
   passthrough unchanged; nothing depends on side channels (files, shared memory).
3. **Binary-safe when possible, text-safe always.** Raw length-prefixed payloads on direct
   connections; base64 as the universal fallback.
4. **Detectable.** An application can find out, with one query and a bounded wait,
   whether Fuzzball is available and which version and features it has.
5. **Bounded.** Terminals publish their limits; applications can rely on them.

## 2. Conventions

- `ESC` = 0x1B, `ST` = `ESC \` (0x1B 0x5C). C1 forms (8-bit) are never used.
- `<n>` is an unsigned decimal integer without leading `+`, at most 10 digits.
- Keys are lowercase ASCII `[a-z0-9_]`, values are printable ASCII without `;` `,` `=`
  or spaces unless stated otherwise.
- MUST / SHOULD / MAY as in RFC 2119.

## 3. Framing

All Fuzzball messages are DCS strings with the private marker `=` and the family tag `Fz`:

```
ESC P = F z <subsystem> <verb> ; <params> [: <payload>] ESC \
```

- `<subsystem>`: one ASCII letter. `c` = common, `g` = graphics, `k` = keyboard, `p` = plugins, `s` = shell actions.
- `<verb>`: one ASCII letter, defined per subsystem.
- `<params>`: zero or more `key=value` pairs separated by `,`.
- `<payload>`: present only when the verb takes data; see §3.1.

Unknown params MUST be ignored. Unknown subsystems or verbs MUST produce an
`ENOTSUP` error reply (§5) when the request carried an `id`, and be ignored otherwise.

DCS was chosen over APC/OSC because tmux and screen already have a documented
passthrough for DCS, and parsers that don't know Fuzzball skip DCS strings cleanly.

### 3.1 Payloads

Every payload declares its encoding and exact length:

| param | meaning |
|---|---|
| `enc=b64` | payload is standard base64 (RFC 4648 §4, padding optional). Default. |
| `enc=raw` | payload is raw bytes. Only allowed after the terminal advertised `raw` (§4). |
| `len=<n>` | **required** for payloads: number of payload bytes on the wire (after `:`). |

With `enc=raw` the terminal reads exactly `len` bytes after `:` without interpreting them
(they may contain `ESC`, NUL or anything else), then MUST see `ST`. If the next two bytes
are not `ST`, the message is malformed: the terminal discards it, replies `EPROTO`, and
resumes normal parsing at the following byte.

With `enc=b64`, `len` counts base64 characters; the terminal MAY stop scanning for `ST`
early once `len` bytes are consumed.

Large payloads are split into **chunks** of at most `max_chunk` bytes (advertised, §4):

```
ESC P = F z g u ; id=7,fmt=rgba,w=64,h=64,size=16384,enc=b64,len=8192,more=1 : <8192 b64 chars> ESC \
ESC P = F z g u ; id=7,enc=b64,len=8192,more=0 : <8192 b64 chars> ESC \
```

Continuation chunks repeat only `id`, `enc`, `len` and `more`. A chunk with a different
`id` arriving mid-upload aborts the pending upload with `EPROTO`.

### 3.2 Multiplexer passthrough

Through tmux (`allow-passthrough on`) the application wraps each message as tmux
documents, doubling every `ESC` inside:

```
ESC P tmux ; ESC ESC P = F z ... ESC ESC \ ESC \
```

Through GNU screen: `ESC P <message> ESC \`, split into pieces of at most 768 bytes.

Rules that make this work:

- Applications MUST use `enc=b64` when `TMUX` or `STY` is set, or when the capability
  reply came back wrapped (the terminal saw the query through a multiplexer).
- Replies are plain DCS from the terminal; multiplexers pass them to the active pane.
- Graphics placements are cell-anchored (graphics.md §4), so a multiplexer that redraws
  the pane does not move images to wrong pixels; a multiplexer that does not understand
  Fuzzball simply hides them when the pane is not visible.

## 4. Capability negotiation and versioning

Query (common subsystem, verb `q`):

```
ESC P = F z c q ; id=1 ESC \
```

Reply:

```
ESC P = F z c r ; id=1,v=0,sub=gkps,raw=1,max_chunk=65536,g_mem=335544320,g_max_dim=10000,g_fmt=rgba:rgb:png,g_max_place=4096,g_fit=w,g_reflow=1,k=1,k_ime=0,k_release=1 ESC \
```

| key | meaning |
|---|---|
| `v` | protocol version implemented (this document: `0`). |
| `sub` | supported subsystems, as letters (`g` graphics, `k` keyboard, `p` plugins: plugins.md). |
| `raw` | `1` if `enc=raw` is accepted on this connection, else `0`. |
| `max_chunk` | largest accepted payload per message, in wire bytes. |
| `g_*`, `k_*` | subsystem limits/features (see graphics.md, keyboard.md). |

Detection procedure for applications:

1. Send the Fuzzball query **followed by** a primary device attributes request `ESC [ c`.
2. Read replies. Every terminal answers DA1; a Fuzzball terminal answers the Fuzzball query
   first. If DA1 arrives without a Fuzzball reply, Fuzzball is unavailable.

This needs no timeout and works through multiplexers that forward both.

Versioning: `v` is a single integer. Minor additions (new optional params, new reply
keys) do not bump it; incompatible changes do. An application requests a version with
`v=<n>` on mode-setting verbs; a terminal that cannot serve it replies `EVERSION`.

## 5. Replies and errors

```
ESC P = F z <subsystem> r ; id=<n>,ok=1[,…] ESC \
ESC P = F z <subsystem> r ; id=<n>,ok=0,err=<CODE>[,msg=<text>] ESC \
```

Replies are sent **only** when the request carried `id=<n>` (n ≥ 1). `quiet=1` suppresses
success replies, `quiet=2` suppresses all. `msg` is ASCII without `,;=`, ≤ 80 chars.

| code | meaning |
|---|---|
| `ENOTSUP` | unknown subsystem/verb/value |
| `EINVAL` | bad or missing parameter |
| `EPROTO` | framing error (bad length, aborted chunking) |
| `ENOENT` | referenced image/placement does not exist |
| `ENOMEM` | over a published limit |
| `EPERM` | refused by policy (e.g. disabled feature) |
| `EVERSION` | requested version not available |

## 6. Security considerations

- **No file system access by default.** Payloads come only from the byte stream. A
  terminal MAY add an opt-in, user-configured path transmission later; it MUST be off by
  default and restricted to regular files the user allowed.
- **Bounded memory.** Terminals MUST enforce `g_mem`, `g_max_dim` and `max_chunk` and
  reply `ENOMEM` rather than grow without limit. Partial uploads are discarded after
  10 s without a chunk, on reset (RIS), and when the pane closes.
- **No reflection of payloads.** Replies never echo payload bytes, only ids and codes, so a
  hostile `cat` of a file cannot inject keystrokes through replies.
- **Keyboard mode is per-screen and reset on RIS**, so a crashed program cannot leave the
  shell in a mode it doesn't understand after `reset`.
- Decoders (PNG) MUST treat input as untrusted: check dimensions before allocating.

## 7. Migration and compatibility

- **Kitty keyboard protocol: the only fallback.** Mouser implements the Kitty keyboard
  protocol (`CSI > u` / `CSI < u` / `CSI = u` / `CSI ? u`) for applications that have not
  adopted Fuzzball. If both are active, Fuzzball keyboard mode wins.
- **No other Kitty protocols.** Kitty graphics, iTerm2 `OSC 1337` images and the
  `xterm-kitty` TERM fallback are not supported; images are Fuzzball graphics only.
- **Detection.** Query Fuzzball first (§4). If absent, use `CSI ? u` for Kitty keyboard,
  then legacy. `TERM` is not used for detection.
- **Legacy fallback.** Without Fuzzball or Kitty keyboard mode, keys are sent as xterm/VT
  sequences.

## 8. Conformance test suite (plan)

A language-neutral suite in `tests/protocol/` of the Mouser repository:

1. **Golden byte tests**: input byte streams → expected reply bytes and expected terminal
   state (images, placements, cursor position, active key mode), as JSON fixtures.
2. **Framing fuzz**: random splits of every fixture across read boundaries; truncated and
   oversized payloads; raw payloads containing `ESC \`.
3. **Passthrough tests**: every fixture wrapped for tmux and screen must give the same
   state.
4. **Keyboard tests**: synthetic key events (physical code, logical key, modifiers, text,
   IME phase) → expected bytes.
5. A small runner (`fuzzball-conformance`) that drives a terminal through a PTY and reports
   pass/fail per fixture, so other terminals can test themselves.

## 9. Reference libraries (plan)

- `fuzzball` (Rust): encoder/decoder for all messages, chunking, base64/raw choice,
  detection helper, tmux/screen wrapping. Used by Mouser itself.
- A C header-only encoder and a Python module for applications, generated from the same
  message tables.
- All libraries ship the conformance fixtures as their own tests.

## 10. Decisions (v0.1)

The v0 open questions, resolved:

| # | Question | Decision | Rationale |
|---|----------|----------|-----------|
| 1 | DCS vs APC framing | **DCS with `=` private marker** (`ESC P = F z …`). | tmux (`allow-passthrough`) and screen pass DCS through; APC is dropped by several multiplexers. No known terminal mangles `=`. |
| 2 | `enc=raw` through multiplexers | **Not allowed.** Terminals advertise `raw=0` unless directly attached and binary-safe; Mouser v0.1 always reports `raw=0` and answers `enc=raw` with `EPERM`. | Raw bytes can contain `ESC \` and break passthrough wrapping; base64 is always safe. |
| 3 | Animation | **Deferred** past v1. | Keeps the image model static and simple; apps can re-upload frames. |
| 4 | Unicode placeholder cells | **Deferred.** | Only needed for text-only pipelines; adds a large grid dependency. |
| 5 | Report Mouser's reserved shortcuts? | **No.** Keys the terminal consumes (tab/split hotkeys, UI) are never reported. | Predictable ownership; matches every other terminal. |
| 6 | Virtual placements | **Deferred.** | No multiplexer renders Fuzzball yet; can be added as a new verb without breaking v0. |
| 7 | Colour management | **sRGB assumed.** `cs=` is reserved: `cs=srgb` is accepted, anything else is `ENOTSUP`. | Forward-compatible hook without implementing colour management. |
| 8 | Version | **`v=0`** for the v0.1 draft. | Wire format may still change; `v=1` will mean stable. |
| 9 | IME and releases | Mouser reports `k_ime=0` (no pre-edit events yet) and `k_release=1`. | winit IME events are not wired through yet. |

Limits Mouser advertises: `max_chunk=65536`, `g_mem=335544320` (320 MiB, least-recently-placed
unplaced images evicted first), `g_max_dim=10000`, `g_max_place=4096`, `g_fmt=rgba:rgb:png`.
Uploads abandoned for more than 10 s are discarded when the next chunk arrives.

Placement lifecycle (graphics.md §4), decided in v0.1:

| # | Question | Decision | Rationale |
|---|----------|----------|-----------|
| 10 | Placement partly evicted from scrollback | Removed when its **anchor row** is evicted. | One rule for every case; the rest of a cut image is not useful. |
| 11 | Free image data when its last placement goes? | **No.** It becomes an eviction candidate under `g_mem` pressure. | Apps may re-place an image by id without re-uploading. |
| 12 | Resize | Placements are **fitted to the screen width** (aspect kept): one that would overhang the right edge moves to **column 0** at its requested size, scaled down only when wider than the screen (no more pinning to the last column at one column wide). A wider screen restores the requested anchor and size, never beyond; the request is kept so this is reversible. Anchor lines cut off below a shorter screen are removed with their text. No height fitting. | Images always fit the width (Michael, t132u, 2026-10-31; replaces the 2026-10-30 shrink-in-place/pin rule). |
| 13 | Alternate screen | Placements scroll with alt text and are removed when they scroll off the top (no scrollback), on resize if cut off, and when the alt screen is left. The alt screen is not re-wrapped; the saved main screen is re-wrapped and its placements kept. | Same as the text; apps redraw the alt screen. |
| 20 | Fill the width | `fit=w` on place (graphics.md §2.2): from the anchor column to the right edge, aspect kept, refitted on every resize in both directions. Advertised as `g_fit=w`. `mouser show` uses it by default. | Michael: "I need the images to always fit the width of the terminal" (t132u). |
| 21 | Re-wrap on resize | The terminal re-wraps soft-wrapped lines of the main screen and the scrollback; placements stay anchored to their logical line and character, and a placement whose height changes gets or gives back the empty rows below it, so text after it moves instead of being covered. Advertised as `g_reflow=1`. | Michael: "Build the rewrap" (t133u), so fitted images no longer overlap the text below after widening. |
| 15 | Downscale quality | Renderers draw downscaled images from a cached area-averaged copy at the displayed size (Mouser: `mouser-render/src/scale.rs`), so CPU and GPU sample 1:1 with no nearest-neighbour stripes. | Quality without rescaling every frame. |
| 14 | Abandoned uploads | Discarded by a timer 10 s after the last chunk (not only when the next chunk arrives). | Frees the buffer of a crashed uploader. |

Kitty graphics fallback (graphics.md §7), decided 2026-10-30:

| # | Question | Decision | Rationale |
|---|----------|----------|-----------|
| 16 | Kitty graphics? | **Supported as a fallback**, full protocol incl. placeholders and animation (Michael: "Full kitty support, but Fuzzball takes precedence"). | Existing tools (icat, chafa, yazi, …) work in Mouser today. |
| 17 | Precedence | Both always accepted; separate id namespaces; one shared memory quota; Fuzzball draws above Kitty at equal z; `mouser show` uses Fuzzball when it answers. | No conflicts to resolve at the wire level; Fuzzball stays primary. |
| 18 | Kitty lifecycle | Kitty's own rules where they differ: only clear-screen erases, scroll regions move only placements fully inside, many `p=0` placements per image. Scrollback, alt screen and resize scaling as for Fuzzball. | Kitty clients expect kitty's behaviour. |
| 19 | `mouser show` inside tmux | tmux 3.x forwards passthrough replies after answering DA1 itself, so `mouser show` waits for the graphics replies. With a Kitty-capable outer terminal it uses **Kitty Unicode placeholders** (even when Fuzzball also answers), otherwise Fuzzball passthrough. | Placeholders are text, so tmux moves and redraws the image; Fuzzball has no placeholder mode. Michael can flip this. |
| 20 | Placeholder ids | Low byte in a 256-colour foreground, high byte in the third diacritic. | tmux downgrades true colour for terminals it does not know take RGB. |

Not yet implemented in Mouser: IME events, lock-key state (`locks` is always 0).

A demo client is in `tools/fuzzball-demo.py`.
