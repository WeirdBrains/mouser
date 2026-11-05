//! The UI model shared by Mouser and mode plugins: plain data a mode lays
//! out and the host draws.

/// A rectangle in window pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// Pixel geometry of the window, so a view can lay out in real cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Viewport {
    pub width: i32,
    pub height: i32,
    pub cell_w: i32,
    pub cell_h: i32,
}

/// How the host draws one panel item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PanelStyle {
    Title,
    Label,
    Hint,
    Error,
    /// Selectable list row.
    Row {
        selected: bool,
    },
    Button {
        primary: bool,
    },
    /// Text input; `valid` false draws it in the error colour.
    Field {
        focused: bool,
        valid: bool,
    },
    /// Colour sample.
    Swatch(u32),
    /// Framed tile (commander grid / carousel item).
    Tile {
        selected: bool,
    },
    /// Background of a dropdown drawn over the panel.
    Popup,
    /// Name tag drawn on top of everything, previews included (commander
    /// carousel): a slightly transparent rounded box with a soft shadow.
    FloatLabel,
    /// Folder icon filling the rect (commander carousel).
    FolderIcon,
    /// Type badge (`mouser_core::commander::Badge` index) on a grid tile or
    /// carousel item, drawn on top like [`PanelStyle::FloatLabel`].
    Badge(u8),
    /// Seek / volume bar: `done` and the hover mark (`u16::MAX`: none) in 1/1000.
    Progress {
        done: u16,
        hover: u16,
    },
    /// Scroll position: a slim track with a thumb from `start` for `len`
    /// (1/1000 of the content). Vertical when taller than wide.
    Scrollbar {
        start: u16,
        len: u16,
    },
    /// Dropdown row; bit `i` of `highlight` marks char `i` as matched.
    Option {
        selected: bool,
        highlight: u64,
    },
    /// A markdown text run (`mouser_core::markdown` span flags; `heading`
    /// level, 0 for body text).
    Md {
        flags: u8,
        heading: u8,
    },
    /// Markdown code block background, rounded at the block's `top` and `bottom`.
    MdCode {
        top: bool,
        bottom: bool,
    },
    /// A highlighted run in a Markdown code block: its token class
    /// (`mouser_core::highlight`), coloured from the theme's palette.
    MdToken(u8),
    /// Markdown block quote bar.
    MdQuote,
    /// Markdown thematic break.
    MdRule,
}

/// What clicking a panel item a mode drew does. Mouser's own windows use a
/// wider action set (`mouser_render::ui::UiAction`); a mode only ever gets
/// its own actions back, as `ui.action` events.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Action {
    #[default]
    None,
    /// The mode's own action id and an argument (e.g. an item index).
    Mode { action: String, arg: i32 },
}

/// One drawn item: a rectangle, its text, how it looks and what a click does.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PanelItem<A = Action> {
    pub rect: Rect,
    pub text: String,
    pub style: PanelStyle,
    pub action: A,
}

/// A dock or floating window: a frame and its items, drawn in order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Panel<A = Action> {
    pub rect: Rect,
    pub items: Vec<PanelItem<A>>,
}

impl<A> Panel<A> {
    /// The same panel with each item's action converted (a plugin's
    /// [`Action`]s into the host's action type).
    pub fn map<B>(self, mut f: impl FnMut(A) -> B) -> Panel<B> {
        Panel {
            rect: self.rect,
            items: self.items.into_iter().map(|i| PanelItem { rect: i.rect, text: i.text, style: i.style, action: f(i.action) }).collect(),
        }
    }
}

/// The dock height the user chose (dragging the dock's bottom edge, the
/// host's taller / shorter keys, or its fullscreen toggle). Mouser sends it as
/// `dock` in the `view.resized` payload (and again whenever it changes);
/// a mode lays its dock out with [`DockSize::height`]. Older modes ignore it
/// and keep their own height.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DockSize {
    /// Share of the terminal area in 1/1000 (`None`: the mode's default).
    #[serde(default)]
    pub permille: Option<u16>,
    /// The dock fills the whole terminal area (the shell is hidden until it
    /// is toggled off; the terminal keeps its size).
    #[serde(default)]
    pub full: bool,
}

impl DockSize {
    /// The dock height for `area`: the whole area when full, else the chosen
    /// share, else `default_h`; at least four rows and leaving two for the shell.
    pub fn height(&self, area: Rect, cell_h: i32, default_h: i32) -> i32 {
        let min = (cell_h * 4).min(area.h.max(0));
        if self.full {
            return area.h.max(min);
        }
        let max = (area.h - cell_h * 2).max(min);
        match self.permille {
            Some(p) => ((area.h as i64 * p as i64 / 1000) as i32).clamp(min, max),
            None => default_h,
        }
    }

    /// From a `view.resized` payload (`dock`, absent from older hosts).
    pub fn from_payload(v: &serde_json::Value) -> Self {
        serde_json::from_value(v["dock"].clone()).unwrap_or_default()
    }
}

/// One entry of a context menu ([`crate::protocol::Out::Menu`]): an action
/// from the mode's own action registry, so the menu and the key bindings
/// never drift apart. Picking it is the same `ui.action` (`id`, `arg`) its
/// key or button sends.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MenuEntry {
    pub id: String,
    pub arg: i32,
    pub label: String,
    /// The action's current key as typed (`s`, `enter`); empty: none.
    pub key: String,
    pub enabled: bool,
}

#[cfg(test)]
mod dock_tests {
    use super::*;

    #[test]
    fn dock_size_heights() {
        let a = Rect { x: 0, y: 0, w: 800, h: 600 };
        assert_eq!(DockSize::default().height(a, 20, 300), 300);
        assert_eq!(DockSize { permille: Some(750), full: false }.height(a, 20, 300), 450);
        assert_eq!(DockSize { permille: Some(1000), full: false }.height(a, 20, 300), 560, "two rows stay the shell's");
        assert_eq!(DockSize { permille: Some(10), full: false }.height(a, 20, 300), 80, "at least four rows");
        assert_eq!(DockSize { permille: None, full: true }.height(a, 20, 300), 600);
        let p = serde_json::json!({"dock": {"permille": 500, "full": false}});
        assert_eq!(DockSize::from_payload(&p).permille, Some(500));
        assert_eq!(DockSize::from_payload(&serde_json::json!({})), DockSize::default(), "an older host");
    }
}
