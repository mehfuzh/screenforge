//! Automatic zoom creation, identical to Screen Studio's "Create automatic zooms":
//! every mouse-down opens a range `[t - 300, t + 2500]`, ranges closer than 2.5 s are merged,
//! clicks in the last second are ignored and no zoom ends later than `duration - 800`.
//!
//! ScreenForge extension (`zoom_on_typing`): key-downs open and extend ranges the same way, so a
//! zoom that starts with the click into a text field stays in while you type. Where the app
//! exposes it, the recorded text caret joins the cursor samples (see `camera::click_groups`), so
//! the camera follows the text being typed.

use sf_project::v4::Zoom;

use crate::cursor::CursorTrack;

pub const BEFORE_MS: f64 = 300.0;
pub const AFTER_MS: f64 = 2500.0;
pub const FILL_GAP_MS: f64 = 2500.0;

pub fn generate(track: &CursorTrack, duration_ms: f64, level: f64) -> Vec<Zoom> {
    generate_with(track, duration_ms, level, false)
}

pub fn generate_with(track: &CursorTrack, duration_ms: f64, level: f64, zoom_on_typing: bool) -> Vec<Zoom> {
    let clicks = track.clicks.iter().filter(|c| c.down).map(|c| c.time_ms);
    let keys = track.key_downs.iter().copied().filter(|_| zoom_on_typing);
    let mut ranges: Vec<(f64, f64)> = clicks
        .chain(keys)
        .filter(|t| *t < duration_ms - 1000.0)
        .map(|t| {
            let t = t.floor();
            ((t - BEFORE_MS).max(1.0), (t + AFTER_MS).min(duration_ms - 800.0))
        })
        .filter(|(s, e)| e > s)
        .collect();
    ranges.sort_by(|a, b| a.0.total_cmp(&b.0));
    let merged = merge_ranges(ranges, FILL_GAP_MS);
    merged.into_iter().map(|(s, e)| Zoom::auto(s.floor(), e.floor(), level)).collect()
}

/// Merge overlapping/adjacent ranges and ranges whose gap is at most `fill_gap`.
pub fn merge_ranges(ranges: Vec<(f64, f64)>, fill_gap: f64) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.0 - last.1 <= fill_gap => last.1 = last.1.max(r.1),
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor::ClickSample;

    #[test]
    fn merges_and_clamps() {
        let mut t = CursorTrack::default();
        for ms in [1000.0, 2000.0, 9000.0, 29_800.0] {
            t.clicks.push(ClickSample { time_ms: ms, x: 0.0, y: 0.0, down: true, right_button: false });
        }
        let z = generate(&t, 30_000.0, 2.0);
        assert_eq!(z.len(), 2, "{z:?}");
        assert_eq!(z[0].source_start_ms, 700.0);
        assert_eq!(z[0].source_end_ms, 4500.0);
        assert_eq!(z[1].source_start_ms, 8700.0);
        assert_eq!(z[1].source_end_ms, 11_500.0);
        assert_eq!(z[0].zoom, 2.0);
    }

    #[test]
    fn typing_extends_and_opens_zooms() {
        let mut t = CursorTrack::default();
        t.clicks.push(ClickSample { time_ms: 1000.0, x: 0.0, y: 0.0, down: true, right_button: false });
        // typing burst 2–8 s keeps the click zoom open; a lone burst at 20 s opens its own
        t.key_downs = vec![2000.0, 3000.0, 4500.0, 6000.0, 8000.0, 20_000.0, 20_500.0];
        let without = generate_with(&t, 30_000.0, 2.0, false);
        assert_eq!(without.len(), 1);
        assert_eq!(without[0].source_end_ms, 3500.0);
        let with = generate_with(&t, 30_000.0, 2.0, true);
        assert_eq!(with.len(), 2, "{with:?}");
        assert_eq!(with[0].source_start_ms, 700.0);
        assert_eq!(with[0].source_end_ms, 10_500.0);
        assert_eq!(with[1].source_start_ms, 19_700.0);
        assert_eq!(with[1].source_end_ms, 23_000.0);
    }
}
