//! Captions for a session: the project's microphone transcripts shifted into source time,
//! grouped into lines, and the current line rasterised into a sprite.

use std::sync::Arc;

use sf_engine::captions::{self, CaptionLine, CaptionStyle, Growth};
use sf_engine::timeline::Timeline;
use sf_project::schema::common::Rect;
use sf_project::transcript::TranscriptFile;
use sf_project::ProjectPackage;
use sf_render::text::{rasterize_caption, CaptionText, FontFamily, TextWord};
use sf_render::{Color, Gpu, Layer, SpriteLayer, Texture};

/// Background box opacity (the colour comes from `captions.backgroundColor`).
const BOX_ALPHA: f32 = 0.78;
/// Gap between the caption and the edge of the video, as a fraction of its height.
const MARGIN01: f64 = 0.045;

#[derive(Default)]
pub struct Captions {
    pub lines: Vec<CaptionLine>,
    /// Last rasterised line: (cache key, texture).
    cached: Option<(String, Arc<Texture>)>,
}

impl Captions {
    /// Load every microphone transcript of the package. Each session's times are shifted by
    /// how much later the microphone started than the screen.
    pub fn load(pkg: &ProjectPackage, screen_process_start_ms: f64) -> Self {
        let cfg = &pkg.project.config.captions;
        let mut words = vec![];
        if let Some(mic) = pkg.recording.as_ref().and_then(|r| r.microphone()) {
            for (i, s) in mic.sessions.iter().enumerate() {
                match TranscriptFile::load_microphone(&pkg.path, i as u32) {
                    Ok(Some(t)) => {
                        let offset = s.process_time_start_ms - screen_process_start_ms;
                        let offset = if offset.is_finite() { offset } else { 0.0 };
                        words.extend(captions::words_from_transcript(&t.transcript, offset, cfg.show_sound_labels));
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!("transcript for microphone session {i}: {e}"),
                }
            }
        }
        words.sort_by(|a, b| a.start_ms.total_cmp(&b.start_ms));
        Captions { lines: captions::build_lines(&words), cached: None }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn to_srt(&self, timeline: &Timeline) -> String {
        captions::to_srt(&self.lines, timeline)
    }

    /// Push the caption sprite for `source_ms` onto `layers` (output size `w`×`h` px).
    pub fn push_layer(&mut self, pkg: &ProjectPackage, gpu: &Gpu, source_ms: f64, w: u32, h: u32, layers: &mut Vec<Layer>) {
        let cfg = &pkg.project.config.captions;
        if !cfg.enable_transcript || self.lines.is_empty() {
            return;
        }
        let style = CaptionStyle::from_config(&cfg.words_reveal, &cfg.word_entrance, &cfg.line_growth);
        let Some(frame) = captions::frame_at(&self.lines, source_ms, style) else { return };
        let text = CaptionText {
            words: frame
                .words
                .iter()
                .map(|v| TextWord { text: v.text.clone(), laid_out: v.revealed || frame.growth != Growth::Centered, alpha: v.alpha, drop01: v.drop01 })
                .collect(),
            family: FontFamily::from_config(&cfg.font),
            size_px: (cfg.size_ratio.clamp(0.01, 0.3) * h as f64) as f32,
            color: Color::from_hex(&cfg.color).unwrap_or(Color::WHITE),
            background: Color::from_hex(&cfg.background_color).unwrap_or(Color::BLACK).with_alpha(BOX_ALPHA),
            box_to_revealed: frame.growth == Growth::LeftAnchored,
            max_width_px: w as f32 * 0.9,
        };
        // quantised so fades re-rasterise a few times, not every frame
        let key = format!(
            "{:?}|{}|{}",
            (text.family, text.size_px.round() as u32, &cfg.color, &cfg.background_color, w),
            text.words.iter().map(|w| format!("{}:{}:{}:{}", w.text, w.laid_out as u8, (w.alpha * 16.0).round(), (w.drop01 * 16.0).round())).collect::<Vec<_>>().join(" "),
            frame.growth as u8
        );
        let tex = match &self.cached {
            Some((k, t)) if *k == key => t.clone(),
            _ => {
                let Some(img) = rasterize_caption(&text) else { return };
                let t = Texture::from_rgba(&gpu.device, &gpu.queue, img.width, img.height, &img.rgba, "caption");
                self.cached = Some((key, t.clone()));
                t
            }
        };
        let (tw, th) = (tex.width as f64, tex.height as f64);
        let (w, h) = (w as f64, h as f64);
        let margin = MARGIN01 * h;
        let p = cfg.position01;
        let cx = margin + tw / 2.0 + p.x.clamp(0.0, 1.0) * (w - 2.0 * margin - tw).max(0.0);
        let top = margin + p.y.clamp(0.0, 1.0) * (h - 2.0 * margin - th).max(0.0);
        let dst = Rect { x: (cx - tw / 2.0).round(), y: top.round(), width: tw, height: th };
        let pivot = [(dst.x + tw / 2.0) as f32, (dst.y + th / 2.0) as f32];
        layers.push(Layer::Sprite(SpriteLayer { texture: tex, dst, pivot, rotation_rad: 0.0, opacity: 1.0 }));
    }
}
