//! Virtual camera: resolves which zoom is active and where it looks, then animates the
//! screen rect with the screen spring.
//!
//! Automatic zooms follow the cursor. For each zoom a focus path is precomputed from the
//! cursor samples (and the text caret while typing): the camera aims a little ahead of the
//! cursor, ignores movement inside a small dead zone around the view centre so a resting
//! hand does not shake the picture, and the path is smoothed over past and future samples,
//! so pans ease in and out without lagging behind. Being precomputed, the same path drives
//! the preview, seeking and export.

use sf_project::schema::common::Rect;
use sf_project::v4::Zoom;

use crate::cursor::CursorTrack;
use crate::spring::{SpringParams, SpringVec};
use crate::stage::StageGeometry;

/// How long before a zoom the camera starts moving when `zooms.startAnimationEarly` is on.
pub const LEAD_IN_MS: f64 = 350.0;
/// Focus path resolution.
pub const PATH_STEP_MS: f64 = 10.0;
/// The camera aims where the cursor will be this much later, so it does not trail behind.
pub const LOOK_AHEAD_MS: f64 = 150.0;
/// The cursor moves freely inside this fraction of the visible area (per axis, around the
/// centre) before the camera follows.
pub const DEAD_ZONE01: f64 = 0.2;
/// Width (σ) of the centred Gaussian that smooths the path: the ease in/out of a pan.
pub const SMOOTHING_MS: f64 = 120.0;

/// Precomputed focus (normalized 0..1 of the crop) over a zoom, one point per [`PATH_STEP_MS`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FocusPath {
    pub start_ms: f64,
    pub points: Vec<[f64; 2]>,
}

impl FocusPath {
    /// Linearly interpolated focus at source time `t` (clamped to the path).
    pub fn at(&self, t: f64) -> Option<[f64; 2]> {
        let last = self.points.len().checked_sub(1)?;
        let f = ((t - self.start_ms) / PATH_STEP_MS).clamp(0.0, last as f64);
        let i = (f.floor() as usize).min(last);
        let j = (i + 1).min(last);
        let k = f - i as f64;
        let (a, b) = (self.points[i], self.points[j]);
        Some([a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k])
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedZoom {
    pub zoom: Zoom,
    pub path: FocusPath,
}

impl ResolvedZoom {
    pub fn start_ms(&self) -> f64 {
        self.zoom.source_start_ms
    }
    pub fn end_ms(&self) -> f64 {
        self.zoom.source_end_ms
    }
    pub fn scale(&self) -> f64 {
        self.zoom.zoom.max(1.0)
    }

    /// Focus (normalized) at source time `t`.
    pub fn focus01(&self, t: f64) -> Option<[f64; 2]> {
        if !self.zoom.is_auto() {
            return self.zoom.manual_target_point.map(|p| [p.x, p.y]);
        }
        self.path.at(t)
    }
}

/// Where the pointer of attention is at `t`: the latest of the mouse sample and the text
/// caret, so typing moves the focus to the text even when the mouse is parked elsewhere.
fn pointer_at(track: &CursorTrack, t: f64) -> Option<[f64; 2]> {
    let mouse = track.raw_at(t).map(|s| (s.time_ms, [s.x, s.y]));
    let i = track.carets.partition_point(|c| c.time_ms <= t);
    let caret = i.checked_sub(1).map(|i| track.carets[i]).map(|c| (c.time_ms, [c.x, c.y]));
    match (mouse, caret) {
        (Some(m), Some(c)) => Some(if c.0 > m.0 { c.1 } else { m.1 }),
        (m, c) => m.or(c).map(|x| x.1),
    }
}

/// Focus path for an automatic zoom over `[start, end)`. `crop_px` is the visible crop in
/// recording points (the space of the cursor samples); `visible01` is the part of the crop
/// visible at the zoom level, per axis.
pub fn focus_path(track: &CursorTrack, start_ms: f64, end_ms: f64, crop_px: Rect, visible01: [f64; 2]) -> FocusPath {
    if end_ms <= start_ms || (track.samples.is_empty() && track.carets.is_empty()) {
        return FocusPath::default();
    }
    let to01 = |p: [f64; 2]| [((p[0] - crop_px.x) / crop_px.width.max(1e-6)).clamp(0.0, 1.0), ((p[1] - crop_px.y) / crop_px.height.max(1e-6)).clamp(0.0, 1.0)];
    let target = |t: f64| pointer_at(track, t + LOOK_AHEAD_MS).map(to01);
    let n = ((end_ms - start_ms) / PATH_STEP_MS).ceil() as usize + 1;
    // pad by the smoothing window on both sides so the ends are smoothed like the middle
    let pad = (3.0 * SMOOTHING_MS / PATH_STEP_MS).ceil() as usize;
    let half = [visible01[0] * DEAD_ZONE01 / 2.0, visible01[1] * DEAD_ZONE01 / 2.0];

    // dead-zone follow
    let mut focus = target(start_ms - pad as f64 * PATH_STEP_MS).unwrap_or([0.5, 0.5]);
    let mut raw = Vec::with_capacity(n + 2 * pad);
    for i in 0..n + 2 * pad {
        let t = start_ms + (i as f64 - pad as f64) * PATH_STEP_MS;
        if let Some(c) = target(t) {
            for a in 0..2 {
                if c[a] > focus[a] + half[a] {
                    focus[a] = c[a] - half[a];
                } else if c[a] < focus[a] - half[a] {
                    focus[a] = c[a] + half[a];
                }
            }
        }
        raw.push(focus);
    }

    // centred Gaussian smoothing
    let sigma = SMOOTHING_MS / PATH_STEP_MS;
    let weights: Vec<f64> = (0..=pad).map(|k| (-(k as f64).powi(2) / (2.0 * sigma * sigma)).exp()).collect();
    let points = (pad..pad + n)
        .map(|i| {
            let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
            for (k, w) in weights.iter().enumerate() {
                for j in [i - k, i + k].into_iter().take(if k == 0 { 1 } else { 2 }) {
                    sx += raw[j][0] * w;
                    sy += raw[j][1] * w;
                    sw += w;
                }
            }
            [sx / sw, sy / sw]
        })
        .collect();
    FocusPath { start_ms, points }
}

#[derive(Debug, Clone)]
pub struct CameraSolver {
    zooms: Vec<ResolvedZoom>,
    geometry: StageGeometry,
    spring: SpringVec<4>,
    initialised: bool,
    pub start_early: bool,
}

impl CameraSolver {
    /// `crop_px` is the visible crop in recording points (the space of the cursor samples).
    pub fn new(zooms: &[Zoom], track: &CursorTrack, crop_px: Rect, geometry: StageGeometry, params: SpringParams, start_early: bool) -> Self {
        let mut resolved: Vec<ResolvedZoom> = zooms
            .iter()
            .filter(|z| !z.is_disabled && z.source_end_ms > z.source_start_ms)
            .map(|z| {
                let vis = geometry.visible_part(z.zoom);
                let path = if z.is_auto() { focus_path(track, z.source_start_ms, z.source_end_ms, crop_px, vis) } else { FocusPath::default() };
                ResolvedZoom { zoom: z.clone(), path }
            })
            .collect();
        resolved.sort_by(|a, b| a.start_ms().total_cmp(&b.start_ms()));
        let i = geometry.initial_screen;
        CameraSolver { zooms: resolved, geometry, spring: SpringVec::new(params, [i.x, i.y, i.width, i.height]), initialised: false, start_early }
    }

    pub fn zooms(&self) -> &[ResolvedZoom] {
        &self.zooms
    }

    pub fn geometry(&self) -> &StageGeometry {
        &self.geometry
    }

    /// Replace the stage geometry (e.g. a split-screen layout shrinks the zoom area). The
    /// spring keeps animating from the current rect, so layout switches are smooth.
    pub fn set_geometry(&mut self, geometry: StageGeometry) {
        self.geometry = geometry;
    }

    /// The zoom driving the screen at source time `t` (with lead-in when enabled).
    pub fn active_zoom(&self, t: f64) -> Option<&ResolvedZoom> {
        if let Some(z) = self.zooms.iter().find(|z| t >= z.start_ms() && t < z.end_ms()) {
            return Some(z);
        }
        if self.start_early {
            return self.zooms.iter().find(|z| t >= z.start_ms() - LEAD_IN_MS && t < z.start_ms());
        }
        None
    }

    /// Target screen rect at `t`. `resting_focus01` is where the camera looks when not zoomed.
    pub fn target_rect(&self, t: f64, resting_focus01: [f64; 2]) -> (Rect, bool) {
        match self.active_zoom(t) {
            Some(z) => {
                let focus = z.focus01(t).unwrap_or([0.5, 0.5]);
                (self.geometry.screen_rect(focus, z.scale()), z.zoom.has_instant_animation)
            }
            None => (self.geometry.screen_rect(resting_focus01, 1.0), false),
        }
    }

    pub fn reset_to(&mut self, rect: Rect) {
        self.spring.snap([rect.x, rect.y, rect.width, rect.height]);
        self.initialised = true;
    }

    pub fn reset(&mut self, t: f64, resting_focus01: [f64; 2]) {
        let (r, _) = self.target_rect(t, resting_focus01);
        self.reset_to(r);
    }

    /// Advance by `dt_ms` and return the animated screen rect (stage units).
    pub fn step(&mut self, t: f64, resting_focus01: [f64; 2], dt_ms: f64) -> Rect {
        let (target, instant) = self.target_rect(t, resting_focus01);
        if !self.initialised || instant {
            self.reset_to(target);
            return target;
        }
        let p = self.spring.step([target.x, target.y, target.width, target.height], dt_ms);
        Rect { x: p[0], y: p[1], width: p[2], height: p[3] }
    }

    pub fn current(&self) -> Rect {
        let p = self.spring.position();
        Rect { x: p[0], y: p[1], width: p[2], height: p[3] }
    }

    /// Previous-frame rect is needed for motion blur; expose velocity for that.
    pub fn velocity(&self) -> [f64; 4] {
        self.spring.velocity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor::{CaretSample, CursorSample};

    const CROP: Rect = Rect { x: 0.0, y: 0.0, width: 1000.0, height: 1000.0 };

    fn track(points: impl IntoIterator<Item = (f64, f64, f64)>) -> CursorTrack {
        CursorTrack { samples: points.into_iter().map(|(time_ms, x, y)| CursorSample { x, y, cursor_id_index: 0, time_ms }).collect(), ..Default::default() }
    }

    fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
    }

    #[test]
    fn typing_moves_focus_to_the_caret() {
        // mouse parked top-left, then typing at the bottom-right
        let mut t = track([(0.0, 100.0, 100.0)]);
        for i in 0..20 {
            t.carets.push(CaretSample { time_ms: 1000.0 + i as f64 * 150.0, x: 800.0 + i as f64 * 5.0, y: 850.0 });
        }
        let p = focus_path(&t, 0.0, 5000.0, CROP, [0.5, 0.5]);
        let end = p.at(4900.0).unwrap();
        assert!(dist(end, [0.845, 0.85]) <= 0.5 * DEAD_ZONE01 / 2.0 * 2f64.sqrt() + 1e-9, "the caret is inside the dead zone: {end:?}");
        assert!(p.at(200.0).unwrap()[0] < 0.2, "starts on the mouse");
    }

    #[test]
    fn follows_a_moving_cursor_closely() {
        // a sweep across the screen in 3 s
        let t = track((0..=300).map(|i| (i as f64 * 10.0, 100.0 + i as f64 * 2.5, 500.0)));
        let p = focus_path(&t, 0.0, 3000.0, CROP, [0.5, 0.5]);
        for ms in [500.0, 1000.0, 1500.0, 2000.0, 2500.0] {
            let cursor = [(100.0 + ms / 10.0 * 2.5) / 1000.0, 0.5];
            let f = p.at(ms).unwrap();
            // the cursor stays near the centre of the view, never near its edge
            assert!(dist(f, cursor) < 0.5 * 0.5 * 0.5, "at {ms}: focus {f:?}, cursor {cursor:?}");
        }
    }

    #[test]
    fn small_movements_do_not_shake_the_view() {
        let t = track((0..=200).map(|i| (i as f64 * 10.0, 500.0 + if i % 2 == 0 { 20.0 } else { -20.0 }, 500.0)));
        let p = focus_path(&t, 0.0, 2000.0, CROP, [0.5, 0.5]);
        let xs: Vec<f64> = (0..p.points.len()).map(|i| p.points[i][0]).collect();
        let spread = xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min);
        assert!(spread < 0.01, "jitter inside the dead zone moved the camera by {spread}");
    }

    #[test]
    fn a_jump_is_eased_in_and_out() {
        // cursor jumps from the left to the right at 1 s
        let t = track([(0.0, 100.0, 500.0), (1000.0, 900.0, 500.0)]);
        let p = focus_path(&t, 0.0, 3000.0, CROP, [0.5, 0.5]);
        let step = |i: usize| p.points[i + 1][0] - p.points[i][0];
        let speeds: Vec<f64> = (0..p.points.len() - 1).map(step).collect();
        let peak = speeds.iter().cloned().enumerate().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        assert!(peak.1 < 0.1, "no instant jump: {}", peak.1);
        // speed rises before the peak and falls after it
        assert!(speeds[peak.0 - 10] < peak.1 && speeds[peak.0 + 10] < peak.1);
        assert!(speeds[peak.0 - 10] > 0.0 && speeds[peak.0 + 10] > 0.0);
        // and settles on the new position with the cursor inside the dead zone
        assert!((p.at(2900.0).unwrap()[0] - 0.9).abs() <= 0.5 * DEAD_ZONE01 / 2.0 + 1e-9);
    }

    #[test]
    fn focus_is_relative_to_the_crop() {
        let t = track([(0.0, 600.0, 400.0)]);
        let crop = Rect { x: 500.0, y: 300.0, width: 200.0, height: 200.0 };
        let f = focus_path(&t, 0.0, 1000.0, crop, [0.5, 0.5]).at(500.0).unwrap();
        assert!(dist(f, [0.5, 0.5]) < 1e-6, "{f:?}");
    }
}
