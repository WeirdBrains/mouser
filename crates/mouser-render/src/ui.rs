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
    let sidebar_w = (width_px as f32 * 0.3).clamp(220.0, 360.0) as i32;
    let sidebar_rect = Rect { x: width_px - sidebar_w, y: 0, w: sidebar_w, h: height_px };
    let line_h = (font_size * 1.3).round() as i32;
    let mut y = 10;
    let mut cats = Vec::new();
    for cat in [Category::Ssh, Category::Waypipe, Category::Tunnels, Category::X11] {
        let header_rect = Rect { x: sidebar_rect.x + 10, y, w: sidebar_rect.w - 20, h: line_h };
        // "Add" button at right side of header
        let add_rect = Rect { x: sidebar_rect.x + sidebar_rect.w - 70, y, w: 60, h: line_h };
        y += line_h + 6;
        let mut items = Vec::new();
        if let Some(list) = groups.get(&cat) {
            for name in list {
                let name_w = (font_size * name.len() as f32 * 0.6).round() as i32;
                let edit_rect = Rect { x: sidebar_rect.x + 20 + name_w + 12, y, w: 44, h: line_h };
                let delete_rect = Rect { x: edit_rect.x + edit_rect.w + 8, y, w: 44, h: line_h };
                items.push(UiItem { name: name.clone(), edit_rect, delete_rect });
                y += line_h + 4;
            }
        }
        y += 8;
        cats.push(UiCategory { category: cat, header_rect, add_rect, items });
    }
    UiLayout { sidebar_rect, categories: cats, modal }
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
