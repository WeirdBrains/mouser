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
pub mod frame;
pub mod gpu;
pub mod keys;
pub mod text;
pub mod ui;

pub use frame::{CellPaint, CursorShape, Frame, ImageDraw, PADDING, grid_size, render_frame};
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
    /// Ctrl+Shift+V.
    pub on_paste: Option<Arc<dyn Fn() + Send + Sync>>,
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

pub type ImagesHook = Arc<dyn Fn(usize, usize, (u32, u32)) -> Vec<ImageDraw> + Send + Sync>;

pub type KeyEventHook = Arc<dyn Fn(Option<Vec<u8>>, mouser_core::kitty::KeyInput) + Send + Sync>;

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

type CpuSurface = Surface<Arc<winit::window::Window>, Arc<winit::window::Window>>;

/// The active backend: wgpu when available, else softbuffer.
enum Painter {
    Cpu(Box<CpuSurface>),
    Gpu(Box<gpu::GpuPainter>, DisplayList),
}

impl Painter {
    /// GPU unless `MOUSER_RENDERER=cpu` or wgpu cannot get an adapter/surface.
    fn pick(window: Arc<winit::window::Window>, width: u32, height: u32) -> Result<Self> {
        let want_cpu = std::env::var("MOUSER_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("cpu"));
        if !want_cpu {
            match gpu::GpuPainter::new(window.clone(), width, height) {
                Ok(g) => {
                    log::info!("renderer: gpu (wgpu, {})", g.description);
                    eprintln!("mouser: renderer: gpu (wgpu, {})", g.description);
                    return Ok(Painter::Gpu(Box::new(g), DisplayList::default()));
                }
                Err(e) => {
                    log::warn!("wgpu unavailable ({e}); falling back to CPU");
                    eprintln!("mouser: wgpu unavailable ({e}); falling back to CPU renderer");
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
            Painter::Gpu(g, _) => g.resize(width, height),
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
            Painter::Gpu(g, list) => {
                list.width = w;
                list.height = h;
                render_frame(list, fonts, frame);
                g.draw(list, fonts)
            }
        }
    }
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
    let attrs = winit::window::WindowAttributes::default().with_title("Mouser").with_inner_size(winit::dpi::LogicalSize::new(1100.0, 700.0));
    let window = Arc::new(event_loop.create_window(attrs)?);
    set_icon(&window);
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
    let mut grid_top = PADDING;
    let mut tab_bar = ui::TabBar::default();

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
                WindowEvent::CursorMoved { position, .. } => {
                    cursor_pos = Some(position);
                    let (px, py) = (position.x.round() as i32, position.y.round() as i32);
                    if dragging && let Some(on_act) = &hooks.on_ui_action {
                        let w = ui::clamp_sidebar_width(width as i32 - px, width as i32, fonts.cell_w);
                        (on_act)(ui::UiAction::ResizeSidebar(w));
                        dirty = true;
                    }
                    if !dragging
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
                    if key_event.state == ElementState::Pressed
                        && modifiers.control_key()
                        && modifiers.shift_key()
                        && let (Key::Character(c), Some(paste)) = (&key_event.logical_key, &hooks.on_paste)
                        && c.eq_ignore_ascii_case("v")
                    {
                        scrolled_back = 0;
                        paste();
                        return;
                    }
                    if let Some(on_event) = &hooks.on_key_event {
                        let legacy = key_bytes(&key_event, modifiers);
                        if let Some(input) = keys::kitty_input(&key_event, modifiers) {
                            if key_event.state == ElementState::Pressed {
                                scrolled_back = 0;
                            }
                            on_event(legacy, input);
                        } else if let Some(seq) = legacy {
                            scrolled_back = 0;
                            (hooks.on_key_bytes)(seq);
                        }
                    } else if let Some(seq) = key_bytes(&key_event, modifiers) {
                        scrolled_back = 0;
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
                elwt.set_control_flow(ControlFlow::Wait);
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
                    window.set_title(if t.is_empty() { "Mouser" } else { &t });
                    title = t;
                }
                let cursor = if scrolled_back == 0 { hooks.get_cursor.as_ref().and_then(|f| f()) } else { None };
                let colors = (hooks.get_colors)();
                let (bg, fg) = (colors.bg, colors.fg);
                let status = hooks.get_status.as_ref().and_then(|f| f());
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
                if !painter.paint(&mut fonts, &frame, width, height) {
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
