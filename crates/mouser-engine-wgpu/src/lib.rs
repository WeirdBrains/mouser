//! Mouser's wgpu renderer as an engine library (engine name `mouser`, ABI v2).
//!
//! The same pipeline as Mouser's built-in renderer (`crates/mouser-render/src/gpu.rs`):
//! every rect and glyph is one instanced quad, glyph coverage lives in an R8
//! atlas, images are textures, and blending matches the CPU canvas. The input
//! is the C ABI frame, so glyph bitmaps come from Mouser (keyed by
//! `MouserGlyph::key`) instead of a font stack. `tests/` and
//! `crates/mouser-render/tests/wgpu_engine_parity.rs` check it pixel by pixel
//! against the built-in renderer.
#![allow(clippy::missing_safety_doc)]

use mouser_engine_abi::*;
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void};

const SHADER: &str = r#"
struct U { sa: vec4<f32>, flags: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};
@vertex
fn vs(@builtin(vertex_index) vi: u32, @location(0) rect: vec4<f32>, @location(1) uvr: vec4<f32>, @location(2) color: vec4<f32>) -> VOut {
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let p = rect.xy + corner * rect.zw;
    var o: VOut;
    o.pos = vec4<f32>(p.x / u.sa.x * 2.0 - 1.0, 1.0 - p.y / u.sa.y * 2.0, 0.0, 1.0);
    o.uv = (uvr.xy + corner * uvr.zw) / u.sa.zw;
    o.color = color;
    return o;
}
@vertex
fn vs_img(@builtin(vertex_index) vi: u32, @location(0) rect: vec4<f32>, @location(1) uvr: vec4<f32>, @location(2) color: vec4<f32>) -> VOut {
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let p = rect.xy + corner * rect.zw;
    var o: VOut;
    o.pos = vec4<f32>(p.x / u.sa.x * 2.0 - 1.0, 1.0 - p.y / u.sa.y * 2.0, 0.0, 1.0);
    o.uv = uvr.xy + corner * uvr.zw;
    o.color = color;
    return o;
}
@fragment
fn fs_img(i: VOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, i.uv);
}
@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    let a = textureSample(tex, samp, i.uv).r;
    return vec4<f32>(i.color.rgb, i.color.a * a);
}
"#;

/// Floats per instance: rect (4), atlas rect (4), colour (4).
const INSTANCE_FLOATS: usize = 12;
const ATLAS_START: u32 = 1024;
const ATLAS_MAX: u32 = 4096;

enum Segment {
    Quads(std::ops::Range<u32>),
    Image(u64, u32),
}

struct Atlas {
    size: u32,
    texture: wgpu::Texture,
    x: u32,
    y: u32,
    row_h: u32,
    /// glyph key -> (x, y, w, h) in texels.
    slots: HashMap<u64, [u32; 4]>,
}

fn new_atlas(device: &wgpu::Device, queue: &wgpu::Queue, size: u32) -> Atlas {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mouser glyph atlas"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_region(queue, &texture, 0, 0, 2, 2, 2, &[255; 4]);
    Atlas { size, texture, x: 3, y: 0, row_h: 2, slots: HashMap::new() }
}

#[allow(clippy::too_many_arguments)]
fn write_region(queue: &wgpu::Queue, texture: &wgpu::Texture, x: u32, y: u32, w: u32, h: u32, stride: u32, data: &[u8]) {
    queue.write_texture(
        wgpu::ImageCopyTexture { texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
        data,
        wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(h) },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Opacity from an ABI v2 colour: the top byte is `255 - opacity` (0 = opaque).
fn opacity(c: u32) -> u32 {
    255 - (c >> 24)
}

/// The renderer, on a window surface or an offscreen texture.
pub struct Renderer {
    surface: Option<wgpu::Surface<'static>>,
    target: Option<wgpu::Texture>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    image_sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    atlas: Atlas,
    instances: wgpu::Buffer,
    capacity: usize,
    srgb: bool,
    textures: HashMap<u64, (wgpu::Texture, wgpu::BindGroup)>,
    /// Adapter and format, for logs.
    pub description: String,
}

impl Renderer {
    /// Draw into an offscreen `Rgba8Unorm` texture (the pixel host, tests).
    pub fn offscreen(width: u32, height: u32) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::all(), ..Default::default() });
        Self::with_target(&instance, None, width, height)
    }

    /// Present to a native window.
    ///
    /// # Safety
    /// The handles must be valid and outlive the renderer.
    pub unsafe fn on_window(
        display: raw_window_handle::RawDisplayHandle,
        window: raw_window_handle::RawWindowHandle,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::PRIMARY, ..Default::default() });
        // SAFETY: the caller guarantees the handles.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle { raw_display_handle: display, raw_window_handle: window })
        }
        .map_err(|e| format!("surface: {e}"))?;
        Self::with_target(&instance, Some(surface), width, height)
    }

    fn with_target(instance: &wgpu::Instance, surface: Option<wgpu::Surface<'static>>, width: u32, height: u32) -> Result<Self, String> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: surface.as_ref(),
            force_fallback_adapter: false,
        }))
        .ok_or("no compatible GPU adapter")?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("mouser"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            },
            None,
        ))
        .map_err(|e| format!("device: {e}"))?;
        let (format, alpha_mode) = match &surface {
            Some(surface) => {
                let caps = surface.get_capabilities(&adapter);
                let format = *caps.formats.iter().find(|f| !f.is_srgb()).or(caps.formats.first()).ok_or("surface has no formats")?;
                (format, caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto))
            }
            None => (wgpu::TextureFormat::Rgba8Unorm, wgpu::CompositeAlphaMode::Auto),
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        if let Some(surface) = &surface {
            surface.configure(&device, &config);
        }
        let target = surface.is_none().then(|| offscreen_target(&device, &config));
        let shader =
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("mouser"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&layout], push_constant_ranges: &[] });
        let make_pipeline = |vs: &str, fs: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mouser quads"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: vs,
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: (INSTANCE_FLOATS * 4) as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: fs,
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
            })
        };
        let pipeline = make_pipeline("vs", "fs");
        let image_pipeline = make_pipeline("vs_img", "fs_img");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let image_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let atlas = new_atlas(&device, &queue, ATLAS_START);
        let bind_group = bind(&device, &layout, &uniforms, &atlas.texture, &sampler);
        let capacity = 4096;
        let instances = instance_buffer(&device, capacity);
        let description = format!("{} ({:?}, {:?})", info.name, info.backend, format);
        Ok(Self {
            surface,
            target,
            device,
            queue,
            config,
            pipeline,
            image_pipeline,
            layout,
            sampler,
            image_sampler,
            uniforms,
            bind_group,
            atlas,
            instances,
            capacity,
            srgb: format.is_srgb(),
            textures: HashMap::new(),
            description,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        match &self.surface {
            Some(s) => s.configure(&self.device, &self.config),
            None => self.target = Some(offscreen_target(&self.device, &self.config)),
        }
    }

    /// Drop a cached glyph or image upload.
    pub fn evict(&mut self, key: u64) {
        self.textures.remove(&key);
        self.atlas.slots.remove(&key);
    }

    /// The offscreen pixels as `0x00RRGGBB` (`None` on a window).
    pub fn read_pixels(&self) -> Option<Vec<u32>> {
        Some(read_texture(&self.device, &self.queue, self.target.as_ref()?, self.config.width, self.config.height))
    }

    pub fn max_texture(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    fn reset_atlas(&mut self, size: u32) {
        self.atlas = new_atlas(&self.device, &self.queue, size);
        self.bind_group = bind(&self.device, &self.layout, &self.uniforms, &self.atlas.texture, &self.sampler);
    }

    fn slot(&mut self, g: &MouserGlyph) -> Option<[u32; 4]> {
        if let Some(s) = self.atlas.slots.get(&g.key) {
            return Some(*s);
        }
        let (w, h) = (g.w, g.h);
        let a = &mut self.atlas;
        if w == 0 || h == 0 || g.coverage.is_null() {
            a.slots.insert(g.key, [0, 0, 0, 0]);
            return Some([0, 0, 0, 0]);
        }
        if a.x + w + 1 > a.size {
            a.x = 0;
            a.y += a.row_h + 1;
            a.row_h = 0;
        }
        if a.y + h > a.size {
            return None;
        }
        // SAFETY: coverage holds `stride * h` bytes during draw (ABI).
        let data = unsafe { std::slice::from_raw_parts(g.coverage, (g.stride * h) as usize) };
        // Repack to a tight `w`-byte stride (wgpu wants a whole row per stride).
        let tight: Vec<u8> = data.chunks(g.stride as usize).flat_map(|r| &r[..w as usize]).copied().collect();
        let slot = [a.x, a.y, w, h];
        write_region(&self.queue, &a.texture, a.x, a.y, w, h, w, &tight);
        a.x += w + 1;
        a.row_h = a.row_h.max(h);
        a.slots.insert(g.key, slot);
        Some(slot)
    }

    fn color(&self, c: u32) -> [f32; 3] {
        let ch = |s: u32| ((c >> s) & 0xFF) as f32 / 255.0;
        let rgb = [ch(16), ch(8), ch(0)];
        if self.srgb { rgb.map(srgb_to_linear) } else { rgb }
    }

    fn image_texture(&mut self, img: &MouserImage) {
        if self.textures.contains_key(&img.key) {
            return;
        }
        let format = if self.srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
        let size = wgpu::Extent3d { width: img.img_w, height: img.img_h, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mouser image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // SAFETY: rgba holds img_w * img_h * 4 bytes during draw (ABI).
        let rgba = unsafe { std::slice::from_raw_parts(img.rgba, (img.img_w * img.img_h * 4) as usize) };
        self.queue.write_texture(
            wgpu::ImageCopyTexture { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            rgba,
            wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(img.img_w * 4), rows_per_image: Some(img.img_h) },
            size,
        );
        let bind = bind(&self.device, &self.layout, &self.uniforms, &texture, &self.image_sampler);
        self.textures.insert(img.key, (texture, bind));
    }

    /// # Safety
    /// The frame's pointers must be valid (true inside the ABI's `draw`).
    unsafe fn build(&mut self, frame: &MouserFrame) -> Option<(Vec<f32>, Vec<Segment>)> {
        // SAFETY: the caller guarantees the frame.
        let (ops, rects, glyphs, images) = unsafe { frame.parts() };
        let max = self.device.limits().max_texture_dimension_2d;
        let mut data = Vec::with_capacity(ops.len() * INSTANCE_FLOATS);
        let mut segs = Vec::new();
        let mut start = 0usize;
        for op in ops {
            let i = op.index as usize;
            if op.kind == OP_IMAGE {
                let Some(img) = images.get(i) else { continue };
                if img.img_w == 0 || img.img_h == 0 || img.img_w > max || img.img_h > max || img.rgba.is_null() {
                    continue;
                }
                self.image_texture(img);
                let n = data.len() / INSTANCE_FLOATS;
                if n > start {
                    segs.push(Segment::Quads(start as u32..n as u32));
                }
                let (iw, ih) = (img.img_w as f32, img.img_h as f32);
                data.extend_from_slice(&[img.x as f32, img.y as f32, img.w as f32, img.h as f32]);
                data.extend_from_slice(&[img.src_x as f32 / iw, img.src_y as f32 / ih, img.src_w as f32 / iw, img.src_h as f32 / ih]);
                data.extend_from_slice(&[1.0, 1.0, 1.0, 1.0]);
                segs.push(Segment::Image(img.key, n as u32));
                start = n + 1;
                continue;
            }
            let (rect, uv, color) = match op.kind {
                OP_RECT => {
                    let Some(r) = rects.get(i) else { continue };
                    ([r.x as f32, r.y as f32, r.w as f32, r.h as f32], [0.5, 0.5, 0.0, 0.0], r.color)
                }
                OP_GLYPH => {
                    let Some(g) = glyphs.get(i) else { continue };
                    let s = self.slot(g)?;
                    if s[2] == 0 {
                        continue;
                    }
                    let f = s.map(|v| v as f32);
                    ([g.x as f32, g.y as f32, f[2], f[3]], f, g.color)
                }
                _ => continue,
            };
            let c = self.color(color);
            data.extend_from_slice(&rect);
            data.extend_from_slice(&uv);
            data.extend_from_slice(&[c[0], c[1], c[2], opacity(color) as f32 / 255.0]);
        }
        let n = data.len() / INSTANCE_FLOATS;
        if n > start {
            segs.push(Segment::Quads(start as u32..n as u32));
        }
        Some((data, segs))
    }

    /// Draw (and present on a window). `DRAW_AGAIN` when the surface was reset.
    ///
    /// # Safety
    /// The frame's pointers must be valid for the call.
    pub unsafe fn draw(&mut self, frame: &MouserFrame) -> c_int {
        // SAFETY: forwarded.
        let built = match unsafe { self.build(frame) } {
            Some(d) => Some(d),
            None => {
                let size = (self.atlas.size * 2).min(ATLAS_MAX).min(self.device.limits().max_texture_dimension_2d);
                self.reset_atlas(size);
                // SAFETY: forwarded.
                unsafe { self.build(frame) }
            }
        };
        let Some((data, segs)) = built else { return DRAW_OK };
        // SAFETY: forwarded.
        let (_, _, _, images) = unsafe { frame.parts() };
        let used: std::collections::HashSet<u64> = images.iter().map(|i| i.key).collect();
        self.textures.retain(|k, _| used.contains(k));
        let count = data.len() / INSTANCE_FLOATS;
        if count > self.capacity {
            self.capacity = count.next_power_of_two();
            self.instances = instance_buffer(&self.device, self.capacity);
        }
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_ne_bytes()).collect();
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.instances, 0, &bytes);
        }
        let u = [self.config.width as f32, self.config.height as f32, self.atlas.size as f32, self.atlas.size as f32, 0.0, 0.0, 0.0, 0.0];
        let ub: Vec<u8> = u.iter().flat_map(|f| f.to_ne_bytes()).collect();
        self.queue.write_buffer(&self.uniforms, 0, &ub);
        let out = match &self.surface {
            None => None,
            Some(surface) => match surface.get_current_texture() {
                Ok(f) => Some(f),
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                    surface.configure(&self.device, &self.config);
                    return DRAW_AGAIN;
                }
                Err(_) => return DRAW_AGAIN,
            },
        };
        let texture = match (&out, &self.target) {
            (Some(f), _) => &f.texture,
            (None, Some(t)) => t,
            (None, None) => return DRAW_AGAIN,
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bg = self.color(frame.clear);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: bg[0] as f64, g: bg[1] as f64, b: bg[2] as f64, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if count > 0 {
                pass.set_vertex_buffer(0, self.instances.slice(..(count * INSTANCE_FLOATS * 4) as u64));
                for seg in &segs {
                    match seg {
                        Segment::Quads(r) => {
                            pass.set_pipeline(&self.pipeline);
                            pass.set_bind_group(0, &self.bind_group, &[]);
                            pass.draw(0..4, r.clone());
                        }
                        Segment::Image(key, i) => {
                            if let Some((_, bind)) = self.textures.get(key) {
                                pass.set_pipeline(&self.image_pipeline);
                                pass.set_bind_group(0, bind, &[]);
                                pass.draw(0..4, *i..*i + 1);
                            }
                        }
                    }
                }
            }
        }
        self.queue.submit(Some(encoder.finish()));
        if let Some(f) = out {
            f.present();
        }
        DRAW_OK
    }
}

fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    texture: &wgpu::Texture,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mouser instances"),
        size: (capacity * INSTANCE_FLOATS * 4) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn offscreen_target(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mouser offscreen"),
        size: wgpu::Extent3d { width: config.width, height: config.height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: config.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn read_texture(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, w: u32, h: u32) -> Vec<u32> {
    let row = (w * 4).div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mouser readback"),
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    enc.copy_texture_to_buffer(
        wgpu::ImageCopyTexture { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::ImageCopyBuffer { buffer: &buf, layout: wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit(Some(enc.finish()));
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::Maintain::Wait);
    let data = slice.get_mapped_range();
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h as usize {
        let r = &data[y * row as usize..][..w as usize * 4];
        out.extend(r.as_chunks::<4>().0.iter().map(|p| (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32));
    }
    out
}

// ---- C ABI -----------------------------------------------------------------

/// An engine instance: the renderer, plus the pixel buffer when hosted.
struct Engine {
    r: Renderer,
    pixels: Option<*mut MouserPixels>,
}

fn raw_handles(s: &MouserSurface) -> Result<(raw_window_handle::RawDisplayHandle, raw_window_handle::RawWindowHandle), String> {
    use raw_window_handle::*;
    use std::ptr::NonNull;
    let nn = |p: *mut c_void, what: &str| NonNull::new(p).ok_or(format!("null {what}"));
    Ok(match s.kind {
        SURFACE_WAYLAND => (
            RawDisplayHandle::Wayland(WaylandDisplayHandle::new(nn(s.display, "wl_display")?)),
            RawWindowHandle::Wayland(WaylandWindowHandle::new(nn(s.window, "wl_surface")?)),
        ),
        SURFACE_XLIB => (
            RawDisplayHandle::Xlib(XlibDisplayHandle::new(NonNull::new(s.display), 0)),
            RawWindowHandle::Xlib(XlibWindowHandle::new(s.window as usize as _)),
        ),
        SURFACE_XCB => (
            RawDisplayHandle::Xcb(XcbDisplayHandle::new(NonNull::new(s.display), 0)),
            RawWindowHandle::Xcb(XcbWindowHandle::new(std::num::NonZeroU32::new(s.window as usize as u32).ok_or("null xcb window")?)),
        ),
        SURFACE_APPKIT => {
            (RawDisplayHandle::AppKit(AppKitDisplayHandle::new()), RawWindowHandle::AppKit(AppKitWindowHandle::new(nn(s.window, "NSView")?)))
        }
        SURFACE_WIN32 => {
            let mut w = Win32WindowHandle::new(std::num::NonZeroIsize::new(s.window as isize).ok_or("null HWND")?);
            w.hinstance = std::num::NonZeroIsize::new(s.display as isize);
            (RawDisplayHandle::Windows(WindowsDisplayHandle::new()), RawWindowHandle::Win32(w))
        }
        k => return Err(format!("unsupported surface kind {k}")),
    })
}

unsafe extern "C" fn create(surface: *const MouserSurface, w: u32, h: u32, err: *mut c_char, err_len: usize) -> *mut c_void {
    let fail = |m: String| {
        // SAFETY: Mouser passes a valid error buffer.
        unsafe { write_err(err, err_len, &m) };
        std::ptr::null_mut()
    };
    // SAFETY: Mouser passes a valid surface.
    let Some(s) = (unsafe { surface.as_ref() }) else { return fail("no surface".into()) };
    let made = if s.kind == SURFACE_PIXELS {
        Renderer::offscreen(w, h).map(|r| Engine { r, pixels: Some(s.window as *mut MouserPixels) })
    } else {
        match raw_handles(s) {
            // SAFETY: the window outlives the engine (ABI).
            Ok((d, win)) => unsafe { Renderer::on_window(d, win, w, h) }.map(|r| Engine { r, pixels: None }),
            Err(e) => Err(e),
        }
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| made)) {
        Ok(Ok(e)) => Box::into_raw(Box::new(e)) as *mut c_void,
        Ok(Err(e)) => fail(e),
        Err(_) => fail("wgpu panicked while starting".into()),
    }
}

unsafe fn engine<'a>(p: *mut c_void) -> Option<&'a mut Engine> {
    // SAFETY: `p` came from `create`.
    unsafe { (p as *mut Engine).as_mut() }
}

unsafe extern "C" fn resize(e: *mut c_void, w: u32, h: u32) {
    if let Some(e) = unsafe { engine(e) } {
        e.r.resize(w, h);
    }
}

unsafe extern "C" fn draw(e: *mut c_void, frame: *const MouserFrame) -> c_int {
    let (Some(e), Some(frame)) = (unsafe { engine(e) }, unsafe { frame.as_ref() }) else { return -1 };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: valid during draw (ABI).
        let r = unsafe { e.r.draw(frame) };
        if let (Some(px), Some(out)) = (e.pixels, e.r.read_pixels()) {
            // SAFETY: Mouser's pixel buffer, valid during draw.
            if let Some(px) = unsafe { px.as_ref() } {
                let (w, h) = (px.width.min(e.r.config.width) as usize, px.height.min(e.r.config.height) as usize);
                for y in 0..h {
                    // SAFETY: `stride * height` pixels.
                    let dst = unsafe { std::slice::from_raw_parts_mut(px.data.add(y * px.stride as usize), w) };
                    dst.copy_from_slice(&out[y * e.r.config.width as usize..][..w]);
                }
            }
        }
        r
    }));
    r.unwrap_or(-2)
}

unsafe extern "C" fn destroy(e: *mut c_void) {
    if !e.is_null() {
        // SAFETY: from `create`.
        drop(unsafe { Box::from_raw(e as *mut Engine) });
    }
}

unsafe extern "C" fn caps(e: *mut c_void, caps: *mut MouserCaps) {
    if let (Some(e), Some(c)) = (unsafe { engine(e) }, unsafe { caps.as_mut() }) {
        c.flags = CAP_DIRECT_SURFACE | CAP_PIXELS;
        c.formats = FORMAT_XRGB8888;
        c.max_texture = e.r.max_texture();
    }
}

unsafe extern "C" fn evict(e: *mut c_void, key: u64) {
    if let Some(e) = unsafe { engine(e) } {
        e.r.evict(key);
    }
}

static TABLE_V2: MouserEngineV2 = MouserEngineV2 {
    base: MouserEngineV1 {
        abi: MOUSER_ENGINE_ABI_V2,
        size: std::mem::size_of::<MouserEngineV2>() as u32,
        name: c"mouser".as_ptr(),
        create,
        resize,
        draw,
        destroy,
    },
    abi_min: MOUSER_ENGINE_ABI_V2,
    abi_max: MOUSER_ENGINE_ABI_V2,
    caps: Some(caps),
    set_scale: None,
    evict: Some(evict),
    set_vsync: None,
};

/// The v2 entry point (this engine needs v2: translucent colours).
#[unsafe(no_mangle)]
pub extern "C" fn mouser_engine_v2() -> *const MouserEngineV2 {
    &TABLE_V2
}
