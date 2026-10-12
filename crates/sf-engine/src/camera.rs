//! Virtual camera: resolves which zoom is active and where it looks, then animates the
//! screen rect with the screen spring.
//!
//! Automatic zooms ("follow-click-groups") do not chase the cursor continuously. The mouse
//! events inside the zoom are split into *groups*: a new group starts whenever the group's
//! bounding box would exceed half of the area visible at that zoom level. The camera looks
//! at the time-weighted centre of the current group and jumps (spring-animated) to the next
//! one when the cursor leaves it.

use sf_project::schema::common::{Point, Rect};
use sf_project::v4::Zoom;

use crate::cursor::{CursorSample, CursorTrack};
use crate::spring::{SpringParams, SpringVec};
use crate::stage::StageGeometry;

/// How long before a zoom the camera starts moving when `zooms.startAnimationEarly` is on.
pub const LEAD_IN_MS: f64 = 350.0;

#[derive(Debug, Clone, PartialEq)]
pub struct ClickGroup {
    pub start_ms: f64,
    pub end_ms: f64,
    /// Normalized (0..1 of the crop) centre.
    pub center01: [f64; 2],
}

#[derive(Debug, Clone)]
pub struct ResolvedZoom {
    pub zoom: Zoom,
    pub groups: Vec<ClickGroup>,
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
        let g = self.groups.iter().rev().find(|g| g.start_ms <= t).or_else(|| self.groups.first())?;
        Some(g.center01)
    }
}

/// Split the cursor samples inside `[start, end)` (plus the last one before) into groups.
/// `max_extent01` is half the visible part of the crop at the zoom level, per axis.
pub fn click_groups(track: &CursorTrack, start_ms: f64, end_ms: f64, crop_size_px: [f64; 2], max_extent01: [f64; 2]) -> Vec<ClickGroup> {
    let samples = &track.samples;
    let lo = samples.partition_point(|s| s.time_ms < start_ms).saturating_sub(1);
    let hi = samples.partition_point(|s| s.time_ms < end_ms);
    if lo >= hi {
        return vec![];
    }
    // decimate to >= 10 ms like Screen Studio
    let mut events: Vec<CursorSample> = Vec::new();
    for s in &samples[lo..hi] {
        if events.last().map(|l| s.time_ms - l.time_ms >= 10.0).unwrap_or(true) {
            events.push(*s);
        }
    }
    let max_w = max_extent01[0] * crop_size_px[0];
    let max_h = max_extent01[1] * crop_size_px[1];
    let mut groups: Vec<ClickGroup> = vec![];
    let mut cur: Vec<CursorSample> = vec![];
    let mut bbox = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let flush = |cur: &mut Vec<CursorSample>, groups: &mut Vec<ClickGroup>, next_time: f64| {
        if cur.is_empty() {
            return;
        }
        // time-weighted centre: each event weighted by the time until the next one
        let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
        for (i, e) in cur.iter().enumerate() {
            let next = cur.get(i + 1).map(|n| n.time_ms).unwrap_or(next_time);
            let w = (next - e.time_ms).max(1.0);
            sx += e.x * w;
            sy += e.y * w;
            sw += w;
        }
        groups.push(ClickGroup {
            start_ms: cur[0].time_ms,
            end_ms: next_time,
            center01: [(sx / sw / crop_size_px[0]).clamp(0.0, 1.0), (sy / sw / crop_size_px[1]).clamp(0.0, 1.0)],
        });
        cur.clear();
    };
    for e in events {
        let nb = (bbox.0.min(e.x), bbox.1.min(e.y), bbox.2.max(e.x), bbox.3.max(e.y));
        if !cur.is_empty() && (nb.2 - nb.0 > max_w || nb.3 - nb.1 > max_h) {
            flush(&mut cur, &mut groups, e.time_ms);
            bbox = (e.x, e.y, e.x, e.y);
        } else {
            bbox = nb;
        }
        cur.push(e);
    }
    flush(&mut cur, &mut groups, end_ms);
    groups
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
    pub fn new(zooms: &[Zoom], track: &CursorTrack, crop_size_px: [f64; 2], geometry: StageGeometry, params: SpringParams, start_early: bool) -> Self {
        let mut resolved: Vec<ResolvedZoom> = zooms
            .iter()
            .filter(|z| !z.is_disabled && z.source_end_ms > z.source_start_ms)
            .map(|z| {
                let vis = geometry.visible_part(z.zoom);
                let groups = if z.is_auto() { click_groups(track, z.source_start_ms, z.source_end_ms, crop_size_px, [vis[0] * 0.5, vis[1] * 0.5]) } else { vec![] };
                ResolvedZoom { zoom: z.clone(), groups }
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

impl From<Point> for ClickGroup {
    fn from(p: Point) -> Self {
        ClickGroup { start_ms: 0.0, end_ms: 0.0, center01: [p.x, p.y] }
    }
}
