//! Stage geometry: Screen Studio lays the frame out on a "stage" that is 1000 units tall.
//!
//! * `stage`      – the whole canvas (output aspect ratio, height 1000).
//! * `zoom_area`  – the stage minus padding; the screen is fitted inside it at zoom 1.
//! * `screen`     – the screen rect; zooming scales it up around a focus point so that the
//!                  focus lands at the zoom-area centre, clamped so the screen always covers
//!                  the zoom area (never shows empty background inside it).
//!
//! Padding (`output.paddingRatio01`) is a ratio of the *screen height*: with auto aspect a
//! 4:3 recording at 0.1 padding yields a 1380×1080 canvas (matches Screen Studio).

use sf_project::schema::common::Rect;

pub const STAGE_HEIGHT: f64 = 1000.0;

/// Output canvas size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputSize {
    pub width: u32,
    pub height: u32,
}

impl OutputSize {
    /// Pixels per stage unit.
    pub fn scale(&self) -> f64 {
        self.height as f64 / STAGE_HEIGHT
    }
}
const WIDE: f64 = 16.0 / 9.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageGeometry {
    pub stage: Rect,
    pub zoom_area: Rect,
    /// Screen rect at zoom level 1.
    pub initial_screen: Rect,
    pub screen_aspect: f64,
}

/// Canvas aspect for `output.aspectRatio == "auto"`.
pub fn auto_aspect(screen_aspect: f64, padding01: f64) -> f64 {
    if screen_aspect >= WIDE - 1e-9 {
        return WIDE;
    }
    // screen height 1, padding p on each side (ratio of screen height)
    let p = padding01.max(0.0);
    ((screen_aspect + 2.0 * p) / (1.0 + 2.0 * p)).clamp(1.0, 2.4)
}

pub fn parse_aspect(s: &str) -> Option<f64> {
    let (w, h) = s.split_once(':').or_else(|| s.split_once('/'))?;
    let (w, h): (f64, f64) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (h > 0.0 && w > 0.0).then_some(w / h)
}

impl StageGeometry {
    /// `aspect`: `"auto"` or `"16:9"`; `screen_aspect` = crop width / height;
    /// `avoid_empty_zoom_area` = `output.avoidEmptyZoomArea`.
    pub fn new(aspect: &str, screen_aspect: f64, padding01: f64, avoid_empty_zoom_area: bool) -> Self {
        let screen_aspect = if screen_aspect.is_finite() && screen_aspect > 0.0 { screen_aspect } else { WIDE };
        let stage_aspect = parse_aspect(aspect).unwrap_or_else(|| auto_aspect(screen_aspect, padding01));
        let stage = Rect { x: 0.0, y: 0.0, width: STAGE_HEIGHT * stage_aspect, height: STAGE_HEIGHT };
        let p = padding01.clamp(0.0, 0.45);
        // Fit the screen (plus padding measured in screen heights) inside the stage.
        let by_height = stage.height / (1.0 + 2.0 * p);
        let by_width = stage.width / (screen_aspect + 2.0 * p);
        let screen_h = by_height.min(by_width);
        let pad = p * screen_h;
        let zoom_area = Rect { x: pad, y: pad, width: stage.width - 2.0 * pad, height: stage.height - 2.0 * pad };
        let initial_screen = if avoid_empty_zoom_area { cover(zoom_area, screen_aspect) } else { fit_inside(zoom_area, screen_aspect) };
        StageGeometry { stage, zoom_area, initial_screen, screen_aspect }
    }

    /// Output pixel size for a given height.
    pub fn output_size(&self, height: u32) -> OutputSize {
        let h = (height.max(2) as f64).floor();
        let w = (h * self.stage.width / self.stage.height).round();
        OutputSize { width: (w as u32) & !1, height: (h as u32) & !1 }
    }

    /// Screen rect for a zoom `scale` focused at `focus01` (normalized inside the screen).
    pub fn screen_rect(&self, focus01: [f64; 2], scale: f64) -> Rect {
        let scale = scale.max(1.0);
        let i = self.initial_screen;
        let w = i.width * scale;
        let h = i.height * scale;
        let za = self.zoom_area;
        let cx = za.x + za.width / 2.0;
        let cy = za.y + za.height / 2.0;
        // put the focus point at the zoom-area centre…
        let mut x = cx - focus01[0].clamp(0.0, 1.0) * w;
        let mut y = cy - focus01[1].clamp(0.0, 1.0) * h;
        // …then clamp so the screen covers the zoom area on each axis it can cover, and stays
        // centred on an axis where it is smaller than the zoom area.
        x = clamp_axis(x, w, za.x, za.width);
        y = clamp_axis(y, h, za.y, za.height);
        Rect { x, y, width: w, height: h }
    }

    /// Fraction of the screen visible on the stage at `scale` (per axis, 0..1).
    pub fn visible_part(&self, scale: f64) -> [f64; 2] {
        let w = self.initial_screen.width * scale.max(1.0);
        let h = self.initial_screen.height * scale.max(1.0);
        [(self.stage.width / w).min(1.0), (self.stage.height / h).min(1.0)]
    }
}

fn clamp_axis(pos: f64, size: f64, area_pos: f64, area_size: f64) -> f64 {
    if size >= area_size {
        // `min` may exceed `max` by floating-point noise when the sizes are equal
        let lo = area_pos + area_size - size;
        pos.max(lo).min(area_pos.max(lo))
    } else {
        area_pos + (area_size - size) / 2.0
    }
}

pub fn fit_inside(area: Rect, aspect: f64) -> Rect {
    let (w, h) = if area.width / area.height > aspect { (area.height * aspect, area.height) } else { (area.width, area.width / aspect) };
    Rect { x: area.x + (area.width - w) / 2.0, y: area.y + (area.height - h) / 2.0, width: w, height: h }
}

pub fn cover(area: Rect, aspect: f64) -> Rect {
    let (w, h) = if area.width / area.height > aspect { (area.width, area.width / aspect) } else { (area.height * aspect, area.height) };
    Rect { x: area.x + (area.width - w) / 2.0, y: area.y + (area.height - h) / 2.0, width: w, height: h }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_screen_studio_auto_canvas() {
        // 1200×900 recording, padding 0.1, auto → Screen Studio renders 1380×1080
        let g = StageGeometry::new("auto", 1200.0 / 900.0, 0.1, false);
        let o = g.output_size(1080);
        assert_eq!((o.width, o.height), (1380, 1080));
        // 2560×1440 at padding 0 → 1920×1080
        let g = StageGeometry::new("auto", 16.0 / 9.0, 0.0, false);
        let o = g.output_size(1080);
        assert_eq!((o.width, o.height), (1920, 1080));
        let g = StageGeometry::new("16:9", 4.0 / 3.0, 0.1, false);
        let o = g.output_size(1080);
        assert_eq!((o.width, o.height), (1920, 1080));
    }

    #[test]
    fn zoom_covers_zoom_area() {
        let g = StageGeometry::new("auto", 4.0 / 3.0, 0.1, false);
        let r1 = g.screen_rect([0.5, 0.5], 1.0);
        assert!((r1.x - g.zoom_area.x).abs() < 1e-6);
        let r2 = g.screen_rect([0.0, 0.0], 2.0);
        // top-left focus: screen's top-left pinned to the zoom area's top-left
        assert!((r2.x - g.zoom_area.x).abs() < 1e-6 && (r2.y - g.zoom_area.y).abs() < 1e-6);
        let r3 = g.screen_rect([1.0, 1.0], 2.0);
        assert!((r3.x + r3.width - (g.zoom_area.x + g.zoom_area.width)).abs() < 1e-6);
        // initial screen is 833.3 tall (0.1 padding) → 1666.7 at 2×, stage shows 60 % of it
        assert!((g.visible_part(2.0)[1] - 0.6).abs() < 1e-9);
    }
}
