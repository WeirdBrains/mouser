#![allow(deprecated)]
use anyhow::Result;
use softbuffer::{Context, Surface};
use std::sync::Arc;
use std::time::Duration;
use winit::event::{ElementState, Event, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::CursorIcon;

pub mod frame;
pub mod text;
pub mod ui;

pub use frame::{Frame, PADDING, grid_size, render_frame};
pub use text::{Canvas, FontStack};

/// Pixel geometry handed to the UI model so it can lay out in real cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiViewport {
    pub width: i32,
    pub height: i32,
    pub cell_w: i32,
    pub cell_h: i32,
}

pub type CursorHook = Arc<dyn Fn() -> Option<(usize, usize)> + Send + Sync>;
pub type UiModelHook = Arc<dyn Fn(UiViewport) -> Option<ui::UiLayout> + Send + Sync>;

pub struct RenderHooks {
    pub on_key_bytes: Arc<dyn Fn(Vec<u8>) + Send + Sync>,
    pub on_resize: Arc<dyn Fn(u16, u16) + Send + Sync>,
    /// `(scrolled_back, rows)` -> (lines to show, clamped scrolled_back).
    /// `scrolled_back` counts lines above the live screen; 0 is the live view.
    pub fetch_lines: Arc<dyn Fn(usize, usize) -> (Vec<String>, usize) + Send + Sync>,
    pub get_colors: Arc<dyn Fn() -> Colors + Send + Sync>,
    /// Current (font size in logical px, font family); changes are applied live.
    pub get_font: Option<FontHook>,
    /// Live cursor (column, row) on the screen, if visible.
    pub get_cursor: Option<CursorHook>,
    /// Sidebar/modal layout, or `None` when the sidebar is closed.
    pub get_ui_model: Option<UiModelHook>,
    pub on_ui_action: Option<Arc<dyn Fn(ui::UiAction) + Send + Sync>>,
    /// Command line / status text drawn in a bar at the bottom, if any.
    pub get_status: Option<StatusHook>,
    /// Returns true once the session has ended (e.g. the shell exited); the
    /// window then closes and `run` returns.
    pub should_exit: Option<ExitHook>,
}

pub type ExitHook = Arc<dyn Fn() -> bool + Send + Sync>;

pub type FontHook = Arc<dyn Fn() -> (f32, Option<String>) + Send + Sync>;

/// Theme colours as 0xRRGGBB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colors {
    pub bg: u32,
    pub fg: u32,
    pub cursor: Option<u32>,
    pub accent: Option<u32>,
}

pub type StatusHook = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub struct Renderer {
    pub cols: usize,
    pub rows: usize,
    pub font_size: f32,
    pub font_family: Option<String>,
}

impl Default for Renderer {
    fn default() -> Self {
        Self { cols: 100, rows: 30, font_size: 15.0, font_family: None }
    }
}

pub fn run(mut renderer: Renderer, hooks: RenderHooks) -> Result<()> {
    let event_loop = EventLoop::new()?;
    let attrs = winit::window::WindowAttributes::default().with_title("Mouser").with_inner_size(winit::dpi::LogicalSize::new(1100.0, 700.0));
    let window = event_loop.create_window(attrs)?;
    // Window icon (cross-platform): simple generated glyph to avoid external assets at runtime
    {
        let w = 64u32;
        let h = 64u32;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let idx = ((y * w + x) * 4) as usize;
                // rounded-square badge with gradient
                let rx = x as i32 - 32;
                let ry = y as i32 - 32;
                let r2 = rx * rx + ry * ry;
                let in_round = r2 < (30 * 30);
                let t = y as f32 / (h - 1) as f32;
                let (r_top, g_top, b_top) = (0x1f, 0x23, 0x35);
                let (r_bot, g_bot, b_bot) = (0x10, 0x13, 0x20);
                let r = ((1.0 - t) * r_top as f32 + t * r_bot as f32) as u8;
                let g = ((1.0 - t) * g_top as f32 + t * g_bot as f32) as u8;
                let b = ((1.0 - t) * b_top as f32 + t * b_bot as f32) as u8;
                buf[idx] = r;
                buf[idx + 1] = g;
                buf[idx + 2] = b;
                buf[idx + 3] = if in_round { 0xFF } else { 0x00 };
            }
        }
        if let Ok(icon) = winit::window::Icon::from_rgba(buf, w, h) {
            window.set_window_icon(Some(icon));
        }
    }
    let context = Context::new(&window).map_err(|e| anyhow::anyhow!(format!("softbuffer ctx: {e}")))?;
    let mut surface = Surface::new(&context, &window).map_err(|e| anyhow::anyhow!(format!("softbuffer surface: {e}")))?;
    let mut width = window.inner_size().width.max(1);
    let mut height = window.inner_size().height.max(1);
    let mut scale = window.scale_factor();
    let font_db = text::system_db();
    let mut fonts = FontStack::from_db(&font_db, renderer.font_family.as_deref(), renderer.font_size * scale as f32);

    let mut scrolled_back = 0usize;
    let mut cursor_pos: Option<winit::dpi::PhysicalPosition<f64>> = None;
    let mut last_ui: Option<ui::UiLayout> = None;
    let mut modifiers = ModifiersState::empty();
    let mut grid = (0usize, 0usize);
    let mut dragging = false;
    let mut resize_cursor = false;

    event_loop.run(|event, elwt| {
        elwt.set_control_flow(ControlFlow::wait_duration(Duration::from_millis(16)));
        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::CursorMoved { position, .. } => {
                    cursor_pos = Some(position);
                    let (px, py) = (position.x.round() as i32, position.y.round() as i32);
                    if dragging && let Some(on_act) = &hooks.on_ui_action {
                        let w = ui::clamp_sidebar_width(width as i32 - px, width as i32, fonts.cell_w);
                        (on_act)(ui::UiAction::ResizeSidebar(w));
                    }
                    let hover = dragging || last_ui.as_ref().is_some_and(|l| ui::on_resize_edge(l, px, py));
                    if hover != resize_cursor {
                        resize_cursor = hover;
                        let icon = if hover { CursorIcon::EwResize } else { CursorIcon::Default };
                        window.set_cursor(icon);
                    }
                }
                WindowEvent::MouseInput { state: ElementState::Released, .. } => {
                    if dragging {
                        dragging = false;
                        if let Some(on_act) = &hooks.on_ui_action {
                            (on_act)(ui::UiAction::ResizeSidebarDone);
                        }
                    }
                }
                WindowEvent::ModifiersChanged(m) => modifiers = m.state(),
                WindowEvent::MouseInput { state: ElementState::Pressed, .. } => {
                    if let (Some(on_act), Some(layout), Some(pos)) = (&hooks.on_ui_action, &last_ui, cursor_pos) {
                        if ui::on_resize_edge(layout, pos.x.round() as i32, pos.y.round() as i32) {
                            dragging = true;
                            return;
                        }
                        let act = ui::hit_test(layout, pos.x.round() as i32, pos.y.round() as i32);
                        (on_act)(act);
                    }
                }
                WindowEvent::KeyboardInput { event: key_event, .. } => {
                    if let (Some(delta), Some(on_act)) = (font_zoom_key(&key_event, modifiers), &hooks.on_ui_action) {
                        (on_act)(ui::UiAction::FontSize(delta));
                        return;
                    }
                    if let Some(seq) = key_bytes(&key_event, modifiers) {
                        scrolled_back = 0;
                        (hooks.on_key_bytes)(seq);
                    }
                }
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    scale = scale_factor;
                    fonts = fonts.resized(renderer.font_size * scale as f32);
                }
                WindowEvent::Resized(new_size) => {
                    width = new_size.width.max(1);
                    height = new_size.height.max(1);
                    use std::num::NonZeroU32;
                    let _ = surface.resize(NonZeroU32::new(width).unwrap(), NonZeroU32::new(height).unwrap());
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => (y * 3.0) as isize,
                        MouseScrollDelta::PixelDelta(p) => (p.y / fonts.cell_h as f64) as isize,
                    };
                    scrolled_back = scrolled_back.saturating_add_signed(lines);
                }
                _ => {}
            },
            Event::AboutToWait => {
                if hooks.should_exit.as_ref().is_some_and(|f| f()) {
                    elwt.exit();
                    return;
                }
                if let Some(get_font) = &hooks.get_font {
                    let (size, family) = get_font();
                    if family != renderer.font_family {
                        renderer.font_family = family;
                        renderer.font_size = size;
                        fonts = FontStack::from_db(&font_db, renderer.font_family.as_deref(), size * scale as f32);
                    } else if (size - renderer.font_size).abs() > f32::EPSILON {
                        renderer.font_size = size;
                        fonts = fonts.resized(size * scale as f32);
                    }
                }
                let viewport = UiViewport { width: width as i32, height: height as i32, cell_w: fonts.cell_w, cell_h: fonts.cell_h };
                let layout = hooks.get_ui_model.as_ref().and_then(|f| f(viewport));
                // The sidebar takes space from the terminal instead of covering it.
                let text_w = layout.as_ref().map(|l| l.sidebar_rect.x).unwrap_or(width as i32);
                let new_grid = grid_size(&fonts, text_w, height as i32);
                if new_grid != grid {
                    grid = new_grid;
                    renderer.cols = grid.0;
                    renderer.rows = grid.1;
                    (hooks.on_resize)(grid.0 as u16, grid.1 as u16);
                }
                let (lines, clamped) = (hooks.fetch_lines)(scrolled_back, renderer.rows);
                scrolled_back = clamped;
                let cursor = if scrolled_back == 0 { hooks.get_cursor.as_ref().and_then(|f| f()) } else { None };
                let colors = (hooks.get_colors)();
                let (bg, fg) = (colors.bg, colors.fg);
                let status = hooks.get_status.as_ref().and_then(|f| f());
                let Ok(mut buf) = surface.buffer_mut() else { return };
                let (w, h) = (width as usize, height as usize);
                if buf.len() >= w * h {
                    let mut canvas = Canvas::new(&mut buf, w, h);
                    render_frame(
                        &mut canvas,
                        &mut fonts,
                        &Frame {
                            lines: &lines,
                            cursor,
                            bg,
                            fg,
                            ui: layout.as_ref(),
                            status: status.as_deref(),
                            accent: colors.accent,
                            cursor_color: colors.cursor,
                        },
                    );
                }
                last_ui = layout;
                let _ = buf.present();
            }
            _ => {}
        }
    })?;
    #[allow(unreachable_code)]
    Ok(())
}

/// Ctrl+= / Ctrl++ (+1), Ctrl+- (-1), Ctrl+0 (reset).
pub fn font_zoom_key(key_event: &winit::event::KeyEvent, mods: ModifiersState) -> Option<i8> {
    if key_event.state != ElementState::Pressed || !mods.control_key() || mods.alt_key() {
        return None;
    }
    match &key_event.logical_key {
        Key::Character(s) => zoom_for_char(s),
        _ => None,
    }
}

/// Font-size step for a Ctrl+<key> character.
pub fn zoom_for_char(s: &str) -> Option<i8> {
    match s {
        "=" | "+" => Some(1),
        "-" | "_" => Some(-1),
        "0" | ")" => Some(0),
        _ => None,
    }
}

/// Bytes to send to the PTY for a key press, xterm style.
pub fn key_bytes(key_event: &winit::event::KeyEvent, mods: ModifiersState) -> Option<Vec<u8>> {
    if key_event.state != ElementState::Pressed {
        return None;
    }
    let seq: Vec<u8> = match &key_event.logical_key {
        Key::Named(NamedKey::Enter) => vec![b'\r'],
        Key::Named(NamedKey::Tab) if mods.shift_key() => b"\x1b[Z".to_vec(),
        Key::Named(NamedKey::Tab) => vec![b'\t'],
        Key::Named(NamedKey::Space) if mods.control_key() => vec![0],
        Key::Named(NamedKey::Space) => vec![b' '],
        Key::Named(NamedKey::Escape) => vec![0x1b],
        Key::Named(NamedKey::Backspace) => vec![0x7f],
        Key::Named(NamedKey::Delete) => b"\x1b[3~".to_vec(),
        Key::Named(NamedKey::Insert) => b"\x1b[2~".to_vec(),
        Key::Named(NamedKey::Home) => b"\x1b[H".to_vec(),
        Key::Named(NamedKey::End) => b"\x1b[F".to_vec(),
        Key::Named(NamedKey::PageUp) => b"\x1b[5~".to_vec(),
        Key::Named(NamedKey::PageDown) => b"\x1b[6~".to_vec(),
        Key::Named(NamedKey::F1) => b"\x1bOP".to_vec(),
        Key::Named(NamedKey::F2) => b"\x1bOQ".to_vec(),
        Key::Named(NamedKey::F3) => b"\x1bOR".to_vec(),
        Key::Named(NamedKey::F4) => b"\x1bOS".to_vec(),
        Key::Named(NamedKey::F5) => b"\x1b[15~".to_vec(),
        Key::Named(NamedKey::F6) => b"\x1b[17~".to_vec(),
        Key::Named(NamedKey::F7) => b"\x1b[18~".to_vec(),
        Key::Named(NamedKey::F8) => b"\x1b[19~".to_vec(),
        Key::Named(NamedKey::F9) => b"\x1b[20~".to_vec(),
        Key::Named(NamedKey::F10) => b"\x1b[21~".to_vec(),
        Key::Named(NamedKey::F11) => b"\x1b[23~".to_vec(),
        Key::Named(NamedKey::F12) => b"\x1b[24~".to_vec(),
        Key::Named(NamedKey::ArrowLeft) => b"\x1b[D".to_vec(),
        Key::Named(NamedKey::ArrowRight) => b"\x1b[C".to_vec(),
        Key::Named(NamedKey::ArrowUp) => b"\x1b[A".to_vec(),
        Key::Named(NamedKey::ArrowDown) => b"\x1b[B".to_vec(),
        Key::Character(s) if !s.is_empty() => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if mods.control_key() && c.is_ascii() => match c.to_ascii_lowercase() {
                    l @ 'a'..='z' => vec![l as u8 - b'a' + 1],
                    '[' => vec![0x1b],
                    '\\' => vec![0x1c],
                    ']' => vec![0x1d],
                    '/' | '_' => vec![0x1f],
                    _ => s.as_bytes().to_vec(),
                },
                _ => s.as_bytes().to_vec(),
            }
        }
        _ => return None,
    };
    if seq.is_empty() {
        return None;
    }
    if mods.alt_key() && !matches!(key_event.logical_key, Key::Named(NamedKey::Escape)) {
        let mut v = vec![0x1b];
        v.extend(seq);
        return Some(v);
    }
    Some(seq)
}
