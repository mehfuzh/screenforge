//! Caption text rasterised on the CPU into a premultiplied RGBA image, then drawn as a
//! sprite. One line per image: a rounded background box with the words on it.

use ab_glyph::{Font, FontRef, Glyph, PxScale, ScaleFont};

use crate::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontFamily {
    Sans,
    Serif,
    Mono,
}

impl FontFamily {
    /// From the `captions.font` config value (`sans-serif` | `serif` | `mono`).
    pub fn from_config(s: &str) -> Self {
        match s {
            "serif" => FontFamily::Serif,
            "mono" | "monospace" => FontFamily::Mono,
            _ => FontFamily::Sans,
        }
    }

    /// The bundled subsets, tried in order for each glyph.
    fn fonts(self) -> Vec<FontRef<'static>> {
        let data: [&'static [u8]; 3] = match self {
            FontFamily::Sans => [
                include_bytes!("../fonts/Inter-SemiBold-latin.ttf"),
                include_bytes!("../fonts/Inter-SemiBold-latin-ext.ttf"),
                include_bytes!("../fonts/Inter-SemiBold-cyrillic.ttf"),
            ],
            FontFamily::Serif => [
                include_bytes!("../fonts/SourceSerif4-SemiBold-latin.ttf"),
                include_bytes!("../fonts/SourceSerif4-SemiBold-latin-ext.ttf"),
                include_bytes!("../fonts/SourceSerif4-SemiBold-cyrillic.ttf"),
            ],
            FontFamily::Mono => [
                include_bytes!("../fonts/JetBrainsMono-SemiBold-latin.ttf"),
                include_bytes!("../fonts/JetBrainsMono-SemiBold-latin-ext.ttf"),
                include_bytes!("../fonts/JetBrainsMono-SemiBold-cyrillic.ttf"),
            ],
        };
        // key symbols (⌘ ⇧ ⌥ ⏎ …) for every family: only Inter carries them
        let symbols: &'static [u8] = include_bytes!("../fonts/Inter-SemiBold-symbols.ttf");
        data.iter().chain([&symbols]).filter_map(|d| FontRef::try_from_slice(d).ok()).collect()
    }
}

/// One word of the caption line.
#[derive(Debug, Clone, PartialEq)]
pub struct TextWord {
    pub text: String,
    /// Takes part in the layout (false = not revealed yet).
    pub laid_out: bool,
    pub alpha: f32,
    /// Slide-in offset: 0 = settled, 1 = lowered by a third of the font size.
    pub drop01: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaptionText {
    pub words: Vec<TextWord>,
    pub family: FontFamily,
    /// Font size in px.
    pub size_px: f32,
    pub color: Color,
    pub background: Color,
    /// Only the part of the line up to the last revealed word gets a background, but the
    /// image keeps the full line's width (so a centred image grows to the right).
    pub box_to_revealed: bool,
    /// Widest the image may be; the font shrinks to fit.
    pub max_width_px: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextImage {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8.
    pub rgba: Vec<u8>,
}

struct Shaper {
    fonts: Vec<FontRef<'static>>,
    scale: PxScale,
}

impl Shaper {
    fn font_for(&self, c: char) -> usize {
        self.fonts.iter().position(|f| f.glyph_id(c).0 != 0).unwrap_or(0)
    }

    fn word_width(&self, text: &str) -> f32 {
        let mut w = 0.0;
        let mut prev: Option<(usize, ab_glyph::GlyphId)> = None;
        for c in text.chars() {
            let fi = self.font_for(c);
            let f = self.fonts[fi].as_scaled(self.scale);
            let id = f.glyph_id(c);
            if let Some((pf, pid)) = prev.filter(|p| p.0 == fi) {
                w += f.kern(pid, id);
                let _ = pf;
            }
            w += f.h_advance(id);
            prev = Some((fi, id));
        }
        w
    }

    fn space(&self) -> f32 {
        self.fonts[0].as_scaled(self.scale).h_advance(self.fonts[0].glyph_id(' '))
    }

    fn ascent(&self) -> f32 {
        self.fonts[0].as_scaled(self.scale).ascent()
    }

    fn line_height(&self) -> f32 {
        let f = self.fonts[0].as_scaled(self.scale);
        f.ascent() - f.descent()
    }

    /// Draw `text` with its baseline at `(x, y)` into an alpha coverage buffer.
    fn draw(&self, text: &str, x: f32, y: f32, cov: &mut [f32], w: usize, h: usize) {
        let mut pen = x;
        let mut prev: Option<(usize, ab_glyph::GlyphId)> = None;
        for c in text.chars() {
            let fi = self.font_for(c);
            let font = &self.fonts[fi];
            let f = font.as_scaled(self.scale);
            let id = f.glyph_id(c);
            if let Some((pfi, pid)) = prev {
                if pfi == fi {
                    pen += f.kern(pid, id);
                }
            }
            let g: Glyph = id.with_scale_and_position(self.scale, ab_glyph::point(pen, y));
            if let Some(o) = font.outline_glyph(g) {
                let b = o.px_bounds();
                o.draw(|gx, gy, c| {
                    let px = b.min.x as i32 + gx as i32;
                    let py = b.min.y as i32 + gy as i32;
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        let i = py as usize * w + px as usize;
                        cov[i] = (cov[i] + c).min(1.0);
                    }
                });
            }
            pen += f.h_advance(id);
            prev = Some((fi, id));
        }
    }
}

/// Signed distance to a rounded rect (negative inside).
fn rounded_rect_sd(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> f32 {
    let cx = (x0 + x1) / 2.0;
    let cy = (y0 + y1) / 2.0;
    let hx = (x1 - x0) / 2.0 - r;
    let hy = (y1 - y0) / 2.0 - r;
    let qx = (px - cx).abs() - hx;
    let qy = (py - cy).abs() - hy;
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

/// Rasterise one caption line. `None` when nothing is visible.
pub fn rasterize_caption(c: &CaptionText) -> Option<TextImage> {
    let visible: Vec<&TextWord> = c.words.iter().filter(|w| w.laid_out).collect();
    if visible.is_empty() || c.size_px < 2.0 {
        return None;
    }
    let fonts = c.family.fonts();
    if fonts.is_empty() {
        return None;
    }
    let mut shaper = Shaper { fonts, scale: PxScale::from(c.size_px) };
    let measure = |s: &Shaper| -> (Vec<f32>, f32) {
        let widths: Vec<f32> = visible.iter().map(|w| s.word_width(&w.text)).collect();
        let total = widths.iter().sum::<f32>() + s.space() * (widths.len().saturating_sub(1)) as f32;
        (widths, total)
    };
    let (mut widths, mut text_w) = measure(&shaper);
    let pad_x = c.size_px * 0.45;
    if text_w + 2.0 * pad_x > c.max_width_px && text_w > 0.0 {
        let k = ((c.max_width_px - 2.0 * pad_x) / text_w).max(0.2);
        shaper.scale = PxScale::from(c.size_px * k);
        (widths, text_w) = measure(&shaper);
    }
    let size = shaper.scale.x;
    let pad_x = size * 0.45;
    let pad_y = size * 0.28;
    let line_h = shaper.line_height();
    let w = (text_w + 2.0 * pad_x).ceil().max(1.0) as usize;
    let h = (line_h + 2.0 * pad_y + size * 0.35).ceil().max(1.0) as usize; // room for the slide
    let box_h = line_h + 2.0 * pad_y;

    // background box: up to the last revealed word, or the whole line
    let space = shaper.space();
    let mut x = pad_x;
    let mut box_right = pad_x;
    let mut placed = vec![];
    for (word, ww) in visible.iter().zip(&widths) {
        if word.alpha > 0.0 || !c.box_to_revealed {
            box_right = x + ww;
        }
        placed.push((x, *word));
        x += ww + space;
    }
    let box_right = if c.box_to_revealed { box_right + pad_x } else { w as f32 };
    let radius = (box_h * 0.22).min(size * 0.35);
    let mut rgba = vec![0u8; w * h * 4];
    let bg = c.background;
    for py in 0..h {
        for px in 0..w {
            let sd = rounded_rect_sd(px as f32 + 0.5, py as f32 + 0.5, 0.0, 0.0, box_right, box_h, radius);
            let a = (0.5 - sd).clamp(0.0, 1.0) * bg.a;
            if a > 0.0 {
                let i = (py * w + px) * 4;
                rgba[i] = (bg.r * a * 255.0).round() as u8;
                rgba[i + 1] = (bg.g * a * 255.0).round() as u8;
                rgba[i + 2] = (bg.b * a * 255.0).round() as u8;
                rgba[i + 3] = (a * 255.0).round() as u8;
            }
        }
    }

    // words, each composited over the box with its own alpha
    let baseline = pad_y + shaper.ascent();
    let mut cov = vec![0f32; w * h];
    for (x, word) in placed {
        if word.alpha <= 0.0 {
            continue;
        }
        cov.iter_mut().for_each(|v| *v = 0.0);
        shaper.draw(&word.text, x, baseline + word.drop01 * size * 0.33, &mut cov, w, h);
        let col = c.color;
        for (i, cv) in cov.iter().enumerate() {
            let a = cv * word.alpha * col.a;
            if a <= 0.0 {
                continue;
            }
            let p = &mut rgba[i * 4..i * 4 + 4];
            for (k, ch) in [col.r, col.g, col.b].into_iter().enumerate() {
                p[k] = (ch * a * 255.0 + p[k] as f32 * (1.0 - a)).round().min(255.0) as u8;
            }
            p[3] = (a * 255.0 + p[3] as f32 * (1.0 - a)).round().min(255.0) as u8;
        }
    }
    Some(TextImage { width: w as u32, height: h as u32, rgba })
}

/// A keyboard shortcut label: one key cap per key on a dark box.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyCaps {
    pub keys: Vec<String>,
    pub family: FontFamily,
    /// Height of a key cap in px.
    pub size_px: f32,
    pub color: Color,
    pub background: Color,
    pub max_width_px: f32,
}

/// Rasterise a shortcut label. `None` when there are no keys.
pub fn rasterize_keys(k: &KeyCaps) -> Option<TextImage> {
    if k.keys.is_empty() || k.size_px < 4.0 {
        return None;
    }
    let fonts = k.family.fonts();
    if fonts.is_empty() {
        return None;
    }
    // the glyphs take a bit over half the cap height
    let layout = |cap_h: f32| {
        let shaper = Shaper { fonts: fonts.clone(), scale: PxScale::from(cap_h * 0.52) };
        let pad = cap_h * 0.3;
        let gap = cap_h * 0.16;
        let caps: Vec<f32> = k.keys.iter().map(|t| (shaper.word_width(t) + 2.0 * pad).max(cap_h)).collect();
        let total = caps.iter().sum::<f32>() + gap * caps.len().saturating_sub(1) as f32 + 2.0 * gap;
        (shaper, caps, gap, total)
    };
    let mut cap_h = k.size_px;
    let (mut shaper, mut caps, mut gap, total) = layout(cap_h);
    if total > k.max_width_px {
        cap_h *= (k.max_width_px / total).max(0.2);
        (shaper, caps, gap, _) = layout(cap_h);
    }
    let w = (caps.iter().sum::<f32>() + gap * (caps.len() + 1) as f32).ceil() as usize;
    let h = (cap_h + 2.0 * gap).ceil() as usize;
    let mut rgba = vec![0u8; w * h * 4];
    let blend = |p: &mut [u8], c: Color, a: f32| {
        for (i, ch) in [c.r, c.g, c.b].into_iter().enumerate() {
            p[i] = (ch * a * 255.0 + p[i] as f32 * (1.0 - a)).round().min(255.0) as u8;
        }
        p[3] = (a * 255.0 + p[3] as f32 * (1.0 - a)).round().min(255.0) as u8;
    };
    let outer_r = (gap + cap_h * 0.22).min(h as f32 / 2.0);
    let cap_r = cap_h * 0.22;
    let mut x0s = vec![];
    let mut x = gap;
    for cw in &caps {
        x0s.push(x);
        x += cw + gap;
    }
    for py in 0..h {
        for px in 0..w {
            let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
            let p = &mut rgba[(py * w + px) * 4..(py * w + px) * 4 + 4];
            let a = (0.5 - rounded_rect_sd(fx, fy, 0.0, 0.0, w as f32, h as f32, outer_r)).clamp(0.0, 1.0) * k.background.a;
            if a > 0.0 {
                blend(p, k.background, a);
            }
            for (x0, cw) in x0s.iter().zip(&caps) {
                // a lighter cap with a darker lip along the bottom edge
                let sd = rounded_rect_sd(fx, fy, *x0, gap, x0 + cw, gap + cap_h, cap_r);
                let a = (0.5 - sd).clamp(0.0, 1.0);
                if a > 0.0 {
                    let lip = fy > gap + cap_h * 0.9;
                    blend(p, k.color, a * if lip { 0.08 } else { 0.16 });
                }
            }
        }
    }
    let line_h = shaper.line_height();
    let baseline = gap + (cap_h - line_h) / 2.0 + shaper.ascent() - cap_h * 0.03;
    let mut cov = vec![0f32; w * h];
    for ((t, x0), cw) in k.keys.iter().zip(&x0s).zip(&caps) {
        let tw = shaper.word_width(t);
        shaper.draw(t, x0 + (cw - tw) / 2.0, baseline, &mut cov, w, h);
    }
    for (i, cv) in cov.iter().enumerate() {
        let a = cv * k.color.a;
        if a > 0.0 {
            blend(&mut rgba[i * 4..i * 4 + 4], k.color, a);
        }
    }
    Some(TextImage { width: w as u32, height: h as u32, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caption(words: &[(&str, bool, f32)], box_to_revealed: bool) -> CaptionText {
        CaptionText {
            words: words.iter().map(|(t, l, a)| TextWord { text: t.to_string(), laid_out: *l, alpha: *a, drop01: 0.0 }).collect(),
            family: FontFamily::Sans,
            size_px: 40.0,
            color: Color::WHITE,
            background: Color::BLACK.with_alpha(0.75),
            box_to_revealed,
            max_width_px: 4000.0,
        }
    }

    fn ink(img: &TextImage) -> usize {
        // bright pixels = text over the dark box
        img.rgba.chunks_exact(4).filter(|p| p[0] > 128).count()
    }

    #[test]
    fn draws_text_on_a_box() {
        let img = rasterize_caption(&caption(&[("Hello", true, 1.0), ("world", true, 1.0)], false)).unwrap();
        assert!(img.width > 150 && img.height >= 40, "{}x{}", img.width, img.height);
        assert!(ink(&img) > 300, "text pixels: {}", ink(&img));
        // corners of the box are transparent (rounded), its middle row is opaque
        assert_eq!(img.rgba[3], 0);
        let mid = (img.height as usize / 2 * img.width as usize) * 4;
        assert!(img.rgba[mid + 3] > 150);
    }

    #[test]
    fn hidden_words_keep_space_but_draw_nothing() {
        let all = rasterize_caption(&caption(&[("Hello", true, 1.0), ("world", true, 1.0)], false)).unwrap();
        let half = rasterize_caption(&caption(&[("Hello", true, 1.0), ("world", true, 0.0)], false)).unwrap();
        assert_eq!(all.width, half.width);
        assert!(ink(&half) < ink(&all) * 2 / 3);
        assert!(rasterize_caption(&caption(&[("Hello", false, 0.0)], false)).is_none());
    }

    #[test]
    fn shrinks_to_the_width_limit_and_draws_cyrillic() {
        let mut c = caption(&[("Привет", true, 1.0), ("мир", true, 1.0)], true);
        c.max_width_px = 120.0;
        let img = rasterize_caption(&c).unwrap();
        assert!(img.width <= 121, "{}", img.width);
        assert!(ink(&img) > 20);
    }

    fn keys(k: &[&str]) -> KeyCaps {
        KeyCaps { keys: k.iter().map(|s| s.to_string()).collect(), family: FontFamily::Sans, size_px: 64.0, color: Color::WHITE, background: Color::BLACK.with_alpha(0.78), max_width_px: 4000.0 }
    }

    #[test]
    fn key_caps_draw_symbols_from_the_fallback_font() {
        let img = rasterize_keys(&keys(&["⇧", "⌘", "S"])).unwrap();
        // three caps at least as wide as they are tall
        assert!(img.width as f32 >= 3.0 * 64.0 && img.height >= 64, "{}x{}", img.width, img.height);
        // each symbol alone draws ink, so ⌘ and ⇧ are not missing glyphs
        for k in ["⌘", "⇧", "⌥", "⌃", "⏎", "⌫"] {
            let one = rasterize_keys(&keys(&[k])).unwrap();
            assert!(ink(&one) > 30, "{k}: {}", ink(&one));
        }
        assert!(rasterize_keys(&keys(&[])).is_none());
    }

    #[test]
    fn key_caps_shrink_to_the_width_limit() {
        let mut k = keys(&["⌃", "⌥", "⇧", "⌘", "Space"]);
        k.max_width_px = 200.0;
        assert!(rasterize_keys(&k).unwrap().width <= 201);
    }
}
