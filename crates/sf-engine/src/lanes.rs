//! Shared audio lanes (CapCut style): detached recorded audio, voice-overs and music clips all
//! sit on one stack of lanes. Any item can go on any lane; items on one lane never overlap,
//! items on different lanes play together.
//!
//! Each kind keeps its own storage (`sfAudioClips`, the Screen Studio `voiceOvers`,
//! `sfMusicClips`). Lane edits work on an *occupancy* list: every item as an [`AudioClip`]
//! with a kind-prefixed id, so the lane rules of `sf_project::audio_clips` (free gaps,
//! neighbours, layer moves, compaction) apply across kinds. [`write_back`] then stores the
//! layers and positions in each kind's own storage.

use sf_project::audio_clips::{self, AudioClip, AudioTrack, SceneAudioExt};
use sf_project::music_clips::{self, MusicClip, SceneMusicExt};
use sf_project::Project;

use crate::timeline::Timeline;

const VOICE: &str = "vo:";
const MUSIC: &str = "mu:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Recorded,
    Voice,
    Music,
}

/// Occupancy id of an item.
pub fn key(kind: Kind, id: &str) -> String {
    match kind {
        Kind::Recorded => id.to_string(),
        Kind::Voice => format!("{VOICE}{id}"),
        Kind::Music => format!("{MUSIC}{id}"),
    }
}

/// Kind and own id of an occupancy id.
pub fn parse(key: &str) -> (Kind, &str) {
    if let Some(id) = key.strip_prefix(VOICE) {
        (Kind::Voice, id)
    } else if let Some(id) = key.strip_prefix(MUSIC) {
        (Kind::Music, id)
    } else {
        (Kind::Recorded, key)
    }
}

/// The background music as clips: the edited clips, or one clip over the whole video (layer 0;
/// [`occupancy`] finds its lane). Empty without a background track.
pub fn music_clips(project: &Project, timeline: &Timeline) -> Vec<MusicClip> {
    if project.config.audio.background_audio_file_name.is_none() {
        return vec![];
    }
    project.scenes[0].music_clips().unwrap_or_else(|| {
        let mut c = MusicClip::new(0.0, timeline.duration_ms());
        c.id = music_clips::DEFAULT_ID.into();
        vec![c]
    })
}

fn pseudo(key: String, start: f64, end: f64, layer: u32) -> AudioClip {
    let mut c = AudioClip::new(AudioTrack::Microphone, start, 0.0, (end - start).max(0.0), 1.0);
    c.id = key;
    c.layer = layer;
    c
}

/// Every item on the lanes, as clips with kind-prefixed ids (see [`key`]).
pub fn occupancy(project: &Project, timeline: &Timeline) -> Vec<AudioClip> {
    let scene = &project.scenes[0];
    let mut occ: Vec<AudioClip> = scene.audio_clips().unwrap_or_default();
    for v in crate::audio::voice_over_spans(scene, timeline) {
        occ.push(pseudo(key(Kind::Voice, &v.id), v.output_start_ms, v.output_end_ms, v.layer));
    }
    let edited = scene.music_clips().is_some();
    for m in music_clips(project, timeline) {
        // the whole-video music goes on the first lane that is free under all of it
        let layer = if edited { m.layer } else { (0..).find(|l| audio_clips::lane_is_free(&occ, *l, m.output_start_ms, m.output_end_ms(), "")).unwrap_or(0) };
        occ.push(pseudo(key(Kind::Music, &m.id), m.output_start_ms, m.output_end_ms(), layer));
    }
    occ
}

/// Lanes in use (at least one).
pub fn lane_count(occ: &[AudioClip]) -> u32 {
    audio_clips::layer_count(occ).max(1)
}

/// Store the layers and start times of `occ` in each kind's own storage. The music is saved
/// as clips from then on, so its lane stays put.
pub fn write_back(project: &mut Project, timeline: &Timeline, occ: &[AudioClip]) {
    let find = |k: &str| occ.iter().find(|c| c.id == k);
    let music = music_clips(project, timeline);
    let scene = &mut project.scenes[0];
    if let Some(mut clips) = scene.audio_clips() {
        for c in &mut clips {
            if let Some(o) = find(&c.id) {
                c.layer = o.layer;
                c.output_start_ms = o.output_start_ms;
            }
        }
        scene.set_audio_clips(Some(clips));
    }
    let spans = crate::audio::voice_over_spans(scene, timeline);
    for v in &mut scene.voice_overs {
        let Some(o) = find(&key(Kind::Voice, &v.id)) else { continue };
        v.layer = o.layer;
        // a voice-over is anchored to a source frame: re-anchor only when it really moved
        let moved = spans.iter().find(|s| s.id == v.id).is_none_or(|s| (s.output_start_ms - o.output_start_ms).abs() > 0.5);
        if moved {
            if let Some(src) = timeline.output_to_source(o.output_start_ms) {
                v.source_start_ms = src;
            }
        }
    }
    if !music.is_empty() {
        let clips = music
            .into_iter()
            .map(|mut m| {
                if let Some(o) = find(&key(Kind::Music, &m.id)) {
                    m.layer = o.layer;
                    m.output_start_ms = o.output_start_ms;
                }
                m
            })
            .collect();
        scene.set_music_clips(Some(clips));
    }
}

/// Restore the lane rules after an edit: an item overlapping an earlier one on its lane
/// (recorded audio first, then voice-overs, then music, each in time order) moves down to the
/// first lane free under it, then empty lanes close up. New voice-over takes land on a free
/// lane this way, and so do items in the way of audio that was just detached.
pub fn settle(project: &mut Project, timeline: &Timeline) {
    let occ = occupancy(project, timeline);
    let rank = |c: &AudioClip| match parse(&c.id).0 {
        Kind::Recorded => 0,
        Kind::Voice => 1,
        Kind::Music => 2,
    };
    let mut order: Vec<&AudioClip> = occ.iter().collect();
    order.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.output_start_ms.total_cmp(&b.output_start_ms)));
    let mut placed: Vec<AudioClip> = Vec::with_capacity(occ.len());
    for c in order {
        let mut c = c.clone();
        let (s, e) = (c.output_start_ms, c.output_end_ms());
        if !audio_clips::lane_is_free(&placed, c.layer, s, e, &c.id) {
            c.layer = (0..).find(|l| audio_clips::lane_is_free(&placed, *l, s, e, &c.id)).unwrap_or(c.layer);
        }
        placed.push(c);
    }
    audio_clips::compact_layers(&mut placed);
    if placed.iter().any(|p| occ.iter().any(|o| o.id == p.id && o.layer != p.layer)) || project.scenes[0].music_clips().is_none() {
        write_back(project, timeline, &placed);
    }
}

/// Put item `key` on `layer` at output time `start`, or at the nearest free spot on that
/// lane. False when the lane has no room inside the video (nothing changes).
pub fn place(project: &mut Project, timeline: &Timeline, key: &str, layer: u32, start: f64) -> bool {
    let mut occ = occupancy(project, timeline);
    if !audio_clips::place_clip(&mut occ, key, layer, start) {
        return false;
    }
    // the nearest gap may be past the last item on the lane: only inside the video counts
    if occ.iter().find(|c| c.id == key).is_some_and(|c| c.output_end_ms() > timeline.duration_ms() + 0.5) {
        return false;
    }
    write_back(project, timeline, &occ);
    true
}

/// Move item `key` by `delta` lanes (see `audio_clips::move_clip_layer`); its new lane, or
/// `None` when the target lane has no room there.
pub fn move_layer(project: &mut Project, timeline: &Timeline, key: &str, delta: i32) -> Option<u32> {
    let mut occ = occupancy(project, timeline);
    let layer = audio_clips::move_clip_layer(&mut occ, key, delta)?;
    write_back(project, timeline, &occ);
    Some(layer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sf_project::v4::{Slice, VoiceOver};

    fn project(vos: Vec<VoiceOver>, music: bool) -> (Project, Timeline) {
        let mut p = Project::new("t");
        let mut scene = sf_project::v4::Scene::recording("Recording", 0);
        scene.slices = vec![Slice::new(0.0, 60_000.0)];
        scene.voice_overs = vos;
        p.scenes = vec![scene];
        if music {
            p.config.audio.background_audio_file_name = Some("track.mp3".into());
        }
        let tl = Timeline::from_scene(&p.scenes[0]);
        (p, tl)
    }

    fn layer_of(p: &Project, tl: &Timeline, k: &str) -> u32 {
        occupancy(p, tl).into_iter().find(|c| c.id == k).unwrap().layer
    }

    #[test]
    fn overlapping_takes_and_music_get_their_own_lanes() {
        let a = VoiceOver::microphone(1_000.0, 5_000.0, "a.wav");
        let b = VoiceOver::microphone(3_000.0, 5_000.0, "b.wav");
        let (ka, kb) = (key(Kind::Voice, &a.id), key(Kind::Voice, &b.id));
        let (mut p, tl) = project(vec![a, b], true);
        settle(&mut p, &tl);
        assert_eq!(layer_of(&p, &tl, &ka), 0);
        assert_eq!(layer_of(&p, &tl, &kb), 1);
        // the whole-video music is below both, saved as a clip from now on
        assert_eq!(layer_of(&p, &tl, &key(Kind::Music, music_clips::DEFAULT_ID)), 2);
        assert!(p.scenes[0].music_clips().is_some());
    }

    #[test]
    fn items_move_between_lanes_of_any_kind() {
        let a = VoiceOver::microphone(1_000.0, 5_000.0, "a.wav");
        let ka = key(Kind::Voice, &a.id);
        let (mut p, tl) = project(vec![a], true);
        settle(&mut p, &tl);
        let km = key(Kind::Music, music_clips::DEFAULT_ID);
        assert_eq!((layer_of(&p, &tl, &ka), layer_of(&p, &tl, &km)), (0, 1));
        // the music lane is taken under the take: the take cannot share it…
        assert!(!place(&mut p, &tl, &ka, 1, 1_000.0));
        // …until the music is split around the take and the middle piece removed
        let mut clips = p.scenes[0].music_clips().unwrap();
        let right = clips[0].split_at_output(7_000.0).unwrap();
        let mid = clips[0].split_at_output(500.0).unwrap();
        clips.push(right);
        let _ = mid;
        p.scenes[0].set_music_clips(Some(clips));
        assert!(place(&mut p, &tl, &ka, 1, 1_000.0));
        settle(&mut p, &tl);
        // lane 0 emptied and closed up: the take and the music share the top lane now
        assert_eq!(layer_of(&p, &tl, &ka), 0);
        assert_eq!(p.scenes[0].voice_overs[0].layer, 0);
    }

    #[test]
    fn moving_a_take_reanchors_it_only_when_it_moved() {
        let a = VoiceOver::microphone(2_000.0, 1_000.0, "a.wav");
        let ka = key(Kind::Voice, &a.id);
        let (mut p, tl) = project(vec![a], false);
        settle(&mut p, &tl);
        assert_eq!(p.scenes[0].voice_overs[0].source_start_ms, 2_000.0);
        assert!(place(&mut p, &tl, &ka, 0, 9_000.0));
        assert_eq!(p.scenes[0].voice_overs[0].source_start_ms, 9_000.0);
    }

    #[test]
    fn keys_round_trip() {
        for k in [Kind::Recorded, Kind::Voice, Kind::Music] {
            let s = key(k, "abc");
            assert_eq!(parse(&s), (k, "abc"));
        }
    }
}
