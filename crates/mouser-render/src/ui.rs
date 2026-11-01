use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Category {
    Ssh,
    Waypipe,
    Tunnels,
    X11,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UiItem {
    pub name: String,
    pub edit_rect: Rect,
    pub delete_rect: Rect,
    /// Clickable name (connects).
    pub name_rect: Rect,
    /// Shown with an "info" button only; cannot be edited or deleted here.
    pub read_only: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UiCategory {
    pub category: Category,
    pub header_rect: Rect,
    pub add_rect: Rect,
    pub items: Vec<UiItem>,
}

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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// Optional notice button under the SSH header (e.g. migration offer).
    pub notice: Option<(Rect, String)>,
    /// "Engines" section above the Appearance button ([`add_engines`]).
    pub engines: Vec<EngineRow>,
    pub engines_title: Rect,
    /// "MCP" section: servers and their tools ([`add_mcp`]).
    #[serde(default)]
    pub mcp: Vec<McpRow>,
    /// Engine row being dragged (set by the window, not the model).
    #[serde(default)]
    pub engine_drag: Option<EngineDrag>,
    /// The current mode's view above the prompt (docs/modes.md); the
    /// terminal grid starts below it.
    #[serde(default)]
    pub dock: Option<Dock>,
}

/// A view docked above the terminal grid.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Dock {
    pub panel: Panel,
    /// Pixels the grid moves down (the panel's height plus a gap).
    pub inset: i32,
}

/// Pixels the pointer must move before a press on an engine row becomes a
/// drag; less is a click (switch to it).
pub const DRAG_THRESHOLD: i32 = 5;

/// Press-and-drag of an engine row in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EngineDrag {
    /// Index of the pressed row.
    pub from: usize,
    pub start: (i32, i32),
    pub pos: (i32, i32),
    /// Moved past [`DRAG_THRESHOLD`]: a drag, not a click.
    pub active: bool,
}

/// What releasing the pointer does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineDrop {
    /// Didn't move: a click on the row.
    Click(String),
    /// New priority order.
    Reorder(Vec<String>),
    /// Dropped outside the section, or in the same place.
    Cancel,
}

impl EngineDrag {
    /// Start a drag if (`px`, `py`) is on an engine row.
    pub fn press(layout: &UiLayout, px: i32, py: i32) -> Option<Self> {
        if layout.panel.is_some() || layout.modal.is_some() {
            return None;
        }
        let from = engine_row_at(layout, px, py)?;
        Some(Self { from, start: (px, py), pos: (px, py), active: false })
    }

    pub fn moved(&mut self, px: i32, py: i32) {
        self.pos = (px, py);
        if (px - self.start.0).abs().max((py - self.start.1).abs()) >= DRAG_THRESHOLD {
            self.active = true;
        }
    }

    /// Insertion index (0..=rows) for the current pointer, or None outside
    /// the section (a drop there cancels).
    pub fn target(&self, layout: &UiLayout) -> Option<usize> {
        if !self.active || !drop_zone(layout).contains(self.pos.0, self.pos.1) {
            return None;
        }
        Some(drop_index(layout, self.pos.1))
    }

    pub fn release(&self, layout: &UiLayout) -> EngineDrop {
        let Some(row) = layout.engines.get(self.from) else { return EngineDrop::Cancel };
        if !self.active {
            return EngineDrop::Click(row.name.clone());
        }
        let Some(at) = self.target(layout) else { return EngineDrop::Cancel };
        let names: Vec<String> = layout.engines.iter().map(|e| e.name.clone()).collect();
        let new = reordered(&names, self.from, at);
        if new == names { EngineDrop::Cancel } else { EngineDrop::Reorder(new) }
    }
}

/// Index of the engine row at (`px`, `py`).
pub fn engine_row_at(layout: &UiLayout, px: i32, py: i32) -> Option<usize> {
    layout.engines.iter().position(|e| e.rect.contains(px, py))
}

/// Where a dragged row can be dropped: the section (title to last row) plus
/// half a row of slack above and below.
pub fn drop_zone(layout: &UiLayout) -> Rect {
    let (Some(first), Some(last)) = (layout.engines.first(), layout.engines.last()) else { return Rect { x: 0, y: 0, w: 0, h: 0 } };
    let slack = first.rect.h / 2;
    let top = layout.engines_title.y.min(first.rect.y) - slack;
    let bottom = last.rect.y + last.rect.h + slack;
    Rect { x: first.rect.x, y: top, w: first.rect.w, h: bottom - top }
}

/// Insertion index for pointer height `py`: before the first row whose middle
/// is below it.
pub fn drop_index(layout: &UiLayout, py: i32) -> usize {
    layout.engines.iter().position(|e| py < e.rect.y + e.rect.h / 2).unwrap_or(layout.engines.len())
}

/// `names` with the item at `from` moved to insertion index `at` (an index
/// into the list before the move, 0..=len).
pub fn reordered(names: &[String], from: usize, at: usize) -> Vec<String> {
    let mut v = names.to_vec();
    if from >= v.len() {
        return v;
    }
    let item = v.remove(from);
    let at = if at > from { at - 1 } else { at }.min(v.len());
    v.insert(at, item);
    v
}

/// How an engine shows in the sidebar's Engines section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EngineState {
    /// This window draws with it.
    Active,
    Ok,
    Skipped,
    Failed,
}

impl EngineState {
    pub fn label(self) -> &'static str {
        match self {
            EngineState::Active => "active",
            EngineState::Ok => "ok",
            EngineState::Skipped => "skipped",
            EngineState::Failed => "failed",
        }
    }
}

/// One engine row; a click makes it this window's engine (`mouser engine use`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EngineRow {
    pub name: String,
    pub state: EngineState,
    /// Why it is skipped or failed (drawn as a second, muted line).
    pub note: Option<String>,
    pub rect: Rect,
}

pub const ENGINES_TITLE: &str = "Engines";

/// Place the Engines section (in priority order) at the bottom of an expanded
/// sidebar, just above the Appearance button; if the connections reach that
/// far it goes below them and the Appearance button moves down.
pub fn add_engines(layout: &mut UiLayout, engines: &[(String, EngineState, Option<String>)], cell_h: i32) {
    if layout.collapsed || layout.sidebar_rect.w == 0 || engines.is_empty() {
        return;
    }
    let line_h = cell_h + 8;
    let row_h = |n: &Option<String>| if n.is_some() { 2 * line_h - 6 } else { line_h };
    let total = line_h + 4 + engines.iter().map(|(_, _, n)| row_h(n) + 2).sum::<i32>();
    let mut content_bottom = layout.collapse_rect.y + layout.collapse_rect.h;
    for c in &layout.categories {
        content_bottom = content_bottom.max(c.header_rect.y + c.header_rect.h);
        for i in &c.items {
            content_bottom = content_bottom.max(i.name_rect.y + i.name_rect.h);
        }
    }
    if let Some((r, _)) = &layout.notice {
        content_bottom = content_bottom.max(r.y + r.h);
    }
    if let Some(r) = layout.mcp.last() {
        content_bottom = content_bottom.max(r.rect.y + r.rect.h);
    }
    let a = layout.appearance_rect;
    let top = (a.y - 12 - total).max(content_bottom + 12);
    let x = a.x;
    let w = (layout.sidebar_rect.x + layout.sidebar_rect.w - SIDEBAR_MARGIN - x).max(0);
    layout.engines_title = Rect { x, y: top, w, h: line_h };
    let mut y = top + line_h + 4;
    layout.engines = engines
        .iter()
        .map(|(name, state, note)| {
            let r = Rect { x, y, w, h: row_h(note) };
            y += r.h + 2;
            EngineRow { name: name.clone(), state: *state, note: note.clone(), rect: r }
        })
        .collect();
    layout.appearance_rect.y = layout.appearance_rect.y.max(y + 10);
}

pub const MCP_TITLE: &str = "MCP";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum McpRowKind {
    Title,
    /// A server; `ok` is connected, `bad` failed/denied (state label drawn right).
    Server {
        state: String,
        ok: bool,
        bad: bool,
    },
    /// A clickable tool under its server.
    Tool,
    /// A muted line (why it failed).
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct McpRow {
    pub kind: McpRowKind,
    /// Server id the row belongs to.
    pub server: String,
    pub text: String,
    pub rect: Rect,
}

/// One server for [`add_mcp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSpec {
    pub id: String,
    /// What the row shows (`▸ id (3)`).
    pub label: String,
    pub state: String,
    pub ok: bool,
    pub bad: bool,
    pub note: Option<String>,
    pub tools: Vec<String>,
}

/// Place the MCP section below the connections (call before [`add_engines`]).
pub fn add_mcp(layout: &mut UiLayout, servers: &[McpServerSpec], cell_h: i32) {
    if layout.collapsed || layout.sidebar_rect.w == 0 || servers.is_empty() {
        return;
    }
    let line_h = cell_h + 8;
    let mut y = layout.collapse_rect.y + layout.collapse_rect.h;
    for c in &layout.categories {
        y = y.max(c.header_rect.y + c.header_rect.h);
        for i in &c.items {
            y = y.max(i.name_rect.y + i.name_rect.h);
        }
    }
    if let Some((r, _)) = &layout.notice {
        y = y.max(r.y + r.h);
    }
    y += 12;
    let x = layout.appearance_rect.x;
    let w = (layout.sidebar_rect.x + layout.sidebar_rect.w - SIDEBAR_MARGIN - x).max(0);
    let mut push = |kind: McpRowKind, server: &str, text: &str, indent: i32, h: i32| {
        layout.mcp.push(McpRow { kind, server: server.into(), text: text.into(), rect: Rect { x: x + indent, y, w: w - indent, h } });
        y += h + 2;
    };
    push(McpRowKind::Title, "", MCP_TITLE, 0, line_h);
    for s in servers {
        push(McpRowKind::Server { state: s.state.clone(), ok: s.ok, bad: s.bad }, &s.id, &s.label, 0, line_h);
        if let Some(n) = &s.note {
            push(McpRowKind::Note, &s.id, n, 16, cell_h + 4);
        }
        for t in &s.tools {
            push(McpRowKind::Tool, &s.id, t, 16, cell_h + 6);
        }
    }
    layout.appearance_rect.y = layout.appearance_rect.y.max(y + 10);
}

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
    /// Seek / volume bar: `done` and the hover mark (`u16::MAX`: none) in 1/1000.
    Progress {
        done: u16,
        hover: u16,
    },
    /// Dropdown row; bit `i` of `highlight` marks char `i` as matched.
    Option {
        selected: bool,
        highlight: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PanelItem {
    pub rect: Rect,
    pub text: String,
    pub style: PanelStyle,
    pub action: UiAction,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Panel {
    pub rect: Rect,
    pub items: Vec<PanelItem>,
}

pub const APPEARANCE_LABEL: &str = "Appearance";

/// Font dropdown under the font family field.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct FontDropdownView {
    /// Visible rows: (index into the filtered list, name, matched char positions).
    pub rows: Vec<(usize, String, Vec<usize>)>,
    pub selected: usize,
    /// Matches in total / families known; `None` while still discovering.
    pub counts: Option<(usize, usize)>,
}

fn highlight_bits(pos: &[usize]) -> u64 {
    pos.iter().filter(|&&p| p < 64).fold(0, |m, &p| m | (1 << p))
}

/// Data for [`editor_panel`]: one entry per field.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    fonts: Option<&FontDropdownView>,
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
    // Font dropdown, drawn last so it sits over the fields below it.
    if let (Some(dd), Some(field)) =
        (fonts, items.iter().find(|i| i.action == UiAction::FocusField(20) && matches!(i.style, PanelStyle::Field { .. })).map(|i| i.rect))
    {
        let opt_h = cell_h + 4;
        let n = dd.rows.len().max(1) as i32 + 1;
        let w = field.w.max(cell_w * 28);
        let popup = Rect { x: field.x, y: field.y + field.h + 2, w, h: n * opt_h + 4 };
        items.push(PanelItem { rect: popup, text: String::new(), style: PanelStyle::Popup, action: UiAction::None });
        let mut y = popup.y + 2;
        if dd.rows.is_empty() {
            let msg = if dd.counts.is_none() { "Loading fonts…" } else { "No match (Enter keeps the typed name)" };
            items.push(PanelItem {
                rect: Rect { x: popup.x + 4, y, w: w - 8, h: opt_h },
                text: msg.into(),
                style: PanelStyle::Hint,
                action: UiAction::None,
            });
            y += opt_h;
        }
        for (i, name, pos) in &dd.rows {
            items.push(PanelItem {
                rect: Rect { x: popup.x + 2, y, w: w - 4, h: opt_h },
                text: name.clone(),
                style: PanelStyle::Option { selected: *i == dd.selected, highlight: highlight_bits(pos) },
                action: UiAction::PickFont(*i),
            });
            y += opt_h;
        }
        if let Some((shown, total)) = dd.counts {
            items.push(PanelItem {
                rect: Rect { x: popup.x + 4, y, w: w - 8, h: opt_h },
                text: format!("{shown} of {total} fonts  ↑↓ move  Enter pick  Esc close"),
                style: PanelStyle::Hint,
                action: UiAction::None,
            });
        }
    }
    Panel { rect, items }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ModalKind {
    Add(Category),
    Edit { category: Category, name: String },
    ConfirmDelete { category: Category, name: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModalState {
    pub kind: ModalKind,
    pub advanced_open: bool,
    pub rect: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UiAction {
    /// Approval popup answer: 0 allow once, 1 always, 2 deny, 3 never.
    Approve(u8),
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
    /// Pick row `n` (index into the filtered list) of the font dropdown.
    PickFont(usize),
    /// Mouse wheel over a panel, in rows (positive = down).
    ScrollPanel(i32),
    SavePanel,
    ClosePanel,
    /// Ctrl+= (+1), Ctrl+- (-1), Ctrl+0 (0 = reset).
    FontSize(i8),
    /// Launch a connection by name.
    Connect {
        category: Category,
        name: String,
    },
    /// Click on the sidebar notice button.
    Notice,
    /// Tabs: Ctrl+Shift+T, click, close (× / middle-click / Ctrl+Shift+W),
    /// Ctrl+(Shift+)Tab, Alt+1..9.
    NewTab,
    SelectTab(usize),
    CloseTab(usize),
    CloseActiveTab,
    CycleTab(i32),
    /// Engines section: make this engine first and switch to it (`mouser engine use`).
    UseEngine(String),
    /// MCP server row: reconnect, or ask to start an MCP plugin.
    McpServer(String),
    /// MCP tool row: (server id, tool).
    McpTool(String, String),
    /// Engines section drag-and-drop: save this priority order (`mouser engine priority`).
    ReorderEngines(Vec<String>),
    /// An action of the current mode (from a click in its view or windows):
    /// the mode's own action id and an argument (e.g. an item index).
    Mode {
        action: String,
        arg: i32,
    },
    /// Ctrl+Shift+K: the key mappings of the current mode.
    ShowKeymap,
    /// Ctrl+Shift+M: all modes.
    ShowModes,
    /// Switch to the mode with this hotkey (click in the modes window).
    SwitchMode(char),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
            notice: None,
            engines: Vec::new(),
            mcp: Vec::new(),
            engines_title: Rect { x: sidebar_rect.x, y: 0, w: 0, h: 0 },
            engine_drag: None,
            dock: None,
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
                let name_rect = Rect { x: inner_x, y, w: (edit_rect.x - 8 - inner_x).max(0), h: line_h };
                items.push(UiItem { name: name.clone(), edit_rect, delete_rect, name_rect, read_only: false });
                y += line_h + 2;
            }
        }
        y += 10;
        cats.push(UiCategory { category: cat, header_rect, add_rect, items });
    }
    let ap_w = button_w(APPEARANCE_LABEL);
    let appearance_rect = Rect { x: inner_x, y: (height_px - SIDEBAR_MARGIN - line_h).max(y), w: ap_w.min(inner_right - inner_x), h: line_h };
    UiLayout {
        sidebar_rect,
        categories: cats,
        modal,
        collapsed: false,
        collapse_rect,
        resize_rect,
        appearance_rect,
        panel: None,
        notice: None,
        engines: Vec::new(),
        mcp: Vec::new(),
        engines_title: Rect { x: inner_x, y: 0, w: 0, h: 0 },
        engine_drag: None,
        dock: None,
    }
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
        notice: None,
        engines: Vec::new(),
        mcp: Vec::new(),
        engines_title: empty,
        engine_drag: None,
        dock: None,
    }
}

pub const INFO_LABEL: &str = "info";

/// Mark `names` in `category` read-only: their edit button becomes "info" and
/// delete is removed.
pub fn mark_read_only(layout: &mut UiLayout, category: &Category, names: &[String]) {
    for cat in layout.categories.iter_mut().filter(|c| &c.category == category) {
        for item in cat.items.iter_mut().filter(|i| names.contains(&i.name)) {
            item.read_only = true;
            let right = item.delete_rect.x + item.delete_rect.w;
            let w = INFO_LABEL.len() as i32 * ((item.edit_rect.w - 2 * BUTTON_PAD) / EDIT_LABEL.len() as i32) + 2 * BUTTON_PAD;
            item.edit_rect = Rect { x: right - w, w, ..item.edit_rect };
            item.delete_rect = Rect { w: 0, h: 0, ..item.delete_rect };
            item.name_rect.w = (item.edit_rect.x - 8 - item.name_rect.x).max(0);
        }
    }
}

/// Put a full-width notice button under `category`'s header, pushing later
/// rows down.
pub fn add_notice(layout: &mut UiLayout, category: &Category, text: &str, cell_h: i32) {
    let Some(idx) = layout.categories.iter().position(|c| &c.category == category) else { return };
    let h = cell_h + 8;
    let hdr = layout.categories[idx].header_rect;
    let rect = Rect { x: hdr.x, y: hdr.y + hdr.h + 4, w: layout.categories[idx].add_rect.x + layout.categories[idx].add_rect.w - hdr.x, h };
    let shift = h + 4;
    for (i, cat) in layout.categories.iter_mut().enumerate() {
        if i > idx {
            cat.header_rect.y += shift;
            cat.add_rect.y += shift;
        }
        for item in cat.items.iter_mut().filter(|_| i >= idx) {
            item.edit_rect.y += shift;
            item.delete_rect.y += shift;
            item.name_rect.y += shift;
        }
    }
    layout.notice = Some((rect, text.to_string()));
}

/// Simple form panel: a title, labelled text fields, an optional note and error.
#[allow(clippy::too_many_arguments)]
pub fn form_panel(
    vp_w: i32,
    vp_h: i32,
    cell_w: i32,
    cell_h: i32,
    title: &str,
    fields: &[EditorFieldView],
    focus: usize,
    note: Option<&str>,
    error: Option<&str>,
    can_save: bool,
) -> Panel {
    let row_h = cell_h + 8;
    let pad = 14;
    let label_w = cell_w * 14;
    let field_w = cell_w * 36;
    let w = 2 * pad + label_w + field_w;
    let extra = note.is_some() as i32 + error.is_some() as i32;
    let h = 2 * pad + row_h * (fields.len() as i32 + 3 + extra);
    let rect = centered(vp_w, vp_h, w, h);
    let (x, mut y) = (rect.x + pad, rect.y + pad);
    let inner_w = rect.w - 2 * pad;
    let mut items =
        vec![PanelItem { rect: Rect { x, y, w: inner_w, h: row_h }, text: title.into(), style: PanelStyle::Title, action: UiAction::None }];
    y += row_h + 4;
    for (i, f) in fields.iter().enumerate() {
        items.push(PanelItem {
            rect: Rect { x, y, w: label_w, h: row_h },
            text: f.label.clone(),
            style: PanelStyle::Label,
            action: UiAction::FocusField(i),
        });
        items.push(PanelItem {
            rect: Rect { x: x + label_w, y, w: (inner_w - label_w).max(0), h: row_h - 4 },
            text: f.value.clone(),
            style: PanelStyle::Field { focused: can_save && i == focus, valid: f.valid },
            action: UiAction::FocusField(i),
        });
        y += row_h;
    }
    for (text, style) in [(note, PanelStyle::Hint), (error, PanelStyle::Error)] {
        if let Some(t) = text {
            items.push(PanelItem { rect: Rect { x, y, w: inner_w, h: row_h }, text: t.into(), style, action: UiAction::None });
            y += row_h;
        }
    }
    y += 6;
    let buttons: &[(&str, UiAction, bool)] = if can_save {
        &[("Save", UiAction::SavePanel, true), ("Cancel", UiAction::ClosePanel, false)]
    } else {
        &[("Close", UiAction::ClosePanel, true)]
    };
    let mut bx = x;
    for (label, act, primary) in buttons {
        let bw = cell_w * label.len() as i32 + 2 * BUTTON_PAD;
        items.push(PanelItem {
            rect: Rect { x: bx, y, w: bw, h: row_h },
            text: (*label).into(),
            style: PanelStyle::Button { primary: *primary },
            action: act.clone(),
        });
        bx += bw + 8;
    }
    Panel { rect, items }
}

/// Yes/no confirmation panel.
pub fn confirm_panel(vp_w: i32, vp_h: i32, cell_w: i32, cell_h: i32, message: &str, confirm: &str) -> Panel {
    let row_h = cell_h + 8;
    let pad = 14;
    let w = 2 * pad + cell_w * (message.chars().count() as i32).clamp(30, 70);
    let h = 2 * pad + row_h * 3;
    let rect = centered(vp_w, vp_h, w, h);
    let (x, y) = (rect.x + pad, rect.y + pad);
    let mut items = vec![PanelItem {
        rect: Rect { x, y, w: rect.w - 2 * pad, h: row_h },
        text: message.into(),
        style: PanelStyle::Title,
        action: UiAction::None,
    }];
    let by = y + 2 * row_h;
    let cw = cell_w * confirm.chars().count() as i32 + 2 * BUTTON_PAD;
    items.push(PanelItem {
        rect: Rect { x, y: by, w: cw, h: row_h },
        text: confirm.into(),
        style: PanelStyle::Button { primary: true },
        action: UiAction::SavePanel,
    });
    let kw = cell_w * 6 + 2 * BUTTON_PAD;
    items.push(PanelItem {
        rect: Rect { x: x + cw + 8, y: by, w: kw, h: row_h },
        text: "Cancel".into(),
        style: PanelStyle::Button { primary: false },
        action: UiAction::ClosePanel,
    });
    Panel { rect, items }
}

/// One button of the approval popup: (label, answer index, primary).
pub const APPROVAL_BUTTONS: [(&str, u8, bool); 4] =
    [("Allow once [y]", 0, true), ("Allow always [a]", 1, false), ("Deny [n]", 2, false), ("Never [N]", 3, false)];

/// The plugin approval popup (docs/plugins.md §Approval). Drawn by Mouser over
/// everything; terminal output can't draw or answer it. `lines` explain the
/// request; `offer` lists the answers shown (0 once, 1 always, 2 deny, 3 never);
/// `armed` false shows the buttons inert for the first moment (no click-through).
#[allow(clippy::too_many_arguments)]
pub fn approval_panel(vp_w: i32, vp_h: i32, cell_w: i32, cell_h: i32, title: &str, lines: &[String], offer: &[u8], armed: bool) -> Panel {
    let row_h = cell_h + 8;
    let pad = 16;
    let longest = lines.iter().map(|l| l.chars().count()).chain([title.chars().count()]).max().unwrap_or(30) as i32;
    let buttons: Vec<_> = APPROVAL_BUTTONS.iter().filter(|b| offer.contains(&b.1)).collect();
    let bw_total: i32 = buttons.iter().map(|b| cell_w * b.0.chars().count() as i32 + 2 * BUTTON_PAD + 8).sum();
    let w = (2 * pad + cell_w * longest.clamp(36, 80)).max(2 * pad + bw_total);
    let h = 2 * pad + row_h * (lines.len() as i32 + 3);
    let rect = centered(vp_w, vp_h, w, h);
    let (x, mut y) = (rect.x + pad, rect.y + pad);
    let mut items =
        vec![PanelItem { rect: Rect { x, y, w: rect.w - 2 * pad, h: row_h }, text: title.into(), style: PanelStyle::Title, action: UiAction::None }];
    y += row_h + row_h / 2;
    for l in lines {
        items.push(PanelItem {
            rect: Rect { x, y, w: rect.w - 2 * pad, h: row_h },
            text: l.clone(),
            style: PanelStyle::Label,
            action: UiAction::None,
        });
        y += row_h;
    }
    y += row_h / 2;
    let mut bx = x;
    for (label, i, primary) in buttons {
        let bw = cell_w * label.chars().count() as i32 + 2 * BUTTON_PAD;
        items.push(PanelItem {
            rect: Rect { x: bx, y, w: bw, h: row_h },
            text: (*label).into(),
            style: PanelStyle::Button { primary: *primary && armed },
            action: if armed { UiAction::Approve(*i) } else { UiAction::None },
        });
        bx += bw + 8;
    }
    Panel { rect, items }
}

pub fn category_title(c: &Category) -> &'static str {
    match c {
        Category::Ssh => "SSH",
        Category::Waypipe => "Waypipe",
        Category::Tunnels => "Tunnels",
        Category::X11 => "X11",
    }
}

fn over_dock(l: &UiLayout, x: i32, y: i32) -> bool {
    l.panel.is_none() && l.dock.as_ref().is_some_and(|d| d.panel.rect.contains(x, y))
}

/// A left press over a mode's view: the mode sees the press (seek bar, image
/// pan), then the item under it is clicked. `None`: not over the view.
pub fn dock_press(l: &UiLayout, x: i32, y: i32) -> Option<Vec<UiAction>> {
    over_dock(l, x, y).then(|| vec![UiAction::Mode { action: "pointer_down".into(), arg: crate::modes::commander::pack_xy(x, y) }, hit_test(l, x, y)])
}

/// The wheel over a mode's view (`lines` > 0 is up): "scroll", or with Ctrl
/// a zoom about the pointer. `None`: not over the view.
pub fn dock_wheel(l: &UiLayout, x: i32, y: i32, lines: isize, ctrl: bool) -> Option<Vec<UiAction>> {
    if !over_dock(l, x, y) {
        return None;
    }
    let n = lines.signum() as i32 * lines.unsigned_abs().div_ceil(3) as i32;
    Some(match (lines, ctrl) {
        (0, _) => vec![],
        (_, true) => vec![
            UiAction::Mode { action: "pointer_move".into(), arg: crate::modes::commander::pack_xy(x, y) },
            UiAction::Mode { action: "zoom_wheel".into(), arg: n },
        ],
        _ => vec![UiAction::Mode { action: "scroll".into(), arg: -n }],
    })
}

pub fn hit_test(layout: &UiLayout, px: i32, py: i32) -> UiAction {
    if let Some(panel) = &layout.panel {
        // Topmost item first; a popup may hang below the panel and swallows
        // clicks on its own background.
        for i in panel.items.iter().rev() {
            if i.rect.contains(px, py) && (i.action != UiAction::None || i.style == PanelStyle::Popup) {
                return i.action.clone();
            }
        }
        if !panel.rect.contains(px, py) {
            return UiAction::ClosePanel;
        }
        return UiAction::None;
    }
    if layout.modal.is_none()
        && let Some(d) = &layout.dock
        && d.panel.rect.contains(px, py)
    {
        // Last item first: labels and previews sit on top of their tiles.
        return d
            .panel
            .items
            .iter()
            .rev()
            .find(|i| i.rect.contains(px, py) && i.action != UiAction::None)
            .map_or(UiAction::None, |i| i.action.clone());
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
    if layout.notice.as_ref().is_some_and(|(r, _)| r.contains(px, py)) {
        return UiAction::Notice;
    }
    if let Some(r) = layout.mcp.iter().find(|r| r.rect.contains(px, py)) {
        match &r.kind {
            McpRowKind::Server { .. } => return UiAction::McpServer(r.server.clone()),
            McpRowKind::Tool => return UiAction::McpTool(r.server.clone(), r.text.clone()),
            McpRowKind::Title | McpRowKind::Note => {}
        }
    }
    if let Some(e) = layout.engines.iter().find(|e| e.rect.contains(px, py)) {
        return UiAction::UseEngine(e.name.clone());
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
            if item.name_rect.contains(px, py) {
                return UiAction::Connect { category: cat.category.clone(), name: item.name.clone() };
            }
        }
    }
    UiAction::None
}

/// Tab strip state handed to the renderer (only drawn with 2+ tabs).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct TabBar {
    pub titles: Vec<String>,
    pub active: usize,
}

/// Height of the tab strip for a cell height.
pub fn tab_bar_height(cell_h: i32) -> i32 {
    cell_h + 10
}

/// (tab, close button) rectangles for `n` tabs across `width` px.
pub fn tab_rects(n: usize, width: i32, cell_w: i32, cell_h: i32) -> Vec<(Rect, Rect)> {
    if n == 0 {
        return Vec::new();
    }
    let h = tab_bar_height(cell_h);
    let w = (width / n as i32).min(cell_w * 28).max(cell_w * 6);
    (0..n)
        .map(|i| {
            let r = Rect { x: i as i32 * w, y: 0, w, h };
            let c = Rect { x: r.x + w - cell_w * 2 - 2, y: 0, w: cell_w * 2, h };
            (r, c)
        })
        .collect()
}

/// What a click at (`px`, `py`) on the strip does; `middle` closes.
pub fn tab_hit(bar: &TabBar, width: i32, cell_w: i32, cell_h: i32, px: i32, py: i32, middle: bool) -> Option<UiAction> {
    if bar.titles.len() < 2 || py < 0 || py >= tab_bar_height(cell_h) {
        return None;
    }
    let rects = tab_rects(bar.titles.len(), width, cell_w, cell_h);
    let (i, (_, close)) = rects.iter().enumerate().find(|(_, (r, _))| px >= r.x && px < r.x + r.w)?;
    let on_close = px >= close.x && px < close.x + close.w;
    Some(if middle || on_close { UiAction::CloseTab(i) } else { UiAction::SelectTab(i) })
}
