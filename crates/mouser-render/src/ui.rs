use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Ssh,
    Waypipe,
    Tunnels,
    X11,
}

#[derive(Debug, Clone)]
pub struct UiItem {
    pub name: String,
    pub edit_rect: Rect,
    pub delete_rect: Rect,
}

#[derive(Debug, Clone)]
pub struct UiCategory {
    pub category: Category,
    pub header_rect: Rect,
    pub add_rect: Rect,
    pub items: Vec<UiItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone)]
pub struct UiLayout {
    pub sidebar_rect: Rect,
    pub categories: Vec<UiCategory>,
    pub modal: Option<ModalState>,
    /// Sidebar is collapsed to a thin strip (`sidebar_rect` is the strip).
    pub collapsed: bool,
    /// Chevron that collapses (or, when collapsed, expands) the sidebar.
    pub collapse_rect: Rect,
    /// Drag handle on the sidebar's left edge (empty when collapsed).
    pub resize_rect: Rect,
    /// "Appearance" button at the bottom of the expanded sidebar.
    pub appearance_rect: Rect,
    /// Floating panel (theme switcher / editor), drawn above everything.
    pub panel: Option<Panel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelItem {
    pub rect: Rect,
    pub text: String,
    pub style: PanelStyle,
    pub action: UiAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub rect: Rect,
    pub items: Vec<PanelItem>,
}

pub const APPEARANCE_LABEL: &str = "Appearance";

/// Data for [`editor_panel`]: one entry per field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorFieldView {
    pub label: String,
    pub value: String,
    pub valid: bool,
    pub swatch: Option<u32>,
}

fn centered(vp_w: i32, vp_h: i32, w: i32, h: i32) -> Rect {
    let w = w.min(vp_w - 16).max(0);
    let h = h.min(vp_h - 16).max(0);
    Rect { x: (vp_w - w) / 2, y: (vp_h - h) / 2, w, h }
}

/// Theme switcher: theme list, mode buttons, actions.
pub fn picker_panel(vp_w: i32, vp_h: i32, cell_w: i32, cell_h: i32, themes: &[String], selected: usize, mode: &str) -> Panel {
    let row_h = cell_h + 6;
    let pad = 14;
    let w = cell_w * 46 + 2 * pad;
    // Fit as many rows as the window allows (at least 3).
    let chrome = 6 * row_h + 2 * pad;
    let max_rows = (((vp_h - 16 - chrome) / row_h).max(3)) as usize;
    let visible = themes.len().min(max_rows).min(14);
    let h = chrome + visible as i32 * row_h;
    let rect = centered(vp_w, vp_h, w, h);
    let x = rect.x + pad;
    let inner_w = rect.w - 2 * pad;
    let mut y = rect.y + pad;
    let mut items =
        vec![PanelItem { rect: Rect { x, y, w: inner_w, h: row_h }, text: "Theme".into(), style: PanelStyle::Title, action: UiAction::None }];
    y += row_h + 4;
    let first = selected.saturating_sub(visible.saturating_sub(1)).min(themes.len().saturating_sub(visible));
    for (i, name) in themes.iter().enumerate().skip(first).take(visible) {
        items.push(PanelItem {
            rect: Rect { x, y, w: inner_w, h: row_h },
            text: name.clone(),
            style: PanelStyle::Row { selected: i == selected },
            action: UiAction::PickTheme(i),
        });
        y += row_h;
    }
    y += 8;
    items.push(PanelItem { rect: Rect { x, y, w: cell_w * 6, h: row_h }, text: "Mode".into(), style: PanelStyle::Label, action: UiAction::None });
    let mut bx = x + cell_w * 6;
    for m in ["dark", "light", "system"] {
        let bw = cell_w * m.len() as i32 + 2 * BUTTON_PAD;
        items.push(PanelItem {
            rect: Rect { x: bx, y, w: bw, h: row_h },
            text: m.into(),
            style: PanelStyle::Row { selected: m == mode },
            action: UiAction::SetMode(m.to_string()),
        });
        bx += bw + 6;
    }
    y += row_h + 12;
    let mut bx = x;
    for (label, act, primary) in
        [("Apply", UiAction::SavePanel, true), ("Edit colours & font", UiAction::EditTheme, false), ("Cancel", UiAction::ClosePanel, false)]
    {
        let bw = cell_w * label.chars().count() as i32 + 2 * BUTTON_PAD;
        items.push(PanelItem { rect: Rect { x: bx, y, w: bw, h: row_h }, text: label.into(), style: PanelStyle::Button { primary }, action: act });
        bx += bw + 8;
    }
    y += row_h + 6;
    items.push(PanelItem {
        rect: Rect { x, y, w: inner_w, h: row_h },
        text: "↑↓ theme  ←→ mode  e edit  Enter apply  Esc cancel".into(),
        style: PanelStyle::Hint,
        action: UiAction::None,
    });
    Panel { rect, items }
}

/// Theme/font editor. `fields` follow the editor order: foreground,
/// background, cursor, accent, 16 palette entries, font family, font size.
#[allow(clippy::too_many_arguments)]
pub fn editor_panel(
    vp_w: i32,
    vp_h: i32,
    cell_w: i32,
    cell_h: i32,
    title: &str,
    fields: &[EditorFieldView],
    focus: usize,
    error: Option<&str>,
) -> Panel {
    let row_h = cell_h + 8;
    let pad = 14;
    let gap = cell_w * 2;
    let main_label = cell_w * 12;
    let main_field = cell_w * 20;
    let swatch = cell_w * 2;
    let pal_label = cell_w * 3;
    let pal_field = cell_w * 8;
    let main_w = main_label + main_field + 6 + swatch;
    let pal_w = pal_label + pal_field + 6 + swatch;
    let w = 2 * pad + main_w + gap + 2 * pal_w + gap;
    let h = 2 * pad + row_h * 12;
    let rect = centered(vp_w, vp_h, w, h);
    let (x0, y0) = (rect.x + pad, rect.y + pad);
    let mut items = vec![PanelItem {
        rect: Rect { x: x0, y: y0, w: rect.w - 2 * pad, h: row_h },
        text: title.to_string(),
        style: PanelStyle::Title,
        action: UiAction::None,
    }];
    let top = y0 + row_h + 6;
    let add_field = |i: usize, x: i32, y: i32, lw: i32, fw: i32, items: &mut Vec<PanelItem>| {
        let Some(f) = fields.get(i) else { return };
        items.push(PanelItem {
            rect: Rect { x, y, w: lw, h: row_h },
            text: f.label.clone(),
            style: PanelStyle::Label,
            action: UiAction::FocusField(i),
        });
        items.push(PanelItem {
            rect: Rect { x: x + lw, y, w: fw, h: row_h - 4 },
            text: f.value.clone(),
            style: PanelStyle::Field { focused: i == focus, valid: f.valid },
            action: UiAction::FocusField(i),
        });
        if let Some(c) = f.swatch {
            items.push(PanelItem {
                rect: Rect { x: x + lw + fw + 6, y: y + 2, w: swatch, h: row_h - 8 },
                text: String::new(),
                style: PanelStyle::Swatch(c),
                action: UiAction::FocusField(i),
            });
        }
    };
    // Main column: colours then font.
    for (row, i) in [0usize, 1, 2, 3, 20, 21].into_iter().enumerate() {
        let extra = if row >= 4 { row_h / 2 } else { 0 };
        add_field(i, x0, top + row as i32 * row_h + extra, main_label, main_field, &mut items);
    }
    // Palette: two columns of eight.
    let px = x0 + main_w + gap;
    items.push(PanelItem {
        rect: Rect { x: px, y: y0, w: 2 * pal_w, h: row_h },
        text: "Palette".into(),
        style: PanelStyle::Label,
        action: UiAction::None,
    });
    for k in 0..16usize {
        let (col, row) = (k / 8, k % 8);
        add_field(4 + k, px + col as i32 * (pal_w + gap / 2), top + row as i32 * row_h, pal_label, pal_field, &mut items);
    }
    let by = top + 8 * row_h + 10;
    if let Some(e) = error {
        items.push(PanelItem {
            rect: Rect { x: x0, y: by, w: rect.w - 2 * pad, h: row_h },
            text: e.to_string(),
            style: PanelStyle::Error,
            action: UiAction::None,
        });
    }
    let by = by + row_h;
    let mut bx = x0;
    for (label, act, primary) in [("Save", UiAction::SavePanel, true), ("Cancel", UiAction::ClosePanel, false)] {
        let bw = cell_w * label.len() as i32 + 2 * BUTTON_PAD;
        items.push(PanelItem {
            rect: Rect { x: bx, y: by, w: bw, h: row_h },
            text: label.into(),
            style: PanelStyle::Button { primary },
            action: act,
        });
        bx += bw + 8;
    }
    items.push(PanelItem {
        rect: Rect { x: bx + 12, y: by, w: rect.x + rect.w - pad - bx - 12, h: row_h },
        text: "Tab next  Ctrl+S save  Esc cancel".into(),
        style: PanelStyle::Hint,
        action: UiAction::None,
    });
    Panel { rect, items }
}

#[derive(Debug, Clone)]
pub enum ModalKind {
    Add(Category),
    Edit { category: Category, name: String },
    ConfirmDelete { category: Category, name: String },
}

#[derive(Debug, Clone)]
pub struct ModalState {
    pub kind: ModalKind,
    pub advanced_open: bool,
    pub rect: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    OpenAdd(Category),
    OpenEdit {
        category: Category,
        name: String,
    },
    OpenDeleteConfirm {
        category: Category,
        name: String,
    },
    ToggleAdvanced,
    ConfirmDelete,
    CancelModal,
    /// Collapse an expanded sidebar, or expand a collapsed one.
    ToggleCollapse,
    /// Live sidebar width (already clamped) while dragging its edge.
    ResizeSidebar(i32),
    /// Drag finished; persist the current width.
    ResizeSidebarDone,
    OpenAppearance,
    PickTheme(usize),
    /// "dark", "light" or "system".
    SetMode(String),
    EditTheme,
    FocusField(usize),
    SavePanel,
    ClosePanel,
    /// Ctrl+= (+1), Ctrl+- (-1), Ctrl+0 (0 = reset).
    FontSize(i8),
    None,
}

/// Width of the collapsed sidebar strip, in pixels.
pub fn collapsed_width(cell_w: i32) -> i32 {
    (cell_w * 2).max(20)
}

/// Sidebar width limits for a window: at least ~22 cells (>= 200 px), at most
/// 60% of the window while leaving the terminal at least 20 columns.
pub fn sidebar_width_bounds(window_w: i32, cell_w: i32) -> (i32, i32) {
    let min = (cell_w * 22).max(200);
    let max = ((window_w as f32 * 0.6) as i32).min(window_w - cell_w * 20);
    (min, max.max(min))
}

pub fn clamp_sidebar_width(requested: i32, window_w: i32, cell_w: i32) -> i32 {
    let (min, max) = sidebar_width_bounds(window_w, cell_w);
    requested.clamp(min, max).min(window_w.max(0))
}

/// Inputs for [`build_sidebar`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidebarSpec {
    pub width_px: i32,
    pub height_px: i32,
    pub cell_w: i32,
    pub cell_h: i32,
    /// Saved width, if any; clamped to the window.
    pub requested_width: Option<i32>,
    pub collapsed: bool,
}

/// Grab zone half-width around the sidebar's left edge, in pixels.
pub const RESIZE_GRIP: i32 = 4;

/// Whether (`px`, `py`) is on the sidebar's draggable left edge.
pub fn on_resize_edge(layout: &UiLayout, px: i32, py: i32) -> bool {
    layout.modal.is_none() && layout.panel.is_none() && layout.resize_rect.contains(px, py)
}

pub fn build_sidebar_layout(
    width_px: i32,
    height_px: i32,
    font_size: f32,
    groups: &BTreeMap<Category, Vec<String>>,
    modal: Option<ModalState>,
) -> UiLayout {
    let cell_w = (font_size * 0.6).round() as i32;
    let cell_h = (font_size * 1.3).round() as i32;
    build_sidebar_layout_metrics(width_px, height_px, cell_w, cell_h, groups, modal)
}

pub const ADD_LABEL: &str = "+ Add";
pub const EDIT_LABEL: &str = "edit";
pub const DELETE_LABEL: &str = "del";
/// Horizontal padding inside buttons, in pixels.
pub const BUTTON_PAD: i32 = 6;
/// Margin between the sidebar edge and its contents, in pixels.
pub const SIDEBAR_MARGIN: i32 = 12;

/// Width in pixels of the sidebar for a window `width_px` wide.
pub fn sidebar_width(width_px: i32, cell_w: i32) -> i32 {
    let min = (cell_w * 26).max(220);
    ((width_px as f32 * 0.3) as i32).clamp(min, min.max(380)).min(width_px)
}

/// Sidebar layout measured in real monospace cells (`cell_w` x `cell_h` px), so
/// every label and button fits inside the sidebar.
pub fn build_sidebar_layout_metrics(
    width_px: i32,
    height_px: i32,
    cell_w: i32,
    cell_h: i32,
    groups: &BTreeMap<Category, Vec<String>>,
    modal: Option<ModalState>,
) -> UiLayout {
    build_sidebar(SidebarSpec { width_px, height_px, cell_w, cell_h, requested_width: None, collapsed: false }, groups, modal)
}

pub const COLLAPSE_LABEL: &str = "»";
pub const EXPAND_LABEL: &str = "«";
pub const SIDEBAR_TITLE: &str = "Connections";

pub fn build_sidebar(spec: SidebarSpec, groups: &BTreeMap<Category, Vec<String>>, modal: Option<ModalState>) -> UiLayout {
    let SidebarSpec { width_px, height_px, cell_w, cell_h, requested_width, collapsed } = spec;
    let line_h = cell_h + 8;
    if collapsed {
        let w = collapsed_width(cell_w).min(width_px);
        let sidebar_rect = Rect { x: width_px - w, y: 0, w, h: height_px };
        return UiLayout {
            sidebar_rect,
            categories: Vec::new(),
            modal,
            collapsed: true,
            collapse_rect: sidebar_rect,
            resize_rect: Rect { x: sidebar_rect.x, y: 0, w: 0, h: 0 },
            appearance_rect: Rect { x: sidebar_rect.x, y: 0, w: 0, h: 0 },
            panel: None,
        };
    }
    let sidebar_w = match requested_width {
        Some(w) => clamp_sidebar_width(w, width_px, cell_w),
        None => sidebar_width(width_px, cell_w),
    };
    let sidebar_rect = Rect { x: width_px - sidebar_w, y: 0, w: sidebar_w, h: height_px };
    let text_w = |s: &str| s.chars().count() as i32 * cell_w;
    let button_w = |s: &str| text_w(s) + 2 * BUTTON_PAD;
    let inner_x = sidebar_rect.x + SIDEBAR_MARGIN;
    let inner_right = sidebar_rect.x + sidebar_rect.w - SIDEBAR_MARGIN;
    let collapse_rect = Rect { x: inner_right - button_w(COLLAPSE_LABEL), y: SIDEBAR_MARGIN, w: button_w(COLLAPSE_LABEL), h: line_h };
    let resize_rect = Rect { x: sidebar_rect.x - RESIZE_GRIP, y: 0, w: 2 * RESIZE_GRIP, h: height_px };
    let mut y = SIDEBAR_MARGIN + line_h + 10;
    let mut cats = Vec::new();
    for cat in [Category::Ssh, Category::Waypipe, Category::Tunnels, Category::X11] {
        let add_w = button_w(ADD_LABEL);
        let add_rect = Rect { x: inner_right - add_w, y, w: add_w, h: line_h };
        let header_rect = Rect { x: inner_x, y, w: (add_rect.x - 8 - inner_x).max(0), h: line_h };
        y += line_h + 4;
        let mut items = Vec::new();
        if let Some(list) = groups.get(&cat) {
            for name in list {
                let del_w = button_w(DELETE_LABEL);
                let edit_w = button_w(EDIT_LABEL);
                let delete_rect = Rect { x: inner_right - del_w, y, w: del_w, h: line_h };
                let edit_rect = Rect { x: delete_rect.x - 6 - edit_w, y, w: edit_w, h: line_h };
                items.push(UiItem { name: name.clone(), edit_rect, delete_rect });
                y += line_h + 2;
            }
        }
        y += 10;
        cats.push(UiCategory { category: cat, header_rect, add_rect, items });
    }
    let ap_w = button_w(APPEARANCE_LABEL);
    let appearance_rect = Rect { x: inner_x, y: (height_px - SIDEBAR_MARGIN - line_h).max(y), w: ap_w.min(inner_right - inner_x), h: line_h };
    UiLayout { sidebar_rect, categories: cats, modal, collapsed: false, collapse_rect, resize_rect, appearance_rect, panel: None }
}

/// Layout with no sidebar (zero width at the right edge), e.g. to show a panel
/// while the sidebar is closed.
pub fn without_sidebar(width_px: i32, height_px: i32) -> UiLayout {
    let empty = Rect { x: width_px, y: 0, w: 0, h: 0 };
    UiLayout {
        sidebar_rect: Rect { x: width_px, y: 0, w: 0, h: height_px },
        categories: Vec::new(),
        modal: None,
        collapsed: false,
        collapse_rect: empty,
        resize_rect: empty,
        appearance_rect: empty,
        panel: None,
    }
}

pub fn category_title(c: &Category) -> &'static str {
    match c {
        Category::Ssh => "SSH",
        Category::Waypipe => "Waypipe",
        Category::Tunnels => "Tunnels",
        Category::X11 => "X11",
    }
}

pub fn hit_test(layout: &UiLayout, px: i32, py: i32) -> UiAction {
    if let Some(panel) = &layout.panel {
        if !panel.rect.contains(px, py) {
            return UiAction::ClosePanel;
        }
        return panel
            .items
            .iter()
            .rev()
            .find(|i| i.action != UiAction::None && i.rect.contains(px, py))
            .map(|i| i.action.clone())
            .unwrap_or(UiAction::None);
    }
    if let Some(modal) = &layout.modal {
        // Modal consumes clicks
        if modal.rect.contains(px, py) {
            return UiAction::ToggleAdvanced;
        }
        // Click outside modal cancels
        return UiAction::CancelModal;
    }
    if !layout.sidebar_rect.contains(px, py) {
        return UiAction::None;
    }
    if layout.collapse_rect.contains(px, py) {
        return UiAction::ToggleCollapse;
    }
    if layout.appearance_rect.contains(px, py) {
        return UiAction::OpenAppearance;
    }
    for cat in &layout.categories {
        if cat.add_rect.contains(px, py) {
            return UiAction::OpenAdd(cat.category.clone());
        }
        for item in &cat.items {
            if item.edit_rect.contains(px, py) {
                return UiAction::OpenEdit { category: cat.category.clone(), name: item.name.clone() };
            }
            if item.delete_rect.contains(px, py) {
                return UiAction::OpenDeleteConfirm { category: cat.category.clone(), name: item.name.clone() };
            }
        }
    }
    UiAction::None
}
