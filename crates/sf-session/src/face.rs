//! Face tracking for the camera picture ("Center the camera on your face").
//!
//! Every [`DETECT_INTERVAL_MS`] of source time the decoded webcam frame goes through
//! `sf_vision`'s face detector. The crop window is framed around the face (the face box fills
//! [`FACE_FILL`] of the window height at framing 1, eyes on the upper third) and a slow spring
//! glides the window toward it, with a dead zone so small head movements leave the shot still.
//! Without a face the window eases back to the whole picture.

use sf_engine::spring::{SpringParams, SpringVec};
use sf_project::schema::common::Rect;
use sf_vision::{Face, FaceDetector};

use crate::layout::cover_crop;

/// Source-time gap between two detections.
pub const DETECT_INTERVAL_MS: f64 = 200.0;
/// Fraction of the window height the face box occupies at framing 1.
const FACE_FILL: f64 = 0.4;
/// The face centre sits this far down the window (0.5 = centred): eyes on the upper third.
const FACE_Y: f64 = 0.46;
/// The window stays while the face centre moves less than this fraction of the window size.
const DEADZONE: f64 = 0.08;
/// … and while the face box changes by less than this ratio.
const SIZE_DEADZONE: f64 = 0.15;
/// Smallest window height as a fraction of the available crop.
const MIN_WINDOW: f64 = 0.3;
/// Forget the face after this long without a detection (source ms).
const LOST_MS: f64 = 2500.0;
/// Critically damped glide that settles in about half a second: quick enough to keep up with
/// someone leaning over, slow enough not to twitch (a camera operator, not a cursor).
const SPRING: SpringParams = SpringParams { stiffness: 90.0, damping: 23.0, mass: 1.5, clamp: true, precision: 0.0002 };

pub struct FaceTracker {
    detector: Option<FaceDetector>,
    /// `pts_ms` of the last frame that was analysed.
    last_detect_pts: Option<f64>,
    /// Last face seen and the source time it was seen at.
    face: Option<(Face, f64)>,
    target: Option<Rect>,
    spring: SpringVec<4>,
    initialised: bool,
}

impl Default for FaceTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl FaceTracker {
    pub fn new() -> Self {
        let detector = match FaceDetector::new() {
            Ok(d) => Some(d),
            Err(e) => {
                tracing::warn!("face detector unavailable: {e:#}");
                None
            }
        };
        FaceTracker { detector, last_detect_pts: None, face: None, target: None, spring: SpringVec::new(SPRING, [0.0, 0.0, 1.0, 1.0]), initialised: false }
    }

    pub fn is_available(&self) -> bool {
        self.detector.is_some()
    }

    /// The face last seen (normalized frame coordinates), if it is still fresh.
    pub fn face(&self) -> Option<Face> {
        self.face.map(|(f, _)| f)
    }

    /// Feed the current camera frame (tightly packed 4-byte pixels) at source time `source_ms`.
    /// Runs the detector when the frame is new and enough source time has passed.
    pub fn observe(&mut self, pixels: &[u8], width: u32, height: u32, bgra: bool, pts_ms: f64, source_ms: f64) {
        let Some(det) = self.detector.as_mut() else { return };
        let due = match self.last_detect_pts {
            None => true,
            Some(prev) => (pts_ms - prev).abs() >= DETECT_INTERVAL_MS - 1e-6,
        };
        if !due {
            return;
        }
        self.last_detect_pts = Some(pts_ms);
        let found = det.detect(pixels, width, height, bgra);
        tracing::debug!(pts_ms, source_ms, width, height, bgra, ?found, "face detection");
        if let Some(f) = found {
            self.face = Some((f, source_ms));
        }
    }

    /// The window to show (normalized texture coordinates inside `crop01`) for this frame.
    /// `dst_aspect` is the on-screen camera aspect (w/h in output px), `tex_aspect` the
    /// texture's (w/h in texels), `framing` the user's tightness (1 = default). `jump` snaps the
    /// spring (first frame, cut, seek); `dt_ms` advances it otherwise.
    pub fn window(&mut self, source_ms: f64, dt_ms: f64, jump: bool, crop01: Rect, dst_aspect: f64, tex_aspect: f64, framing: f64) -> Rect {
        // window aspect in normalized units
        let na = if tex_aspect > 0.0 { dst_aspect / tex_aspect } else { 1.0 };
        let full = cover_crop(crop01, tex_aspect * crop01.width / crop01.height.max(1e-9), dst_aspect);
        let fresh = self.face.filter(|(_, seen)| (source_ms - seen).abs() <= LOST_MS).map(|(f, _)| f);
        let wanted = match fresh {
            Some(f) => face_window(&f, crop01, na, framing.max(0.05)),
            None => full,
        };
        let target = match (self.target, fresh.is_some()) {
            (Some(cur), true) if within_deadzone(&cur, &wanted) => cur,
            _ => wanted,
        };
        self.target = Some(target);
        tracing::trace!(?fresh, ?wanted, ?target, crop01 = ?crop01, dst_aspect, tex_aspect, "face window");
        let t = [target.x, target.y, target.width, target.height];
        let p = if jump || !self.initialised {
            self.spring.snap(t);
            self.initialised = true;
            t
        } else {
            self.spring.step(t, dt_ms)
        };
        // keep the animated window inside the crop (the spring may overshoot a hair)
        clamp_into(Rect { x: p[0], y: p[1], width: p[2].max(1e-6), height: p[3].max(1e-6) }, crop01)
    }
}

/// Window framed around `face` (both normalized to the full frame), at normalized aspect `na`.
fn face_window(face: &Face, crop01: Rect, na: f64, framing: f64) -> Rect {
    let min_h = crop01.height * MIN_WINDOW;
    let mut h = (face.height / FACE_FILL / framing).clamp(min_h.min(crop01.height), crop01.height);
    let mut w = h * na;
    if w > crop01.width {
        w = crop01.width;
        h = w / na.max(1e-9);
    }
    let [cx, cy] = face.center();
    clamp_into(Rect { x: cx - w / 2.0, y: cy - FACE_Y * h, width: w, height: h }, crop01)
}

fn within_deadzone(cur: &Rect, next: &Rect) -> bool {
    let dx = (next.x + next.width / 2.0) - (cur.x + cur.width / 2.0);
    let dy = (next.y + next.height / 2.0) - (cur.y + cur.height / 2.0);
    let size_ratio = next.height / cur.height.max(1e-9);
    dx.abs() < DEADZONE * cur.width && dy.abs() < DEADZONE * cur.height && (size_ratio - 1.0).abs() < SIZE_DEADZONE
}

fn clamp_into(r: Rect, bounds: Rect) -> Rect {
    let width = r.width.min(bounds.width);
    let height = r.height.min(bounds.height);
    let x = r.x.clamp(bounds.x, bounds.x + bounds.width - width);
    let y = r.y.clamp(bounds.y, bounds.y + bounds.height - height);
    Rect { x, y, width, height }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(cx: f64, cy: f64, h: f64) -> Face {
        Face { x: cx - h * 0.4, y: cy - h / 2.0, width: h * 0.8, height: h, score: 5.0 }
    }

    #[test]
    fn window_frames_the_face_and_stays_inside() {
        // square window on a 16:9 frame: normalized aspect = 1 / (16/9)
        let na = 1.0 / (16.0 / 9.0);
        let w = face_window(&face(0.5, 0.5, 0.2), Rect::UNIT, na, 1.0);
        assert!((w.height - 0.5).abs() < 1e-9, "face 0.2 high fills 40 % of a 0.5 window: {w:?}");
        assert!((w.width - 0.5 * na).abs() < 1e-9);
        // the face centre sits slightly above the middle
        let cy = w.y + FACE_Y * w.height;
        assert!((cy - 0.5).abs() < 1e-9);
        // a face at the edge keeps the window inside the frame
        let w = face_window(&face(0.02, 0.02, 0.2), Rect::UNIT, na, 1.0);
        assert!(w.x >= 0.0 && w.y >= 0.0);
        // tighter framing → smaller window, never below the minimum
        let tight = face_window(&face(0.5, 0.5, 0.2), Rect::UNIT, na, 2.0);
        assert!(tight.height < 0.5 && tight.height >= MIN_WINDOW - 1e-9);
    }

    #[test]
    fn tracker_without_a_face_shows_the_cover_crop_and_snaps_on_jumps() {
        let mut t = FaceTracker { detector: None, last_detect_pts: None, face: None, target: None, spring: SpringVec::new(SPRING, [0.0; 4]), initialised: false };
        let w = t.window(0.0, 0.0, true, Rect::UNIT, 1.0, 16.0 / 9.0, 1.0);
        assert!((w.height - 1.0).abs() < 1e-9 && (w.width - 9.0 / 16.0).abs() < 1e-9, "{w:?}");
        // a face appears: the target moves, the spring glides (not there after one 16 ms step)
        t.face = Some((face(0.7, 0.5, 0.2), 16.0));
        let w2 = t.window(16.0, 16.0, false, Rect::UNIT, 1.0, 16.0 / 9.0, 1.0);
        assert!(w2.height < 1.0 && w2.height > 0.5, "gliding: {w2:?}");
        // a jump snaps to the face window
        let w3 = t.window(5000.0, 0.0, true, Rect::UNIT, 1.0, 16.0 / 9.0, 1.0);
        // … but by then the face is stale (seen at 16 ms), so it is the full picture again
        assert!((w3.height - 1.0).abs() < 1e-9, "{w3:?}");
    }

    #[test]
    fn small_moves_stay_in_the_deadzone() {
        let a = Rect { x: 0.2, y: 0.2, width: 0.4, height: 0.4 };
        let b = Rect { x: 0.21, y: 0.21, width: 0.4, height: 0.4 };
        let c = Rect { x: 0.3, y: 0.2, width: 0.4, height: 0.4 };
        assert!(within_deadzone(&a, &b));
        assert!(!within_deadzone(&a, &c));
    }
}
