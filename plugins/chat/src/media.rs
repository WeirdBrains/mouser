//! Media in the chat: pictures, video and sound named in a message or a
//! tool's output (`mouser_media::detect`) get a player under it. A picture
//! shows as a thumbnail; video and sound get play / pause and a progress
//! bar (Mouser's player: ffmpeg + cpal, `mouser_media::player`). Any of them
//! opens fullscreen within the chat's space with the same ease-in-out flight
//! and dimmed backdrop as commander's viewer (`mouser_media::anim`).
//!
//! Nothing here knows about the chat: it draws into a rect and answers
//! `media_*` actions, so it can move into `mouser-media` as a view any mode
//! can use.

use mouser_media::Waker;
use mouser_media::anim::FsAnim;
use mouser_media::detect::{Kind, MediaRef};
use mouser_media::player::{self, Player};
use mouser_media::still::{Slot, Stills, fit};
use mouser_mode_api::{Action, PanelItem, PanelStyle, Rect};
use std::sync::Arc;
use std::time::Instant;

/// Rows a picture or video takes under a message.
pub const PICTURE_ROWS: usize = 9;
/// Rows a sound takes (name, controls).
pub const SOUND_ROWS: usize = 2;

/// A picture to send with the view.
#[derive(Clone)]
pub struct Pic {
    pub key: u64,
    pub rgba: Arc<Vec<u8>>,
    pub w: u32,
    pub h: u32,
    pub rect: Rect,
}

#[derive(Default)]
pub struct MediaView {
    pub stills: Stills,
    pub player: Option<Player>,
    /// Open fullscreen.
    pub full: Option<MediaRef>,
    pub anim: Option<FsAnim>,
    pub waker: Option<Waker>,
    pub reduce_motion: bool,
    /// The media drawn this frame (action args index it) and where.
    pub shown: Vec<(MediaRef, Rect)>,
}

fn act(id: &str, arg: i32) -> Action {
    Action::Mode { action: id.into(), arg }
}

fn item(rect: Rect, text: impl Into<String>, style: PanelStyle, action: Action) -> PanelItem {
    PanelItem { rect, text: text.into(), style, action }
}

fn name(m: &MediaRef) -> String {
    m.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| m.path.display().to_string())
}

pub fn rows(kind: Kind) -> usize {
    match kind {
        Kind::Image | Kind::Video => PICTURE_ROWS,
        Kind::Audio => SOUND_ROWS,
    }
}

impl MediaView {
    /// Start a frame: forget what was drawn.
    pub fn begin(&mut self) {
        self.shown.clear();
    }

    pub fn is_open(&self) -> bool {
        self.full.is_some()
    }

    fn playing_this(&self, m: &MediaRef) -> Option<&Player> {
        self.player.as_ref().filter(|p| p.path == m.path)
    }

    fn index(&mut self, m: &MediaRef, r: Rect) -> i32 {
        self.shown.push((m.clone(), r));
        self.shown.len() as i32 - 1
    }

    /// Play `m` (or pause it if it is playing), opening it first. `width`:
    /// how wide video is decoded.
    fn play(&mut self, m: &MediaRef, width: i32) {
        if let Some(p) = self.player.as_mut().filter(|p| p.path == m.path) {
            p.toggle();
            return;
        }
        if !player::available() {
            self.player = None;
            return;
        }
        let mut p = Player::open(&m.path, width.clamp(320, 1280) as u32, self.waker.clone());
        p.play();
        self.player = Some(p);
    }

    fn wake(&self) {
        if let Some(w) = &self.waker {
            w()
        }
    }

    /// Open `m` fullscreen in `area`, flying from where it was drawn.
    fn open_full(&mut self, i: usize, area: Rect) {
        let Some((m, from)) = self.shown.get(i).cloned() else { return };
        if m.kind != Kind::Image && self.playing_this(&m).is_none() {
            self.play(&m, area.w);
        }
        self.full = Some(m);
        if !self.reduce_motion {
            let mut a = FsAnim::new(true, from, area);
            a.keep_redrawing(self.waker.clone());
            self.anim = Some(a);
        }
        self.wake();
    }

    /// Close the fullscreen view (flying back when the thumbnail is known).
    pub fn close(&mut self, area: Rect) {
        let Some(m) = self.full.clone() else { return };
        let back = self.shown.iter().find(|(s, _)| *s == m).map(|(_, r)| *r);
        match back.filter(|_| !self.reduce_motion) {
            Some(to) => {
                let mut a = FsAnim::new(false, area, to);
                a.keep_redrawing(self.waker.clone());
                self.anim = Some(a);
            }
            None => {
                self.full = None;
                self.anim = None;
            }
        }
        self.wake();
    }

    /// A `media_*` action; `area`: the chat's space (fullscreen).
    pub fn action(&mut self, id: &str, arg: i32, area: Rect) -> bool {
        let i = arg.max(0) as usize;
        match id {
            "media_play" => {
                if let Some((m, _)) = self.shown.get(i).cloned() {
                    self.play(&m, area.w);
                }
            }
            "media_full" => {
                if self.full.is_some() {
                    self.close(area);
                } else {
                    self.open_full(i, area);
                }
            }
            "media_close" => self.close(area),
            "media_toggle" => {
                if let Some(p) = self.player.as_mut() {
                    p.toggle()
                }
            }
            "media_back" => {
                if let Some(p) = self.player.as_mut() {
                    p.seek(-10.0)
                }
            }
            "media_fwd" => {
                if let Some(p) = self.player.as_mut() {
                    p.seek(10.0)
                }
            }
            "media_stop" => self.player = None,
            _ => return false,
        }
        true
    }

    /// Keys while fullscreen: space plays / pauses, ←/→ seek 10 s, f or
    /// Enter closes. `true`: taken.
    pub fn key(&mut self, s: &str, area: Rect) -> bool {
        if self.full.is_none() {
            return false;
        }
        match s {
            " " | "k" => self.action("media_toggle", 0, area),
            "\x1b[D" | "\x1bOD" | "j" => self.action("media_back", 0, area),
            "\x1b[C" | "\x1bOC" | "l" => self.action("media_fwd", 0, area),
            "f" | "q" | "\r" | "\n" => self.action("media_close", 0, area),
            _ => true,
        };
        true
    }

    /// Draw `m` inline in `r` (its rows under a message).
    pub fn inline(&mut self, m: &MediaRef, r: Rect, (cw, row_h): (i32, i32), out: &mut Vec<PanelItem>, pics: &mut Vec<Pic>) {
        let n = name(m);
        let i = self.index(m, r);
        match m.kind {
            Kind::Image => match self.stills.get(&m.path, self.waker.clone()) {
                Slot::Ready(s) => {
                    let pr = fit(Rect { x: r.x, y: r.y, w: r.w.min(cw * 60), h: r.h }, s.w, s.h, false);
                    let pr = Rect { x: r.x, ..pr };
                    self.shown[i as usize].1 = pr;
                    // Clicking the picture opens it fullscreen.
                    out.push(item(pr, "", PanelStyle::Dim(0), act("media_full", i)));
                    if self.full.is_none() {
                        pics.push(Pic { key: s.key, rgba: s.rgba, w: s.w, h: s.h, rect: pr });
                    }
                }
                Slot::Loading => out.push(item(Rect { h: row_h, ..r }, format!("🖼 {n} …"), PanelStyle::Hint, Action::None)),
                Slot::Failed(e) => out.push(item(Rect { h: row_h, ..r }, format!("🖼 {e}"), PanelStyle::Error, Action::None)),
            },
            Kind::Video => {
                let pic = Rect { x: r.x, y: r.y, w: r.w.min(cw * 60), h: r.h - row_h - 2 };
                self.shown[i as usize].1 = pic;
                let frame = self.player.as_mut().filter(|p| p.path == m.path).and_then(|p| {
                    p.tick();
                    p.frame()
                });
                match frame {
                    Some(f) if self.full.is_none() => {
                        let fr = fit(pic, f.w, f.h, true);
                        let fr = Rect { x: pic.x, ..fr };
                        out.push(item(fr, "", PanelStyle::Dim(0), act("media_full", i)));
                        pics.push(Pic { key: f.key, rgba: f.rgba, w: f.w, h: f.h, rect: fr });
                    }
                    _ => out.push(item(pic, format!("▶  {n}"), PanelStyle::Bubble { mine: false }, act("media_play", i))),
                }
                self.controls(m, i, Rect { x: r.x, y: r.y + r.h - row_h, w: pic.w, h: row_h }, cw, out);
            }
            Kind::Audio => {
                out.push(item(Rect { h: row_h, ..r }, format!("♪ {n}"), PanelStyle::Label, act("media_play", i)));
                let w = r.w.min(cw * 60);
                self.shown[i as usize].1 = Rect { w, ..r };
                self.controls(m, i, Rect { x: r.x, y: r.y + row_h, w, h: row_h }, cw, out);
            }
        }
    }

    /// ▶ / ❚❚, the time and progress, ⤢.
    fn controls(&mut self, m: &MediaRef, i: i32, r: Rect, cw: i32, out: &mut Vec<PanelItem>) {
        let p = self.playing_this(m);
        let label = if p.is_some_and(|p| p.playing()) { "❚❚" } else { "▶" };
        let bw = 4 * cw;
        out.push(item(Rect { w: bw, ..r }, label, PanelStyle::Button { primary: true }, act("media_play", i)));
        let fw = 4 * cw;
        out.push(item(Rect { x: r.x + r.w - fw, w: fw, ..r }, "⤢", PanelStyle::Button { primary: false }, act("media_full", i)));
        if !player::available() {
            out.push(item(Rect { x: r.x + bw + cw, w: r.w - bw - fw - 2 * cw, ..r }, "needs ffmpeg and ffprobe", PanelStyle::Hint, Action::None));
            return;
        }
        let (pos, dur, err) = p.map_or((0.0, 0.0, None), |p| (p.position(), p.info.duration, p.error.clone()));
        let t = format!("{} / {}", player::clock(pos), player::clock(dur));
        let tw = (t.chars().count() as i32 + 1) * cw;
        let x = r.x + bw + cw;
        out.push(item(Rect { x, w: tw, ..r }, t, PanelStyle::Label, Action::None));
        let bar = Rect { x: x + tw, y: r.y + 3, w: (r.w - bw - fw - tw - 3 * cw).max(cw), h: r.h - 6 };
        let done = if dur > 0.0 { ((pos / dur).clamp(0.0, 1.0) * 1000.0) as u16 } else { 0 };
        match err {
            Some(e) if p.is_some_and(|p| p.info.duration == 0.0) => out.push(item(bar, e, PanelStyle::Error, Action::None)),
            _ => out.push(item(bar, "", PanelStyle::Progress { done, hover: u16::MAX }, Action::None)),
        }
    }

    /// The fullscreen view over `area`, if open. Returns `true` when drawn.
    pub fn fullscreen(&mut self, area: Rect, (cw, row_h): (i32, i32), out: &mut Vec<PanelItem>, pics: &mut Vec<Pic>) -> bool {
        let Some(m) = self.full.clone() else { return false };
        let now = Instant::now();
        let (veil, flying) = match &self.anim {
            Some(a) if a.finished(now) => {
                let closing = !a.opening;
                self.anim = None;
                if closing {
                    self.full = None;
                    return false;
                }
                (200, None)
            }
            Some(a) => (a.veil(now), Some(a.rect(now))),
            None => (200, None),
        };
        // The backdrop takes the clicks (closing) so nothing under it is hit.
        out.push(item(area, "", PanelStyle::Dim(veil), act("media_close", 0)));
        let title = Rect { x: area.x + cw, y: area.y + 4, w: area.w - 2 * cw, h: row_h };
        let ctl = Rect { x: area.x + cw, y: area.y + area.h - row_h - 6, w: area.w - 2 * cw, h: row_h };
        let body = match m.kind {
            Kind::Image => Rect { x: area.x + cw, y: title.y + row_h + 4, w: area.w - 2 * cw, h: area.h - row_h - 12 },
            _ => Rect { x: area.x + cw, y: title.y + row_h + 4, w: area.w - 2 * cw, h: ctl.y - title.y - row_h - 8 },
        };
        if flying.is_none() {
            let cbw = 12 * cw;
            out.push(item(Rect { w: title.w - cbw - cw, ..title }, name(&m), PanelStyle::Title, Action::None));
            out.push(item(
                Rect { x: title.x + title.w - cbw, w: cbw, ..title },
                "⤡ close esc",
                PanelStyle::Button { primary: false },
                act("media_close", 0),
            ));
        }
        let place = |w: u32, h: u32| flying.map_or_else(|| fit(body, w, h, true), |r| fit(r, w, h, true));
        match m.kind {
            Kind::Image => match self.stills.get(&m.path, self.waker.clone()) {
                Slot::Ready(s) => {
                    let r = place(s.w, s.h);
                    out.push(item(r, "", PanelStyle::Dim(0), act("media_close", 0)));
                    pics.push(Pic { key: s.key, rgba: s.rgba, w: s.w, h: s.h, rect: r });
                }
                Slot::Loading => out.push(item(Rect { h: row_h, ..body }, "loading…", PanelStyle::Hint, Action::None)),
                Slot::Failed(e) => out.push(item(Rect { h: row_h, ..body }, e, PanelStyle::Error, Action::None)),
            },
            Kind::Video => {
                let f = self.player.as_mut().filter(|p| p.path == m.path).and_then(|p| {
                    p.tick();
                    p.frame()
                });
                match f {
                    Some(f) => {
                        let r = place(f.w, f.h);
                        out.push(item(r, "", PanelStyle::Dim(0), act("media_toggle", 0)));
                        pics.push(Pic { key: f.key, rgba: f.rgba, w: f.w, h: f.h, rect: r });
                    }
                    None => out.push(item(Rect { h: row_h, ..body }, "loading…", PanelStyle::Hint, Action::None)),
                }
            }
            Kind::Audio => {
                if let Some(p) = self.player.as_mut() {
                    p.tick();
                }
                let r = Rect { x: body.x, y: body.y + body.h / 2 - row_h, w: body.w, h: row_h * 2 };
                out.push(item(r, format!("♪  {}", name(&m)), PanelStyle::Title, act("media_toggle", 0)));
            }
        }
        if flying.is_none() && m.kind != Kind::Image {
            let i = self.index(&m, body);
            // « 10s and 10s » beside the usual controls.
            let sw = 7 * cw;
            out.push(item(Rect { w: sw, ..ctl }, "« 10s", PanelStyle::Button { primary: false }, act("media_back", 0)));
            out.push(item(Rect { x: ctl.x + ctl.w - sw, w: sw, ..ctl }, "10s »", PanelStyle::Button { primary: false }, act("media_fwd", 0)));
            self.controls(&m, i, Rect { x: ctl.x + sw + cw, w: ctl.w - 2 * (sw + cw), ..ctl }, cw, out);
            // In fullscreen ⤢ closes.
            if let Some(b) = out.iter_mut().rev().find(|b| b.text == "⤢") {
                b.text = "⤡".into();
                b.action = act("media_close", 0);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn png(dir: &std::path::Path) -> MediaRef {
        let p = dir.join("cat.png");
        image::RgbaImage::from_pixel(64, 32, image::Rgba([0, 128, 255, 255])).save(&p).unwrap();
        MediaRef { path: p, kind: Kind::Image }
    }

    fn wait_ready(v: &MediaView, m: &MediaRef) {
        for _ in 0..500 {
            if matches!(v.stills.get(&m.path, None), Slot::Ready(_)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("not decoded");
    }

    #[test]
    fn picture_inline_then_fullscreen_and_back() {
        let d = tempfile::tempdir().unwrap();
        let m = png(d.path());
        let mut v = MediaView { reduce_motion: true, ..MediaView::default() };
        let (mut out, mut pics) = (Vec::new(), Vec::new());
        let r = Rect { x: 10, y: 100, w: 400, h: 9 * 20 };
        v.inline(&m, r, (8, 20), &mut out, &mut pics);
        assert!(out[0].text.contains("cat.png"), "loading first");
        wait_ready(&v, &m);
        v.begin();
        (out, pics) = (Vec::new(), Vec::new());
        v.inline(&m, r, (8, 20), &mut out, &mut pics);
        assert_eq!(pics.len(), 1);
        assert_eq!((pics[0].rect.w, pics[0].rect.h), (64, 32), "never blown up inline");
        let click = out.iter().find(|i| i.action == act("media_full", 0)).expect("click opens fullscreen");
        assert_eq!(click.rect, pics[0].rect);
        let area = Rect { x: 0, y: 0, w: 1000, h: 700 };
        assert!(v.action("media_full", 0, area));
        assert!(v.is_open());
        (out, pics) = (Vec::new(), Vec::new());
        assert!(v.fullscreen(area, (8, 20), &mut out, &mut pics));
        assert!(matches!(out[0].style, PanelStyle::Dim(200)));
        assert!(pics[0].rect.w > 400, "fills the chat's space: {:?}", pics[0].rect);
        assert!(v.key("f", area));
        assert!(!v.is_open(), "reduced motion: closes at once");
    }

    #[test]
    fn opening_flies_from_the_thumbnail_with_the_ease() {
        let d = tempfile::tempdir().unwrap();
        let m = png(d.path());
        let mut v = MediaView::default();
        v.shown.push((m.clone(), Rect { x: 10, y: 100, w: 64, h: 32 }));
        let area = Rect { x: 0, y: 0, w: 1000, h: 700 };
        v.action("media_full", 0, area);
        let a = v.anim.expect("animated");
        assert!(a.opening && a.from.w == 64 && a.to == area);
        assert!(v.anim.as_ref().unwrap().veil(a.start) < 10);
        v.anim.as_mut().unwrap().start -= FsAnim::DUR * 2;
        let (mut out, mut pics) = (Vec::new(), Vec::new());
        v.fullscreen(area, (8, 20), &mut out, &mut pics);
        assert!(v.anim.is_none() && v.is_open(), "the open ends");
        v.close(area);
        assert!(v.anim.is_some_and(|a| !a.opening), "flies back");
        v.anim.as_mut().unwrap().start -= FsAnim::DUR * 2;
        v.fullscreen(area, (8, 20), &mut out, &mut pics);
        assert!(!v.is_open());
    }

    #[test]
    fn sound_has_controls() {
        let mut v = MediaView::default();
        let m = MediaRef { path: PathBuf::from("/nonexistent/a.mp3"), kind: Kind::Audio };
        let (mut out, mut pics) = (Vec::new(), Vec::new());
        v.inline(&m, Rect { x: 0, y: 0, w: 600, h: 40 }, (8, 20), &mut out, &mut pics);
        assert!(out.iter().any(|i| i.text == "♪ a.mp3"));
        assert!(out.iter().any(|i| i.text == "▶" && i.action == act("media_play", 0)));
        assert!(out.iter().any(|i| i.text == "⤢"));
        assert!(pics.is_empty());
    }
}
