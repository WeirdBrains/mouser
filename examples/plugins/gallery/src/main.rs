//! A gallery mode on host views and media (docs/building-modes.md §Host
//! views and media). The plugin only says what to show: the files in the
//! shell's folder as a [`Hosted::Items`] view, and the one opened as a
//! [`Hosted::Media`] player. Mouser lays them out (grid, list or carousel,
//! `v` switches), loads the thumbnails, moves the selection with the
//! arrows, plays video and sound (fullscreen with `f` or ⤢) and sends back
//! `view.*` and `media.*` events, which this plugin shows in its title.
//!
//! On a Mouser without the `views.1` feature (before 1.6) it says so
//! instead of sending a message that host couldn't read.

use mouser_mode_api::events::{Event, Listener};
use mouser_mode_api::hosted::{FEATURE_MEDIA, FEATURE_VIEWS};
use mouser_mode_api::protocol::{Accepted, ApiRange, Handled, Hello, Provide, Wants};
use mouser_mode_api::{
    Action, Context, Host, Hosted, HostedItem, HostedView, ItemIcon, Mode, Panel, PanelItem, PanelStyle, Rect, ViewLayout, Viewport, serve,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

const PICTURES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];
const MOVING: &[&str] = &["mp4", "m4v", "mkv", "webm", "mov", "avi", "ogv", "mp3", "wav", "flac", "ogg", "oga", "opus", "m4a", "aac"];

#[derive(Default)]
pub struct Gallery {
    ctx: Option<Context>,
    views: bool,
    media: bool,
    dir: Option<PathBuf>,
    files: Vec<PathBuf>,
    selected: usize,
    layout: ViewLayout,
    /// The file opened in the player.
    open: Option<usize>,
    /// What Mouser last told us (shown in the title).
    status: String,
}

fn ext(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase()
}

/// The pictures, videos and sounds in `dir`, by name.
pub fn list(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && (PICTURES.contains(&ext(p).as_str()) || MOVING.contains(&ext(p).as_str())))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl Gallery {
    /// The dock: a title row (drawn by the plugin) over the body Mouser runs.
    pub fn render(&self) -> (Panel, Vec<HostedView>, Wants) {
        let ctx = self.ctx.unwrap_or(Context {
            viewport: Viewport { width: 800, height: 600, cell_w: 8, cell_h: 16 },
            area: Rect { x: 0, y: 0, w: 800, h: 600 },
            reduce_motion: false,
        });
        let (cw, ch) = (ctx.viewport.cell_w.max(1), ctx.viewport.cell_h.max(1));
        let a = ctx.area;
        let rect = Rect { x: a.x + 8, y: a.y + 8, w: a.w - 16, h: a.h - 16 };
        let title = Rect { x: rect.x + cw, y: rect.y + 4, w: rect.w - 2 * cw, h: ch + 4 };
        let body = Rect { x: rect.x + cw, y: title.y + title.h + 6, w: rect.w - 2 * cw, h: rect.h - title.h - 16 };
        let mut items = Vec::new();
        let dir = self.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "(no folder yet)".into());
        let head = format!(
            "Gallery: {dir} · {} files{}",
            self.files.len(),
            if self.status.is_empty() { String::new() } else { format!(" · {}", self.status) }
        );
        items.push(PanelItem { rect: title, text: head, style: PanelStyle::Title, action: Action::None });
        if !self.views {
            let r = Rect { y: body.y, h: ch + 4, ..body };
            let t = "This Mouser can't run views for plugins (needs Mouser 1.6: views.1).";
            items.push(PanelItem { rect: r, text: t.into(), style: PanelStyle::Hint, action: Action::None });
            return (Panel { rect, items }, vec![], Wants::default());
        }
        let mut hosted = Vec::new();
        match self.open.and_then(|i| self.files.get(i)).filter(|_| self.media) {
            Some(p) => {
                let hint = Rect { y: body.y + body.h - ch - 4, h: ch + 4, ..body };
                items.push(PanelItem {
                    rect: hint,
                    text: "esc back to the folder · ⤢ or click: fullscreen".into(),
                    style: PanelStyle::Hint,
                    action: Action::None,
                });
                hosted.push(HostedView {
                    id: "player".into(),
                    rect: Rect { h: body.h - ch - 8, ..body },
                    in_panel: false,
                    content: Hosted::Media { path: p.display().to_string(), autoplay: true },
                });
            }
            None => hosted.push(HostedView {
                id: "files".into(),
                rect: body,
                in_panel: false,
                content: Hosted::Items {
                    layout: self.layout,
                    items: self
                        .files
                        .iter()
                        .map(|p| {
                            let picture = PICTURES.contains(&ext(p).as_str());
                            HostedItem {
                                label: name(p),
                                detail: ext(p),
                                image: picture.then(|| p.display().to_string()),
                                icon: if picture { ItemIcon::None } else { ItemIcon::Badge(1) },
                            }
                        })
                        .collect(),
                    select: Some(self.selected as u32),
                    focused: true,
                    tile: 0,
                },
            }),
        }
        // Escape comes here while the player is open (back to the folder).
        let wants = Wants { text: false, keys: vec![], escape: self.open.is_some() };
        (Panel { rect, items }, hosted, wants)
    }

    fn show(&self, host: &Host) {
        let (dock, hosted, wants) = self.render();
        let _ = host.wants(wants);
        if self.views {
            let _ = host.hosted(hosted);
        }
        let _ = host.view(Some(dock), None, vec![], false);
    }

    /// One event; `true`: redraw.
    pub fn on(&mut self, event: &str, p: &Value) -> bool {
        let index = p["index"].as_u64().unwrap_or(0) as usize;
        match event {
            "shell.cwd_changed" => {
                let dir = PathBuf::from(p["dir"].as_str().unwrap_or_default());
                self.files = list(&dir);
                self.dir = Some(dir);
                (self.selected, self.open) = (0, None);
            }
            "ui.action" if p["id"] == "reload" => {
                if let Some(d) = &self.dir {
                    self.files = list(d);
                }
            }
            "view.selected" => {
                self.selected = index;
                self.status = format!("selected {}", self.files.get(index).map(|p| name(p)).unwrap_or_default());
            }
            "view.activated" => {
                self.selected = index;
                self.open = Some(index);
                self.status = format!("opened {}", self.files.get(index).map(|p| name(p)).unwrap_or_default());
            }
            "view.layout" => {
                self.layout = ViewLayout::from_name(p["layout"].as_str().unwrap_or_default()).unwrap_or_default();
                self.status = format!("layout {}", self.layout.name());
            }
            "media.state" => {
                self.status = if p["fullscreen"] == true {
                    "fullscreen".into()
                } else if p["playing"] == true {
                    "playing".into()
                } else {
                    "paused".into()
                }
            }
            "media.ended" => self.status = "ended".into(),
            "input.escape" if self.open.is_some() => self.open = None,
            "view.resized" => {
                if let (Some(c), Some(a)) = (self.ctx.as_mut(), serde_json::from_value(p["area"].clone()).ok()) {
                    c.area = a;
                }
            }
            "mode.activated" => {}
            _ => return false,
        }
        true
    }
}

impl Mode for Gallery {
    fn hello(&self) -> Hello {
        Hello {
            api: ApiRange::CURRENT,
            name: "gallery".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            provides: vec![Provide::Mode { hotkey: 'g', bindings: vec![], activate_on: vec![] }],
            listeners: ["mode.*", "ui.action", "input.escape", "view.*", "media.*", "shell.cwd_changed"].iter().map(|e| Listener::on(e)).collect(),
            emits: vec![],
            wants_mcp: false,
            shm: false,
        }
    }

    fn welcome(&mut self, w: &Accepted, host: &Host) {
        self.ctx = Some(w.ctx);
        // What this Mouser runs for plugins (none before 1.6).
        (self.views, self.media) = (w.has(FEATURE_VIEWS), w.has(FEATURE_MEDIA));
        self.show(host);
    }

    fn event(&mut self, ev: &Event, host: &Host) -> Handled {
        if self.on(&ev.name, &ev.payload) {
            self.show(host);
        }
        Handled::Handled
    }
}

fn main() {
    if let Err(e) = serve(Gallery::default()) {
        eprintln!("mouser-gallery: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_mouser_gets_a_notice_and_no_hosted_views() {
        let g = Gallery::default();
        let (dock, hosted, _) = g.render();
        assert!(hosted.is_empty());
        assert!(dock.items.iter().any(|i| i.text.contains("views.1")));
    }

    #[test]
    fn events_drive_the_selection_and_the_player() {
        let d = tempfile::tempdir().unwrap();
        for f in ["b.png", "a.mp4", "notes.txt"] {
            std::fs::write(d.path().join(f), b"x").unwrap();
        }
        let mut g = Gallery { views: true, media: true, ..Default::default() };
        g.on("shell.cwd_changed", &serde_json::json!({"dir": d.path().display().to_string()}));
        assert_eq!(g.files.iter().map(|p| name(p)).collect::<Vec<_>>(), ["a.mp4", "b.png"]);
        let (_, h, w) = g.render();
        let Hosted::Items { items, focused: true, .. } = &h[0].content else { panic!("{h:?}") };
        assert_eq!((items[0].image.is_none(), items[1].image.is_some()), (true, true));
        assert!(!w.escape);
        g.on("view.activated", &serde_json::json!({"id": "files", "index": 0}));
        let (_, h, w) = g.render();
        assert!(matches!(&h[0].content, Hosted::Media { path, autoplay: true } if path.ends_with("a.mp4")));
        assert!(w.escape);
        g.on("input.escape", &serde_json::json!({}));
        assert!(g.open.is_none());
    }
}
