use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8,
    Bgra8,
}

impl PixelFormat {
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            PixelFormat::Rgba8 => "rgba",
            PixelFormat::Bgra8 => "bgra",
        }
    }
    pub const fn bytes_per_pixel(self) -> usize {
        4
    }
}

/// One decoded video frame. Pixel data is shared so frames can be cached cheaply.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// Presentation time in ms from the start of the file.
    pub pts_ms: f64,
    pub data: Arc<[u8]>,
}

impl Frame {
    pub fn stride(&self) -> usize {
        self.width as usize * self.format.bytes_per_pixel()
    }
}
