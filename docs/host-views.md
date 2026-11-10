# Host views and media for modes

A mode plugin can have Mouser run its views and players. The plugin says
*what* to show: a list of items, or a picture, video or sound file. Mouser then
does the rest:

- lays the items out as a list, grid or carousel;
- loads the thumbnails;
- moves the selection with the keys and the mouse;
- plays video and sound, inline and fullscreen;
- redraws while something animates or plays;
- tells the plugin what happened.

The plugin needs no drawing, decoding or player code, so a mode written in any
language gets the same views commander and chat have.

This needs **Mouser 1.6** or later and **mouser-mode-api 0.4** (protocol api 1,
unchanged). Older Mouser versions don't offer it; see [Supporting older
Mouser](#supporting-older-mouser).

## In short

1. In `welcome`, check `accepted.has(FEATURE_VIEWS)` / `has(FEATURE_MEDIA)`.
2. Send your own panel as usual (`Host::view`): titles, buttons, hints.
3. Send the views Mouser runs (`Host::hosted`): one `HostedView` per area,
   each with a stable `id` and a `rect` inside your dock or panel
   (`offscreen: true` keeps one registered without drawing it).
4. Listen for `view.*` and `media.*` events and update your state. Send
   `hosted` again whenever what you show changes; the latest list wins.

```rust
use mouser_mode_api::hosted::{FEATURE_VIEWS, Hosted, HostedItem, HostedView, ViewLayout};

let files = HostedView {
    id: "files".into(),
    rect: body, // window pixels, inside your dock
    in_panel: false,
    content: Hosted::Items {
        layout: ViewLayout::Grid,
        items: vec![HostedItem { label: "cat.png".into(), image: Some("/home/me/cat.png".into()), ..Default::default() }],
        select: None,  // keep the user's selection
        focused: true, // this view takes the arrow keys
        tile: 0,       // Mouser picks the tile size
    },
    offscreen: false,
};
host.hosted(vec![files])?;
host.view(Some(my_panel), None, vec![], false)?;
```

`examples/plugins/gallery` is a complete plugin (about 250 lines, mouser-mode-api
only): the pictures, videos and sounds in the shell's folder, shown in a grid,
with a player for the one you open. Its test
(`examples/plugins/gallery/tests/`) runs the real program under Mouser's
host. It checks that the tiles and thumbnails are drawn, that the arrows,
`v` and clicks work, and that Enter, fullscreen and Esc do what they should.
It plays a real clip through the host player, to the end. It checks that
taking back `files.read` stops the reads, that a restarted plugin registers
its views again, and that a host without the features gets no `Hosted`.

## Item views

`Hosted::Items { layout, items, select, focused, tile }`

| Field | Meaning |
|---|---|
| `layout` | `List`, `Grid` or `Carousel`. The user's `v` key cycles it; Mouser keeps their choice until you send a different `layout`. |
| `items` | `HostedItem { label, detail, image, icon }`. `detail` is a second column in the list; `image` is a picture file to use as the thumbnail; `icon` is `Folder` or `Badge(n)` when there is no picture. |
| `select` | Move the selection here. It only applies when the value changes from the one you sent last time, so the user's own moves stick while you resend the view. `None` keeps the selection. |
| `focused` | This view takes the navigation keys (if several are focused, the last one wins). |
| `tile` | The grid tile side, or the carousel item height, in pixels (`0`: Mouser picks). |

Keys while the mode has the keyboard and the view is focused:

| Key | List | Grid | Carousel |
|---|---|---|---|
| ↑ / ↓ | previous / next | a row up / down | — |
| ← / → | — | previous / next | previous / next |
| Page Up / Page Down | a page | a page | five items |
| Home / End | first / last | first / last | first / last |
| Enter | activate | activate | activate |
| `v` | next layout | next layout | next layout |

A click selects an item, and a click on the selected item activates it. Keys
the view doesn't use still go to your bindings. If one of your text fields has
focus (a `Field`, or `Wants.text`), the typing goes to you, not to the view.

Events (listen for `view.*`):

- `view.selected` `{id, index}`: the selection moved.
- `view.activated` `{id, index}`: Enter, or a second click.
- `view.layout` `{id, layout}`: the user switched layout (`"list"`,
  `"grid"` or `"carousel"`); store it if you want it to survive restarts.

## Media

`Hosted::Media { path, autoplay }`: one picture, video or sound file, drawn in
`rect`.

| File | Shown as |
|---|---|
| Picture | Its thumbnail. A click opens it fullscreen. |
| Video | A frame, with play / pause, the time, a progress bar and ⤢ fullscreen. |
| Sound | Its name, with the same controls as a video. |

Fullscreen fills your dock (or panel, with `in_panel`). Its keys: space plays
or pauses, ← / → seek 10 s, `f`, `q` or Enter close it, and Esc closes it
before any Escape of yours. `autoplay` starts video and sound the first time
the view appears. A player stops when you stop sending its view, and pauses
when your mode is switched away.

To keep something without drawing it, send its view with `offscreen: true`.
A clip that scrolled out of a transcript keeps playing that way (chat does
this for the clip that is playing), and an item view keeps its selection and
scroll. An off-screen view takes no keys and draws nothing; send it again
with `offscreen: false` to show it where it was.

Video and sound use ffmpeg and ffprobe from `PATH`; without them the player
says so. Mouser plays one file at a time.

Events (listen for `media.*`):

- `media.state` `{id, playing, position, duration, fullscreen, error}`: sent
  when the player starts, pauses, seeks or opens / closes fullscreen, and
  about once a second while it plays (`position` in whole seconds).
- `media.ended` `{id}`: playback reached the end (the player stopped by
  itself, not because the user or a mode switch paused it).

## Permissions

The paths are on the user's computer, so thumbnails and media need
`files.read` in your `plugin.toml`, granted by the user. Without it Mouser
reads nothing: an item has no thumbnail, and a media view shows its name with
"(needs files.read)". Mouser reads the files itself, so the plugin never sees
their contents.

`view.*` and `media.*` are host events: a plugin can't emit them.

## Supporting older Mouser

Mouser before 1.6 offers no features (`accepted.features` is empty) and can't
read `Hosted` messages: sending one makes that host stop reading anything
more from your plugin.
Always check first:

```rust
fn welcome(&mut self, w: &Accepted, host: &Host) {
    self.hosted = w.has(mouser_mode_api::hosted::FEATURE_VIEWS);
}
```

On an older Mouser, either draw the view yourself (the `mouser-views` and
`mouser-media` crates do what the host does, inside your plugin;
docs/building-modes.md §Views and media) or say that the mode needs Mouser
1.6, as the gallery example does. A plugin built on mouser-mode-api 0.3 keeps
working on Mouser 1.6: nothing changes for it.

## Limits

- Three view kinds: items (list, grid, carousel) and media. Anything else is
  still drawn by your plugin with panel items and images.
- One player at a time, fullscreen within your dock or panel. Commander's
  viewer, with zoom, volume, scrubbing and whole-window fullscreen, is its
  own code (on `mouser-media`).
- Paths are local files. Remote files over SSH need to be fetched first.
