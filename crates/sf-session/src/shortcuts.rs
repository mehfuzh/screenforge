//! Keyboard shortcut overlay for a session: key caps for the shortcut pressed most recently,
//! centred near the bottom of the video.

use std::sync::Arc;

use sf_engine::cursor::CursorTrack;
use sf_engine::shortcuts::{self, Shortcut};
use sf_project::schema::common::Rect;
use sf_project::ProjectPackage;
use sf_render::text::{rasterize_keys, FontFamily, KeyCaps};
use sf_render::{Color, Gpu, Layer, SpriteLayer, Texture};

/// Background box opacity, like the captions'.
const BOX_ALPHA: f32 = 0.78;
/// Gap between the label and the bottom of the video, as a fraction of its height.
const MARGIN01: f64 = 0.045;
/// Key cap height per unit of `shortcutsSizeRatio` × video height.
const CAP_PER_RATIO: f64 = 0.7;

#[derive(Default)]
pub struct Shortcuts {
    pub list: Vec<Shortcut>,
    /// Last rasterised label: (cache key, texture).
    cached: Option<(String, Arc<Texture>)>,
}

impl Shortcuts {
    pub fn load(pkg: &ProjectPackage, track: &CursorTrack) -> Self {
        let single = pkg.project.config.captions.show_shortcuts_with_single_letters;
        Shortcuts { list: shortcuts::build(&track.key_presses, single), cached: None }
    }

    /// Push the label for `source_ms` onto `layers` (output size `w`×`h` px).
    pub fn push_layer(&mut self, pkg: &ProjectPackage, gpu: &Gpu, source_ms: f64, w: u32, h: u32, layers: &mut Vec<Layer>) {
        let cfg = &pkg.project.config.captions;
        if !cfg.enable_shortcuts {
            return;
        }
        let Some((shortcut, opacity)) = shortcuts::frame_at(&self.list, source_ms) else { return };
        let caps = KeyCaps {
            keys: shortcut.keys.clone(),
            family: FontFamily::Sans,
            size_px: (cfg.shortcuts_size_ratio.clamp(0.02, 0.3) * CAP_PER_RATIO * h as f64) as f32,
            color: Color::WHITE,
            background: Color::from_hex(&cfg.background_color).unwrap_or(Color::BLACK).with_alpha(BOX_ALPHA),
            max_width_px: w as f32 * 0.9,
        };
        let key = format!("{}|{}|{}|{w}", shortcut.label(), caps.size_px.round() as u32, cfg.background_color);
        let tex = match &self.cached {
            Some((k, t)) if *k == key => t.clone(),
            _ => {
                let Some(img) = rasterize_keys(&caps) else { return };
                let t = Texture::from_rgba(&gpu.device, &gpu.queue, img.width, img.height, &img.rgba, "shortcut");
                self.cached = Some((key, t.clone()));
                t
            }
        };
        let (tw, th) = (tex.width as f64, tex.height as f64);
        let (w, h) = (w as f64, h as f64);
        let mut bottom = h - MARGIN01 * h;
        // stay clear of a caption line in the lower half
        if cfg.enable_transcript && cfg.position01.y > 0.5 {
            bottom -= cfg.size_ratio.clamp(0.01, 0.3) * h * 1.9;
        }
        let dst = Rect { x: ((w - tw) / 2.0).round(), y: (bottom - th).max(0.0).round(), width: tw, height: th };
        let pivot = [(dst.x + tw / 2.0) as f32, (dst.y + th / 2.0) as f32];
        layers.push(Layer::Sprite(SpriteLayer { texture: tex, dst, pivot, rotation_rad: 0.0, opacity }));
    }
}
