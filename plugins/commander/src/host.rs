//! The in-process mode shape the commander view was written against (it was
//! `mouser_render::modes::Mode`). `main.rs` adapts it to the mode protocol:
//! events become these calls, and every `view()` is sent to Mouser.

pub use mouser_mode_api::ui::{PanelStyle, Rect};
pub use mouser_mode_api::{Context, Effect};
use std::path::Path;
use std::sync::Arc;

pub type UiAction = mouser_mode_api::Action;
pub type UiViewport = mouser_mode_api::Viewport;
pub type Panel = mouser_mode_api::ui::Panel<UiAction>;
pub type PanelItem = mouser_mode_api::ui::PanelItem<UiAction>;
pub type Waker = Arc<dyn Fn() + Send + Sync>;
pub use crate::modes::{Binding, Keymap};

/// Gap between the terminal area's edge and the view (Mouser's frame padding).
pub const PADDING: i32 = 8;
/// Mouser's button padding and dock corner radius (the view lines up with them).
pub const BUTTON_PAD: i32 = 6;
pub const RADIUS: i32 = 8;

/// Pixels to show inside `rect` (fitted, aspect kept).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeImage {
    pub key: u64,
    pub rgba: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    /// Part of the image shown, in image pixels (x, y, w, h).
    pub src: (u32, u32, u32, u32),
    pub rect: Rect,
}

/// A mode's contribution to the frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    /// Docked above the prompt.
    pub dock: Option<Panel>,
    /// A floating window (options, manual ...).
    pub panel: Option<Panel>,
    pub images: Vec<ModeImage>,
    /// The dock fills the whole window (commander's fullscreen media): no
    /// grid, tab bar or sidebar is drawn, and the terminal keeps its size.
    pub fullscreen: bool,
}

pub trait Mode: Send {
    fn name(&self) -> &'static str;
    fn hotkey(&self) -> char;
    fn bindings(&self) -> &'static [Binding];
    fn activate(&mut self) {}
    fn deactivate(&mut self) {}
    fn cwd_changed(&mut self, _dir: &Path) {}
    fn remote_changed(&mut self, _label: &str, _dir: Option<&Path>, _listing: Option<Result<Vec<crate::commander::Entry>, String>>) {}
    fn remote_ssh(&mut self, _ssh: Option<&crate::remote::SshTarget>) {}
    fn action(&mut self, id: &str, arg: i32) -> Effect;
    fn text(&mut self, _bytes: &[u8]) -> Option<Effect> {
        None
    }
    fn wants_key(&self, _bytes: &[u8]) -> bool {
        false
    }
    fn wants_text(&self) -> bool {
        false
    }
    fn escape(&mut self) -> bool {
        false
    }
    fn view(&mut self, ctx: &Context, keys: &Keymap) -> View;
    fn set_waker(&mut self, _w: Waker) {}
}

pub fn list_panel(vp: UiViewport, title: &str, lines: &[(String, String)], action: impl Fn(usize) -> UiAction) -> Panel {
    let (cw, ch) = (vp.cell_w, vp.cell_h);
    let row_h = ch + 6;
    let left = lines.iter().map(|l| l.0.chars().count()).max().unwrap_or(0) as i32 + 2;
    let right = lines.iter().map(|l| l.1.chars().count()).max().unwrap_or(0) as i32;
    let pad = 14;
    let w = ((left + right) * cw + 2 * pad).max(cw * 30).min(vp.width - 16);
    let h = (row_h * (lines.len() as i32 + 2) + 2 * pad).min(vp.height - 16);
    let rect = Rect { x: (vp.width - w) / 2, y: (vp.height - h) / 2, w, h };
    let (x, mut y) = (rect.x + pad, rect.y + pad);
    let mut items =
        vec![PanelItem { rect: Rect { x, y, w: w - 2 * pad, h: row_h }, text: title.into(), style: PanelStyle::Title, action: UiAction::None }];
    y += row_h + 6;
    for (i, (a, b)) in lines.iter().enumerate() {
        let act = action(i);
        let style = if act == UiAction::None { PanelStyle::Label } else { PanelStyle::Row { selected: false } };
        items.push(PanelItem { rect: Rect { x, y, w: left * cw, h: row_h }, text: a.clone(), style, action: act.clone() });
        items.push(PanelItem {
            rect: Rect { x: x + left * cw, y, w: (w - 2 * pad - left * cw).max(0), h: row_h },
            text: b.clone(),
            style: PanelStyle::Hint,
            action: act,
        });
        y += row_h;
        if y > rect.y + h - row_h {
            break;
        }
    }
    Panel { rect, items }
}

pub fn fit(r: Rect, w: u32, h: u32) -> Rect {
    if w == 0 || h == 0 || r.w <= 0 || r.h <= 0 {
        return Rect { w: 0, h: 0, ..r };
    }
    let s = (r.w as f64 / w as f64).min(r.h as f64 / h as f64);
    let (fw, fh) = (((w as f64 * s) as i32).max(1), ((h as f64 * s) as i32).max(1));
    Rect { x: r.x + (r.w - fw) / 2, y: r.y + (r.h - fh) / 2, w: fw, h: fh }
}
