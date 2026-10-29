#![allow(deprecated)]
use anyhow::Result;
use fontdb::Database;
use fontdue::Font;
use softbuffer::{Context, Surface};
use std::sync::Arc;
use winit::event::{ElementState, Event, MouseScrollDelta, WindowEvent};
use winit::event_loop::EventLoop;
use winit::keyboard::{Key, NamedKey};

pub mod ui;

pub struct RenderHooks {
    pub on_key_bytes: Arc<dyn Fn(Vec<u8>) + Send + Sync>,
    pub on_resize: Arc<dyn Fn(u16, u16) + Send + Sync>,
    pub fetch_lines: Arc<dyn Fn(usize, usize) -> Vec<String> + Send + Sync>,
    pub get_colors: Arc<dyn Fn() -> (u32, u32) + Send + Sync>, // (bg, fg) as 0xRRGGBB
    pub get_ui_model: Option<Arc<dyn Fn() -> ui::UiLayout + Send + Sync>>,
    pub on_ui_action: Option<Arc<dyn Fn(ui::UiAction) + Send + Sync>>,
}

pub struct Renderer {
    pub cols: usize,
    pub rows: usize,
    pub font_size: f32,
}

impl Default for Renderer {
    fn default() -> Self {
        Self { cols: 100, rows: 30, font_size: 16.0 }
    }
}

pub fn run(mut renderer: Renderer, hooks: RenderHooks) -> Result<()> {
    let event_loop = EventLoop::new()?;
    let attrs = winit::window::WindowAttributes::default().with_title("Mouser");
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
    {
        use std::num::NonZeroU32;
        surface
            .resize(NonZeroU32::new(width).unwrap(), NonZeroU32::new(height).unwrap())
            .map_err(|e| anyhow::anyhow!(format!("softbuffer resize: {e}")))?;
    }

    // Load a monospace system font
    let mut db = Database::new();
    db.load_system_fonts();
    let face = db
        .query(&fontdb::Query { families: &[fontdb::Family::Monospace], ..Default::default() })
        .or_else(|| db.faces().next().map(|f| f.id))
        .ok_or_else(|| anyhow::anyhow!("No system fonts found"))?;
    let mut font_bytes: Option<Vec<u8>> = None;
    db.with_face_data(face, |data, _idx| {
        font_bytes = Some(data.to_vec());
    });
    let font_data = font_bytes.ok_or_else(|| anyhow::anyhow!("Font data not found"))?;
    let font = Font::from_bytes(font_data, fontdue::FontSettings::default()).map_err(|e| anyhow::anyhow!(e))?;
    let line_height = (renderer.font_size * 1.3) as i32;
    let char_width = (renderer.font_size * 0.6) as i32;

    let mut viewport_start = 0usize;
    let mut cursor_pos: Option<winit::dpi::PhysicalPosition<f64>> = None;
    let mut last_ui: Option<ui::UiLayout> = None;

    event_loop.run(|event, elwt| {
        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::CursorMoved { position, .. } => {
                    cursor_pos = Some(position);
                }
                WindowEvent::MouseInput { state: ElementState::Pressed, .. } => {
                    if let (Some(get_ui), Some(on_act)) = (&hooks.get_ui_model, &hooks.on_ui_action) {
                        if let Some(layout) = &last_ui {
                            if let Some(pos) = cursor_pos {
                                let act = ui::hit_test(layout, pos.x.round() as i32, pos.y.round() as i32);
                                (on_act)(act);
                            }
                        } else {
                            let layout = get_ui();
                            last_ui = Some(layout);
                        }
                    }
                }
                WindowEvent::KeyboardInput { event: key_event, .. } => {
                    handle_key_event(&key_event, &hooks.on_key_bytes);
                }
                WindowEvent::Resized(new_size) => {
                    width = new_size.width.max(1);
                    height = new_size.height.max(1);
                    {
                        use std::num::NonZeroU32;
                        let _ = surface.resize(NonZeroU32::new(width).unwrap(), NonZeroU32::new(height).unwrap());
                    }
                    renderer.cols = (width as f32 / (renderer.font_size * 0.6)).floor() as usize;
                    renderer.rows = (height as f32 / (renderer.font_size * 1.3)).floor() as usize;
                    (hooks.on_resize)(renderer.cols as u16, renderer.rows as u16);
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let scroll_lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y as isize,
                        MouseScrollDelta::PixelDelta(p) => (p.y / (renderer.font_size as f64)) as isize,
                    };
                    if scroll_lines < 0 {
                        viewport_start = viewport_start.saturating_sub(scroll_lines.unsigned_abs());
                    } else {
                        viewport_start = viewport_start.saturating_add(scroll_lines as usize);
                    }
                }
                _ => {}
            },
            Event::AboutToWait => {
                let lines = (hooks.fetch_lines)(viewport_start, renderer.rows);

                // Draw text into softbuffer surface
                let mut buf = surface.buffer_mut().expect("buffer");
                let width_usize = width as usize;
                let height_usize = height as usize;
                // Colors
                let (bg, fg) = (hooks.get_colors)();
                let bg_r = (bg >> 16) & 0xFF;
                let bg_g = (bg >> 8) & 0xFF;
                let bg_b = bg & 0xFF;
                let fg_r = (fg >> 16) & 0xFF;
                let fg_g = (fg >> 8) & 0xFF;
                let fg_b = fg & 0xFF;
                // Clear
                for y in 0..height_usize {
                    for x in 0..width_usize {
                        buf[y * width_usize + x] = (bg_r << 16) | (bg_g << 8) | bg_b;
                    }
                }
                // Render
                for (row, line) in lines.iter().enumerate() {
                    let baseline = 10 + (row as i32) * line_height;
                    let mut pen_x = 10i32;
                    for ch in line.chars() {
                        let (metrics, bitmap) = font.rasterize(ch, renderer.font_size);
                        for iy in 0..metrics.height {
                            let y0 = baseline + iy as i32 - metrics.ymin;
                            if y0 < 0 || y0 >= height as i32 {
                                continue;
                            }
                            for ix in 0..metrics.width {
                                let x0 = pen_x + ix as i32 + metrics.xmin;
                                if x0 < 0 || x0 >= width as i32 {
                                    continue;
                                }
                                let alpha = bitmap[iy * metrics.width + ix] as u32;
                                let r = (fg_r * alpha + bg_r * (255 - alpha)) / 255;
                                let g = (fg_g * alpha + bg_g * (255 - alpha)) / 255;
                                let b = (fg_b * alpha + bg_b * (255 - alpha)) / 255;
                                buf[y0 as usize * width_usize + x0 as usize] = (r << 16) | (g << 8) | b;
                            }
                        }
                        pen_x += char_width;
                    }
                }
                // Draw UI overlay: sidebar and modal, if provided
                if let Some(get_ui) = &hooks.get_ui_model {
                    let layout = get_ui();
                    // Sidebar background
                    let s = layout.sidebar_rect;
                    // fill rect helper (inline)
                    {
                        let screen_h = buf.height().get() as usize;
                        let color = 0x061f2b;
                        let (r8, g8, b8) = ((color >> 16) & 0xFF, (color >> 8) & 0xFF, color & 0xFF);
                        let x0 = s.x.max(0) as usize;
                        let y0 = s.y.max(0) as usize;
                        let x1 = (s.x + s.w).max(0) as usize;
                        let y1 = (s.y + s.h).max(0) as usize;
                        for y in y0..y1.min(screen_h) {
                            for x in x0..x1.min(width_usize) {
                                buf[y * width_usize + x] = (r8 << 16) | (g8 << 8) | b8;
                            }
                        }
                    }
                    // Headers and items as simple labels and buttons
                    for cat in &layout.categories {
                        let screen_h = buf.height().get() as usize;
                        let data: &mut [u32] = &mut buf;
                        draw_label_slice(
                            data,
                            width_usize,
                            screen_h,
                            &font,
                            renderer.font_size,
                            cat.header_rect.x,
                            cat.header_rect.y + line_height - 4,
                            category_title(&cat.category),
                            0xE6FBFA,
                            (bg_r << 16) | (bg_g << 8) | bg_b,
                        );
                        draw_label_slice(
                            data,
                            width_usize,
                            screen_h,
                            &font,
                            renderer.font_size,
                            cat.add_rect.x,
                            cat.add_rect.y + line_height - 4,
                            "[Add]",
                            0x2FD8E5,
                            (bg_r << 16) | (bg_g << 8) | bg_b,
                        );
                        for item in &cat.items {
                            draw_label_slice(
                                data,
                                width_usize,
                                screen_h,
                                &font,
                                renderer.font_size,
                                s.x + 20,
                                item.edit_rect.y + line_height - 4,
                                &item.name,
                                0xE6FBFA,
                                (bg_r << 16) | (bg_g << 8) | bg_b,
                            );
                            draw_label_slice(
                                data,
                                width_usize,
                                screen_h,
                                &font,
                                renderer.font_size,
                                item.edit_rect.x,
                                item.edit_rect.y + line_height - 4,
                                "[edit]",
                                0x94F3F2,
                                (bg_r << 16) | (bg_g << 8) | bg_b,
                            );
                            draw_label_slice(
                                data,
                                width_usize,
                                screen_h,
                                &font,
                                renderer.font_size,
                                item.delete_rect.x,
                                item.delete_rect.y + line_height - 4,
                                "[del]",
                                0x94F3F2,
                                (bg_r << 16) | (bg_g << 8) | bg_b,
                            );
                        }
                    }
                    if let Some(modal) = &layout.modal {
                        // Modal bg
                        let m = modal.rect;
                        {
                            let screen_h = buf.height().get() as usize;
                            let color = 0x04192A;
                            let (r8, g8, b8) = ((color >> 16) & 0xFF, (color >> 8) & 0xFF, color & 0xFF);
                            let x0 = m.x.max(0) as usize;
                            let y0 = m.y.max(0) as usize;
                            let x1 = (m.x + m.w).max(0) as usize;
                            let y1 = (m.y + m.h).max(0) as usize;
                            for y in y0..y1.min(screen_h) {
                                for x in x0..x1.min(width_usize) {
                                    buf[y * width_usize + x] = (r8 << 16) | (g8 << 8) | b8;
                                }
                            }
                        }
                        // Title + Advanced
                        let title = match &modal.kind {
                            ui::ModalKind::Add(c) => format!("Add {:?}", c),
                            ui::ModalKind::Edit { category, name } => format!("Edit {:?}: {}", category, name),
                            ui::ModalKind::ConfirmDelete { category, name } => format!("Delete {:?}: {}", category, name),
                        };
                        let screen_h = buf.height().get() as usize;
                        let data: &mut [u32] = &mut buf;
                        draw_label_slice(
                            data,
                            width_usize,
                            screen_h,
                            &font,
                            renderer.font_size,
                            m.x + 10,
                            m.y + 10 + line_height,
                            &title,
                            0xE6FBFA,
                            0x04192A,
                        );
                        let adv = if modal.advanced_open { "[Advanced ▾]" } else { "[Advanced ▸]" };
                        draw_label_slice(
                            data,
                            width_usize,
                            screen_h,
                            &font,
                            renderer.font_size,
                            m.x + 10,
                            m.y + 10 + 3 * line_height,
                            adv,
                            0x2FD8E5,
                            0x04192A,
                        );
                    }
                    last_ui = Some(layout);
                }
                buf.present().expect("present");
                window.request_redraw();
            }
            _ => {}
        }
    })?;
    // Unreachable because run is ! returning, but signature requires Result
    #[allow(unreachable_code)]
    Ok(())
}

fn handle_key_event(key_event: &winit::event::KeyEvent, on_key_bytes: &Arc<dyn Fn(Vec<u8>) + Send + Sync>) {
    if key_event.state == ElementState::Pressed {
        let seq = match &key_event.logical_key {
            Key::Named(NamedKey::Enter) => vec![b'\r'],
            Key::Named(NamedKey::Tab) => vec![b'\t'],
            Key::Named(NamedKey::ArrowLeft) => b"\x1b[D".to_vec(),
            Key::Named(NamedKey::ArrowRight) => b"\x1b[C".to_vec(),
            Key::Named(NamedKey::ArrowUp) => b"\x1b[A".to_vec(),
            Key::Named(NamedKey::ArrowDown) => b"\x1b[B".to_vec(),
            Key::Named(NamedKey::Backspace) => vec![0x08],
            Key::Character(s) if !s.is_empty() => s.as_bytes().to_vec(),
            _ => Vec::new(),
        };
        if !seq.is_empty() {
            (on_key_bytes)(seq);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_label_slice(
    buf: &mut [u32],
    screen_width: usize,
    screen_height: usize,
    font: &fontdue::Font,
    font_size: f32,
    x: i32,
    baseline: i32,
    text: &str,
    fg: u32,
    bg: u32,
) {
    let (bg_r, bg_g, bg_b) = ((bg >> 16) & 0xFF, (bg >> 8) & 0xFF, bg & 0xFF);
    let (fg_r, fg_g, fg_b) = ((fg >> 16) & 0xFF, (fg >> 8) & 0xFF, fg & 0xFF);
    let mut pen_x = x;
    for ch in text.chars() {
        let (metrics, bitmap) = font.rasterize(ch, font_size);
        for iy in 0..metrics.height {
            let y0 = baseline + iy as i32 - metrics.ymin;
            if y0 < 0 || y0 as usize >= screen_height {
                continue;
            }
            for ix in 0..metrics.width {
                let x0 = pen_x + ix as i32 + metrics.xmin;
                if x0 < 0 || x0 as usize >= screen_width {
                    continue;
                }
                let alpha = bitmap[iy * metrics.width + ix] as u32;
                let r = (fg_r * alpha + bg_r * (255 - alpha)) / 255;
                let g = (fg_g * alpha + bg_g * (255 - alpha)) / 255;
                let b = (fg_b * alpha + bg_b * (255 - alpha)) / 255;
                let idx = y0 as usize * screen_width + x0 as usize;
                buf[idx] = (r << 16) | (g << 8) | b;
            }
        }
        pen_x += (font_size * 0.6) as i32;
    }
}

fn category_title(c: &ui::Category) -> &'static str {
    match c {
        ui::Category::Ssh => "SSH",
        ui::Category::Waypipe => "Waypipe",
        ui::Category::Tunnels => "Tunnels",
        ui::Category::X11 => "X11",
    }
}
