#![allow(deprecated)]
use anyhow::Result;
use fontdb::Database;
use fontdue::Font;
use softbuffer::{Context, Surface};
use std::sync::Arc;
use winit::event::{ElementState, Event, MouseScrollDelta, WindowEvent};
use winit::event_loop::EventLoop;
use winit::keyboard::{Key, NamedKey};

pub struct RenderHooks {
    pub on_key_bytes: Arc<dyn Fn(Vec<u8>) + Send + Sync>,
    pub on_resize: Arc<dyn Fn(u16, u16) + Send + Sync>,
    pub fetch_lines: Arc<dyn Fn(usize, usize) -> Vec<String> + Send + Sync>,
    pub get_colors: Arc<dyn Fn() -> (u32, u32) + Send + Sync>, // (bg, fg) as 0xRRGGBB
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
    let window = event_loop.create_window(winit::window::WindowAttributes::default().with_title("Mouser"))?;
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

    event_loop.run(|event, elwt| {
        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => elwt.exit(),
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
