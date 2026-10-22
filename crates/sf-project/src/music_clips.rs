//! ScreenForge extension: background music as clips on the shared audio lanes.
//!
//! Screen Studio plays the background track looped under the whole video. ScreenForge can
//! place it as clips (`sfMusicClips` in the scene's unknown-field map, ignored by Screen
//! Studio): each plays the track from `file_offset_ms`, looping when the clip is longer than
//! the track, at its own place on the output timeline and on its own lane. Without the key the
//! music is one clip over the whole video, as in Screen Studio.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::new_id;
use crate::v4::Scene;

pub const KEY: &str = "sfMusicClips";
/// Id of the clip that stands for the whole-video music before it is edited.
pub const DEFAULT_ID: &str = "music";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicClip {
    pub id: String,
    /// Position on the output timeline.
    pub output_start_ms: f64,
    pub duration_ms: f64,
    /// Where in the track the clip starts (ms; the track loops past its end).
    #[serde(default)]
    pub file_offset_ms: f64,
    /// Lane on the shared audio timeline.
    #[serde(default)]
    pub layer: u32,
    /// Fade in at the start and out at the end (ms; 0 = cut). An edge that touches another
    /// music clip does not fade, so a plain split does not dip.
    #[serde(default = "default_fade")]
    pub fade_in_ms: f64,
    #[serde(default = "default_fade")]
    pub fade_out_ms: f64,
}

fn default_fade() -> f64 {
    MusicClip::DEFAULT_FADE_MS
}

impl MusicClip {
    pub const MIN_DURATION_MS: f64 = 200.0;
    pub const DEFAULT_FADE_MS: f64 = 1000.0;

    pub fn new(output_start_ms: f64, duration_ms: f64) -> Self {
        MusicClip { id: new_id(), output_start_ms, duration_ms, file_offset_ms: 0.0, layer: 0, fade_in_ms: Self::DEFAULT_FADE_MS, fade_out_ms: Self::DEFAULT_FADE_MS }
    }

    pub fn output_end_ms(&self) -> f64 {
        self.output_start_ms + self.duration_ms
    }

    pub fn contains_output(&self, t: f64) -> bool {
        t >= self.output_start_ms && t < self.output_end_ms()
    }

    /// Position in the track (before looping) heard at output time `t`.
    pub fn file_ms_at(&self, t: f64) -> f64 {
        self.file_offset_ms + (t - self.output_start_ms)
    }

    /// Split at output time `t`; returns the right-hand part (self keeps the left), which
    /// continues the track where the left part stops.
    pub fn split_at_output(&mut self, t: f64) -> Option<MusicClip> {
        if t < self.output_start_ms + Self::MIN_DURATION_MS || t > self.output_end_ms() - Self::MIN_DURATION_MS {
            return None;
        }
        let local = t - self.output_start_ms;
        let mut right = self.clone();
        // the new inner edges fade too once the pieces are apart (touching, they do not)
        right.fade_in_ms = Self::DEFAULT_FADE_MS;
        self.fade_out_ms = Self::DEFAULT_FADE_MS;
        right.id = new_id();
        right.output_start_ms = t;
        right.duration_ms = self.duration_ms - local;
        right.file_offset_ms = self.file_offset_ms + local;
        self.duration_ms = local;
        Some(right)
    }
}

/// Fade lengths (in, out) that apply to each of `clips`: an edge that touches another clip
/// (within 5 ms, on any lane) does not fade, and the fades of a short clip are shortened so
/// they never overlap.
pub fn effective_fades(clips: &[MusicClip]) -> Vec<(f64, f64)> {
    const TOUCH_MS: f64 = 5.0;
    clips
        .iter()
        .map(|c| {
            let joined_before = clips.iter().any(|o| o.id != c.id && (o.output_end_ms() - c.output_start_ms).abs() < TOUCH_MS);
            let joined_after = clips.iter().any(|o| o.id != c.id && (o.output_start_ms - c.output_end_ms()).abs() < TOUCH_MS);
            let fin = if joined_before { 0.0 } else { c.fade_in_ms.max(0.0) };
            let fout = if joined_after { 0.0 } else { c.fade_out_ms.max(0.0) };
            let total = fin + fout;
            if total > c.duration_ms && total > 0.0 {
                let k = c.duration_ms / total;
                (fin * k, fout * k)
            } else {
                (fin, fout)
            }
        })
        .collect()
}

/// Level (0..1) of a clip at output time `t` with fades `(fade_in, fade_out)`; linear ramps.
pub fn fade_gain(c: &MusicClip, (fade_in, fade_out): (f64, f64), t: f64) -> f64 {
    let from_start = t - c.output_start_ms;
    let to_end = c.output_end_ms() - t;
    let a = if fade_in > 0.0 { (from_start / fade_in).clamp(0.0, 1.0) } else { 1.0 };
    let b = if fade_out > 0.0 { (to_end / fade_out).clamp(0.0, 1.0) } else { 1.0 };
    a.min(b)
}

/// Typed access to the extension stored in `Scene::extra`.
pub trait SceneMusicExt {
    /// `None` = the music plays under the whole video (Screen Studio behaviour).
    fn music_clips(&self) -> Option<Vec<MusicClip>>;
    fn set_music_clips(&mut self, clips: Option<Vec<MusicClip>>);
}

impl SceneMusicExt for Scene {
    fn music_clips(&self) -> Option<Vec<MusicClip>> {
        serde_json::from_value(self.extra.get(KEY)?.clone()).ok()
    }

    fn set_music_clips(&mut self, clips: Option<Vec<MusicClip>>) {
        match clips {
            Some(c) => {
                self.extra.insert(KEY.into(), serde_json::to_value(c).unwrap_or(Value::Null));
            }
            None => {
                self.extra.remove(KEY);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_continues_the_track() {
        let mut c = MusicClip::new(1_000.0, 10_000.0);
        c.file_offset_ms = 500.0;
        let r = c.split_at_output(4_000.0).unwrap();
        assert_eq!((c.output_start_ms, c.duration_ms), (1_000.0, 3_000.0));
        assert_eq!((r.output_start_ms, r.duration_ms, r.file_offset_ms), (4_000.0, 7_000.0, 3_500.0));
        assert_eq!(r.file_ms_at(4_000.0), c.file_ms_at(4_000.0));
        assert!(c.split_at_output(1_050.0).is_none());
    }

    #[test]
    fn fades_skip_touching_edges_and_shrink_on_short_clips() {
        let mut a = MusicClip::new(0.0, 10_000.0);
        let b = a.split_at_output(4_000.0).unwrap();
        // a plain split: the pieces touch, so the cut does not fade
        let f = effective_fades(&[a.clone(), b.clone()]);
        assert_eq!(f, vec![(1_000.0, 0.0), (0.0, 1_000.0)]);
        assert_eq!(fade_gain(&a, f[0], 3_999.0), 1.0);
        // pieces apart (the middle went to a voice-over): every edge fades
        let mut b2 = b.clone();
        b2.output_start_ms = 6_000.0;
        let f = effective_fades(&[a.clone(), b2.clone()]);
        assert_eq!(f, vec![(1_000.0, 1_000.0), (1_000.0, 1_000.0)]);
        assert!((fade_gain(&a, f[0], 3_500.0) - 0.5).abs() < 1e-9);
        assert!((fade_gain(&b2, f[1], 6_250.0) - 0.25).abs() < 1e-9);
        // a 1 s clip with 1 s fades each way: halved so they meet in the middle
        let short = MusicClip::new(20_000.0, 1_000.0);
        assert_eq!(effective_fades(&[short]), vec![(500.0, 500.0)]);
    }

    #[test]
    fn old_clips_get_the_default_fades() {
        let c: MusicClip = serde_json::from_value(serde_json::json!({ "id": "m", "outputStartMs": 0.0, "durationMs": 5000.0 })).unwrap();
        assert_eq!((c.fade_in_ms, c.fade_out_ms), (1_000.0, 1_000.0));
    }

    #[test]
    fn clips_round_trip_through_the_scene() {
        let mut s = Scene::recording("Recording", 0);
        assert!(s.music_clips().is_none());
        s.set_music_clips(Some(vec![MusicClip::new(0.0, 1_000.0)]));
        assert_eq!(s.music_clips().unwrap().len(), 1);
        s.set_music_clips(None);
        assert!(!s.extra.contains_key(KEY));
    }
}
