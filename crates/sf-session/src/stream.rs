//! A sequentially decoded video source (screen recording or webcam) with its last frame kept
//! on the CPU and uploaded to a GPU texture.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use sf_media::{Frame, VideoReader, VideoReaderOptions};
use sf_render::{Gpu, Texture};

/// Decoder + last frame + texture for one media file. Frames are produced most efficiently
/// in increasing time order; seeking backwards or far forwards re-opens the decoder.
pub struct SourceStream {
    pub path: PathBuf,
    pub texture: Arc<Texture>,
    /// Decode width cap (None = native).
    pub max_width: Option<u32>,
    reader: Option<VideoReader>,
    current: Option<Frame>,
    label: &'static str,
}

const FRAME_MS: f64 = 1000.0 / 60.0;
/// Seeking further ahead than this re-opens the decoder instead of decoding through.
const MAX_DECODE_AHEAD_MS: f64 = 3000.0;

impl SourceStream {
    pub fn new(gpu: &Gpu, path: PathBuf, label: &'static str) -> Self {
        let texture = Texture::from_rgba(&gpu.device, &gpu.queue, 2, 2, &[0; 16], label);
        SourceStream { path, texture, max_width: None, reader: None, current: None, label }
    }

    /// The last decoded frame (CPU copy).
    pub fn frame(&self) -> Option<&Frame> {
        self.current.as_ref()
    }

    fn reopen(&mut self, start_ms: f64) -> Result<()> {
        let opts = VideoReaderOptions { start_ms: start_ms.max(0.0), max_width: self.max_width, ..Default::default() };
        self.reader = Some(VideoReader::open(&self.path, opts)?);
        self.current = None;
        Ok(())
    }

    /// Make the current frame the one for `time_ms` (nearest earlier frame) and upload it if it
    /// changed. Returns whether the texture was updated.
    pub fn seek(&mut self, gpu: &Gpu, time_ms: f64) -> Result<bool> {
        let time_ms = time_ms.max(0.0);
        let cur_pts = self.current.as_ref().map(|f| f.pts_ms);
        let need_reopen = match (&self.reader, cur_pts) {
            (None, _) => true,
            (Some(_), Some(pts)) => time_ms < pts - FRAME_MS * 0.5 || time_ms > pts + MAX_DECODE_AHEAD_MS,
            (Some(_), None) => false,
        };
        if need_reopen {
            self.reopen(time_ms - FRAME_MS * 0.5)?;
        }
        let mut changed = false;
        loop {
            let ahead = self.current.as_ref().map(|f| f.pts_ms + FRAME_MS * 0.5 > time_ms).unwrap_or(false);
            if ahead {
                break;
            }
            let Some(reader) = self.reader.as_mut() else { break };
            match reader.next_frame()? {
                Some(f) => {
                    self.current = Some(f);
                    changed = true;
                }
                None => break,
            }
        }
        if changed {
            if let Some(f) = &self.current {
                if (self.texture.width, self.texture.height) != (f.width, f.height) {
                    self.texture = Arc::new(Texture::new(&gpu.device, f.width, f.height, self.label));
                }
                self.texture.write_rgba(&gpu.queue, &f.data);
            }
        }
        Ok(changed)
    }

    /// Drop the decoder (e.g. after an error) so the next seek re-opens it.
    pub fn close(&mut self) {
        self.reader = None;
    }
}
