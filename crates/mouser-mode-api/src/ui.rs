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
