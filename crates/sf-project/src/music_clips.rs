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
}

impl MusicClip {
    pub const MIN_DURATION_MS: f64 = 200.0;

    pub fn new(output_start_ms: f64, duration_ms: f64) -> Self {
        MusicClip { id: new_id(), output_start_ms, duration_ms, file_offset_ms: 0.0, layer: 0 }
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
        right.id = new_id();
        right.output_start_ms = t;
        right.duration_ms = self.duration_ms - local;
        right.file_offset_ms = self.file_offset_ms + local;
        self.duration_ms = local;
        Some(right)
    }
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
    fn clips_round_trip_through_the_scene() {
        let mut s = Scene::recording("Recording", 0);
        assert!(s.music_clips().is_none());
        s.set_music_clips(Some(vec![MusicClip::new(0.0, 1_000.0)]));
        assert_eq!(s.music_clips().unwrap().len(), 1);
        s.set_music_clips(None);
        assert!(!s.extra.contains_key(KEY));
    }
}
