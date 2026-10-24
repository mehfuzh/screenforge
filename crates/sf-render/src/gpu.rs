use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use sf_project::schema::common::Rect;

use crate::{BackgroundLayer, CameraGrade, Color, FrameDesc, Layer, LutTexture, MaskLayer, ShadowLayer, Texture};

/// Device + queue, either created headless (export, tests) or borrowed from egui/eframe.
#[derive(Clone)]
pub struct Gpu {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
}

impl Gpu {
    pub fn headless() -> anyhow::Result<Self> {
        pollster::block_on(async {
            let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
            // WGPU_BACKEND=vulkan|dx12|metal|gl narrows the choice (a broken driver or Vulkan
            // layer can crash the whole process before an adapter is even listed)
            desc.backends = wgpu::Backends::from_env().unwrap_or(Self::default_backends());
            let instance = wgpu::Instance::new(desc);
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() })
                .await?;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("screenforge"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                    ..Default::default()
                })
                .await?;
            Ok(Gpu { device: Arc::new(device), queue: Arc::new(queue) })
        })
    }

    /// Backends tried when `WGPU_BACKEND` is not set: Direct3D 12 on Windows (the Vulkan path
    /// crashes inside some drivers/layers before an adapter is listed), the platform's primary
    /// ones elsewhere.
    pub fn default_backends() -> wgpu::Backends {
        if cfg!(windows) { wgpu::Backends::DX12 } else { wgpu::Backends::PRIMARY }
    }

    pub fn from_parts(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        Gpu { device, queue }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    out_size: [f32; 2],
    pivot: [f32; 2],
    dst: [f32; 4],
    uv: [f32; 4],
    radius: f32,
    mode: u32,
    blur: f32,
    alpha: f32,
    color: [f32; 4],
    grad_start: [f32; 2],
    grad_end: [f32; 2],
    rotation: f32,
    mirror: u32,
    stop_count: u32,
    /// 1 = apply the `grade_*` block (mode 1 only).
    grade: u32,
    extra: [f32; 4],
    stops: [[f32; 4]; 8],
    grade_lift: [f32; 4],
    grade_gamma: [f32; 4],
    grade_gain: [f32; 4],
    grade_offset: [f32; 4],
    /// temperature, tint, contrast, pivot
    grade_a: [f32; 4],
    /// shadows, highlights, color boost, saturation
    grade_b: [f32; 4],
    /// hue (turns), lut mix, grain, sharpen
    grade_c: [f32; 4],
    /// lut size, texel w, texel h, grain seed
    grade_d: [f32; 4],
}

impl Default for Params {
    fn default() -> Self {
        Params {
            out_size: [1.0, 1.0],
            pivot: [0.0, 0.0],
            dst: [0.0, 0.0, 1.0, 1.0],
            uv: [0.0, 0.0, 1.0, 1.0],
            radius: 0.0,
            mode: 0,
            blur: 0.0,
            alpha: 1.0,
            color: [0.0, 0.0, 0.0, 1.0],
            grad_start: [0.0, 0.0],
            grad_end: [1.0, 1.0],
            rotation: 0.0,
            mirror: 0,
            stop_count: 0,
            grade: 0,
            extra: [0.0; 4],
            stops: [[0.0; 4]; 8],
            grade_lift: [0.0; 4],
            grade_gamma: [1.0; 4],
            grade_gain: [1.0; 4],
            grade_offset: [0.0; 4],
            grade_a: [0.0, 0.0, 1.0, 0.435],
            grade_b: [0.0, 0.0, 0.0, 1.0],
            grade_c: [0.0; 4],
            grade_d: [2.0, 0.0, 0.0, 0.0],
        }
    }
}

impl Params {
    fn with_grade(mut self, g: &CameraGrade, lut_size: u32, texel: [f32; 2]) -> Params {
        self.grade = 1;
        self.grade_lift = [g.lift[0], g.lift[1], g.lift[2], 0.0];
        self.grade_gamma = [g.gamma_exp[0], g.gamma_exp[1], g.gamma_exp[2], 1.0];
        self.grade_gain = [g.gain[0], g.gain[1], g.gain[2], 1.0];
        self.grade_offset = [g.offset[0], g.offset[1], g.offset[2], 0.0];
        self.grade_a = [g.temperature, g.tint, g.contrast, g.pivot];
        self.grade_b = [g.shadows, g.highlights, g.color_boost, g.saturation];
        self.grade_c = [g.hue_turns, g.lut_mix, g.grain, g.sharpen];
        self.grade_d = [lut_size as f32, texel[0], texel[1], g.seed];
        self
    }
}

const MODE_SOLID: u32 = 0;
const MODE_TEXTURE: u32 = 1;
const MODE_GRADIENT: u32 = 2;
const MODE_SHADOW: u32 = 3;
const MODE_SPRITE: u32 = 4;
const MODE_BLUR: u32 = 5;
const MODE_DIM: u32 = 6;
const MODE_RING: u32 = 7;

/// Maximum draws per frame (uniform ring size).
const MAX_DRAWS: usize = 256;

pub struct Compositor {
    gpu: Gpu,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
    uniform_stride: u64,
    white: Arc<Texture>,
    identity_lut: Arc<LutTexture>,
    target: Option<Texture>,
    readback: Option<(wgpu::Buffer, u32)>,
}

impl Compositor {
    pub fn new(gpu: Gpu) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let uniform_stride = {
            let align = device.limits().min_uniform_buffer_offset_alignment as u64;
            let sz = std::mem::size_of::<Params>() as u64;
            sz.div_ceil(align) * align
        };
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Params>() as u64) },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D3, multisampled: false },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bind_layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState { format: Texture::FORMAT, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear-clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("composite-uniforms"),
            size: uniform_stride * MAX_DRAWS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let white = Texture::from_rgba(device, &gpu.queue, 1, 1, &[255, 255, 255, 255], "white");
        let identity_lut = LutTexture::identity(device, &gpu.queue);
        Compositor { gpu, pipeline, bind_layout, sampler, uniform_buf, uniform_stride, white, identity_lut, target: None, readback: None }
    }

    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    pub fn target(&self) -> Option<&Texture> {
        self.target.as_ref()
    }

    fn ensure_target(&mut self, w: u32, h: u32) {
        if self.target.as_ref().map(|t| (t.width, t.height)) != Some((w, h)) {
            self.target = Some(Texture::new(&self.gpu.device, w, h, "composite-target"));
            self.readback = None;
        }
    }

    /// Render `desc` into the internal target texture.
    pub fn render(&mut self, desc: &FrameDesc) {
        self.ensure_target(desc.width, desc.height);
        let out_size = [desc.width as f32, desc.height as f32];
        let mut draws: Vec<(Params, Arc<Texture>, Option<Arc<LutTexture>>)> = Vec::with_capacity(desc.layers.len() * 2 + 1);

        // background
        match &desc.background {
            BackgroundLayer::Transparent => {}
            BackgroundLayer::Color(c) => draws.push((Params { out_size, mode: MODE_SOLID, dst: [0.0, 0.0, out_size[0], out_size[1]], color: color4(*c), ..Default::default() }, self.white.clone(), None)),
            BackgroundLayer::Gradient { start, end, stops } => {
                let mut p = Params { out_size, mode: MODE_GRADIENT, dst: [0.0, 0.0, out_size[0], out_size[1]], grad_start: *start, grad_end: *end, ..Default::default() };
                for (i, (c, at)) in stops.iter().take(8).enumerate() {
                    p.stops[i] = [c.r, c.g, c.b, *at];
                }
                p.stop_count = stops.len().min(8) as u32;
                draws.push((p, self.white.clone(), None));
            }
            BackgroundLayer::Image { texture, blur: _ } => {
                // cover fit
                let (tw, th) = (texture.width as f32, texture.height as f32);
                let scale = (out_size[0] / tw).max(out_size[1] / th);
                let (vw, vh) = (out_size[0] / (tw * scale), out_size[1] / (th * scale));
                let (u0, v0) = ((1.0 - vw) / 2.0, (1.0 - vh) / 2.0);
                draws.push((Params { out_size, mode: MODE_TEXTURE, dst: [0.0, 0.0, out_size[0], out_size[1]], uv: [u0, v0, u0 + vw, v0 + vh], ..Default::default() }, texture.clone(), None));
            }
        }

        for layer in &desc.layers {
            match layer {
                Layer::Screen(s) => {
                    if let Some(sh) = s.shadow {
                        draws.push((shadow_params(out_size, &s.dst, s.corner_radius, sh), self.white.clone(), None));
                    }
                    draws.push((Params { out_size, mode: MODE_TEXTURE, dst: rect4(&s.dst), uv: uv4(&s.src01), radius: s.corner_radius, ..Default::default() }, s.texture.clone(), None));
                    if let Some((size, color)) = s.inset {
                        if size > 0.0 {
                            // Draw the ring as a solid rounded rect behind a transparent-ish center:
                            // approximate by an outer solid rounded rect of `color` minus inner – done
                            // with two solids (outer colour, then re-draw screen slightly inset).
                            draws.push((Params { out_size, mode: MODE_SOLID, dst: rect4(&s.dst), radius: s.corner_radius, color: color4(color), ..Default::default() }, self.white.clone(), None));
                            let inner = Rect { x: s.dst.x + size as f64, y: s.dst.y + size as f64, width: s.dst.width - 2.0 * size as f64, height: s.dst.height - 2.0 * size as f64 };
                            draws.push((Params { out_size, mode: MODE_TEXTURE, dst: rect4(&inner), uv: uv4(&s.src01), radius: (s.corner_radius - size).max(0.0), ..Default::default() }, s.texture.clone(), None));
                        }
                    }
                }
                Layer::Camera(c) => {
                    if let Some(sh) = c.shadow {
                        draws.push((shadow_params(out_size, &c.dst, c.corner_radius, sh), self.white.clone(), None));
                    }
                    let mut p = Params { out_size, mode: MODE_TEXTURE, dst: rect4(&c.dst), uv: uv4(&c.src01), radius: c.corner_radius, mirror: c.mirror as u32, ..Default::default() };
                    let mut lut = None;
                    if let Some(g) = c.grade.as_ref().filter(|g| !g.is_neutral()) {
                        let lut_size = c.lut.as_ref().map(|l| l.size).unwrap_or(2);
                        lut = c.lut.clone();
                        p = p.with_grade(g, lut_size, [1.0 / c.texture.width.max(1) as f32, 1.0 / c.texture.height.max(1) as f32]);
                    }
                    draws.push((p, c.texture.clone(), lut));
                }
                Layer::Sprite(sp) => {
                    draws.push((Params { out_size, mode: MODE_SPRITE, dst: rect4(&sp.dst), pivot: sp.pivot, rotation: sp.rotation_rad, alpha: sp.opacity, ..Default::default() }, sp.texture.clone(), None));
                }
                Layer::Solid(so) => {
                    if let Some(sh) = so.shadow {
                        draws.push((shadow_params(out_size, &so.dst, so.corner_radius, sh), self.white.clone(), None));
                    }
                    draws.push((Params { out_size, mode: MODE_SOLID, dst: rect4(&so.dst), radius: so.corner_radius, color: color4(so.color), ..Default::default() }, self.white.clone(), None));
                }
                Layer::Mask(MaskLayer::Blur { texture, src01, clip01, dst, corner_radius, radius_px, cover, cover_mix }) => {
                    let (tw, th) = (texture.width.max(1) as f32, texture.height.max(1) as f32);
                    draws.push((
                        Params {
                            out_size,
                            mode: MODE_BLUR,
                            dst: rect4(dst),
                            uv: uv4(src01),
                            radius: *corner_radius,
                            color: color4(*cover),
                            grad_start: [clip01.x as f32, clip01.y as f32],
                            grad_end: [(clip01.x + clip01.width) as f32, (clip01.y + clip01.height) as f32],
                            extra: [radius_px / tw, radius_px / th, cover_mix.clamp(0.0, 1.0), 0.0],
                            ..Default::default()
                        },
                        texture.clone(),
                        None,
                    ));
                }
                Layer::Mask(MaskLayer::Highlight { dst, corner_radius, opacity, holes }) => {
                    let mut p = Params { out_size, mode: MODE_DIM, dst: rect4(dst), radius: *corner_radius, color: color4(Color::BLACK.with_alpha(*opacity)), ..Default::default() };
                    for (i, h) in holes.iter().take(8).enumerate() {
                        p.stops[i] = rect4(h);
                    }
                    p.stop_count = holes.len().min(8) as u32;
                    draws.push((p, self.white.clone(), None));
                }
                Layer::Ring(r) => {
                    let reach = r.radius + r.ring_width + 2.0;
                    let dst = Rect { x: (r.center[0] - reach) as f64, y: (r.center[1] - reach) as f64, width: (reach * 2.0) as f64, height: (reach * 2.0) as f64 };
                    let mut p = Params { out_size, mode: MODE_RING, dst: rect4(&dst), color: color4(r.fill), extra: [r.radius, r.ring_width, 0.0, 0.0], ..Default::default() };
                    p.stops[0] = color4(r.edge);
                    draws.push((p, self.white.clone(), None));
                }
            }
        }
        let draws: Vec<_> = draws.into_iter().take(MAX_DRAWS).collect();

        // upload uniforms
        let mut bytes = vec![0u8; self.uniform_stride as usize * draws.len().max(1)];
        for (i, (p, _, _)) in draws.iter().enumerate() {
            let off = i * self.uniform_stride as usize;
            bytes[off..off + std::mem::size_of::<Params>()].copy_from_slice(bytemuck::bytes_of(p));
        }
        self.gpu.queue.write_buffer(&self.uniform_buf, 0, &bytes);

        let bind_groups: Vec<wgpu::BindGroup> = draws
            .iter()
            .map(|(_, tex, lut)| {
                self.gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.bind_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &self.uniform_buf, offset: 0, size: wgpu::BufferSize::new(std::mem::size_of::<Params>() as u64) }) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&tex.view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&lut.as_ref().unwrap_or(&self.identity_lut).view) },
                    ],
                })
            })
            .collect();

        let target = self.target.as_ref().unwrap();
        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("composite") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            for (i, bg) in bind_groups.iter().enumerate() {
                pass.set_bind_group(0, bg, &[(i as u64 * self.uniform_stride) as u32]);
                pass.draw(0..6, 0..1);
            }
        }
        self.gpu.queue.submit([encoder.finish()]);
    }

    /// Copy the last rendered frame back to the CPU as tightly packed RGBA8.
    pub fn read_pixels(&mut self) -> anyhow::Result<Vec<u8>> {
        let target = self.target.as_ref().ok_or_else(|| anyhow::anyhow!("nothing rendered"))?;
        let (w, h) = (target.width, target.height);
        let padded = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        if self.readback.as_ref().map(|(_, p)| *p) != Some(padded) {
            self.readback = Some((
                self.gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (padded * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false }),
                padded,
            ));
        }
        let (buf, _) = self.readback.as_ref().unwrap();
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &target.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.gpu.queue.submit([encoder.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()??;
        let data = slice.get_mapped_range()?;
        let mut out = vec![0u8; (w * h * 4) as usize];
        for row in 0..h as usize {
            let src = &data[row * padded as usize..row * padded as usize + (w * 4) as usize];
            out[row * (w * 4) as usize..(row + 1) * (w * 4) as usize].copy_from_slice(src);
        }
        drop(data);
        buf.unmap();
        Ok(out)
    }
}

fn color4(c: Color) -> [f32; 4] {
    [c.r, c.g, c.b, c.a]
}
fn rect4(r: &Rect) -> [f32; 4] {
    [r.x as f32, r.y as f32, r.width as f32, r.height as f32]
}
fn uv4(r: &Rect) -> [f32; 4] {
    [r.x as f32, r.y as f32, (r.x + r.width) as f32, (r.y + r.height) as f32]
}
fn shadow_params(out_size: [f32; 2], dst: &Rect, radius: f32, sh: ShadowLayer) -> Params {
    Params {
        out_size,
        mode: MODE_SHADOW,
        dst: rect4(dst),
        radius,
        blur: sh.blur.max(0.5),
        alpha: sh.opacity,
        color: color4(sh.color),
        grad_start: sh.offset,
        extra: [sh.falloff.max(0.0), 0.0, 0.0, 0.0],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CameraLayer, RingLayer, ScreenLayer, SolidLayer};

    #[test]
    fn renders_and_reads_back() {
        let Ok(gpu) = Gpu::headless() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let mut c = Compositor::new(gpu.clone());
        let screen_tex = Texture::from_rgba(&gpu.device, &gpu.queue, 2, 2, &[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255], "t");
        let screen = screen_tex.clone();
        let desc = FrameDesc {
            width: 64,
            height: 32,
            background: BackgroundLayer::Gradient { start: [0.0, 0.0], end: [1.0, 1.0], stops: vec![(Color::from_hex("#4776e6").unwrap(), 0.0), (Color::from_hex("#8e54e9").unwrap(), 1.0)] },
            layers: vec![
                Layer::Screen(ScreenLayer { texture: screen, src01: Rect::UNIT, dst: Rect { x: 8.0, y: 4.0, width: 48.0, height: 24.0 }, corner_radius: 4.0, shadow: Some(ShadowLayer { color: Color::BLACK, offset: [2.0, 2.0], blur: 4.0, opacity: 0.5, falloff: 0.0 }), inset: None }),
                Layer::Solid(SolidLayer { dst: Rect { x: 0.0, y: 0.0, width: 4.0, height: 4.0 }, color: Color::WHITE, corner_radius: 0.0, shadow: None }),
                // mirrored webcam in the bottom-left corner: its top-left quadrant shows the texture's top-right (green)
                Layer::Camera(CameraLayer { texture: screen_tex.clone(), src01: Rect::UNIT, dst: Rect { x: 0.0, y: 16.0, width: 16.0, height: 16.0 }, corner_radius: 8.0, mirror: true, shadow: None, grade: None, lut: None }),
            ],
        };
        c.render(&desc);
        let px = c.read_pixels().unwrap();
        assert_eq!(px.len(), 64 * 32 * 4);
        let i = ((20 * 64 + 4) * 4) as usize;
        assert!(px[i + 1] > 200 && px[i] < 60, "mirrored camera should show green, got {:?}", &px[i..i + 4]);
        // inside the solid white square (pixel 1,1 avoids the anti-aliased edge)
        let i = ((1 * 64 + 1) * 4) as usize;
        assert_eq!(&px[i..i + 4], &[255, 255, 255, 255]);
        // background bottom-right is gradient end-ish, opaque
        let i = ((31 * 64 + 63) * 4) as usize;
        assert_eq!(px[i + 3], 255);
        assert!(px[i] > 100, "expected purple-ish, got {:?}", &px[i..i + 4]);
        // screen centre-left quadrant is red
        let i = ((10 * 64 + 16) * 4) as usize;
        assert!(px[i] > 200 && px[i + 1] < 60, "got {:?}", &px[i..i + 4]);
    }

    #[test]
    fn camera_grade_and_lut() {
        let Ok(gpu) = Gpu::headless() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let mut c = Compositor::new(gpu.clone());
        // 2×2 mid-red texture
        let tex = Texture::from_rgba(&gpu.device, &gpu.queue, 2, 2, &[200, 40, 40, 255].repeat(4), "t");
        let dst = Rect { x: 0.0, y: 0.0, width: 16.0, height: 16.0 };
        let layer = |grade: CameraGrade, lut: Option<Arc<LutTexture>>| FrameDesc {
            width: 16,
            height: 16,
            background: BackgroundLayer::Color(Color::BLACK),
            layers: vec![Layer::Camera(CameraLayer { texture: tex.clone(), src01: Rect::UNIT, dst, corner_radius: 0.0, mirror: false, shadow: None, grade: Some(grade), lut })],
        };
        let centre = |px: &[u8]| px[(8 * 16 + 8) * 4..(8 * 16 + 8) * 4 + 3].to_vec();
        // neutral grade leaves the pixel alone
        c.render(&layer(CameraGrade::NEUTRAL, None));
        let px = centre(&c.read_pixels().unwrap());
        assert!((px[0] as i32 - 200).abs() <= 1 && (px[1] as i32 - 40).abs() <= 1, "neutral: {px:?}");
        // saturation 0 → grey (Rec. 709 luma of (200, 40, 40) ≈ 76)
        c.render(&layer(CameraGrade { saturation: 0.0, ..CameraGrade::NEUTRAL }, None));
        let px = centre(&c.read_pixels().unwrap());
        assert!(px[0].abs_diff(px[1]) <= 1 && px[1].abs_diff(px[2]) <= 1 && (px[0] as i32 - 76).abs() <= 3, "desaturated: {px:?}");
        // gain 0.5 halves the signal
        c.render(&layer(CameraGrade { gain: [0.5; 3], ..CameraGrade::NEUTRAL }, None));
        let px = centre(&c.read_pixels().unwrap());
        assert!((px[0] as i32 - 100).abs() <= 2 && (px[1] as i32 - 20).abs() <= 2, "gain: {px:?}");
        // an inverting LUT at full mix
        let invert = crate::CubeLut::parse("LUT_3D_SIZE 2\n1 1 1\n0 1 1\n1 0 1\n0 0 1\n1 1 0\n0 1 0\n1 0 0\n0 0 0\n").unwrap();
        let lut = LutTexture::upload(&gpu.device, &gpu.queue, &invert, "invert");
        c.render(&layer(CameraGrade { lut_mix: 1.0, ..CameraGrade::NEUTRAL }, Some(lut)));
        let px = centre(&c.read_pixels().unwrap());
        assert!((px[0] as i32 - 55).abs() <= 2 && (px[1] as i32 - 215).abs() <= 2, "lut: {px:?}");
    }

    #[test]
    fn masks_rings_and_directional_shadow() {
        let Ok(gpu) = Gpu::headless() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let mut c = Compositor::new(gpu.clone());
        // 4×4 texture: left half red, right half blue
        let mut px = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                let _ = y;
                px.extend_from_slice(if x < 2 { &[255, 0, 0, 255] } else { &[0, 0, 255, 255] });
            }
        }
        let screen = Texture::from_rgba(&gpu.device, &gpu.queue, 4, 4, &px, "t");
        let dst = Rect { x: 0.0, y: 0.0, width: 64.0, height: 64.0 };
        let desc = FrameDesc {
            width: 64,
            height: 64,
            background: BackgroundLayer::Color(Color::WHITE),
            layers: vec![
                Layer::Screen(ScreenLayer { texture: screen.clone(), src01: Rect::UNIT, dst, corner_radius: 0.0, shadow: ShadowLayer::from_style(0.5, 4.0, 90.0, 4.0, true), inset: None }),
                // blur the whole screen with a 1-texel radius: the centre column mixes red and blue
                Layer::Mask(MaskLayer::Blur { texture: screen, src01: Rect::UNIT, clip01: Rect::UNIT, dst, corner_radius: 0.0, radius_px: 1.0, cover: Color::WHITE, cover_mix: 0.0 }),
                // dim the bottom half except a hole at bottom-left
                Layer::Mask(MaskLayer::Highlight { dst: Rect { x: 0.0, y: 32.0, width: 64.0, height: 32.0 }, corner_radius: 0.0, opacity: 1.0, holes: vec![Rect { x: 0.0, y: 32.0, width: 16.0, height: 32.0 }] }),
                Layer::Ring(RingLayer { center: [48.0, 16.0], radius: 6.0, ring_width: 1.0, fill: Color::WHITE, edge: Color::WHITE }),
            ],
        };
        c.render(&desc);
        let out = c.read_pixels().unwrap();
        let at = |x: usize, y: usize| &out[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
        // centre column is a red/blue mix after the blur
        let m = at(32, 8);
        assert!(m[0] > 60 && m[2] > 60, "expected purple, got {m:?}");
        // bottom-right dimmed to black, hole still coloured
        assert_eq!(at(60, 60)[1], 0);
        assert!(at(60, 60)[0] < 20 && at(60, 60)[2] < 20, "dimmed: {:?}", at(60, 60));
        assert!(at(4, 60)[0] > 100, "hole: {:?}", at(4, 60));
        // ring centre is filled white over blue
        assert!(at(48, 16)[0] > 200 && at(48, 16)[1] > 200, "ring fill: {:?}", at(48, 16));
    }
}
