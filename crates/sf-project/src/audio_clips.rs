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
    /// Lane on the audio timeline: 0 is the top lane, higher layers are drawn below it.
    /// Clips on different layers may overlap in time and are mixed together.
    #[serde(default)]
    pub layer: u32,
    /// Gain in dB with a limiter, for a quiet clip (`sfBoostDb`). `None`: a microphone clip
    /// takes `config.audio.microphoneBoostDb`, the whole-track boost of older projects.
    #[serde(default, rename = "sfBoostDb", skip_serializing_if = "Option::is_none")]
    pub boost_db: Option<f64>,
}

fn one() -> f64 {
    1.0
}

impl AudioClip {
    pub const MIN_DURATION_MS: f64 = 50.0;

    pub fn new(track: AudioTrack, output_start_ms: f64, source_start_ms: f64, source_end_ms: f64, speed: f64) -> Self {
        AudioClip { id: new_id(), track, output_start_ms, source_start_ms, source_end_ms, speed: speed.max(0.01), volume: 1.0, muted: false, layer: 0, boost_db: None }
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

/// Number of lanes needed to show `clips` (the highest layer in use plus one; 0 when empty).
pub fn layer_count(clips: &[AudioClip]) -> u32 {
    clips.iter().map(|c| c.layer + 1).max().unwrap_or(0)
}

/// Renumber layers so there are no empty lanes between used ones (order is kept).
pub fn compact_layers(clips: &mut [AudioClip]) {
    let mut used: Vec<u32> = clips.iter().map(|c| c.layer).collect();
    used.sort_unstable();
    used.dedup();
    for c in clips.iter_mut() {
        c.layer = used.iter().position(|l| *l == c.layer).map(|i| i as u32).unwrap_or(0);
    }
}

/// Clips on `layer` other than `exclude`, in timeline order.
fn lane_clips<'a>(clips: &'a [AudioClip], layer: u32, exclude: &str) -> Vec<&'a AudioClip> {
    let mut lane: Vec<&AudioClip> = clips.iter().filter(|c| c.layer == layer && c.id != exclude).collect();
    lane.sort_by(|a, b| a.output_start_ms.total_cmp(&b.output_start_ms));
    lane
}

/// Clips on one lane never overlap (CapCut rule): true when `[start, end)` on `layer` is
/// clear of every clip but `exclude`.
pub fn lane_is_free(clips: &[AudioClip], layer: u32, start: f64, end: f64, exclude: &str) -> bool {
    lane_clips(clips, layer, exclude).iter().all(|c| end <= c.output_start_ms || start >= c.output_end_ms())
}

/// The start nearest to `want` on `layer` where a clip `dur` long fits without overlapping
/// another clip (ignoring `exclude`); None when no gap is wide enough.
pub fn nearest_free_start(clips: &[AudioClip], layer: u32, dur: f64, want: f64, exclude: &str) -> Option<f64> {
    let want = want.max(0.0);
    let mut best: Option<f64> = None;
    let mut consider = |lo: f64, hi: f64| {
        if hi - lo + 1e-6 >= dur {
            let s = want.clamp(lo, (hi - dur).max(lo));
            if best.is_none_or(|b| (s - want).abs() < (b - want).abs()) {
                best = Some(s);
            }
        }
    };
    let mut lo = 0.0f64;
    for c in lane_clips(clips, layer, exclude) {
        consider(lo, c.output_start_ms);
        lo = lo.max(c.output_end_ms());
    }
    consider(lo, f64::INFINITY);
    best
}

/// Ends of the neighbours of clip `id` on its own lane: (end of the clip before, start of the
/// clip after). Trimming must stay inside them.
pub fn lane_neighbours(clips: &[AudioClip], id: &str) -> (Option<f64>, Option<f64>) {
    let Some(c) = clips.iter().find(|c| c.id == id) else { return (None, None) };
    let lane = lane_clips(clips, c.layer, id);
    let before = lane.iter().filter(|o| o.output_start_ms < c.output_start_ms).map(|o| o.output_end_ms()).fold(None, |m: Option<f64>, e| Some(m.map_or(e, |m| m.max(e))));
    let after = lane.iter().filter(|o| o.output_end_ms() > c.output_end_ms()).map(|o| o.output_start_ms).fold(None, |m: Option<f64>, s| Some(m.map_or(s, |m| m.min(s))));
    (before, after)
}

/// Put clip `id` on `layer` at `start`, or at the nearest free spot on that lane when `start`
/// would overlap another clip. Returns false (clip untouched) when the lane has no room.
pub fn place_clip(clips: &mut [AudioClip], id: &str, layer: u32, start: f64) -> bool {
    let Some(i) = clips.iter().position(|c| c.id == id) else { return false };
    let dur = clips[i].output_duration_ms();
    let start = start.max(0.0);
    let s = if lane_is_free(clips, layer, start, start + dur, id) { Some(start) } else { nearest_free_start(clips, layer, dur, start, id) };
    let Some(s) = s else { return false };
    clips[i].layer = layer;
    clips[i].output_start_ms = s;
    true
}

/// Split the clip `id` at output time `t`; the right-hand part is inserted after it. With
/// `next_layer` the right-hand part keeps its time and moves down to the first lane below
/// that is free there (a new lane when none is), so both pieces can overlap later.
/// Returns the id of the new right-hand clip.
pub fn split_clip(clips: &mut Vec<AudioClip>, id: &str, t: f64, next_layer: bool) -> Option<String> {
    let i = clips.iter().position(|c| c.id == id)?;
    let mut right = clips[i].split_at_output(t)?;
    if next_layer {
        let (s, e) = (right.output_start_ms, right.output_end_ms());
        right.layer = (right.layer + 1..).find(|l| lane_is_free(clips, *l, s, e, "")).unwrap_or(right.layer + 1);
    }
    let rid = right.id.clone();
    clips.insert(i + 1, right);
    Some(rid)
}

/// Move the clip `id` by `delta` layers (clamped to the top lane and to one lane past the
/// current bottom one), sliding to the nearest free gap on the target lane, then compact
/// empty lanes. Returns the clip's new layer; None when the target lane has no room.
pub fn move_clip_layer(clips: &mut [AudioClip], id: &str, delta: i32) -> Option<u32> {
    let max = layer_count(clips);
    let c = clips.iter().find(|c| c.id == id)?;
    let target = (c.layer as i64 + delta as i64).clamp(0, max as i64) as u32;
    if !place_clip(clips, id, target, c.output_start_ms) {
        return None;
    }
    compact_layers(clips);
    clips.iter().find(|c| c.id == id).map(|c| c.layer)
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
        // a title card has no recorded audio
        for t in tracks.iter().filter(|_| !s.is_card()) {
            let vol = match t {
                AudioTrack::Microphone => s.volume,
                AudioTrack::SystemAudio => s.system_audio_volume,
            };
            let mut c = AudioClip::new(t.clone(), cursor, s.source_start_ms, s.source_end_ms, 1.0 / ts);
            c.volume = vol;
            out.push(c);
        }
        cursor += s.output_duration_ms();
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

    #[test]
    fn layers_split_move_and_compact() {
        let mut scene = Scene::recording("x", 0);
        scene.slices = vec![Slice::new(0.0, 4000.0)];
        let mut clips = clips_from_slices(&scene, &[AudioTrack::Microphone]);
        assert_eq!(layer_count(&clips), 1);
        let id = clips[0].id.clone();
        // split to the next layer: the right part lands one lane down
        let rid = split_clip(&mut clips, &id, 1000.0, true).unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].layer, 0);
        assert_eq!(clips[1].id, rid);
        assert_eq!(clips[1].layer, 1);
        assert_eq!(layer_count(&clips), 2);
        // too close to an edge: nothing happens
        assert!(split_clip(&mut clips, &id, 10.0, true).is_none());
        // a clip without `layer` in the file loads on the top lane
        let v = serde_json::json!({"id":"a","track":"microphone","outputStartMs":0.0,"sourceStartMs":0.0,"sourceEndMs":100.0});
        let c: AudioClip = serde_json::from_value(v).unwrap();
        assert_eq!(c.layer, 0);
        // moving the only clip of lane 1 up compacts the empty lane away
        assert_eq!(move_clip_layer(&mut clips, &rid, -1), Some(0));
        assert_eq!(layer_count(&clips), 1);
        // can move one past the bottom lane, but not further
        assert_eq!(move_clip_layer(&mut clips, &rid, 5), Some(1));
        assert_eq!(move_clip_layer(&mut clips, &id, -3), Some(0));
        // deleting lane 0's clip and compacting pulls the rest up
        clips.retain(|c| c.id != id);
        compact_layers(&mut clips);
        assert_eq!(clips[0].layer, 0);
    }

    #[test]
    fn clips_on_one_lane_never_overlap() {
        let mk = |layer: u32, s: f64, e: f64| {
            let mut c = AudioClip::new(AudioTrack::Microphone, s, s, e, 1.0);
            c.layer = layer;
            c
        };
        // lane 0: [0,1000) [2000,3000)   lane 1: [500,2500)
        let mut clips = vec![mk(0, 0.0, 1000.0), mk(0, 2000.0, 3000.0), mk(1, 500.0, 2500.0)];
        let (a, b, c) = (clips[0].id.clone(), clips[1].id.clone(), clips[2].id.clone());
        assert!(lane_is_free(&clips, 0, 1000.0, 2000.0, ""));
        assert!(!lane_is_free(&clips, 0, 900.0, 1500.0, ""));
        assert!(lane_is_free(&clips, 0, 900.0, 1500.0, &a)); // ignoring itself
        // a 600 ms clip wanted at 1300 slides into the 1000..2000 gap
        assert_eq!(nearest_free_start(&clips, 0, 600.0, 1300.0, ""), Some(1300.0));
        assert_eq!(nearest_free_start(&clips, 0, 600.0, 1700.0, ""), Some(1400.0));
        // 1500 ms only fits after the last clip
        assert_eq!(nearest_free_start(&clips, 0, 1500.0, 100.0, ""), Some(3000.0));
        // nothing fits between fixed neighbours when the whole lane is one clip
        let full = vec![mk(0, 0.0, 1000.0), mk(1, 0.0, 10.0)];
        assert!(nearest_free_start(&full, 0, 100.0, 50.0, "").is_some()); // after it, unbounded
        // trimming bounds come from the same lane only
        assert_eq!(lane_neighbours(&clips, &b), (Some(1000.0), None));
        assert_eq!(lane_neighbours(&clips, &a), (None, Some(2000.0)));
        assert_eq!(lane_neighbours(&clips, &c), (None, None));
        // moving lane 1's clip up: lane 0 has no 2000 ms gap under it, so it goes after 3000
        assert_eq!(move_clip_layer(&mut clips, &c, -1), Some(0));
        assert_eq!(clips[2].output_start_ms, 3000.0);
        // place at an occupied spot slides to the nearest free one; an exact fit stays
        assert!(place_clip(&mut clips, &c, 1, 100.0));
        assert_eq!((clips[2].layer, clips[2].output_start_ms), (1, 100.0));
        assert!(place_clip(&mut clips, &a, 1, 1500.0));
        assert_eq!(clips[0].output_start_ms, 2100.0);
        // split to a layer: the right part keeps its time and skips occupied lanes below
        let mut clips = vec![mk(0, 0.0, 4000.0), mk(1, 1500.0, 1600.0), mk(2, 3900.0, 5000.0)];
        let id = clips[0].id.clone();
        let rid = split_clip(&mut clips, &id, 1000.0, true).unwrap();
        let r = clips.iter().find(|c| c.id == rid).unwrap();
        assert_eq!((r.layer, r.output_start_ms), (3, 1000.0));
    }
}
