//! Captions for a session: the project's microphone and voice-over transcripts shifted into
//! source time, grouped into lines, and the current line rasterised into a sprite.

use std::sync::Arc;

use sf_engine::captions::{self, CaptionLine, CaptionStyle, Growth, Word};
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

/// Words of a voice-over file (times in the file) that `span` plays, moved to source time:
/// the take plays at normal speed from its output start, so a word's output time is its
/// place in the file past the take's offset.
fn voice_over_words(words: &[Word], span: &sf_engine::audio::VoiceOverSpan, timeline: &Timeline) -> Vec<Word> {
    let to_source = |file_ms: f64| timeline.output_to_source((span.output_start_ms + file_ms - span.file_offset_ms).min(span.output_end_ms));
    words
        .iter()
        .filter(|w| w.start_ms >= span.file_offset_ms && span.file_ms_at(span.output_end_ms) > w.start_ms)
        .filter_map(|w| {
            let (start, end) = (to_source(w.start_ms)?, to_source(w.end_ms)?);
            Some(Word { start_ms: start, end_ms: end.max(start), text: w.text.clone() })
        })
        .collect()
}

impl Captions {
    /// Load every microphone transcript of the package. Each session's times are shifted by
    /// how much later the microphone started than the screen. The words of each voice-over
    /// take that is heard go where the take plays on `timeline`.
    pub fn load(pkg: &ProjectPackage, screen_process_start_ms: f64, timeline: &Timeline) -> Self {
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
        if let Some(scene) = pkg.project.scenes.first() {
            let mut transcripts: std::collections::HashMap<&str, Option<TranscriptFile>> = Default::default();
            for span in sf_engine::audio::voice_over_spans(scene, timeline).iter().filter(|s| !s.muted && s.volume > 0.0) {
                let t = transcripts.entry(span.file_name.as_str()).or_insert_with(|| {
                    TranscriptFile::load_voice_over(&pkg.path, &span.file_name).unwrap_or_else(|e| {
                        tracing::warn!("transcript for voice-over {}: {e}", span.file_name);
                        None
                    })
                });
                if let Some(t) = t {
                    words.extend(voice_over_words(&captions::words_from_transcript(&t.transcript, 0.0, cfg.show_sound_labels), span, timeline));
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

    pub fn to_vtt(&self, timeline: &Timeline) -> String {
        captions::to_vtt(&self.lines, timeline)
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

#[cfg(test)]
mod tests {
    use sf_engine::audio::VoiceOverSpan;
    use sf_engine::captions::Word;
    use sf_engine::timeline::Timeline;
    use sf_project::v4::Slice;

    fn word(a: f64, b: f64, text: &str) -> Word {
        Word { start_ms: a, end_ms: b, text: text.into() }
    }

    #[test]
    fn voice_over_words_play_where_the_take_plays() {
        // 0–10 s and 20–30 s kept: output 12 s is source 22 s
        let tl = Timeline::from_slices(&[Slice::new(0.0, 10_000.0), Slice::new(20_000.0, 30_000.0)]);
        // the right half of a split take: it plays the file from 1 s, at output 12–14 s
        let span = VoiceOverSpan { id: "v".into(), file_name: "a.wav".into(), output_start_ms: 12_000.0, output_end_ms: 14_000.0, file_offset_ms: 1_000.0, volume: 1.0, muted: false, layer: 0, boost_db: 0.0 };
        let words = [word(200.0, 600.0, "left"), word(1_500.0, 1_900.0, "heard"), word(3_500.0, 3_800.0, "trimmed")];
        let out = super::voice_over_words(&words, &span, &tl);
        assert_eq!(out, vec![word(22_500.0, 22_900.0, "heard")]);
    }
}
