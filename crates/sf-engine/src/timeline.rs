//! Output time ↔ source time mapping.
//!
//! A scene's `slices` are laid end-to-end on the output timeline in array order. Each slice
//! covers `[sourceStartMs, sourceEndMs)` of the recording and plays for
//! `(sourceEnd - sourceStart) * timeScale` ms of output (timeScale 0.5 = 2× speed).
//! Source ranges not covered by any slice are cut.

use sf_project::v4::{Scene, Slice};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceSpan {
    pub index: usize,
    pub output_start_ms: f64,
    pub output_end_ms: f64,
    pub source_start_ms: f64,
    pub source_end_ms: f64,
    pub time_scale: f64,
}

impl SliceSpan {
    pub fn output_duration_ms(&self) -> f64 {
        self.output_end_ms - self.output_start_ms
    }
    pub fn source_duration_ms(&self) -> f64 {
        self.source_end_ms - self.source_start_ms
    }
    pub fn contains_output(&self, t: f64) -> bool {
        t >= self.output_start_ms && t < self.output_end_ms
    }
    pub fn contains_source(&self, t: f64) -> bool {
        t >= self.source_start_ms && t < self.source_end_ms
    }
    pub fn output_to_source(&self, t: f64) -> f64 {
        self.source_start_ms + (t - self.output_start_ms) / self.time_scale
    }
    pub fn source_to_output(&self, t: f64) -> f64 {
        self.output_start_ms + (t - self.source_start_ms) * self.time_scale
    }
}

#[derive(Debug, Clone, Default)]
pub struct Timeline {
    spans: Vec<SliceSpan>,
}

impl Timeline {
    pub fn from_slices(slices: &[Slice]) -> Self {
        let mut spans = Vec::with_capacity(slices.len());
        let mut cursor = 0.0;
        for (index, s) in slices.iter().enumerate() {
            let ts = if s.time_scale > 0.0 { s.time_scale } else { 1.0 };
            let out = s.source_duration_ms() * ts;
            spans.push(SliceSpan {
                index,
                output_start_ms: cursor,
                output_end_ms: cursor + out,
                source_start_ms: s.source_start_ms,
                source_end_ms: s.source_end_ms,
                time_scale: ts,
            });
            cursor += out;
        }
        Timeline { spans }
    }

    pub fn from_scene(scene: &Scene) -> Self {
        Self::from_slices(&scene.slices)
    }

    pub fn spans(&self) -> &[SliceSpan] {
        &self.spans
    }

    pub fn duration_ms(&self) -> f64 {
        self.spans.last().map(|s| s.output_end_ms).unwrap_or(0.0)
    }

    /// Slice span containing output time `t` (clamped to the last span at the end).
    pub fn span_at_output(&self, t: f64) -> Option<&SliceSpan> {
        if self.spans.is_empty() {
            return None;
        }
        let t = t.max(0.0);
        let idx = self.spans.partition_point(|s| s.output_end_ms <= t);
        Some(&self.spans[idx.min(self.spans.len() - 1)])
    }

    /// Source time for output time `t`.
    pub fn output_to_source(&self, t: f64) -> Option<f64> {
        let span = self.span_at_output(t)?;
        Some(span.output_to_source(t.clamp(span.output_start_ms, span.output_end_ms)))
    }

    /// Output time for a source time, or `None` if that part of the source is cut.
    pub fn source_to_output(&self, t: f64) -> Option<f64> {
        self.spans.iter().find(|s| s.contains_source(t)).map(|s| s.source_to_output(t))
    }

    /// Playback speed at output time `t` (1 / timeScale).
    pub fn speed_at_output(&self, t: f64) -> f64 {
        self.span_at_output(t).map(|s| 1.0 / s.time_scale).unwrap_or(1.0)
    }

    /// Is `t_src` in a slice boundary region, i.e. is the previous source ms cut? Used to
    /// reset springs across cuts so the camera does not fly across a jump.
    pub fn is_cut_at_output(&self, t: f64, epsilon_ms: f64) -> bool {
        let Some(span) = self.span_at_output(t) else { return false };
        if span.index == 0 {
            return false;
        }
        let prev = &self.spans[span.index - 1];
        (t - span.output_start_ms) < epsilon_ms && (span.source_start_ms - prev.source_end_ms).abs() > 1.0
    }
}

/// Editing operations on the slice list; each returns a new Vec so callers can push undo state.
pub mod edit {
    use sf_project::v4::Slice;

    /// Split the slice containing `output_ms` into two at that point.
    pub fn split_at_output(slices: &[Slice], output_ms: f64) -> Vec<Slice> {
        let tl = super::Timeline::from_slices(slices);
        let Some(span) = tl.span_at_output(output_ms) else { return slices.to_vec() };
        let src = span.output_to_source(output_ms);
        if src <= span.source_start_ms + 1.0 || src >= span.source_end_ms - 1.0 {
            return slices.to_vec();
        }
        let mut out = slices.to_vec();
        let orig = out[span.index].clone();
        let mut left = orig.clone();
        left.source_end_ms = src;
        let mut right = orig;
        right.id = sf_project::ids::new_id();
        right.source_start_ms = src;
        out.splice(span.index..=span.index, [left, right]);
        out
    }

    /// Remove slice `index` (a cut: the gap stays removed from the timeline).
    pub fn delete(slices: &[Slice], index: usize) -> Vec<Slice> {
        let mut out = slices.to_vec();
        if index < out.len() {
            out.remove(index);
        }
        out
    }

    pub fn set_speed(slices: &[Slice], index: usize, speed: f64) -> Vec<Slice> {
        let mut out = slices.to_vec();
        if let Some(s) = out.get_mut(index) {
            s.time_scale = 1.0 / speed.max(0.01);
        }
        out
    }

    /// Trim the slice's source range (ms).
    pub fn trim(slices: &[Slice], index: usize, source_start_ms: f64, source_end_ms: f64) -> Vec<Slice> {
        let mut out = slices.to_vec();
        if let Some(s) = out.get_mut(index) {
            s.source_start_ms = source_start_ms;
            s.source_end_ms = source_end_ms.max(source_start_ms + 1.0);
        }
        out
    }

    /// Join slice `index` with the next one if they are source-contiguous.
    pub fn merge_with_next(slices: &[Slice], index: usize) -> Vec<Slice> {
        let mut out = slices.to_vec();
        if index + 1 < out.len() && (out[index].source_end_ms - out[index + 1].source_start_ms).abs() < 1.0 {
            let next = out.remove(index + 1);
            out[index].source_end_ms = next.source_end_ms;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slices() -> Vec<Slice> {
        let mut a = Slice::new(0.0, 1000.0);
        a.time_scale = 1.0;
        let mut b = Slice::new(2000.0, 3000.0); // 1000..2000 is cut
        b.time_scale = 0.5; // 2x speed → 500ms output
        vec![a, b]
    }

    #[test]
    fn maps_time() {
        let tl = Timeline::from_slices(&slices());
        assert_eq!(tl.duration_ms(), 1500.0);
        assert_eq!(tl.output_to_source(500.0), Some(500.0));
        assert_eq!(tl.output_to_source(1250.0), Some(2500.0));
        assert_eq!(tl.source_to_output(1500.0), None);
        assert_eq!(tl.source_to_output(2500.0), Some(1250.0));
        assert_eq!(tl.speed_at_output(1200.0), 2.0);
        assert!(tl.is_cut_at_output(1000.5, 5.0));
        assert!(!tl.is_cut_at_output(500.0, 5.0));
    }

    #[test]
    fn split_and_merge() {
        let s = edit::split_at_output(&slices(), 400.0);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].source_end_ms, 400.0);
        assert_eq!(s[1].source_start_ms, 400.0);
        let m = edit::merge_with_next(&s, 0);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].source_end_ms, 1000.0);
    }
}
