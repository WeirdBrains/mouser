//! wgpu renderer: glyphs rasterised by fontdue into an R8 atlas texture, and
//! every rect and glyph of a [`DisplayList`] drawn as one instanced quad.
//! Blending matches the CPU canvas (`src * a + dst * (1 - a)`).

use crate::text::{DisplayList, DrawOp, FontStack};
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::Arc;
use winit::window::Window;

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

/// A run of glyph/rect quads, or one image quad (texture key, instance index).
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
    /// char -> (x, y, w, h) in texels.
    slots: HashMap<char, [u32; 4]>,
    generation: u64,
}

pub struct GpuPainter {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    atlas: Atlas,
    instances: wgpu::Buffer,
    capacity: usize,
    srgb: bool,
    image_pipeline: wgpu::RenderPipeline,
    image_sampler: wgpu::Sampler,
    /// Image textures by upload key; dropped when a frame no longer uses them.
    textures: HashMap<u64, (wgpu::Texture, wgpu::BindGroup)>,
    /// Human-readable adapter description for the startup log.
    pub description: String,
}

fn new_atlas(device: &wgpu::Device, queue: &wgpu::Queue, size: u32, generation: u64) -> Atlas {
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
    // A solid 2x2 block at the origin: rect fills sample full coverage there.
    write_region(queue, &texture, 0, 0, 2, 2, &[255; 4]);
    Atlas { size, texture, x: 3, y: 0, row_h: 2, slots: HashMap::new(), generation }
}

fn write_region(queue: &wgpu::Queue, texture: &wgpu::Texture, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
    queue.write_texture(
        wgpu::ImageCopyTexture { texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
        data,
        wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(w), rows_per_image: Some(h) },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

impl GpuPainter {
    /// Set up wgpu on `window`; errors when no adapter, device or surface is available.
    pub fn new(window: Arc<Window>, width: u32, height: u32) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::PRIMARY, ..Default::default() });
        let surface = instance.create_surface(window).map_err(|e| anyhow!("surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| anyhow!("no compatible GPU adapter"))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("mouser"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            },
            None,
        ))
        .map_err(|e| anyhow!("device: {e}"))?;
        let caps = surface.get_capabilities(&adapter);
        let format = *caps.formats.iter().find(|f| !f.is_srgb()).or(caps.formats.first()).ok_or_else(|| anyhow!("surface has no formats"))?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

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
        let atlas = new_atlas(&device, &queue, ATLAS_START, 0);
        let bind_group = Self::bind(&device, &layout, &uniforms, &atlas, &sampler);
        let capacity = 4096;
        let instances = Self::instance_buffer(&device, capacity);
        let description = format!("{} ({:?}, {:?})", info.name, info.backend, format);
        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            layout,
            sampler,
            uniforms,
            bind_group,
            atlas,
            instances,
            capacity,
            srgb: format.is_srgb(),
            image_pipeline,
            image_sampler,
            textures: HashMap::new(),
            description,
        })
    }

    fn bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniforms: &wgpu::Buffer,
        atlas: &Atlas,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        let view = atlas.texture.create_view(&wgpu::TextureViewDescriptor::default());
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

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    fn reset_atlas(&mut self, size: u32, generation: u64) {
        self.atlas = new_atlas(&self.device, &self.queue, size, generation);
        self.bind_group = Self::bind(&self.device, &self.layout, &self.uniforms, &self.atlas, &self.sampler);
    }

    /// Atlas slot for `ch`, uploading it if needed. `None` means the atlas is full.
    fn slot(&mut self, fonts: &mut FontStack, ch: char) -> Option<[u32; 4]> {
        if let Some(s) = self.atlas.slots.get(&ch) {
            return Some(*s);
        }
        let g = fonts.glyph(ch);
        let (w, h) = (g.metrics.width as u32, g.metrics.height as u32);
        let a = &mut self.atlas;
        if w == 0 || h == 0 {
            a.slots.insert(ch, [0, 0, 0, 0]);
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
        let slot = [a.x, a.y, w, h];
        write_region(&self.queue, &a.texture, a.x, a.y, w, h, &g.bitmap);
        a.x += w + 1;
        a.row_h = a.row_h.max(h);
        a.slots.insert(ch, slot);
        Some(slot)
    }

    fn color(&self, c: u32) -> [f32; 3] {
        let ch = |s: u32| ((c >> s) & 0xFF) as f32 / 255.0;
        let rgb = [ch(16), ch(8), ch(0)];
        if self.srgb { rgb.map(srgb_to_linear) } else { rgb }
    }

    /// Texture + bind group for an image, uploading it on first use.
    fn image_texture(&mut self, img: &crate::frame::ImageDraw) {
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
        self.queue.write_texture(
            wgpu::ImageCopyTexture { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &img.rgba,
            wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(img.img_w * 4), rows_per_image: Some(img.img_h) },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.image_sampler) },
            ],
        });
        self.textures.insert(img.key, (texture, bind));
    }

    /// Build instance data and draw segments for `list`; `None` if the atlas overflowed.
    fn build(&mut self, list: &DisplayList, fonts: &mut FontStack) -> Option<(Vec<f32>, Vec<Segment>)> {
        let mut data = Vec::with_capacity(list.ops.len() * INSTANCE_FLOATS);
        let mut segs = Vec::new();
        let mut start = 0usize;
        for op in &list.ops {
            if let DrawOp::Image { index, x, y } = *op {
                let img = &list.images[index];
                if img.img_w == 0
                    || img.img_h == 0
                    || img.img_w > self.device.limits().max_texture_dimension_2d
                    || img.img_h > self.device.limits().max_texture_dimension_2d
                {
                    continue;
                }
                self.image_texture(img);
                let n = data.len() / INSTANCE_FLOATS;
                if n > start {
                    segs.push(Segment::Quads(start as u32..n as u32));
                }
                let (sx, sy, sw, sh) = img.src;
                let (iw, ih) = (img.img_w as f32, img.img_h as f32);
                data.extend_from_slice(&[x as f32, y as f32, img.width as f32, img.height as f32]);
                data.extend_from_slice(&[sx as f32 / iw, sy as f32 / ih, sw as f32 / iw, sh as f32 / ih]);
                data.extend_from_slice(&[1.0, 1.0, 1.0, 1.0]);
                segs.push(Segment::Image(img.key, n as u32));
                start = n + 1;
                continue;
            }
            let (rect, uv, color) = match *op {
                DrawOp::Rect { x, y, w, h, color } => ([x as f32, y as f32, w as f32, h as f32], [0.5, 0.5, 0.0, 0.0], color),
                DrawOp::Glyph { x, y, ch, color } => {
                    let s = self.slot(fonts, ch)?;
                    if s[2] == 0 {
                        continue;
                    }
                    let f = s.map(|v| v as f32);
                    ([x as f32, y as f32, f[2], f[3]], f, color)
                }
                DrawOp::Image { .. } => unreachable!(),
            };
            let c = self.color(color);
            data.extend_from_slice(&rect);
            data.extend_from_slice(&uv);
            data.extend_from_slice(&[c[0], c[1], c[2], 1.0]);
        }
        let n = data.len() / INSTANCE_FLOATS;
        if n > start {
            segs.push(Segment::Quads(start as u32..n as u32));
        }
        Some((data, segs))
    }

    /// Draw and present one frame. Returns false if the surface had to be
    /// reconfigured and the frame should be redrawn.
    pub fn draw(&mut self, list: &DisplayList, fonts: &mut FontStack) -> bool {
        if self.atlas.generation != fonts.generation {
            self.reset_atlas(self.atlas.size, fonts.generation);
        }
        let (data, segs) = match self.build(list, fonts) {
            Some(d) => d,
            None => {
                // Atlas full: start over, bigger if allowed.
                let size = (self.atlas.size * 2).min(ATLAS_MAX).min(self.device.limits().max_texture_dimension_2d);
                self.reset_atlas(size, fonts.generation);
                match self.build(list, fonts) {
                    Some(d) => d,
                    None => return true,
                }
            }
        };
        let used: std::collections::HashSet<u64> = list.images.iter().map(|i| i.key).collect();
        self.textures.retain(|k, _| used.contains(k));
        let count = data.len() / INSTANCE_FLOATS;
        if count > self.capacity {
            self.capacity = count.next_power_of_two();
            self.instances = Self::instance_buffer(&self.device, self.capacity);
        }
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_ne_bytes()).collect();
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.instances, 0, &bytes);
        }
        let u = [self.config.width as f32, self.config.height as f32, self.atlas.size as f32, self.atlas.size as f32, 0.0, 0.0, 0.0, 0.0];
        let ub: Vec<u8> = u.iter().flat_map(|f| f.to_ne_bytes()).collect();
        self.queue.write_buffer(&self.uniforms, 0, &ub);

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            Err(e) => {
                log::warn!("gpu frame skipped: {e}");
                return false;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bg = self.color(list.clear);
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
        frame.present();
        true
    }
}
