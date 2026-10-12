//! Cursor position sampling, shake removal, smoothing and tilt.
//!
//! Matches Screen Studio's pipeline: events are made crop-relative, spikes are removed
//! (`removeShakeTreshold`), decimated to ≥10 ms, the position is spring-smoothed with
//! `mouseMovementSpring` (null = raw), a click within 500 ms snaps the target to the click,
//! and the sprite tilts with horizontal velocity (`rotationMode: "velocity"`).

use sf_project::recording::InputEvent;
use sf_project::schema::common::Rect;

use crate::spring::{SpringParams, SpringTrack, SpringVec};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorSample {
    /// Position in recording-local points (bounds relative, top-left origin).
    pub x: f64,
    pub y: f64,
    pub cursor_id_index: usize,
    /// Source-time ms (session relative).
    pub time_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClickSample {
    pub time_ms: f64,
    pub x: f64,
    pub y: f64,
    pub down: bool,
    pub right_button: bool,
}

/// Pre-processed cursor track for one session.
#[derive(Debug, Clone, Default)]
pub struct CursorTrack {
    pub samples: Vec<CursorSample>,
    pub cursor_ids: Vec<String>,
    pub clicks: Vec<ClickSample>,
    /// Source-time ms of key-down events (for "hide while typing").
    pub key_downs: Vec<f64>,
}

impl CursorTrack {
    pub fn build(moves: &[InputEvent], clicks: &[InputEvent], keys: &[InputEvent], bounds: Rect, process_time_start_ms: f64, shake_threshold: f64) -> Self {
        let mut ids: Vec<String> = Vec::new();
        let mut id_index = |id: &str| -> usize {
            if let Some(i) = ids.iter().position(|x| x == id) {
                i
            } else {
                ids.push(id.to_string());
                ids.len() - 1
            }
        };
        #[derive(Clone, Copy)]
        struct Ev {
            s: CursorSample,
            down: bool,
            up: bool,
        }
        let mut events: Vec<Ev> = Vec::with_capacity(moves.len() + clicks.len());
        for e in moves.iter().chain(clicks.iter()) {
            let (Some(x), Some(y)) = (e.x, e.y) else { continue };
            events.push(Ev {
                s: CursorSample { x: x - bounds.x, y: y - bounds.y, cursor_id_index: id_index(e.cursor_id.as_deref().unwrap_or("arrow")), time_ms: e.process_time_ms - process_time_start_ms },
                down: e.kind.ends_with("Down") && e.kind.starts_with("mouse") || e.kind == "rightMouseDown",
                up: e.kind.ends_with("Up") && e.kind.starts_with("mouse") || e.kind == "rightMouseUp",
            });
        }
        events.sort_by(|a, b| a.s.time_ms.total_cmp(&b.s.time_ms));

        // Shake removal (Screen Studio `al`): drop an event when both the next and the
        // second-next events are farther than the threshold (Manhattan distance); a dropped
        // mouseDown also drops its mouseUp.
        let dist = |a: &Ev, b: &Ev| (a.s.x - b.s.x).abs() + (a.s.y - b.s.y).abs();
        let mut kept: Vec<Ev> = Vec::with_capacity(events.len());
        let mut skip_up = false;
        for i in 0..events.len() {
            let e = events[i];
            if skip_up && e.up {
                skip_up = false;
                continue;
            }
            let spike = shake_threshold > 0.0
                && events.get(i + 1).map(|n| dist(&e, n) > shake_threshold).unwrap_or(false)
                && events.get(i + 2).map(|n| dist(&e, n) > shake_threshold).unwrap_or(false);
            if spike {
                if e.down {
                    skip_up = true;
                }
                continue;
            }
            kept.push(e);
        }
        let samples = kept.iter().map(|e| e.s).collect();
        let click_samples = clicks
            .iter()
            .filter_map(|e| {
                Some(ClickSample {
                    time_ms: e.process_time_ms - process_time_start_ms,
                    x: e.x? - bounds.x,
                    y: e.y? - bounds.y,
                    down: e.kind == "mouseDown" || e.kind == "rightMouseDown",
                    right_button: e.button.as_deref() == Some("right") || e.kind.starts_with("right"),
                })
            })
            .collect();
        let key_downs = keys.iter().filter(|k| k.kind == "keyDown").map(|k| k.process_time_ms - process_time_start_ms).collect();
        CursorTrack { samples, cursor_ids: ids, clicks: click_samples, key_downs }
    }

    /// Raw position at `t`: the last sample at or before `t` (Screen Studio holds the last
    /// event; the spring produces the in-between motion).
    pub fn raw_at(&self, t: f64) -> Option<CursorSample> {
        if self.samples.is_empty() {
            return None;
        }
        let i = self.samples.partition_point(|s| s.time_ms <= t);
        Some(if i == 0 { self.samples[0] } else { self.samples[i - 1] })
    }

    /// A mouse-down within `window_ms` before `t`, if any (the cursor snaps to clicks).
    pub fn recent_click(&self, t: f64, window_ms: f64) -> Option<ClickSample> {
        self.clicks.iter().rev().find(|c| c.down && c.time_ms <= t && t - c.time_ms <= window_ms).copied()
    }

    pub fn is_pressed(&self, t: f64) -> bool {
        self.clicks.iter().rev().find(|c| c.time_ms <= t).map(|c| c.down).unwrap_or(false)
    }

    /// Any mouse event within `[t - window, t]`? (hide-when-idle)
    pub fn moved_within(&self, t: f64, window_ms: f64) -> bool {
        let lo = self.samples.partition_point(|s| s.time_ms < t - window_ms);
        let hi = self.samples.partition_point(|s| s.time_ms <= t);
        hi > lo
    }

    /// Typing with no later mouse movement (cursor hidden while typing).
    pub fn is_typing(&self, t: f64, window_ms: f64) -> bool {
        let last_key = self.key_downs.iter().rev().find(|k| **k <= t).copied();
        let Some(k) = last_key else { return false };
        if t - k > window_ms {
            return false;
        }
        let last_move = self.raw_at(t).map(|s| s.time_ms).unwrap_or(f64::MIN);
        last_move < k
    }

    /// Horizontal velocity in px/ms around `t`, from raw samples.
    pub fn raw_velocity_x(&self, t: f64) -> f64 {
        let i = self.samples.partition_point(|s| s.time_ms <= t);
        if i < 2 {
            return 0.0;
        }
        let (a, b) = (self.samples[i - 2], self.samples[i - 1]);
        let dt = (b.time_ms - a.time_ms).max(1.0);
        if t - b.time_ms > 120.0 {
            return 0.0;
        }
        (b.x - a.x) / dt
    }
}

/// Screen Studio's velocity tilt constants (`Vl`).
pub const TILT_SMOOTHING_MS: f64 = 120.0;
pub const TILT_DEG_PER_VELOCITY: f64 = 12.0;
pub const TILT_MAX_DEG: f64 = 20.0;
pub const TILT_STARTUP_CALM_MS: f64 = 500.0;
pub const TILT_SPRING: SpringParams = SpringParams { stiffness: 120.0, damping: 8.0, mass: 1.0, clamp: false, precision: 0.002 };
pub const PRESSED_SCALE: f64 = 0.9;
pub const CLICK_SNAP_MS: f64 = 500.0;

/// Stateful per-frame cursor animator (position spring, pressed scale, tilt).
pub struct CursorSmoother {
    position: SpringVec<2>,
    scale: SpringTrack,
    tilt: SpringTrack,
    smoothed_vx: f64,
    initialised: bool,
    start_ms: f64,
}

impl CursorSmoother {
    pub fn new(params: SpringParams) -> Self {
        CursorSmoother {
            position: SpringVec::new(params, [0.0, 0.0]),
            scale: SpringTrack::new(params, 1.0),
            tilt: SpringTrack::new(TILT_SPRING, 0.0),
            smoothed_vx: 0.0,
            initialised: false,
            start_ms: 0.0,
        }
    }

    pub fn set_params(&mut self, params: SpringParams) {
        self.position.set_params(params);
        self.scale.params = params;
    }

    /// Jump to `pos` with no animation (used across cuts / seeks).
    pub fn reset(&mut self, pos: [f64; 2], t: f64) {
        self.position.snap(pos);
        self.scale.snap(1.0);
        self.tilt.snap(0.0);
        self.smoothed_vx = 0.0;
        self.initialised = true;
        self.start_ms = t;
    }

    /// Returns `(position, scale, tilt_degrees)`.
    pub fn step(&mut self, track: &CursorTrack, t: f64, dt_ms: f64, rotate_on_x_ratio: f64, base_rotation_deg: f64, tilt_enabled: bool) -> Option<([f64; 2], f64, f64)> {
        let raw = track.raw_at(t)?;
        let mut target = [raw.x, raw.y];
        if let Some(c) = track.recent_click(t, CLICK_SNAP_MS) {
            target = [c.x, c.y];
        }
        if !self.initialised {
            self.reset(target, t);
        }
        let pos = self.position.step(target, dt_ms);
        let scale = self.scale.step(if track.is_pressed(t) { PRESSED_SCALE } else { 1.0 }, dt_ms);
        // tilt: exponentially smoothed horizontal velocity → degrees, springed
        let vx = track.raw_velocity_x(t);
        let k = 1.0 - (-dt_ms / TILT_SMOOTHING_MS).exp();
        self.smoothed_vx += (vx - self.smoothed_vx) * k;
        let calm = t - self.start_ms < TILT_STARTUP_CALM_MS;
        let target_tilt = if tilt_enabled && !calm { (self.smoothed_vx * TILT_DEG_PER_VELOCITY * rotate_on_x_ratio).clamp(-TILT_MAX_DEG, TILT_MAX_DEG) } else { 0.0 };
        let tilt = self.tilt.step(target_tilt, dt_ms);
        Some((pos, scale, base_rotation_deg + tilt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, t: f64, x: f64, y: f64) -> InputEvent {
        serde_json::from_value(serde_json::json!({"type": kind, "processTimeMs": t, "unixTimeMs": 0.0, "x": x, "y": y, "cursorId": "arrow"})).unwrap()
    }

    #[test]
    fn removes_spikes() {
        let moves = vec![ev("mouseMoved", 0.0, 10.0, 10.0), ev("mouseMoved", 10.0, 900.0, 900.0), ev("mouseMoved", 20.0, 12.0, 11.0), ev("mouseMoved", 30.0, 13.0, 11.0)];
        let t = CursorTrack::build(&moves, &[], &[], Rect { x: 0.0, y: 0.0, width: 1000.0, height: 1000.0 }, 0.0, 500.0);
        assert_eq!(t.samples.len(), 3);
        assert!(t.samples.iter().all(|s| s.x < 100.0));
    }

    #[test]
    fn smoother_follows() {
        let moves: Vec<InputEvent> = (0..50).map(|i| ev("mouseMoved", i as f64 * 16.0, i as f64 * 10.0, 100.0)).collect();
        let t = CursorTrack::build(&moves, &[], &[], Rect::UNIT, 0.0, 0.0);
        let mut s = CursorSmoother::new(SpringParams::CURSOR_SMOOTH);
        let mut last = None;
        for i in 0..60 {
            last = s.step(&t, i as f64 * 16.0, 16.0, 0.5, 0.0, true);
        }
        let (pos, scale, tilt) = last.unwrap();
        assert!(pos[0] > 300.0 && (pos[1] - 100.0).abs() < 1.0);
        assert_eq!(scale, 1.0);
        assert!(tilt > 0.5, "moving right should tilt, got {tilt}");
    }
}
