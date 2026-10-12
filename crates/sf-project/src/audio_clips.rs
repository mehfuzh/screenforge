//! ScreenForge extension: audio clips that are edited independently from the video slices.
//!
//! Screen Studio ties audio to the video slices (cuts and speed apply to both). ScreenForge
//! can *detach* the audio: the scene then carries `sfAudioClips`, a list of clips placed on the
//! output timeline with their own source ranges, volume and mute. The key lives in the scene's
//! unknown-field map, so Screen Studio ignores it (and would drop it on its own save).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::new_id;
use crate::v4::Scene;

pub const KEY: &str = "sfAudioClips";

/// Which recorded track a clip plays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioTrack {
    Microphone,
    SystemAudio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioClip {
    pub id: String,
    pub track: AudioTrack,
    /// Position on the output timeline.
    pub output_start_ms: f64,
    /// Source (recording) range played by this clip.
    pub source_start_ms: f64,
    pub source_end_ms: f64,
    /// Playback speed (2 = twice as fast). Inherited from the video slice when detached.
    #[serde(default = "one")]
    pub speed: f64,
    #[serde(default = "one")]
    pub volume: f64,
    #[serde(default)]
    pub muted: bool,
}

fn one() -> f64 {
    1.0
}

impl AudioClip {
    pub const MIN_DURATION_MS: f64 = 50.0;

    pub fn new(track: AudioTrack, output_start_ms: f64, source_start_ms: f64, source_end_ms: f64, speed: f64) -> Self {
        AudioClip { id: new_id(), track, output_start_ms, source_start_ms, source_end_ms, speed: speed.max(0.01), volume: 1.0, muted: false }
    }

    pub fn source_duration_ms(&self) -> f64 {
        (self.source_end_ms - self.source_start_ms).max(0.0)
    }

    pub fn output_duration_ms(&self) -> f64 {
        self.source_duration_ms() / self.speed
    }

    pub fn output_end_ms(&self) -> f64 {
        self.output_start_ms + self.output_duration_ms()
    }

    pub fn contains_output(&self, t: f64) -> bool {
        t >= self.output_start_ms && t < self.output_end_ms()
    }

    pub fn output_to_source(&self, t: f64) -> f64 {
        self.source_start_ms + (t - self.output_start_ms) * self.speed
    }

    /// Split at output time `t`; returns the right-hand part (self keeps the left).
    pub fn split_at_output(&mut self, t: f64) -> Option<AudioClip> {
        if t <= self.output_start_ms + Self::MIN_DURATION_MS || t >= self.output_end_ms() - Self::MIN_DURATION_MS {
            return None;
        }
        let src = self.output_to_source(t);
        let mut right = self.clone();
        right.id = new_id();
        right.output_start_ms = t;
        right.source_start_ms = src;
        self.source_end_ms = src;
        Some(right)
    }
}

/// Typed access to the extension stored in `Scene::extra`.
pub trait SceneAudioExt {
    /// `None` = audio linked to the video slices (Screen Studio behaviour).
    fn audio_clips(&self) -> Option<Vec<AudioClip>>;
    fn set_audio_clips(&mut self, clips: Option<Vec<AudioClip>>);
    fn is_audio_detached(&self) -> bool {
        self.audio_clips().is_some()
    }
}

impl SceneAudioExt for Scene {
    fn audio_clips(&self) -> Option<Vec<AudioClip>> {
        let v = self.extra.get(KEY)?;
        serde_json::from_value(v.clone()).ok()
    }

    fn set_audio_clips(&mut self, clips: Option<Vec<AudioClip>>) {
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

/// Build detached clips that reproduce the current linked behaviour (one clip per slice
/// and per available track), so detaching is visually a no-op until the user edits.
pub fn clips_from_slices(scene: &Scene, tracks: &[AudioTrack]) -> Vec<AudioClip> {
    let mut out = vec![];
    let mut cursor = 0.0;
    for s in &scene.slices {
        let ts = if s.time_scale > 0.0 { s.time_scale } else { 1.0 };
        for t in tracks {
            let vol = match t {
                AudioTrack::Microphone => s.volume,
                AudioTrack::SystemAudio => s.system_audio_volume,
            };
            let mut c = AudioClip::new(t.clone(), cursor, s.source_start_ms, s.source_end_ms, 1.0 / ts);
            c.volume = vol;
            out.push(c);
        }
        cursor += s.source_duration_ms() * ts;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v4::Slice;

    #[test]
    fn detach_and_split() {
        let mut scene = Scene::recording("x", 0);
        let mut a = Slice::new(0.0, 1000.0);
        a.time_scale = 0.5;
        scene.slices = vec![a, Slice::new(2000.0, 3000.0)];
        let clips = clips_from_slices(&scene, &[AudioTrack::Microphone]);
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].output_duration_ms(), 500.0);
        assert_eq!(clips[1].output_start_ms, 500.0);
        scene.set_audio_clips(Some(clips));
        let mut clips = scene.audio_clips().unwrap();
        let right = clips[1].split_at_output(1000.0).unwrap();
        assert_eq!(clips[1].source_end_ms, 2500.0);
        assert_eq!(right.source_start_ms, 2500.0);
        assert_eq!(right.output_start_ms, 1000.0);
        // round trip through JSON keeps the extension
        let json = serde_json::to_value(&scene).unwrap();
        assert!(json.get(KEY).is_some());
    }
}
