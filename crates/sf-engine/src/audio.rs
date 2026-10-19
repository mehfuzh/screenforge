//! Audio timeline: which part of which recorded track plays at an output time.
//!
//! Linked mode (default, like Screen Studio): audio follows the video slices. Detached mode
//! (ScreenForge extension `sfAudioClips`): clips are placed independently.

use sf_project::audio_clips::{AudioTrack, SceneAudioExt};
use sf_project::v4::Scene;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioSpan {
    pub track: AudioTrack,
    /// Clip id in detached mode, slice id in linked mode.
    pub id: String,
    pub output_start_ms: f64,
    pub output_end_ms: f64,
    pub source_start_ms: f64,
    pub source_end_ms: f64,
    /// Playback speed (source ms per output ms).
    pub speed: f64,
    pub volume: f64,
    pub muted: bool,
    /// Timeline lane (detached mode); linked audio is always on lane 0.
    pub layer: u32,
}

impl AudioSpan {
    pub fn contains_output(&self, t: f64) -> bool {
        t >= self.output_start_ms && t < self.output_end_ms
    }
    pub fn output_to_source(&self, t: f64) -> f64 {
        self.source_start_ms + (t - self.output_start_ms) * self.speed
    }
    pub fn output_duration_ms(&self) -> f64 {
        self.output_end_ms - self.output_start_ms
    }
}

/// Resolve the audio spans of a scene for the tracks that exist in the recording.
/// `base_volume` is `config.audio.volume` for the microphone (system audio uses 1).
pub fn audio_spans(scene: &Scene, tracks: &[AudioTrack], mic_volume: f64, mute_mic: bool, mute_system: bool) -> Vec<AudioSpan> {
    let track_muted = |t: &AudioTrack| match t {
        AudioTrack::Microphone => mute_mic,
        AudioTrack::SystemAudio => mute_system,
    };
    let base = |t: &AudioTrack| match t {
        AudioTrack::Microphone => mic_volume,
        AudioTrack::SystemAudio => 1.0,
    };
    if let Some(clips) = scene.audio_clips() {
        return clips
            .into_iter()
            .filter(|c| tracks.contains(&c.track))
            .map(|c| AudioSpan {
                output_start_ms: c.output_start_ms,
                output_end_ms: c.output_end_ms(),
                source_start_ms: c.source_start_ms,
                source_end_ms: c.source_end_ms,
                speed: c.speed,
                volume: c.volume * base(&c.track),
                muted: c.muted || track_muted(&c.track),
                layer: c.layer,
                id: c.id,
                track: c.track,
            })
            .collect();
    }
    let mut out = vec![];
    let mut cursor = 0.0;
    for s in &scene.slices {
        let ts = if s.time_scale > 0.0 { s.time_scale } else { 1.0 };
        let out_len = s.source_duration_ms() * ts;
        for t in tracks {
            let vol = match t {
                AudioTrack::Microphone => s.volume,
                AudioTrack::SystemAudio => s.system_audio_volume,
            };
            out.push(AudioSpan {
                track: t.clone(),
                id: s.id.clone(),
                output_start_ms: cursor,
                output_end_ms: cursor + out_len,
                source_start_ms: s.source_start_ms,
                source_end_ms: s.source_end_ms,
                speed: 1.0 / ts,
                volume: vol * base(t),
                muted: track_muted(t),
                layer: 0,
            });
        }
        cursor += out_len;
    }
    out
}
