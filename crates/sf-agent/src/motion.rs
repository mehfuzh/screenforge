//! What the picture does over time, for recordings with no mouse or keyboard log (imported
//! videos, iPhone recordings) and as a second opinion on the others: grey frames (about 640
//! pixels wide) are compared with the frame one second later, so a blinking text cursor looks
//! still while typed text adds up, and the recording is split into
//!
//!  * `still`: nothing changes (idle, reading, a paused screen);
//!  * `small`: a few pixels change in one area, like text being typed or a spinner;
//!  * `large`: much of the screen changes (scrolling, switching windows, video playing).
//!
//! It is a cheap measure, not an understanding: `view_frames` lets the agent look.

/// A pixel changed when its grey level moved by more than this.
const PIXEL_DELTA: u8 = 14;
/// Below this share of changed pixels the frame is still: about 25 pixels of a 640×360 frame,
/// above a blinking caret and compression noise, below a few typed characters.
const STILL_SHARE: f64 = 0.00011;
/// Frames are compared with the one this long after them.
const LAG_MS: f64 = 1000.0;
/// Changes inside a box smaller than this share of the frame are small (typing).
const SMALL_AREA: f64 = 0.12;
/// Sections shorter than this are folded into their neighbours.
const MIN_SECTION_MS: f64 = 1000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Still,
    Small,
    Large,
}

impl Motion {
    pub fn label(self) -> &'static str {
        match self {
            Motion::Still => "still",
            Motion::Small => "small",
            Motion::Large => "large",
        }
    }
}

/// How frame `b` differs from frame `a` (same size, `w` pixels wide, one grey byte each).
fn classify(a: &[u8], b: &[u8], w: usize) -> Motion {
    let h = a.len() / w.max(1);
    let (mut n, mut x0, mut y0, mut x1, mut y1) = (0usize, usize::MAX, usize::MAX, 0usize, 0usize);
    for (i, (pa, pb)) in a.iter().zip(b).enumerate() {
        if pa.abs_diff(*pb) > PIXEL_DELTA {
            n += 1;
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    let total = (w * h).max(1) as f64;
    if (n as f64) / total < STILL_SHARE {
        return Motion::Still;
    }
    let area = ((x1 - x0 + 1) * (y1 - y0 + 1)) as f64 / total;
    if area < SMALL_AREA {
        Motion::Small
    } else {
        Motion::Large
    }
}

/// Sections `(start ms, end ms, motion)` of grey frames `w` pixels wide taken `fps` times a
/// second from time 0. Frames are read one at a time, so a long video never sits in memory.
pub fn sections(frames: impl IntoIterator<Item = Vec<u8>>, w: usize, fps: f64) -> Vec<(f64, f64, Motion)> {
    let step = 1000.0 / fps.max(0.1);
    let lag = ((LAG_MS / step).round() as usize).max(1);
    let mut window: std::collections::VecDeque<Vec<u8>> = std::collections::VecDeque::with_capacity(lag + 1);
    let mut runs: Vec<(f64, f64, Motion)> = vec![];
    let mut i = 0usize;
    for f in frames {
        window.push_back(f);
        if window.len() <= lag {
            continue;
        }
        let m = classify(&window[0], &window[lag], w);
        window.pop_front();
        // comparison i spans frames i…i+lag: credit it to the step in the middle
        let mid = i as f64 + lag as f64 / 2.0;
        let (t0, t1) = (((mid - 0.5) * step).max(0.0), (mid + 0.5) * step);
        i += 1;
        match runs.last_mut() {
            Some(last) if last.2 == m => last.1 = t1,
            _ => runs.push((runs.last().map_or(0.0, |l| l.1).min(t0), t1, m)),
        }
    }
    if let Some(last) = runs.last_mut() {
        last.1 += (lag as f64 / 2.0) * step;
    }
    // fold short runs into the run before them (or after, at the start), then join equals
    let mut out: Vec<(f64, f64, Motion)> = vec![];
    for r in runs {
        match out.last_mut() {
            Some(last) if r.1 - r.0 < MIN_SECTION_MS || last.2 == r.2 => last.1 = r.1,
            _ => out.push(r),
        }
    }
    if out.len() > 1 && out[0].1 - out[0].0 < MIN_SECTION_MS {
        let first = out.remove(0);
        out[0].0 = first.0;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 40;
    const H: usize = 20;

    fn frame(fill: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        (0..W * H).map(|i| fill(i % W, i / W)).collect()
    }

    #[test]
    fn still_small_and_large_changes() {
        let blank = frame(|_, _| 20);
        assert_eq!(classify(&blank, &blank, W), Motion::Still);
        // a few characters appear in one corner
        let typed = frame(|x, y| if x < 3 && y < 2 { 230 } else { 20 });
        assert_eq!(classify(&blank, &typed, W), Motion::Small);
        // the whole screen changes
        let scrolled = frame(|x, _| (x * 6) as u8);
        assert_eq!(classify(&blank, &scrolled, W), Motion::Large);
    }

    #[test]
    fn sections_fold_flicker_into_its_neighbours() {
        let blank = frame(|_, _| 20);
        // 4 fps: 3 s still, then 3 s of typing (one more character every frame), 3 s still
        let mut frames = vec![blank.clone(); 12];
        for k in 1..=12 {
            frames.push(frame(|x, y| if y == 1 && x < k { 230 } else { 20 }));
        }
        let typed_all = frames.last().unwrap().clone();
        frames.extend(std::iter::repeat_n(typed_all, 12));
        let s = sections(frames, W, 4.0);
        let kinds: Vec<Motion> = s.iter().map(|r| r.2).collect();
        assert_eq!(kinds, vec![Motion::Still, Motion::Small, Motion::Still]);
        // typing from 3 s to 6 s; comparing frames 1 s apart blurs the edges by half a second
        assert!((2000.0..=3500.0).contains(&s[1].0) && (5500.0..=7000.0).contains(&s[1].1), "{s:?}");
        assert!((s.last().unwrap().1 - 9000.0).abs() <= 250.0, "covers the whole video: {s:?}");
    }
}
