//! On-device computer vision for the camera feed.
//!
//! Face detection runs the SeetaFace frontal cascade through `rustface` (pure Rust, CPU). The
//! 1.2 MB model is compiled into the binary, so nothing is downloaded and detection works
//! offline on every platform. Frames are converted to greyscale and shrunk to
//! [`DETECT_WIDTH`] pixels wide before the cascade runs, which keeps one detection around a
//! few milliseconds on a webcam frame.

use std::io::Cursor;

use rustface::{Detector, ImageData};

/// A detected face in normalized frame coordinates (0..1, origin top-left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Cascade score; larger is more confident.
    pub score: f64,
}

impl Face {
    pub fn center(&self) -> [f64; 2] {
        [self.x + self.width / 2.0, self.y + self.height / 2.0]
    }
}

/// Detection resolution: frames wider than this are shrunk first.
pub const DETECT_WIDTH: u32 = 320;
/// Faces smaller than this (in detection pixels) are ignored: on a 320 px wide frame a face
/// has to be at least a tenth of the width, which filters out background clutter.
const MIN_FACE_PX: u32 = 32;
/// Cascade acceptance threshold (`rustface` suggests 0.95 … 4.5; 2 trades a few false
/// positives for catching turned heads).
const SCORE_THRESHOLD: f64 = 2.0;

const MODEL: &[u8] = include_bytes!("../model/seeta_fd_frontal_v1.0.bin");

/// A frontal face detector with its scratch buffers. Not `Sync`; keep one per thread.
pub struct FaceDetector {
    inner: Box<dyn Detector>,
    gray: Vec<u8>,
}

// SAFETY: `rustface`'s detector (`FuStDetector`, the only implementation behind
// `create_detector_with_model`) owns plain data: the model's `Vec`s of classifiers and the
// feature-map scratch `Vec`s and integers. It holds no `Rc`, `RefCell`, thread-local or raw
// pointer across calls (raw pointers appear only inside its SIMD helpers, scoped to one call),
// so moving it to another thread is sound. The trait object merely lacks the `Send` bound.
unsafe impl Send for FaceDetector {}

impl FaceDetector {
    pub fn new() -> anyhow::Result<Self> {
        let model = rustface::model::read_model(Cursor::new(MODEL))?;
        let mut inner = rustface::create_detector_with_model(model);
        inner.set_min_face_size(MIN_FACE_PX);
        inner.set_score_thresh(SCORE_THRESHOLD);
        inner.set_pyramid_scale_factor(0.8);
        inner.set_slide_window_step(4, 4);
        Ok(FaceDetector { inner, gray: Vec::new() })
    }

    /// Find the most prominent face in a tightly packed 4-byte-per-pixel frame (RGBA, or BGRA
    /// when `bgra`). The largest face wins; among equals the higher score.
    pub fn detect(&mut self, pixels: &[u8], width: u32, height: u32, bgra: bool) -> Option<Face> {
        if width == 0 || height == 0 || pixels.len() < (width as usize * height as usize * 4) {
            return None;
        }
        let scale = (DETECT_WIDTH as f64 / width as f64).min(1.0);
        let dw = ((width as f64 * scale).round() as u32).max(1);
        let dh = ((height as f64 * scale).round() as u32).max(1);
        downscale_gray(pixels, width, height, bgra, dw, dh, &mut self.gray);
        let image = ImageData::new(&self.gray, dw, dh);
        let faces = self.inner.detect(&image);
        let best = faces.iter().max_by(|a, b| {
            let area = |f: &rustface::FaceInfo| f.bbox().width() as f64 * f.bbox().height() as f64;
            area(a).total_cmp(&area(b)).then(a.score().total_cmp(&b.score()))
        })?;
        let b = best.bbox();
        Some(Face {
            x: b.x() as f64 / dw as f64,
            y: b.y() as f64 / dh as f64,
            width: b.width() as f64 / dw as f64,
            height: b.height() as f64 / dh as f64,
            score: best.score(),
        })
    }
}

/// Area-average a 4-byte-per-pixel frame down to `dw × dh` greyscale (Rec. 601 luma).
pub fn downscale_gray(pixels: &[u8], width: u32, height: u32, bgra: bool, dw: u32, dh: u32, out: &mut Vec<u8>) {
    out.clear();
    out.reserve((dw * dh) as usize);
    let (w, h) = (width as usize, height as usize);
    let (ri, bi) = if bgra { (2, 0) } else { (0, 2) };
    for dy in 0..dh as usize {
        let y0 = dy * h / dh as usize;
        let y1 = ((dy + 1) * h / dh as usize).max(y0 + 1).min(h);
        for dx in 0..dw as usize {
            let x0 = dx * w / dw as usize;
            let x1 = ((dx + 1) * w / dw as usize).max(x0 + 1).min(w);
            let mut sum = 0u64;
            for y in y0..y1 {
                let row = &pixels[y * w * 4..(y + 1) * w * 4];
                for x in x0..x1 {
                    let p = &row[x * 4..x * 4 + 4];
                    // 0.299 R + 0.587 G + 0.114 B in 1/256 units
                    sum += 77 * p[ri] as u64 + 150 * p[1] as u64 + 29 * p[bi] as u64;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            out.push((sum / (n * 256)).min(255) as u8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_averages_blocks() {
        // 4×2 frame: left half white, right half black → 2×1 grey: [255, 0]
        let mut px = vec![];
        for _ in 0..2 {
            px.extend_from_slice(&[255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
        }
        let mut out = vec![];
        downscale_gray(&px, 4, 2, false, 2, 1, &mut out);
        assert_eq!(out, vec![255, 0]);
        // BGRA: a pure blue pixel reads as blue (low luma), a pure red one as red
        let mut out = vec![];
        downscale_gray(&[255, 0, 0, 255], 1, 1, true, 1, 1, &mut out);
        assert_eq!(out, vec![(29 * 255 / 256) as u8]);
    }

    #[test]
    fn model_loads_and_blank_frame_has_no_face() {
        let mut d = FaceDetector::new().expect("embedded model");
        let frame = vec![128u8; 320 * 240 * 4];
        assert!(d.detect(&frame, 320, 240, false).is_none());
        assert!(d.detect(&[], 0, 0, false).is_none());
    }
}
