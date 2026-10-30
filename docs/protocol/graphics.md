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
ESC P = F z g p ; id=<image>,pid=<n>[,cols=<n>,rows=<n>][,sx=,sy=,sw=,sh=][,z=<-1|0|1>][,move=<0|1>] ESC \
```

- Anchored at the **cursor cell**.
- `cols`,`rows`: size in cells. If one is omitted it is derived from the aspect ratio; if
  both are omitted the image's pixel size is rounded up to whole cells using the current
  cell size.
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
- On resize the terminal moves placements with their lines. Terminals that rewrap text
  move the anchor with its cell; terminals that don't (Mouser) keep it in place and
  remove placements whose anchor cell was cut off (right of the last column or below
  the last row). A placement wider than the new screen is clipped, not removed.
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

Mouser does not implement Kitty graphics; these notes explain the design choices.


- One encoding of each concept (no `a=T` shortcut, no `I=` numbers, no `t=f/t/s`).
- Explicit payload length; base64 only in v0.1 (`raw=0`, see README §10).
- Placements are text-grid objects with defined erase/reflow rules.
- Limits are advertised rather than implicit.
