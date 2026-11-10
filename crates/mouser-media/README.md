# mouser-media

Media for [Mouser](https://github.com/WeirdBrains/mouser) mode plugins:

- `player::Player`: video and audio with ffmpeg / ffprobe (found on `PATH`)
  and cpal, frames sent as images through the mode protocol;
- `audio`: the output device (`[audio]` in Mouser's preferences.toml);
- `still::Stills`: pictures decoded off the UI thread;
- `detect::find`: media named in text (Markdown images and links, bare paths);
- `anim::FsAnim`: the 220 ms ease-in-out flight to fullscreen and back;
- `view::MediaView`: inline players with fullscreen any mode can draw.

Mouser's commander and chat modes use it. Linux needs the ALSA headers
(`libasound2-dev`) to build; the `jack` feature adds JACK output.

```toml
[dependencies]
mouser-media = "0.2"
```

See [Building a mode plugin §Views and media](https://github.com/WeirdBrains/mouser/blob/main/docs/building-modes.md#views-and-media).
License: BSD-3-Clause.
