//! Audio timeline: which part of which recorded track plays at an output time.
//!
//! Linked mode (default, like Screen Studio): audio follows the video slices. Detached mode
//! (ScreenForge extension `sfAudioClips`): clips are placed independently.

use sf_project::audio_clips::{AudioTrack, SceneAudioExt};
use sf_project::v4::{AudioConfig, Scene};

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
    /// Boost in dB (0 = none), applied after the microphone's audio mode.
    pub boost_db: f64,
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
pub fn audio_spans(scene: &Scene, tracks: &[AudioTrack], audio: &AudioConfig) -> Vec<AudioSpan> {
    let (mic_volume, mute_mic, mute_system) = (audio.volume, audio.mute_microphone, audio.mute_system_audio);
    // the whole-microphone boost of older projects, for linked audio and clips without their own
    let track_boost = |t: &AudioTrack| match t {
        AudioTrack::Microphone => audio.microphone_boost_db(),
        AudioTrack::SystemAudio => 0.0,
    };
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
                boost_db: c.boost_db.filter(|b| b.is_finite()).map_or_else(|| track_boost(&c.track), |b| b.clamp(0.0, AudioConfig::MAX_MICROPHONE_BOOST_DB)),
                id: c.id,
                track: c.track,
            })
            .collect();
    }
    let mut out = vec![];
    let mut cursor = 0.0;
    for s in &scene.slices {
        let ts = if s.time_scale > 0.0 { s.time_scale } else { 1.0 };
        let out_len = s.output_duration_ms();
        // a title card plays no recorded audio
        for t in tracks.iter().filter(|_| !s.is_card()) {
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
                boost_db: track_boost(t),
            });
        }
        cursor += out_len;
    }
    out
}

/// A voice-over placed on the output timeline. Voice-overs are anchored to a source time (the
/// frame they start on), so they move with the cuts before them, and play at normal speed
/// whatever the speed of the clips underneath.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceOverSpan {
    pub id: String,
    pub file_name: String,
    pub output_start_ms: f64,
    pub output_end_ms: f64,
    /// Where in the file playback starts (ms).
    pub file_offset_ms: f64,
    pub volume: f64,
    pub muted: bool,
    /// Lane on the shared audio timeline.
    pub layer: u32,
    /// The take's boost in dB (0 = none).
    pub boost_db: f64,
}

impl VoiceOverSpan {
    pub fn contains_output(&self, t: f64) -> bool {
        t >= self.output_start_ms && t < self.output_end_ms
    }
    pub fn output_duration_ms(&self) -> f64 {
        self.output_end_ms - self.output_start_ms
    }
    /// Position in the audio file (ms) heard at output time `t`.
    pub fn file_ms_at(&self, t: f64) -> f64 {
        self.file_offset_ms + (t - self.output_start_ms)
    }
}

/// Voice-overs of a scene on the output timeline, in start order. One whose start frame was
/// cut away is left out (like a zoom over a cut part); the end is clipped to the video.
pub fn voice_over_spans(scene: &Scene, timeline: &crate::timeline::Timeline) -> Vec<VoiceOverSpan> {
    let end = timeline.duration_ms();
    let mut out: Vec<VoiceOverSpan> = scene
        .voice_overs
        .iter()
        .filter(|v| !v.file_name.is_empty() && v.voiceover_duration_ms > 0.0)
        .filter_map(|v| {
            let start = timeline.source_to_output(v.source_start_ms)?;
            let ts = if v.time_scale > 0.0 { v.time_scale } else { 1.0 };
            Some(VoiceOverSpan {
                id: v.id.clone(),
                file_name: v.file_name.clone(),
                output_start_ms: start,
                output_end_ms: (start + v.voiceover_duration_ms * ts).min(end),
                file_offset_ms: v.file_offset_ms.max(0.0),
                volume: v.volume,
                muted: v.is_disabled,
                layer: v.layer,
                boost_db: v.boost(),
            })
        })
        .filter(|s| s.output_duration_ms() > 0.0)
        .collect();
    out.sort_by(|a, b| a.output_start_ms.total_cmp(&b.output_start_ms));
    out
}

/// Shortest piece a voice-over can be split or trimmed to (ms).
pub const MIN_VOICE_OVER_MS: f64 = 200.0;

/// Cut voice-over `id` at output time `output_ms` into two takes playing the same file: the
/// right-hand part starts on the frame under the cut. Returns the new part's id, or `None` when
/// the cut is not inside the voice-over (or would leave a part shorter than the minimum).
pub fn split_voice_over(scene: &mut Scene, timeline: &crate::timeline::Timeline, id: &str, output_ms: f64) -> Option<String> {
    let span = voice_over_spans(scene, timeline).into_iter().find(|s| s.id == id)?;
    let local = output_ms - span.output_start_ms;
    if local < MIN_VOICE_OVER_MS || span.output_end_ms - output_ms < MIN_VOICE_OVER_MS {
        return None;
    }
    let source_ms = timeline.output_to_source(output_ms)?;
    let i = scene.voice_overs.iter().position(|v| v.id == id)?;
    let left = &mut scene.voice_overs[i];
    let mut right = left.clone();
    let total = left.voiceover_duration_ms;
    left.voiceover_duration_ms = local;
    right.id = sf_project::ids::new_id();
    right.source_start_ms = source_ms;
    right.file_offset_ms = left.file_offset_ms + local;
    right.voiceover_duration_ms = total - local;
    let new_id = right.id.clone();
    scene.voice_overs.insert(i + 1, right);
    Some(new_id)
}

#[cfg(test)]
mod voice_over_tests {
    use super::*;
    use crate::timeline::Timeline;
    use sf_project::v4::{Scene, Slice, VoiceOver};

    fn scene(slices: Vec<Slice>, vos: Vec<VoiceOver>) -> Scene {
        let mut s = Scene::recording("Recording", 0);
        s.slices = slices;
        s.voice_overs = vos;
        s
    }

    #[test]
    fn voice_over_follows_cuts_before_it() {
        // 0–10 s and 20–30 s kept: a voice-over at source 25 s plays at output 15 s
        let s = scene(vec![Slice::new(0.0, 10_000.0), Slice::new(20_000.0, 30_000.0)], vec![VoiceOver::microphone(25_000.0, 2_000.0, "a.wav")]);
        let spans = voice_over_spans(&s, &Timeline::from_scene(&s));
        assert_eq!(spans.len(), 1);
        assert!((spans[0].output_start_ms - 15_000.0).abs() < 1e-6);
        assert!((spans[0].output_end_ms - 17_000.0).abs() < 1e-6);
    }

    #[test]
    fn voice_over_on_cut_part_is_hidden_and_end_is_clipped() {
        let s = scene(vec![Slice::new(0.0, 10_000.0)], vec![VoiceOver::microphone(12_000.0, 1_000.0, "cut.wav"), VoiceOver::microphone(9_000.0, 5_000.0, "long.wav")]);
        let spans = voice_over_spans(&s, &Timeline::from_scene(&s));
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].file_name, "long.wav");
        assert!((spans[0].output_end_ms - 10_000.0).abs() < 1e-6);
    }

    #[test]
    fn split_keeps_both_halves_playing_the_same_audio() {
        let mut s = scene(vec![Slice::new(0.0, 10_000.0)], vec![VoiceOver::microphone(1_000.0, 4_000.0, "a.wav")]);
        let tl = Timeline::from_scene(&s);
        let id = s.voice_overs[0].id.clone();
        let right = split_voice_over(&mut s, &tl, &id, 2_500.0).unwrap();
        let spans = voice_over_spans(&s, &tl);
        assert_eq!(spans.len(), 2);
        assert!((spans[0].output_end_ms - 2_500.0).abs() < 1e-6);
        assert_eq!(spans[1].id, right);
        assert!((spans[1].output_start_ms - 2_500.0).abs() < 1e-6 && (spans[1].output_end_ms - 5_000.0).abs() < 1e-6);
        // the audio continues where the left part stopped
        assert!((spans[1].file_ms_at(2_500.0) - 1_500.0).abs() < 1e-6);
        // outside the take, or too close to an edge: no cut
        assert!(split_voice_over(&mut s, &tl, &id, 6_000.0).is_none());
        assert!(split_voice_over(&mut s, &tl, &id, 1_050.0).is_none());
    }

    #[test]
    fn file_offset_survives_save() {
        let mut v = VoiceOver::microphone(0.0, 1_000.0, "a.wav");
        assert!(!serde_json::to_string(&v).unwrap().contains("sfFileOffsetMs"));
        v.file_offset_ms = 250.0;
        let back: VoiceOver = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(back.file_offset_ms, 250.0);
    }

    #[test]
    fn disabled_voice_over_is_muted() {
        let mut v = VoiceOver::microphone(0.0, 1_000.0, "a.wav");
        v.is_disabled = true;
        let s = scene(vec![Slice::new(0.0, 10_000.0)], vec![v]);
        assert!(voice_over_spans(&s, &Timeline::from_scene(&s))[0].muted);
    }
}
