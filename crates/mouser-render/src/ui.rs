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
    OpenEdit { category: Category, name: String },
    OpenDeleteConfirm { category: Category, name: String },
    ToggleAdvanced,
    ConfirmDelete,
    CancelModal,
    None,
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
    let sidebar_w = sidebar_width(width_px, cell_w);
    let sidebar_rect = Rect { x: width_px - sidebar_w, y: 0, w: sidebar_w, h: height_px };
    let line_h = cell_h + 8;
    let text_w = |s: &str| s.chars().count() as i32 * cell_w;
    let button_w = |s: &str| text_w(s) + 2 * BUTTON_PAD;
    let inner_x = sidebar_rect.x + SIDEBAR_MARGIN;
    let inner_right = sidebar_rect.x + sidebar_rect.w - SIDEBAR_MARGIN;
    let mut y = SIDEBAR_MARGIN;
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
    UiLayout { sidebar_rect, categories: cats, modal }
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
