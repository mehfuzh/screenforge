use std::sync::Arc;

#[derive(Debug)]
pub struct Texture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

impl Texture {
    pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    pub fn new(device: &wgpu::Device, width: u32, height: u32, label: &str) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: Self::FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Texture { texture, view, width: width.max(1), height: height.max(1) }
    }

    /// Upload tightly packed RGBA8 pixels (premultiplied alpha expected for sprites).
    pub fn write_rgba(&self, queue: &wgpu::Queue, rgba: &[u8]) {
        debug_assert_eq!(rgba.len(), (self.width * self.height * 4) as usize);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.width * 4), rows_per_image: Some(self.height) },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
    }

    pub fn from_rgba(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32, rgba: &[u8], label: &str) -> Arc<Self> {
        let t = Self::new(device, width, height, label);
        t.write_rgba(queue, rgba);
        Arc::new(t)
    }

    /// Decode a PNG/JPEG and upload it; alpha is premultiplied.
    pub fn from_image_bytes(device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8], label: &str) -> anyhow::Result<Arc<Self>> {
        let img = image::load_from_memory(bytes)?.to_rgba8();
        let (w, h) = img.dimensions();
        let mut data = img.into_raw();
        premultiply(&mut data);
        Ok(Self::from_rgba(device, queue, w, h, &data, label))
    }

    pub fn from_image_path(device: &wgpu::Device, queue: &wgpu::Queue, path: &std::path::Path) -> anyhow::Result<Arc<Self>> {
        let bytes = std::fs::read(path)?;
        Self::from_image_bytes(device, queue, &bytes, &path.display().to_string())
    }
}

pub fn premultiply(rgba: &mut [u8]) {
    for p in rgba.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a < 255 {
            p[0] = ((p[0] as u32 * a + 127) / 255) as u8;
            p[1] = ((p[1] as u32 * a + 127) / 255) as u8;
            p[2] = ((p[2] as u32 * a + 127) / 255) as u8;
        }
    }
}
