//! Titles and overlay media drawn over the recording (`sfTracks`, see `sf_project::tracks`):
//! linked clips placed on the edited timeline, their entrances and exits, titles rasterised
//! into sprites, images uploaded once and videos decoded per media file.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use sf_engine::timeline::Timeline;
use sf_engine::tracks::{anim_state, clip_start, resolve_links, word_progress, AnimState};
use sf_project::schema::common::Rect;
use sf_project::tracks::{Clip, MediaItem, MediaKind, SceneTracksExt, TextStyle, Track};
use sf_project::ProjectPackage;
use sf_render::text::{rasterize_title, Align, FontFamily, TitleText, TitleWord};
use sf_render::{Color, Gpu, Layer, OverlayLayer, ShadowLayer, Texture};

use crate::layout::cover_crop;
use crate::stream::SourceStream;

/// Room around a title for its blur, as a fraction of the font size.
const TITLE_PAD: f32 = 0.6;
/// Titles wrap at this fraction of the video width.
const TITLE_MAX_WIDTH01: f64 = 0.88;
/// Blur of a fully blurred title or overlay, as a fraction of the video height.
const MAX_BLUR01: f64 = 0.03;
/// Blur radius of the recording behind a clip with `backdropBlur` 1, in texture pixels per
/// 1000 px of texture height.
const BACKDROP_BLUR: f64 = 28.0;

/// One visual clip at an output time.
#[derive(Debug, Clone)]
pub struct ActiveClip {
    pub clip: Clip,
    pub start_ms: f64,
    pub anim: AnimState,
}

#[derive(Default)]
pub struct VisualTracks {
    /// Title and overlay tracks, linked clips moved to where their moment plays; the first
    /// track is drawn on top.
    pub tracks: Vec<Track>,
    media: HashMap<String, MediaItem>,
    images: HashMap<String, Option<Arc<Texture>>>,
    videos: HashMap<String, SourceStream>,
    /// Last rasterised title per clip: (cache key, texture).
    titles: HashMap<String, (String, Arc<Texture>)>,
    /// Where each clip was drawn in the last frame (output px; a title's rect is its text
    /// block without the blur margin), for moving it on the preview.
    pub drawn: HashMap<String, Rect>,
}

/// Words of a title's content: `*` around words draws them in the accent colour.
pub fn title_words(content: &str) -> Vec<(String, bool, usize)> {
    let mut out = vec![];
    let mut accent = false;
    for (line, text) in content.lines().enumerate() {
        for raw in text.split_whitespace() {
            let opens = raw.starts_with('*');
            let closes = raw.len() > 1 && raw.ends_with('*') || (raw == "*" && accent);
            let word = raw.trim_matches('*');
            if opens {
                accent = true;
            }
            if !word.is_empty() {
                out.push((word.to_string(), accent, line));
            }
            if closes {
                accent = false;
            }
        }
    }
    out
}

impl VisualTracks {
    pub fn load(pkg: &ProjectPackage, timeline: &Timeline) -> Self {
        let mut v = VisualTracks::default();
        v.reload(pkg, timeline);
        v
    }

    /// Re-read tracks and media after an edit, keeping decoded media and rasterised titles.
    pub fn reload(&mut self, pkg: &ProjectPackage, timeline: &Timeline) {
        let Some(scene) = pkg.project.scenes.first() else { return };
        let mut tracks: Vec<Track> = scene.tracks().into_iter().filter(Track::is_visual).collect();
        resolve_links(&mut tracks, timeline);
        self.tracks = tracks;
        self.media = scene.media().into_iter().map(|m| (m.id.clone(), m)).collect();
        let used: std::collections::HashSet<&str> = self.tracks.iter().flat_map(|t| &t.clips).filter_map(|c| c.media.as_deref()).collect();
        self.videos.retain(|id, _| used.contains(id.as_str()));
        let clips: std::collections::HashSet<&str> = self.tracks.iter().flat_map(|t| &t.clips).map(|c| c.id.as_str()).collect();
        self.titles.retain(|id, _| clips.contains(id.as_str()));
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.iter().all(|t| t.clips.is_empty())
    }

    /// Clips showing at output time `t`, bottom first.
    pub fn active(&self, timeline: &Timeline, t: f64) -> Vec<ActiveClip> {
        let mut out = vec![];
        for track in self.tracks.iter().rev().filter(|tr| !tr.hidden) {
            for c in &track.clips {
                let Some(start) = clip_start(c, timeline) else { continue };
                if let Some(anim) = anim_state(c, start, t) {
                    out.push(ActiveClip { clip: c.clone(), start_ms: start, anim });
                }
            }
        }
        out
    }

    /// How much to blur and darken the recording behind the clips at `t` (each 0..1).
    pub fn backdrop(active: &[ActiveClip]) -> (f64, f64) {
        active.iter().fold((0.0f64, 0.0f64), |(b, d), a| {
            let k = a.anim.opacity as f64;
            (b.max(a.clip.backdrop_blur.clamp(0.0, 1.0) * k), d.max(a.clip.backdrop_dim.clamp(0.0, 1.0) * k))
        })
    }

    /// Push the layers of `active` for a `w`×`h` output.
    #[allow(clippy::too_many_arguments)]
    pub fn push_layers(&mut self, gpu: &Gpu, package: &Path, active: &[ActiveClip], t: f64, w: u32, h: u32, max_decode_width: Option<u32>, layers: &mut Vec<Layer>) {
        self.drawn.clear();
        for a in active {
            let layer = match &a.clip.text {
                Some(style) => self.title_layer(gpu, &a.clip.id, style, &a.anim, w, h),
                None => self.media_layer(gpu, package, a, t, w, h, max_decode_width),
            };
            if let Some(Layer::Overlay(o)) = &layer {
                let inset = if a.clip.text.is_some() { a.clip.text.as_ref().map(|s| s.size_ratio.clamp(0.01, 0.3) * h as f64 * TITLE_PAD as f64 * o.dst.width / o.texture.width.max(1) as f64).unwrap_or(0.0) } else { 0.0 };
                let r = Rect { x: o.dst.x + inset, y: o.dst.y + inset, width: (o.dst.width - 2.0 * inset).max(1.0), height: (o.dst.height - 2.0 * inset).max(1.0) };
                self.drawn.insert(a.clip.id.clone(), r);
            }
            layers.extend(layer);
        }
    }

    fn title_layer(&mut self, gpu: &Gpu, id: &str, style: &TextStyle, anim: &AnimState, w: u32, h: u32) -> Option<Layer> {
        let (wf, hf) = (w as f64, h as f64);
        let size = (style.size_ratio.clamp(0.01, 0.3) * hf) as f32;
        let parsed = title_words(&style.content);
        let n = parsed.len();
        let words: Vec<TitleWord> = parsed
            .into_iter()
            .enumerate()
            .map(|(i, (text, accent, line))| {
                let k = anim.words01.map(|p| word_progress(p, i, n)).unwrap_or(1.0);
                TitleWord { text, accent, line, alpha: k, blur_px: (1.0 - k) * size * 0.35 }
            })
            .collect();
        // the subtitle follows the last word in
        let subtitle_alpha = anim.words01.map(|p| word_progress(p, n, n + 1)).unwrap_or(1.0);
        let title = TitleText {
            words,
            subtitle: style.subtitle.lines().map(str::to_string).filter(|l| !l.trim().is_empty()).collect(),
            family: FontFamily::from_config(&style.font),
            size_px: size,
            color: Color::from_hex(&style.color).unwrap_or(Color::WHITE),
            accent: Color::from_hex(&style.accent_color).unwrap_or(Color::WHITE),
            subtitle_color: Color::from_hex(&style.subtitle_color).unwrap_or(Color::WHITE),
            subtitle_alpha,
            align: Align::from_name(&style.align),
            shadow: style.shadow,
            max_width_px: (wf * TITLE_MAX_WIDTH01) as f32,
            pad_px: size * TITLE_PAD,
        };
        // quantised, so an entrance re-rasterises a few dozen times, not every frame at 60 fps
        let key = format!(
            "{:?}|{}|{w}x{h}|{}",
            style,
            (subtitle_alpha * 24.0).round(),
            title.words.iter().map(|w| format!("{}:{}", (w.alpha * 24.0).round(), (w.blur_px * 2.0).round())).collect::<Vec<_>>().join(",")
        );
        let tex = match self.titles.get(id) {
            Some((k, t)) if *k == key => t.clone(),
            _ => {
                let img = rasterize_title(&title)?;
                let t = Texture::from_rgba(&gpu.device, &gpu.queue, img.width, img.height, &img.rgba, "title");
                self.titles.insert(id.to_string(), (key, t.clone()));
                t
            }
        };
        let (tw, th) = (tex.width as f64, tex.height as f64);
        let pad = title.pad_px as f64;
        let (ax, ay) = (style.position01.x * wf, style.position01.y * hf);
        let x = match title.align {
            Align::Left => ax - pad,
            Align::Center => ax - tw / 2.0,
            Align::Right => ax - tw + pad,
        };
        let dst = place(Rect { x, y: ay - th / 2.0, width: tw, height: th }, anim, wf, hf);
        Some(Layer::Overlay(OverlayLayer { texture: tex, src01: Rect::UNIT, dst, corner_radius: 0.0, opacity: anim.opacity, blur_px: (anim.blur01 as f64 * MAX_BLUR01 * hf) as f32, shadow: None }))
    }

    #[allow(clippy::too_many_arguments)]
    fn media_layer(&mut self, gpu: &Gpu, package: &Path, a: &ActiveClip, t: f64, w: u32, h: u32, max_decode_width: Option<u32>) -> Option<Layer> {
        let item = self.media.get(a.clip.media.as_deref()?)?.clone();
        let tex = match item.kind {
            MediaKind::Image => self
                .images
                .entry(item.id.clone())
                .or_insert_with(|| {
                    Texture::from_image_path(&gpu.device, &gpu.queue, &package.join(&item.path))
                        .map_err(|e| tracing::warn!("overlay image {}: {e:#}", item.path))
                        .ok()
                })
                .clone()?,
            MediaKind::Video => {
                let stream = self.videos.entry(item.id.clone()).or_insert_with(|| SourceStream::new(gpu, package.join(&item.path), "overlay"));
                stream.max_width = max_decode_width;
                let media_ms = a.clip.source_start_ms + (t - a.start_ms) * a.clip.speed;
                if let Err(e) = stream.seek(gpu, media_ms) {
                    tracing::warn!("overlay video {} at {media_ms:.0} ms: {e:#}", item.path);
                    return None;
                }
                stream.texture.clone()
            }
            MediaKind::Audio => return None,
        };
        let (wf, hf) = (w as f64, h as f64);
        let aspect = tex.width as f64 / tex.height.max(1) as f64;
        let placement = a.clip.placement.as_deref().unwrap_or("center");
        let (region, src01) = match placement {
            "full" => (Rect { x: 0.0, y: 0.0, width: wf, height: hf }, cover_crop(Rect::UNIT, aspect, wf / hf)),
            "custom" => {
                let r = a.clip.rect01.unwrap_or(Rect { x: 0.15, y: 0.15, width: 0.7, height: 0.7 });
                (fit(Rect { x: r.x * wf, y: r.y * hf, width: r.width * wf, height: r.height * hf }, aspect), Rect::UNIT)
            }
            p if p.starts_with("pip-") => {
                let (bw, bh, m) = (wf * 0.32, hf * 0.32, hf * 0.04);
                let x = if p.ends_with('l') { m } else { wf - bw - m };
                let y = if p.starts_with("pip-t") { m } else { hf - bh - m };
                (fit_corner(Rect { x, y, width: bw, height: bh }, aspect, p), Rect::UNIT)
            }
            _ => (fit(Rect { x: wf * 0.15, y: hf * 0.15, width: wf * 0.7, height: hf * 0.7 }, aspect), Rect::UNIT),
        };
        let dst = place(region, &a.anim, wf, hf);
        let full = placement == "full";
        let corner = if full { 0.0 } else { (a.clip.corner_ratio.clamp(0.0, 0.5) * dst.width.min(dst.height)) as f32 };
        let shadow = (!full && a.clip.shadow > 0.0).then(|| ShadowLayer { color: Color::BLACK, offset: [0.0, (hf * 0.012) as f32], blur: (hf * 0.035) as f32, opacity: a.clip.shadow.clamp(0.0, 1.0) as f32, falloff: 0.0 });
        Some(Layer::Overlay(OverlayLayer { texture: tex, src01, dst, corner_radius: corner, opacity: a.anim.opacity, blur_px: (a.anim.blur01 as f64 * MAX_BLUR01 * hf) as f32, shadow }))
    }

    /// Blur radius (texture px) for the recording behind clips with `backdrop_blur` `k`.
    pub fn backdrop_radius_px(k: f64, texture_height: u32) -> f32 {
        (k * BACKDROP_BLUR * texture_height as f64 / 1000.0) as f32
    }
}

/// The largest rect of `aspect` centred in `r`.
fn fit(r: Rect, aspect: f64) -> Rect {
    let (w, h) = if r.width / r.height.max(1e-6) > aspect { (r.height * aspect, r.height) } else { (r.width, r.width / aspect.max(1e-6)) };
    Rect { x: r.x + (r.width - w) / 2.0, y: r.y + (r.height - h) / 2.0, width: w, height: h }
}

/// Like [`fit`], but pushed into the corner a `pip-…` placement names.
fn fit_corner(r: Rect, aspect: f64, placement: &str) -> Rect {
    let f = fit(r, aspect);
    let x = if placement.ends_with('l') { r.x } else { r.x + r.width - f.width };
    let y = if placement.starts_with("pip-t") { r.y } else { r.y + r.height - f.height };
    Rect { x, y, ..f }
}

/// `r` moved and scaled by an animation (offsets are fractions of the video).
fn place(r: Rect, a: &AnimState, w: f64, h: f64) -> Rect {
    let s = a.scale as f64;
    let (cx, cy) = (r.x + r.width / 2.0 + a.dx01 as f64 * w, r.y + r.height / 2.0 + a.dy01 as f64 * h);
    Rect { x: cx - r.width * s / 2.0, y: cy - r.height * s / 2.0, width: r.width * s, height: r.height * s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_mark_accent_words_across_lines() {
        let w = title_words("The *probability* of *success.*\nWhat *worked last* time");
        let accents: Vec<(&str, bool, usize)> = w.iter().map(|(t, a, l)| (t.as_str(), *a, *l)).collect();
        assert_eq!(
            accents,
            [("The", false, 0), ("probability", true, 0), ("of", false, 0), ("success.", true, 0), ("What", false, 1), ("worked", true, 1), ("last", true, 1), ("time", false, 1)]
        );
    }

    #[test]
    fn placements_keep_the_aspect() {
        let r = fit(Rect { x: 0.0, y: 0.0, width: 1000.0, height: 500.0 }, 1.0);
        assert_eq!((r.x, r.width, r.height), (250.0, 500.0, 500.0));
        let c = fit_corner(Rect { x: 0.0, y: 0.0, width: 400.0, height: 400.0 }, 2.0, "pip-br");
        assert_eq!((c.x, c.y, c.width, c.height), (0.0, 200.0, 400.0, 200.0));
        let moved = place(Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 }, &AnimState { scale: 0.5, dx01: 0.1, ..AnimState::SETTLED }, 1000.0, 1000.0);
        assert!((moved.x - 125.0).abs() < 1e-3 && moved.width == 50.0, "{moved:?}");
    }
}
