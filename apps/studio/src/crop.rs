//! Crop editor: while it is open the preview shows the whole recording and a crop frame with
//! eight handles over it. Drag inside the frame to move it, drag a handle to resize; ⏎ applies,
//! ⎋ cancels. The result is `config.crop.rect01`, which the renderer already honours.

use egui::{pos2, vec2, CornerRadius, Pos2, Rect, Sense, Stroke};
use sf_project::schema::common::Rect as Rect01;

use crate::theme;

/// Smallest crop, as a fraction of the recording's width / height.
pub const MIN01: f64 = 0.05;
const HANDLE: f32 = 9.0;
/// Pointer distance at which a handle or an edge is grabbed.
const GRAB: f32 = 10.0;

/// Part of the frame being dragged: -1 / 0 / 1 per axis for the left/top edge, none, or the
/// right/bottom edge; `(0, 0)` moves the whole frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle(pub i8, pub i8);

pub enum Action {
    Apply,
    Cancel,
}

pub struct CropEditor {
    pub draft: Rect01,
    grab: Option<(Handle, Rect01, Pos2)>,
}

impl CropEditor {
    pub fn new(current: Rect01) -> Self {
        CropEditor { draft: clamp(current), grab: None }
    }

    /// Paint the frame over `img` (the preview image of the whole recording) and handle drags
    /// and keys. Called after the image was put.
    pub fn ui(&mut self, ui: &mut egui::Ui, img: Rect) -> Option<Action> {
        let pal = theme::p();
        let resp = ui.interact(img.expand(GRAB), ui.id().with("crop_frame"), Sense::drag());
        let frame = to_screen(self.draft, img);
        let hovered = resp.hover_pos().and_then(|p| hit(frame, p));
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                self.grab = hit(frame, p).map(|h| (h, self.draft, p));
            }
        }
        if let (Some((h, start, from)), Some(p)) = (self.grab, resp.interact_pointer_pos()) {
            let d = p - from;
            self.draft = drag(h, start, [(d.x / img.width()) as f64, (d.y / img.height()) as f64]);
        }
        if resp.drag_stopped() {
            self.grab = None;
        }
        if let Some(h) = self.grab.map(|g| g.0).or(hovered) {
            ui.ctx().set_cursor_icon(cursor_for(h));
        }

        let frame = to_screen(self.draft, img);
        let p = ui.painter_at(img);
        // dim everything outside the frame
        let shade = pal.bg.gamma_multiply(0.72);
        for r in [
            Rect::from_min_max(img.min, pos2(img.max.x, frame.min.y)),
            Rect::from_min_max(pos2(img.min.x, frame.max.y), img.max),
            Rect::from_min_max(pos2(img.min.x, frame.min.y), pos2(frame.min.x, frame.max.y)),
            Rect::from_min_max(pos2(frame.max.x, frame.min.y), pos2(img.max.x, frame.max.y)),
        ] {
            p.rect_filled(r, CornerRadius::ZERO, shade);
        }
        // rule-of-thirds guides while dragging
        if self.grab.is_some() {
            let guide = Stroke::new(1.0, pal.text.gamma_multiply(0.35));
            for k in [1.0 / 3.0, 2.0 / 3.0] {
                let x = frame.left() + frame.width() * k;
                let y = frame.top() + frame.height() * k;
                p.line_segment([pos2(x, frame.top()), pos2(x, frame.bottom())], guide);
                p.line_segment([pos2(frame.left(), y), pos2(frame.right(), y)], guide);
            }
        }
        p.rect_stroke(frame, CornerRadius::ZERO, Stroke::new(1.5, pal.accent), egui::StrokeKind::Middle);
        let p = ui.painter();
        for (hx, hy) in HANDLES {
            let c = pos2(lerp(frame.left(), frame.right(), hx), lerp(frame.top(), frame.bottom(), hy));
            p.rect(Rect::from_center_size(c, vec2(HANDLE, HANDLE)), CornerRadius::same(2), pal.text, Stroke::new(1.0, pal.accent), egui::StrokeKind::Middle);
        }
        // size readout under the frame
        let label = format!("{:.0} % × {:.0} %", self.draft.width * 100.0, self.draft.height * 100.0);
        p.text(pos2(frame.center().x, (frame.bottom() + 8.0).min(img.bottom() - 18.0)), egui::Align2::CENTER_TOP, label, egui::FontId::proportional(12.0), pal.text);

        ui.input(|i| {
            if i.key_pressed(egui::Key::Enter) {
                Some(Action::Apply)
            } else if i.key_pressed(egui::Key::Escape) {
                Some(Action::Cancel)
            } else {
                None
            }
        })
    }
}

/// Handle positions as fractions of the frame (corners and edge midpoints).
const HANDLES: [(f32, f32); 8] = [(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (1.0, 0.5), (1.0, 1.0), (0.5, 1.0), (0.0, 1.0), (0.0, 0.5)];

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn to_screen(r: Rect01, img: Rect) -> Rect {
    let x = |v: f64| img.left() + v as f32 * img.width();
    let y = |v: f64| img.top() + v as f32 * img.height();
    Rect::from_min_max(pos2(x(r.x), y(r.y)), pos2(x(r.x + r.width), y(r.y + r.height)))
}

/// What a press at `p` grabs: a handle or edge first, else the inside of the frame.
fn hit(frame: Rect, p: Pos2) -> Option<Handle> {
    let axis = |v: f32, lo: f32, hi: f32| -> Option<i8> {
        if (v - lo).abs() <= GRAB {
            Some(-1)
        } else if (v - hi).abs() <= GRAB {
            Some(1)
        } else if v > lo && v < hi {
            Some(0)
        } else {
            None
        }
    };
    let (hx, hy) = (axis(p.x, frame.left(), frame.right())?, axis(p.y, frame.top(), frame.bottom())?);
    Some(Handle(hx, hy))
}

fn cursor_for(h: Handle) -> egui::CursorIcon {
    use egui::CursorIcon::*;
    match (h.0, h.1) {
        (0, 0) => Grab,
        (-1, -1) | (1, 1) => ResizeNwSe,
        (1, -1) | (-1, 1) => ResizeNeSw,
        (_, 0) => ResizeHorizontal,
        _ => ResizeVertical,
    }
}

/// Keep a crop inside the recording and at least [`MIN01`] in each direction.
pub fn clamp(r: Rect01) -> Rect01 {
    let width = r.width.clamp(MIN01, 1.0);
    let height = r.height.clamp(MIN01, 1.0);
    Rect01 { x: r.x.clamp(0.0, 1.0 - width), y: r.y.clamp(0.0, 1.0 - height), width, height }
}

/// The crop after dragging `h` by `d` (fractions of the recording) from `start`. Moving keeps
/// the size; resizing moves only the grabbed edges and stops at the minimum size and the
/// recording's edges.
pub fn drag(h: Handle, start: Rect01, d: [f64; 2]) -> Rect01 {
    if h == Handle(0, 0) {
        return clamp(Rect01 { x: start.x + d[0], y: start.y + d[1], ..start });
    }
    let edge = |lo: f64, size: f64, side: i8, dv: f64| -> (f64, f64) {
        let hi = lo + size;
        match side {
            -1 => {
                let lo = (lo + dv).clamp(0.0, hi - MIN01);
                (lo, hi - lo)
            }
            1 => (lo, ((hi + dv).clamp(lo + MIN01, 1.0)) - lo),
            _ => (lo, size),
        }
    };
    let (x, width) = edge(start.x, start.width, h.0, d[0]);
    let (y, height) = edge(start.y, start.height, h.1, d[1]);
    Rect01 { x, y, width, height }
}

/// Whether a crop is (nearly) the whole recording.
pub fn is_full(r: Rect01) -> bool {
    r.x.abs() < 1e-4 && r.y.abs() < 1e-4 && (r.width - 1.0).abs() < 1e-4 && (r.height - 1.0).abs() < 1e-4
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect01 {
        Rect01 { x, y, width: w, height: h }
    }

    fn close(a: Rect01, b: Rect01) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9 && (a.width - b.width).abs() < 1e-9 && (a.height - b.height).abs() < 1e-9
    }

    #[test]
    fn moving_keeps_size_and_stays_inside() {
        let s = r(0.2, 0.2, 0.5, 0.5);
        assert!(close(drag(Handle(0, 0), s, [0.1, -0.1]), r(0.3, 0.1, 0.5, 0.5)));
        assert!(close(drag(Handle(0, 0), s, [0.9, 0.9]), r(0.5, 0.5, 0.5, 0.5)));
    }

    #[test]
    fn resizing_moves_only_the_grabbed_edges() {
        let s = r(0.2, 0.2, 0.5, 0.5);
        // bottom-right corner
        assert!(close(drag(Handle(1, 1), s, [0.1, 0.2]), r(0.2, 0.2, 0.6, 0.7)));
        // left edge: right edge stays at 0.7
        assert!(close(drag(Handle(-1, 0), s, [-0.1, 0.3]), r(0.1, 0.2, 0.6, 0.5)));
        // stops at the recording's edge and at the minimum size
        assert!(close(drag(Handle(1, 0), s, [0.9, 0.0]), r(0.2, 0.2, 0.8, 0.5)));
        assert!(close(drag(Handle(0, -1), s, [0.0, 0.9]), r(0.2, 0.7 - MIN01, 0.5, MIN01)));
    }

    #[test]
    fn hit_prefers_edges_then_inside() {
        let f = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));
        assert_eq!(hit(f, pos2(101.0, 199.0)), Some(Handle(-1, 1)));
        assert_eq!(hit(f, pos2(200.0, 102.0)), Some(Handle(0, -1)));
        assert_eq!(hit(f, pos2(200.0, 150.0)), Some(Handle(0, 0)));
        assert_eq!(hit(f, pos2(50.0, 150.0)), None);
    }

    #[test]
    fn clamp_and_full() {
        assert!(close(clamp(r(0.9, -0.2, 0.5, 2.0)), r(0.5, 0.0, 0.5, 1.0)));
        assert!(is_full(Rect01::UNIT));
        assert!(!is_full(r(0.0, 0.0, 0.9, 1.0)));
    }
}
