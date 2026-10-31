#![allow(deprecated)]
use anyhow::Result;
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use winit::event::{ElementState, Event, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::CursorIcon;

pub mod boxdraw;
pub mod clipboard;
pub mod engine;
pub mod frame;
pub mod gpu;
pub mod keys;
pub mod scale;
#[cfg(feature = "skia")]
pub mod skia;
pub mod text;
pub mod ui;

pub use frame::{CellPaint, CursorShape, Frame, ImageDraw, KITTY_BELOW_BG, PADDING, grid_size, image_draws, layered_image_draws, render_frame};
pub use scale::ImageScaler;
pub use text::{Canvas, DisplayList, Draw, FontStack};

/// Pixel geometry handed to the UI model so it can lay out in real cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiViewport {
    pub width: i32,
    pub height: i32,
    pub cell_w: i32,
    pub cell_h: i32,
}

pub type CursorHook = Arc<dyn Fn() -> Option<(usize, usize)> + Send + Sync>;
/// `(scrolled_back, rows)` -> (lines, per-cell paint, clamped scrolled_back).
pub type ViewHook = Arc<dyn Fn(usize, usize) -> (Vec<String>, Vec<Vec<CellPaint>>, usize) + Send + Sync>;
/// A mouse event over the terminal grid; return true when it was consumed
/// (reported to the app or a link opened) so the renderer does not scroll.
pub type MouseHook = Arc<dyn Fn(mouser_core::input::MouseEvent) -> bool + Send + Sync>;
pub type UiModelHook = Arc<dyn Fn(UiViewport) -> Option<ui::UiLayout> + Send + Sync>;

pub struct RenderHooks {
    pub on_key_bytes: Arc<dyn Fn(Vec<u8>) + Send + Sync>,
    /// Every key event (press, repeat, release) with its legacy bytes (none for
    /// releases) and a kitty-protocol description. When set, it replaces
    /// `on_key_bytes` for keyboard input.
    pub on_key_event: Option<KeyEventHook>,
    /// Image placements for the view (`scrolled_back`, rows, cell size in px).
    pub get_images: Option<ImagesHook>,
    /// (cols, rows, pixel width, pixel height) of the terminal grid.
    pub on_resize: Arc<dyn Fn(u16, u16, u16, u16) + Send + Sync>,
    /// Styled view; preferred over `fetch_lines` when set.
    pub fetch_view: Option<ViewHook>,
    pub get_cursor_shape: Option<Arc<dyn Fn() -> CursorShape + Send + Sync>>,
    pub on_mouse: Option<MouseHook>,
    pub on_focus: Option<Arc<dyn Fn(bool) + Send + Sync>>,
    /// Ctrl+Shift+V (Cmd+V on macOS).
    pub on_paste: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Mouse selection on the grid (not reported to the app).
    pub on_select: Option<Arc<dyn Fn(SelectEvent) + Send + Sync>>,
    /// Ctrl+Shift+C (Cmd+C on macOS): copy the selection.
    pub on_copy: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Middle click not taken by the app: paste the primary selection.
    pub on_paste_primary: Option<Arc<dyn Fn() + Send + Sync>>,
    /// True while the app holds output (synchronized update, mode 2026).
    pub hold_frame: Option<ExitHook>,
    /// Window title (OSC 0/2).
    pub get_title: Option<StatusHook>,
    /// Tab strip; drawn when there are 2+ tabs.
    pub get_tabs: Option<Arc<dyn Fn() -> ui::TabBar + Send + Sync>>,
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
    /// Receives a [`Waker`] that requests a redraw from any thread. The loop is
    /// event driven: without a wake-up or input it sleeps.
    pub set_waker: Option<Arc<dyn Fn(Waker) + Send + Sync>>,
}

pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Selection input in view coordinates: `row` is a row of the view, which
/// shows the live screen `scrolled_back` lines up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectEvent {
    /// Left press: `clicks` 1 = characters, 2 = words, 3 = lines; `extend`
    /// (Shift) moves the end of the current selection; `block` (Alt) selects a rectangle.
    Press {
        col: usize,
        row: usize,
        scrolled_back: usize,
        clicks: u8,
        extend: bool,
        block: bool,
    },
    Drag {
        col: usize,
        row: usize,
        scrolled_back: usize,
    },
    Release,
    /// Input was typed: drop the selection.
    Clear,
}

/// Counts consecutive clicks on the same cell (double/triple click).
#[derive(Debug, Clone, Copy, Default)]
pub struct ClickCounter {
    last: Option<(Instant, usize, usize, u8)>,
}

pub const MULTI_CLICK: Duration = Duration::from_millis(400);

impl ClickCounter {
    /// Register a press at (`col`, `row`) and return the click count (1..=3, then back to 1).
    pub fn press(&mut self, now: Instant, col: usize, row: usize) -> u8 {
        let n = match self.last {
            Some((t, c, r, n)) if now.duration_since(t) <= MULTI_CLICK && c == col && r == row => n % 3 + 1,
            _ => 1,
        };
        self.last = Some((now, col, row, n));
        n
    }
}

/// View cell under a window position for selection, clamped to the grid, and
/// whether the pointer is above (-1) or below (1) it.
pub fn select_cell(px: f64, py: f64, cell: (i32, i32), grid: (usize, usize), top: i32) -> (usize, usize, i8) {
    let (x, y) = (px as i32 - PADDING, py as i32 - top);
    let cols = grid.0.max(1);
    let rows = grid.1.max(1);
    let col = if x < 0 { 0 } else { ((x / cell.0.max(1)) as usize).min(cols - 1) };
    if y < 0 {
        return (col, 0, -1);
    }
    let row = (y / cell.1.max(1)) as usize;
    if row >= rows { (col, rows - 1, 1) } else { (col, row, 0) }
}

/// Logs how long a frame took (`RUST_LOG=mouser_frame=debug`).
struct FrameTimer(Instant);

impl Drop for FrameTimer {
    fn drop(&mut self) {
        log::debug!(target: "mouser_frame", "frame {:.2} ms", self.0.elapsed().as_secs_f64() * 1e3);
    }
}

const AUTOSCROLL_EVERY: Duration = Duration::from_millis(50);

pub type ImagesHook = Arc<dyn Fn(usize, usize, (u32, u32)) -> Vec<ImageDraw> + Send + Sync>;

pub type KeyEventHook = Arc<dyn Fn(Option<Vec<u8>>, mouser_core::kitty::KeyInput, Option<mouser_core::fuzzball::KeyEvent>) + Send + Sync>;

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
    /// Initial window title (default "Mouser").
    pub title: Option<String>,
    /// Wayland app_id / X11 WM_CLASS (default "Mouser").
    pub app_id: Option<String>,
}

impl Default for Renderer {
    fn default() -> Self {
        Self { cols: 100, rows: 30, font_size: 15.0, font_family: None, title: None, app_id: None }
    }
}

type CpuSurface = Surface<Arc<winit::window::Window>, Arc<winit::window::Window>>;

/// The active backend: a GPU engine (docs/engines.md) when available, else softbuffer.
enum Painter {
    Cpu(Box<CpuSurface>),
    Gpu { engine: Box<dyn engine::Engine>, list: DisplayList, window: Arc<winit::window::Window> },
}

impl Painter {
    /// The chosen GPU engine (falling back to `mouser`) unless
    /// `MOUSER_RENDERER=cpu` or no GPU engine can start.
    fn pick(window: Arc<winit::window::Window>, width: u32, height: u32) -> Result<Self> {
        let want_cpu = std::env::var("MOUSER_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("cpu"));
        if !want_cpu {
            let choice = mouser_core::engines::chosen();
            match engine::create(&choice, &window, width, height) {
                Ok((e, why)) => {
                    if let Some(why) = why {
                        log::warn!("{why}");
                        eprintln!("mouser: {why}");
                        set_notice(why);
                    }
                    log::info!("renderer: gpu engine {} ({})", e.name(), e.description());
                    eprintln!("mouser: renderer: gpu engine {} ({})", e.name(), e.description());
                    return Ok(Painter::Gpu { engine: e, list: DisplayList::default(), window });
                }
                Err(e) => {
                    log::warn!("no GPU engine ({e:#}); falling back to CPU");
                    eprintln!("mouser: no GPU engine ({e:#}); falling back to CPU renderer");
                }
            }
        }
        let context = Context::new(window.clone()).map_err(|e| anyhow::anyhow!(format!("softbuffer ctx: {e}")))?;
        let mut surface = Surface::new(&context, window).map_err(|e| anyhow::anyhow!(format!("softbuffer surface: {e}")))?;
        let _ = surface.resize(NonZeroU32::new(width.max(1)).unwrap(), NonZeroU32::new(height.max(1)).unwrap());
        log::info!("renderer: cpu (softbuffer)");
        eprintln!("mouser: renderer: cpu (softbuffer){}", if want_cpu { " [MOUSER_RENDERER=cpu]" } else { "" });
        Ok(Painter::Cpu(Box::new(surface)))
    }

    fn resize(&mut self, width: u32, height: u32) {
        match self {
            Painter::Cpu(s) => {
                let _ = s.resize(NonZeroU32::new(width.max(1)).unwrap(), NonZeroU32::new(height.max(1)).unwrap());
            }
            Painter::Gpu { engine, .. } => engine.resize(width, height),
        }
    }

    /// Paint and present; false asks for another frame (surface was reset).
    fn paint(&mut self, fonts: &mut FontStack, frame: &Frame, width: u32, height: u32) -> bool {
        let (w, h) = (width as usize, height as usize);
        match self {
            Painter::Cpu(surface) => {
                let Ok(mut buf) = surface.buffer_mut() else { return true };
                if buf.len() >= w * h {
                    let mut canvas = Canvas::new(&mut buf, w, h);
                    render_frame(&mut canvas, fonts, frame);
                }
                let _ = buf.present();
                true
            }
            Painter::Gpu { engine, list, window } => {
                list.width = w;
                list.height = h;
                render_frame(list, fonts, frame);
                match engine.draw(list, fonts) {
                    Ok(done) => done,
                    Err(e) if engine.name() != "mouser" => {
                        let name = engine.name().to_string();
                        mouser_core::engines::clear_sentinel(&name);
                        let why = engine::fallback_message(&name, &format!("{e:#}"), "failed while drawing");
                        log::warn!("{why}");
                        eprintln!("mouser: {why}");
                        set_notice(why);
                        match engine::create_default(window, width, height) {
                            Ok(m) => *engine = m,
                            Err(e) => log::error!("mouser engine unavailable: {e:#}"),
                        }
                        false
                    }
                    Err(e) => {
                        log::warn!("frame failed: {e:#}");
                        false
                    }
                }
            }
        }
    }
}

/// How long an engine fallback notice stays in the status bar.
const NOTICE_TTL: Duration = Duration::from_secs(15);

static NOTICE: std::sync::Mutex<Option<(String, Instant)>> = std::sync::Mutex::new(None);

fn set_notice(msg: String) {
    *NOTICE.lock().unwrap() = Some((msg, Instant::now()));
}

/// The current engine fallback notice, if still fresh.
fn notice() -> Option<String> {
    NOTICE.lock().unwrap().as_ref().filter(|(_, at)| at.elapsed() < NOTICE_TTL).map(|(m, _)| m.clone())
}

/// Events delivered to the render loop from other threads.
enum UserEvent {
    /// Something changed (PTY output, shell exit, config reload): redraw.
    Wake,
    /// A font stack finished loading off the UI thread.
    Fonts(u64, Box<FontStack>),
}

/// Upper bound on frame rate while output streams in; redraws are coalesced
/// so a flood of PTY data costs at most one frame per interval.
const MIN_FRAME: Duration = Duration::from_millis(8);

fn set_icon(window: &winit::window::Window) {
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
}

pub fn run(mut renderer: Renderer, hooks: RenderHooks) -> Result<()> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let attrs = winit::window::WindowAttributes::default()
        .with_title(renderer.title.clone().unwrap_or_else(|| "Mouser".into()))
        .with_inner_size(winit::dpi::LogicalSize::new(1100.0, 700.0));
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut attrs = attrs;
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(id) = &renderer.app_id {
        use winit::platform::wayland::WindowAttributesExtWayland;
        attrs = WindowAttributesExtWayland::with_name(attrs, id.clone(), id.clone());
        use winit::platform::x11::WindowAttributesExtX11;
        attrs = WindowAttributesExtX11::with_name(attrs, id.clone(), id.clone());
    }
    let window = Arc::new(event_loop.create_window(attrs)?);
    set_icon(&window);
    clipboard::init(&window);
    let proxy = event_loop.create_proxy();
    // Coalesce wakeups: at most one Wake event is queued at a time.
    let wake_pending = Arc::new(AtomicBool::new(false));
    if let Some(set_waker) = &hooks.set_waker {
        let proxy = proxy.clone();
        let pending = wake_pending.clone();
        set_waker(Arc::new(move || {
            if !pending.swap(true, Ordering::AcqRel) {
                let _ = proxy.send_event(UserEvent::Wake);
            }
        }));
    }
    let mut width = window.inner_size().width.max(1);
    let mut height = window.inner_size().height.max(1);
    let mut scale = window.scale_factor();
    let mut painter = Painter::pick(window.clone(), width, height)?;
    let font_db = Arc::new(text::system_db());
    let mut fonts = FontStack::from_db(&font_db, renderer.font_family.as_deref(), renderer.font_size * scale as f32);
    // Family switches load on a worker thread; `font_gen` drops stale results.
    let mut font_gen = 0u64;

    let mut scrolled_back = 0usize;
    let mut cursor_pos: Option<winit::dpi::PhysicalPosition<f64>> = None;
    let mut last_ui: Option<ui::UiLayout> = None;
    let mut modifiers = ModifiersState::empty();
    let mut grid = (0usize, 0usize);
    let mut dragging = false;
    let mut resize_cursor = false;
    let mut dirty = true;
    let mut last_frame: Option<Instant> = None;
    let mut mouse_down: Option<mouser_core::input::MouseButton> = None;
    let mut text_w = width as i32;
    let mut held_since: Option<Instant> = None;
    let mut title = String::new();
    let default_title = renderer.title.clone().unwrap_or_else(|| "Mouser".into());
    let mut grid_top = PADDING;
    let mut tab_bar = ui::TabBar::default();
    let mut selecting = false;
    let mut clicks = ClickCounter::default();
    let mut last_autoscroll = Instant::now();

    event_loop.run(|event, elwt| {
        match event {
            Event::UserEvent(UserEvent::Wake) => {
                wake_pending.store(false, Ordering::Release);
                dirty = true;
            }
            Event::UserEvent(UserEvent::Fonts(generation, stack)) => {
                if generation == font_gen {
                    fonts = *stack;
                    dirty = true;
                }
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => elwt.exit(),
                // Some compositors stop sending motion once the pointer leaves
                // the surface mid-drag; park it past the nearer edge so the
                // selection keeps auto-scrolling.
                WindowEvent::CursorLeft { .. } if selecting => {
                    if let Some(pos) = cursor_pos.as_mut() {
                        pos.y = if pos.y < height as f64 / 2.0 { -1.0 } else { height as f64 + 1.0 };
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    cursor_pos = Some(position);
                    let (px, py) = (position.x.round() as i32, position.y.round() as i32);
                    if dragging && let Some(on_act) = &hooks.on_ui_action {
                        let w = ui::clamp_sidebar_width(width as i32 - px, width as i32, fonts.cell_w);
                        (on_act)(ui::UiAction::ResizeSidebar(w));
                        dirty = true;
                    }
                    if selecting && let Some(on_select) = &hooks.on_select {
                        let (col, row, _) = select_cell(position.x, position.y, (fonts.cell_w, fonts.cell_h), grid, grid_top);
                        on_select(SelectEvent::Drag { col, row, scrolled_back });
                        dirty = true;
                    }
                    if !dragging
                        && !selecting
                        && let (Some(on_mouse), Some(hit)) =
                            (&hooks.on_mouse, grid_hit(position.x, position.y, (fonts.cell_w, fonts.cell_h), grid, text_w, grid_top))
                    {
                        let b = mouse_down.unwrap_or(mouser_core::input::MouseButton::None);
                        on_mouse(mouse_event(mouser_core::input::MouseKind::Motion, b, hit, modifiers));
                    }
                    let hover = dragging || last_ui.as_ref().is_some_and(|l| ui::on_resize_edge(l, px, py));
                    if hover != resize_cursor {
                        resize_cursor = hover;
                        let icon = if hover { CursorIcon::EwResize } else { CursorIcon::Default };
                        window.set_cursor(icon);
                    }
                }
                WindowEvent::MouseInput { state: ElementState::Released, button, .. } => {
                    if selecting && button == winit::event::MouseButton::Left {
                        selecting = false;
                        mouse_down = None;
                        if let Some(on_select) = &hooks.on_select {
                            on_select(SelectEvent::Release);
                        }
                        dirty = true;
                        return;
                    }
                    if !dragging
                        && let (Some(on_mouse), Some(pos), Some(b)) = (&hooks.on_mouse, cursor_pos, winit_button(button))
                        && let Some(hit) = grid_hit(pos.x, pos.y, (fonts.cell_w, fonts.cell_h), grid, text_w, grid_top)
                    {
                        on_mouse(mouse_event(mouser_core::input::MouseKind::Release, b, hit, modifiers));
                    }
                    mouse_down = None;
                    if dragging {
                        dragging = false;
                        if let Some(on_act) = &hooks.on_ui_action {
                            (on_act)(ui::UiAction::ResizeSidebarDone);
                        }
                        dirty = true;
                    }
                }
                WindowEvent::ModifiersChanged(m) => modifiers = m.state(),
                WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => {
                    if let (Some(on_act), Some(pos)) = (&hooks.on_ui_action, cursor_pos)
                        && let Some(act) = ui::tab_hit(
                            &tab_bar,
                            text_w,
                            fonts.cell_w,
                            fonts.cell_h,
                            pos.x as i32,
                            pos.y as i32,
                            button == winit::event::MouseButton::Middle,
                        )
                    {
                        on_act(act);
                        scrolled_back = 0;
                        dirty = true;
                        return;
                    }
                    if let (Some(on_mouse), Some(pos), Some(b)) = (&hooks.on_mouse, cursor_pos, winit_button(button))
                        && let Some(hit) = grid_hit(pos.x, pos.y, (fonts.cell_w, fonts.cell_h), grid, text_w, grid_top)
                        && last_ui.as_ref().is_none_or(|l| l.panel.is_none())
                    {
                        mouse_down = Some(b);
                        if on_mouse(mouse_event(mouser_core::input::MouseKind::Press, b, hit, modifiers)) {
                            dirty = true;
                            return;
                        }
                        if b == mouser_core::input::MouseButton::Left
                            && let Some(on_select) = &hooks.on_select
                        {
                            let n = clicks.press(Instant::now(), hit.0, hit.1);
                            on_select(SelectEvent::Press {
                                col: hit.0,
                                row: hit.1,
                                scrolled_back,
                                clicks: n,
                                extend: modifiers.shift_key() && n == 1,
                                block: modifiers.alt_key(),
                            });
                            selecting = true;
                            dirty = true;
                        }
                        if b == mouser_core::input::MouseButton::Middle
                            && let Some(paste) = &hooks.on_paste_primary
                        {
                            mouse_down = None;
                            scrolled_back = 0;
                            paste();
                            dirty = true;
                            return;
                        }
                    }
                    if let (Some(on_act), Some(layout), Some(pos)) = (&hooks.on_ui_action, &last_ui, cursor_pos) {
                        if ui::on_resize_edge(layout, pos.x.round() as i32, pos.y.round() as i32) {
                            dragging = true;
                            return;
                        }
                        let act = ui::hit_test(layout, pos.x.round() as i32, pos.y.round() as i32);
                        (on_act)(act);
                        dirty = true;
                    }
                }
                WindowEvent::KeyboardInput { event: key_event, .. } => {
                    dirty = true;
                    if let (Some(delta), Some(on_act)) = (font_zoom_key(&key_event, modifiers), &hooks.on_ui_action) {
                        (on_act)(ui::UiAction::FontSize(delta));
                        return;
                    }
                    if let (Some(act), Some(on_act)) = (tab_key(&key_event, modifiers), &hooks.on_ui_action) {
                        scrolled_back = 0;
                        on_act(act);
                        return;
                    }
                    // Ctrl+Shift+C/V, and Cmd+C/V on macOS.
                    let clip_mods = (modifiers.control_key() && modifiers.shift_key())
                        || (cfg!(target_os = "macos") && modifiers.super_key() && !modifiers.control_key());
                    if key_event.state == ElementState::Pressed
                        && clip_mods
                        && let Key::Character(c) = &key_event.logical_key
                    {
                        if c.eq_ignore_ascii_case("v")
                            && let Some(paste) = &hooks.on_paste
                        {
                            scrolled_back = 0;
                            paste();
                            return;
                        }
                        if c.eq_ignore_ascii_case("c")
                            && let Some(copy) = &hooks.on_copy
                        {
                            copy();
                            return;
                        }
                    }
                    // Shift+Insert: the primary selection on Linux (like xterm, foot,
                    // alacritty), the clipboard elsewhere.
                    if key_event.state == ElementState::Pressed
                        && modifiers.shift_key()
                        && !modifiers.control_key()
                        && key_event.logical_key == Key::Named(NamedKey::Insert)
                    {
                        let hook = if cfg!(target_os = "linux") { hooks.on_paste_primary.as_ref() } else { hooks.on_paste.as_ref() };
                        if let Some(paste) = hook {
                            scrolled_back = 0;
                            paste();
                            return;
                        }
                    }
                    let clear_selection = || {
                        if key_event.state == ElementState::Pressed
                            && let Some(on_select) = &hooks.on_select
                        {
                            on_select(SelectEvent::Clear);
                        }
                    };
                    if let Some(on_event) = &hooks.on_key_event {
                        let legacy = key_bytes(&key_event, modifiers);
                        if let Some(input) = keys::kitty_input(&key_event, modifiers) {
                            if key_event.state == ElementState::Pressed {
                                scrolled_back = 0;
                            }
                            if legacy.is_some() {
                                clear_selection();
                            }
                            on_event(legacy, input, keys::fuzzball_event(&key_event, modifiers));
                        } else if let Some(seq) = legacy {
                            scrolled_back = 0;
                            clear_selection();
                            (hooks.on_key_bytes)(seq);
                        }
                    } else if let Some(seq) = key_bytes(&key_event, modifiers) {
                        scrolled_back = 0;
                        clear_selection();
                        (hooks.on_key_bytes)(seq);
                    }
                }
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    scale = scale_factor;
                    fonts = fonts.resized(renderer.font_size * scale as f32);
                    dirty = true;
                }
                WindowEvent::Resized(new_size) => {
                    width = new_size.width.max(1);
                    height = new_size.height.max(1);
                    painter.resize(width, height);
                    dirty = true;
                }
                WindowEvent::Focused(f) => {
                    if let Some(on_focus) = &hooks.on_focus {
                        on_focus(f);
                    }
                    dirty = true;
                }
                WindowEvent::RedrawRequested | WindowEvent::Occluded(false) => dirty = true,
                WindowEvent::MouseWheel { delta, .. } => {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => (y * 3.0) as isize,
                        MouseScrollDelta::PixelDelta(p) => (p.y / fonts.cell_h as f64) as isize,
                    };
                    dirty = true;
                    // Over a panel (e.g. the font list) the wheel scrolls the panel.
                    if let (Some(on_act), Some(true)) = (&hooks.on_ui_action, last_ui.as_ref().map(|l| l.panel.is_some())) {
                        if lines != 0 {
                            (on_act)(ui::UiAction::ScrollPanel(-lines.signum() as i32 * lines.unsigned_abs().div_ceil(3) as i32));
                        }
                        return;
                    }
                    if let (Some(on_mouse), Some(pos)) = (&hooks.on_mouse, cursor_pos)
                        && let Some(hit) = grid_hit(pos.x, pos.y, (fonts.cell_w, fonts.cell_h), grid, text_w, grid_top)
                        && lines != 0
                        && scrolled_back == 0
                    {
                        use mouser_core::input::{MouseButton as B, MouseKind as K};
                        let b = if lines > 0 { B::WheelUp } else { B::WheelDown };
                        let mut used = false;
                        for _ in 0..lines.unsigned_abs().div_ceil(3) {
                            used |= on_mouse(mouse_event(K::Press, b, hit, modifiers));
                        }
                        if used {
                            return;
                        }
                    }
                    scrolled_back = scrolled_back.saturating_add_signed(lines);
                }
                _ => {}
            },
            Event::AboutToWait => {
                if hooks.should_exit.as_ref().is_some_and(|f| f()) {
                    elwt.exit();
                    return;
                }
                // Dragging a selection past the top or bottom edge scrolls the view.
                let mut autoscroll_next = None;
                if selecting && let (Some(on_select), Some(pos)) = (&hooks.on_select, cursor_pos) {
                    let (col, row, edge) = select_cell(pos.x, pos.y, (fonts.cell_w, fonts.cell_h), grid, grid_top);
                    if edge != 0 {
                        autoscroll_next = Some(last_autoscroll + AUTOSCROLL_EVERY);
                        if last_autoscroll.elapsed() >= AUTOSCROLL_EVERY {
                            last_autoscroll = Instant::now();
                            scrolled_back = if edge < 0 { scrolled_back.saturating_add(1) } else { scrolled_back.saturating_sub(1) };
                            on_select(SelectEvent::Drag { col, row, scrolled_back });
                            dirty = true;
                        }
                        if !dirty {
                            elwt.set_control_flow(ControlFlow::WaitUntil(last_autoscroll + AUTOSCROLL_EVERY));
                            return;
                        }
                    }
                }
                if !dirty {
                    // Nothing to do: sleep until an input event or a wake-up.
                    elwt.set_control_flow(ControlFlow::Wait);
                    return;
                }
                // Synchronized output (2026): hold the old frame until the app
                // finishes its update, but never longer than 150 ms.
                if hooks.hold_frame.as_ref().is_some_and(|f| f()) {
                    let since = *held_since.get_or_insert_with(Instant::now);
                    if since.elapsed() < Duration::from_millis(150) {
                        elwt.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(8)));
                        return;
                    }
                }
                held_since = None;
                if let Some(at) = last_frame
                    && at.elapsed() < MIN_FRAME
                {
                    elwt.set_control_flow(ControlFlow::WaitUntil(at + MIN_FRAME));
                    return;
                }
                dirty = false;
                last_frame = Some(Instant::now());
                let _frame_timer = FrameTimer(Instant::now());
                elwt.set_control_flow(match autoscroll_next {
                    Some(_) => ControlFlow::WaitUntil(last_autoscroll + AUTOSCROLL_EVERY),
                    None => ControlFlow::Wait,
                });
                if let Some(get_font) = &hooks.get_font {
                    let (size, family) = get_font();
                    if family != renderer.font_family {
                        // Resolve the new family off the UI thread; keep drawing
                        // with the current fonts until it is ready.
                        renderer.font_family = family.clone();
                        renderer.font_size = size;
                        font_gen += 1;
                        let (generation, db, proxy, px) = (font_gen, font_db.clone(), proxy.clone(), size * scale as f32);
                        std::thread::spawn(move || {
                            let stack = FontStack::from_db(&db, family.as_deref(), px);
                            let _ = proxy.send_event(UserEvent::Fonts(generation, Box::new(stack)));
                        });
                    } else if (size - renderer.font_size).abs() > f32::EPSILON {
                        renderer.font_size = size;
                        fonts = fonts.resized(size * scale as f32);
                    }
                }
                let viewport = UiViewport { width: width as i32, height: height as i32, cell_w: fonts.cell_w, cell_h: fonts.cell_h };
                let layout = hooks.get_ui_model.as_ref().and_then(|f| f(viewport));
                // The sidebar takes space from the terminal instead of covering it.
                text_w = layout.as_ref().map(|l| l.sidebar_rect.x).unwrap_or(width as i32);
                if let Some(f) = &hooks.get_tabs {
                    tab_bar = f();
                }
                let tab_h = if tab_bar.titles.len() > 1 { ui::tab_bar_height(fonts.cell_h) } else { 0 };
                grid_top = PADDING + tab_h;
                let new_grid = grid_size(&fonts, text_w, height as i32 - tab_h);
                if new_grid != grid {
                    grid = new_grid;
                    renderer.cols = grid.0;
                    renderer.rows = grid.1;
                    let (pw, ph) = (grid.0 as i32 * fonts.cell_w, grid.1 as i32 * fonts.cell_h);
                    (hooks.on_resize)(grid.0 as u16, grid.1 as u16, pw.clamp(0, u16::MAX as i32) as u16, ph.clamp(0, u16::MAX as i32) as u16);
                }
                let (lines, styles, clamped) = match &hooks.fetch_view {
                    Some(f) => f(scrolled_back, renderer.rows),
                    None => {
                        let (l, c) = (hooks.fetch_lines)(scrolled_back, renderer.rows);
                        (l, Vec::new(), c)
                    }
                };
                scrolled_back = clamped;
                if let Some(t) = hooks.get_title.as_ref().and_then(|f| f())
                    && t != title
                {
                    window.set_title(if t.is_empty() { &default_title } else { &t });
                    title = t;
                }
                let cursor = if scrolled_back == 0 { hooks.get_cursor.as_ref().and_then(|f| f()) } else { None };
                let colors = (hooks.get_colors)();
                let (bg, fg) = (colors.bg, colors.fg);
                let status = hooks.get_status.as_ref().and_then(|f| f()).or_else(notice);
                let cell = (fonts.cell_w as u32, fonts.cell_h as u32);
                let images = hooks.get_images.as_ref().map(|f| f(scrolled_back, renderer.rows, cell)).unwrap_or_default();
                let frame = Frame {
                    lines: &lines,
                    cursor,
                    bg,
                    fg,
                    ui: layout.as_ref(),
                    status: status.as_deref(),
                    accent: colors.accent,
                    cursor_color: colors.cursor,
                    images: &images,
                    styles: &styles,
                    tabs: Some(&tab_bar),
                    cursor_shape: hooks.get_cursor_shape.as_ref().map(|f| f()).unwrap_or_default(),
                };
                last_ui = layout.clone();
                let paint_start = Instant::now();
                let painted = painter.paint(&mut fonts, &frame, width, height);
                log::debug!(target: "mouser_frame", "prepare {:.2} ms paint {:.2} ms", (paint_start - _frame_timer.0).as_secs_f64() * 1e3, paint_start.elapsed().as_secs_f64() * 1e3);
                if !painted {
                    dirty = true;
                }
            }
            _ => {}
        }
    })?;
    Ok(())
}

/// Grid cell and in-grid pixel under a window position, if it is over the terminal.
pub fn grid_hit(px: f64, py: f64, cell: (i32, i32), grid: (usize, usize), text_w: i32, top: i32) -> Option<(usize, usize, u32, u32)> {
    let (x, y) = (px as i32 - PADDING, py as i32 - top);
    if x < 0 || y < 0 || px as i32 >= text_w {
        return None;
    }
    let (c, r) = ((x / cell.0) as usize, (y / cell.1) as usize);
    (c < grid.0 && r < grid.1).then_some((c, r, x as u32, y as u32))
}

fn mouse_event(
    kind: mouser_core::input::MouseKind,
    button: mouser_core::input::MouseButton,
    hit: (usize, usize, u32, u32),
    m: ModifiersState,
) -> mouser_core::input::MouseEvent {
    mouser_core::input::MouseEvent {
        kind,
        button,
        col: hit.0,
        row: hit.1,
        px: hit.2,
        py: hit.3,
        shift: m.shift_key(),
        alt: m.alt_key(),
        ctrl: m.control_key(),
    }
}

fn winit_button(b: winit::event::MouseButton) -> Option<mouser_core::input::MouseButton> {
    use mouser_core::input::MouseButton as B;
    match b {
        winit::event::MouseButton::Left => Some(B::Left),
        winit::event::MouseButton::Middle => Some(B::Middle),
        winit::event::MouseButton::Right => Some(B::Right),
        _ => None,
    }
}

/// Tab shortcuts: Ctrl+Shift+T new, Ctrl+Shift+W close, Ctrl+Tab /
/// Ctrl+Shift+Tab cycle, Alt+1..9 jump.
pub fn tab_key(ev: &winit::event::KeyEvent, m: ModifiersState) -> Option<ui::UiAction> {
    if ev.state != ElementState::Pressed {
        return None;
    }
    let (ctrl, shift, alt) = (m.control_key(), m.shift_key(), m.alt_key());
    match &ev.logical_key {
        Key::Named(NamedKey::Tab) if ctrl && !alt => Some(ui::UiAction::CycleTab(if shift { -1 } else { 1 })),
        Key::Character(c) if ctrl && shift && !alt && c.eq_ignore_ascii_case("t") => Some(ui::UiAction::NewTab),
        Key::Character(c) if ctrl && shift && !alt && c.eq_ignore_ascii_case("w") => Some(ui::UiAction::CloseActiveTab),
        _ if alt && !ctrl && !shift => {
            // Use the physical digit row so layouts with shifted digits work.
            use winit::keyboard::{KeyCode, PhysicalKey};
            let codes = [
                KeyCode::Digit1,
                KeyCode::Digit2,
                KeyCode::Digit3,
                KeyCode::Digit4,
                KeyCode::Digit5,
                KeyCode::Digit6,
                KeyCode::Digit7,
                KeyCode::Digit8,
                KeyCode::Digit9,
            ];
            match ev.physical_key {
                PhysicalKey::Code(k) => codes.iter().position(|c| *c == k).map(ui::UiAction::SelectTab),
                _ => None,
            }
        }
        _ => None,
    }
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
        Key::Named(NamedKey::Backspace) if mods.control_key() => vec![0x08],
        Key::Named(NamedKey::Backspace) => vec![0x7f],
        Key::Named(NamedKey::ContextMenu) => b"\x1b[29~".to_vec(),
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
            // Kitty legacy text keys: Ctrl+Shift (or Super) combos have no
            // legacy form and go out as `CSI code ; mods u`.
            let base = match winit::platform::modifier_supplement::KeyEventExtModifierSupplement::key_without_modifiers(key_event) {
                Key::Character(b) => b.chars().next(),
                _ => None,
            };
            if let Some(b) = base.filter(|b| b.is_ascii_graphic() || *b == ' ')
                && ((mods.control_key() && mods.shift_key()) || mods.super_key())
            {
                let m =
                    1 + u8::from(mods.shift_key()) + 2 * u8::from(mods.alt_key()) + 4 * u8::from(mods.control_key()) + 8 * u8::from(mods.super_key());
                return Some(format!("\x1b[{};{m}u", b.to_ascii_lowercase() as u32).into_bytes());
            }
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if mods.control_key() && c.is_ascii() => match legacy_ctrl(base.unwrap_or(c)) {
                    Some(b) => vec![b],
                    None => s.as_bytes().to_vec(),
                },
                _ => s.as_bytes().to_vec(),
            }
        }
        _ => return None,
    };
    if seq.is_empty() {
        return None;
    }
    if let Some(m) = modified_csi(&seq, mods) {
        return Some(m);
    }
    if mods.alt_key() && !matches!(key_event.logical_key, Key::Named(NamedKey::Escape)) {
        let mut v = vec![0x1b];
        v.extend(seq);
        return Some(v);
    }
    Some(seq)
}

/// Kitty's "legacy ctrl mapping of ASCII keys" table.
pub fn legacy_ctrl(c: char) -> Option<u8> {
    Some(match c.to_ascii_lowercase() {
        ' ' | '2' | '@' => 0,
        l @ 'a'..='z' => l as u8 - b'a' + 1,
        '3' | '[' => 27,
        '4' | '\\' => 28,
        '5' | ']' => 29,
        '6' | '^' | '~' => 30,
        '7' | '/' | '_' => 31,
        '8' | '?' => 127,
        _ => return None,
    })
}

/// xterm modifier encoding for cursor/edit/function keys: `CSI A` + Ctrl becomes
/// `CSI 1;5A`, `CSI 3~` becomes `CSI 3;5~`, `SS3 P` becomes `CSI 1;5P`.
pub fn modified_csi(seq: &[u8], mods: ModifiersState) -> Option<Vec<u8>> {
    let m = 1 + u8::from(mods.shift_key()) + 2 * u8::from(mods.alt_key()) + 4 * u8::from(mods.control_key());
    if m == 1 || seq.first() != Some(&0x1b) || seq.len() < 3 {
        return None;
    }
    let last = *seq.last()?;
    match (seq[1], last) {
        (b'[', b'A'..=b'D' | b'H' | b'F') if seq.len() == 3 => Some(format!("\x1b[1;{m}{}", last as char).into_bytes()),
        (b'O', b'P'..=b'S') if seq.len() == 3 => Some(format!("\x1b[1;{m}{}", last as char).into_bytes()),
        (b'[', b'~') => {
            let num = std::str::from_utf8(&seq[2..seq.len() - 1]).ok()?;
            Some(format!("\x1b[{num};{m}~").into_bytes())
        }
        _ => None,
    }
}

#[cfg(test)]
mod select_tests {
    use super::*;

    #[test]
    fn click_counter_counts_quick_clicks_on_one_cell() {
        let t = Instant::now();
        let mut c = ClickCounter::default();
        assert_eq!(c.press(t, 1, 1), 1);
        assert_eq!(c.press(t + Duration::from_millis(100), 1, 1), 2);
        assert_eq!(c.press(t + Duration::from_millis(200), 1, 1), 3);
        assert_eq!(c.press(t + Duration::from_millis(300), 1, 1), 1);
        assert_eq!(c.press(t + Duration::from_millis(350), 2, 1), 1, "another cell");
        assert_eq!(c.press(t + Duration::from_secs(2), 2, 1), 1, "too slow");
    }

    #[test]
    fn select_cell_clamps_and_reports_edges() {
        let cell = (10, 20);
        let grid = (80, 24);
        assert_eq!(select_cell(PADDING as f64 + 15.0, PADDING as f64 + 45.0, cell, grid, PADDING), (1, 2, 0));
        assert_eq!(select_cell(5000.0, 0.0, cell, grid, PADDING + 30), (79, 0, -1));
        assert_eq!(select_cell(0.0, 9000.0, cell, grid, PADDING), (0, 23, 1));
    }
}
