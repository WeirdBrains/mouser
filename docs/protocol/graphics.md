# Fuzzball graphics — v0.1 (implementable draft)

Subsystem letter: `g`. Framing, replies and errors: see [README](README.md).

## 1. Model

- An **image** is decoded pixel data stored by the terminal, named by an application-
  chosen `id` (1 … 4294967295). Re-uploading an `id` replaces the image and removes its
  placements.
- A **placement** shows (part of) an image at a **cell** with a size in cells. Placements
  are named by `pid` (unique per image). A placement belongs to the text grid: it scrolls
  with the line it was placed on, goes into scrollback with it, is reflowed with its line,
  and is removed when its cells are erased (ED, EL, ECH, DL, scroll-off past the
  scrollback limit, RIS, or switching away from the screen it lives on).
- Each screen (main, alternate) has its own placements; images are shared.

## 2. Verbs

| verb | name | payload |
|---|---|---|
| `u` | upload | yes |
| `p` | place | no |
| `d` | delete | no |
| `q` | query | no |
| `r` | reply (terminal → app) | no |

### 2.1 Upload — `u`

```
ESC P = F z g u ; id=<n>,fmt=<fmt>[,w=<px>,h=<px>],size=<bytes>,enc=<b64|raw>,len=<n>[,more=1] : <payload> ESC \
```

| param | |
|---|---|
| `fmt` | `rgba` (8-bit RGBA, straight alpha), `rgb` (8-bit), `png`. |
| `w`,`h` | required for `rgba`/`rgb`; ignored for `png`. |
| `size` | total decoded payload bytes across all chunks (before base64). Lets the terminal reject oversize uploads on the first chunk. |
| `more` | `1` if more chunks follow. |

Reply on completion (for uploads, `id` is both the image id and the reply id; `quiet` applies as usual):

```
ESC P = F z g r ; id=7,ok=1,w=64,h=64 ESC \
```

### 2.2 Place — `p`

```
ESC P = F z g p ; id=<image>,pid=<n>[,cols=<n>,rows=<n>][,sx=,sy=,sw=,sh=][,fit=w][,z=<-1|0|1>][,move=<0|1>] ESC \
```

- Anchored at the **cursor cell**.
- `cols`,`rows`: size in cells. If one is omitted it is derived from the aspect ratio; if
  both are omitted the image's pixel size is rounded up to whole cells using the current
  cell size.
- `fit=w`: fill the width. The placement spans from the anchor column to the right edge
  (`cols` is ignored, `rows` follows the aspect ratio) and is refitted on every resize,
  scaled up or down, so it always fills the width; it moves to column 0 if the anchor
  column is cut off. Any other `fit` value is `EINVAL`. Terminals that do not know `fit`
  use `cols`/`rows`, so send them too. `cols` is ignored, and `rows` too: the height
  always follows the aspect ratio. If the filled size would exceed the 1000-cell limit the
  place fails with `EINVAL`; clients should then place a fixed size instead (`mouser show`
  shrinks to 1000 rows and drops `fit=w`). Support is advertised as `g_fit=w` in the
  capability reply (§3); without it, send `cols`/`rows` for the current width only.
- `sx,sy,sw,sh`: source rectangle in image pixels (default: whole image).
- `z`: `-1` behind text (above cell backgrounds), `0` (default) above text, `1` above
  text and cursor.
- `move`: `1` (default) moves the cursor to the cell after the placement's last column on
  its last row, scrolling if needed, like printing text; `0` leaves it.
- The image is scaled to fill `cols × rows` cells exactly. Scaling filter is the
  terminal's choice (bilinear recommended).

Placing an existing `(id, pid)` moves it.

### 2.3 Delete — `d`

```
ESC P = F z g d ; what=<target>[,id=<n>][,pid=<n>][,free=1] ESC \
```

| `what` | removes |
|---|---|
| `all` | all placements on the current screen |
| `image` | placements of `id` (all, or just `pid`) |
| `cell` | placements covering the cursor cell |

`free=1` also frees image data that no longer has placements on any screen.

### 2.4 Query — `q`

```
ESC P = F z g q ; id=1 ESC \
→ ESC P = F z g r ; id=1,ok=1,mem_used=1048576,mem_max=335544320,images=3,cell_w=9,cell_h=18 ESC \
```

Also answers with the current cell pixel size so applications can pick image sizes.

## 3. Limits (advertised in the common capability reply)

| key | meaning | Mouser default |
|---|---|---|
| `g_mem` | max bytes of decoded image data held | 320 MiB |
| `g_max_dim` | max width/height in pixels | 10000 |
| `g_fmt` | supported formats | `rgba:rgb:png` |
| `g_max_place` | max placements per screen | 4096 |
| `g_fit` | `fit` modes accepted on place, `:`-separated (`w` = fill the width, §2.2) | `w` |
| `g_reflow` | `1` if the terminal re-wraps text on resize and placements follow it (§4) | `1` |

When `g_mem` would be exceeded the terminal evicts the least recently placed images that
have no visible placements; if that is not enough the upload fails with `ENOMEM`.

## 4. Interaction with text

- Placements occupy cells but do not change their text; text written over a `z=0`
  placement is drawn under it, over a `z=-1` placement on top of it.
- Erasing any cell of a placement removes the whole placement.
- A placement follows its **anchor cell** (top-left) when text moves: scrolling (also
  inside scroll regions, SU/SD, IL/DL) and lines moving into the scrollback. A placement
  scrolled out of its scroll region, or out of the top of the alternate screen (which has
  no scrollback), is removed.
- When the anchor row is dropped from the scrollback (the scrollback limit, or ED 3
  "clear scrollback"), the whole placement is removed, even if it spans later rows.
- **Scaling to the screen width.** The terminal keeps the requested anchor column and
  `cols`/`rows`, and shows each placement fitted to the current width (also when it is
  first placed): unchanged if it fits; if it would overhang the right edge it moves to
  column 0 at its requested size, scaled down (aspect kept) to the screen width when it is
  wider than the screen. It is never narrower than the smaller of its request and the
  screen, and never dropped. The rows it covers scale by the same factor. A wider screen
  restores the requested anchor and size, never beyond; `fit=w` placements instead
  always fill the width. Images are not
  scaled to the screen height: tall placements scroll like text.
- **Re-wrapping on resize** (`g_reflow=1`). When the width changes the terminal re-wraps
  the main screen and the scrollback: rows joined by soft wraps (autowrap) form one logical
  line, which is split again at the new width; hard line breaks stay, and a double-width
  character is never split (the row ends one cell early). The cursor stays on the same
  character. Each placement stays anchored to its logical line and its column offset in
  it (an anchor past the end of the line's text keeps its requested column and is then
  fitted as above), so images move with the text. After fitting, a placement whose height
  changed (a `fit=w` placement, or one scaled for overhang) takes or gives back the empty
  lines directly below its anchor line, so the text after it moves down or up instead of
  being covered; only empty lines are removed. Placements are moved before the width fit
  is applied. The old screen's first row stays at the top unless the cursor would fall
  below the last row; rows above then go to the scrollback. The **alternate screen is not
  re-wrapped** (applications redraw it): its rows are cut or padded and its placements
  are fitted only. The saved main screen behind it is re-wrapped and keeps its
  placements. Kitty placements with explicit `c`/`r` move with their text the same way
  and keep their size whenever it fits. Text in placeholder cells (Kitty `U=1`) is
  re-wrapped like any text. A terminal may re-wrap older scrollback lines shortly after
  the resize instead of at once (Mouser: the newest 2000 rows with the screen, the rest
  in steps); placements in those lines are moved and refitted when their lines are.
- On a shorter screen, lines pushed into the scrollback keep their placements; a
  placement whose anchor line is cut off below the last row is removed with that line
  (its text is gone too).
- Placements are sized in cells: when the cell size changes (font zoom, DPI) they keep
  their cells and their pixels are rescaled.
- Removing a placement never frees its image: unplaced images stay until deleted
  (`gd` with `free=1`, re-upload) or evicted for memory (§3).
- An upload that receives no chunk for 10 s is discarded.
- On RIS: all images, placements and partial uploads are discarded.

## 5. Examples

Upload a 2×1 RGBA image (red, blue) and show it in a 4×2 cell box:

```
ESC P = F z g u ; id=1,fmt=rgba,w=2,h=1,size=8,enc=b64,len=12 : /wAA/wAA//8= ESC \
ESC P = F z g p ; id=1,pid=1,cols=4,rows=2 ESC \
```

Same upload, raw (only after `raw=1` was advertised):

```
ESC P = F z g u ; id=1,fmt=rgba,w=2,h=1,size=8,enc=raw,len=8 : FF 00 00 FF 00 00 FF FF ESC \
```

Delete it and free the data:

```
ESC P = F z g d ; what=image,id=1,free=1 ESC \
```

## 6. Design notes (compared with Kitty graphics)

Mouser also implements Kitty graphics as a fallback (§7); these notes explain why Fuzzball is designed differently.


- One encoding of each concept (no `a=T` shortcut, no `I=` numbers, no `t=f/t/s`).
- Explicit payload length; base64 only in v0.1 (`raw=0`, see README §10).
- Placements are text-grid objects with defined erase/reflow rules.
- Limits are advertised rather than implicit.

## 7. Kitty graphics fallback in Mouser

Mouser implements the [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
for applications that do not speak Fuzzball yet (`kitten icat`, chafa, timg, yazi, …).
**Fuzzball takes precedence:** both are always accepted, Kitty images live in their own id
namespace (Kitty image 1 and Fuzzball image 1 are different images), and at equal z
Fuzzball images draw above Kitty images. Code: `crates/mouser-core/src/kitty_graphics.rs`;
tests: `crates/mouser-core/tests/kitty_graphics_tests.rs`.

Supported:

- Framing: APC `ESC _ G <keys> ; <payload> ESC \` (bodies over 16 MiB are skipped).
- Media: `t=d` (chunked with `m=1`/`m=0`; a delete aborts a partial upload; idle uploads
  are dropped after 10 s), `t=f`, `t=t` (deleted after reading only when it is in a temp
  directory and its path contains `tty-graphics-protocol`), `t=s` (POSIX shared memory,
  unlinked after reading; Unix only). `S`/`O` size and offset. Files: symlinks followed,
  regular files only, `/proc`, `/sys` and `/dev` (except `/dev/shm`) refused before
  opening, at most 320 MiB, and every failure is the same `EBADF:Failed to read image file`.
- Formats `f=24`, `f=32`, `f=100` (PNG), and the indexed `f=8`/`f=16`; `o=z` (zlib,
  output bounded by the expected size).
- Actions `a=t`, `T`, `p`, `q` (also feature queries with `e=`), `d`, `f`, `a`, `c`.
- Ids: `i`, image numbers `I` (the terminal picks the id and replies `i=…,I=…`), `p`;
  `i` with `I` is `EINVAL`. Images sent without `i`/`I` get hidden ids and no replies.
- Replies `OK` / `ENOENT` / `EINVAL` / `ENODATA` / `EBADPNG` / `EBADF` / `ENOSPC` /
  `ENOPARENT` / `ECYCLE` / `ETOODEEP` / `EFBIG`, suppressed by `q=1` (OK) and `q=2` (all).
- Placement keys `x,y,w,h` (source rectangle), `X,Y` (pixel offset in the cell), `c,r`
  (one given: the other follows the aspect; both: letterboxed), `C=1`, `z`. Several
  `p=0` placements of one image coexist; the same `i`/`p` replaces (and moves its
  relative children with it). The cursor moves right by the columns and down by the
  rows minus one unless `C=1` or the placement is relative.
- z layers: `z < -1,073,741,824` under cell backgrounds, other negative z between
  backgrounds and text, `z >= 0` above text.
- Every delete specifier `a c f i n p q r x y z`, lowercase and uppercase (uppercase
  frees image data left without placements). Virtual placements are only removed by
  `i`/`I`/`n`/`N`/`r`/`R`.
- Relative placements `P`,`Q`,`H`,`V`: chains of up to 9, cycles rejected, children
  deleted with their parent (and their image once it has no placements).
- Unicode placeholders: `U=1` virtual placements and `U+10EEEE` cells with row, column
  and image-id-byte diacritics (omitted ones inferred from the cell to the left), image
  id from the foreground (256-colour or RGB), placement id from the underline colour.
  The image is fitted, aspect kept, into the placement's `c` x `r` box. Relative
  placements may hang off a virtual placement (positioned from its placeholder cells).
- Animation: `a=f` frames (`x`,`y`,`s`,`v` rectangle, base frame `c`, edit frame `r`,
  background `Y`, overwrite `X=1`, gap `z`, gapless negative gaps), `a=a` (`s=1/2/3`,
  `c`, `r`+`z`, loop count `v`), `a=c` composition (`ENOENT`/`EINVAL` as specified) and
  `d=f`. Frames play on the tab's graphics timer.
- Lifecycle: Kitty placements scroll with their text (in a scroll region only when
  entirely inside it), follow the scrollback and are removed with their anchor row,
  scale on resize like Fuzzball placements (§4) if they fitted when placed, and are
  cleared by `ED 2`, `ED 3` (scrollback), leaving or entering the alternate screen and
  RIS. Erasing lines or characters leaves them alone, as the Kitty protocol requires.
- Memory: one 320 MiB quota shared with Fuzzball images; frames beyond the first have a
  separate 320 MiB quota.

Deliberate limits:

- Placeholders are drawn on the live screen only: scrollback lines keep no colours, so a
  placeholder scrolled into the scrollback shows as blank cells.
- Placements scrolled across a scroll-region margin are not clipped (the protocol asks
  for clipping); they keep moving with their anchor and go once it leaves the region.
- A placement wider than the screen when placed is truncated on the right (as the
  protocol says) and is not scaled on resize; one that fitted is scaled like Fuzzball's.
- `f=8`/`f=16` colours are resolved when the image arrives; a later theme change does not
  recolour it.
- Placements relative to a virtual placement are treated as virtual themselves
  (screen-position deletes do not remove them).
- No disk-backed storage for animation frames (kitty's 5x frame quota).
