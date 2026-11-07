//! Another project added to the end of this one (a project dropped on an open one).
//!
//! `sf_recording::merge::append_recording` joins the other recording onto this one's (screen,
//! sound, camera, cursor and key logs) and says where it landed ([`Appended`]). This module
//! does the rest: the other project's clips, zooms, layouts, masks, voice-overs, titles and
//! overlays move to the end of the timeline, its voice-over and media files and its caption
//! transcript are copied in. This project's settings stay: its background, frame, cursor and
//! caption style, output size and music apply to the whole video, so changing the background
//! afterwards restyles both parts. The other project's music clips are not taken over.
//! Recordings of different shapes each keep their own: the screen frame changes shape at the
//! join, inside the same canvas (see [`crate::recording::Part`]).

use std::collections::HashMap;
use std::path::Path;

use crate::audio_clips::{clips_from_slices, AudioTrack, SceneAudioExt};
use crate::ids::new_id;
use crate::package::ProjectPackage;
use crate::recording::RecordingMetadata;
use crate::tracks::{add_clip, SceneTracksExt};
use crate::transcript::TranscriptFile;
use crate::v4::{Layout, Scene, Slice, VoiceOver};
use crate::{Error, Result};

/// Where the other recording went in this one, all in this recording's source time (ms).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Appended {
    /// Start of the other recording (this recording's old length) and its length.
    pub start_ms: f64,
    pub len_ms: f64,
    /// Where time 0 of the other recording's microphone file is in the joined microphone file;
    /// `None` when it had no microphone.
    pub microphone_shift_ms: Option<f64>,
    /// Source ranges of the joined camera with no picture (a part recorded without a camera):
    /// they are shown screen only.
    pub camera_gaps: Vec<[f64; 2]>,
    /// Added to the other recording's wall-clock times (its markers).
    pub unix_shift_ms: f64,
    /// The joined recording has these tracks.
    pub has_microphone: bool,
    pub has_system_audio: bool,
    /// The recordings had different shapes (or crops), so the joined one keeps each picture
    /// in its own part ([`crate::recording::Part`]) with each project's crop built in: this
    /// project's crop goes back to the whole picture, and from now on crops every part.
    pub crops_in_parts: bool,
}

/// Add scene `b` after scene `a`: `b`'s source times move by `at.start_ms`, its output times
/// (titles, overlays, detached audio) by `a`'s output length. Everything gets new ids, so a
/// project dropped on a copy of itself does not clash. `voice_overs` renames voice-over files
/// that were copied under another name.
pub fn append_scene(a: &mut Scene, b: &Scene, at: &Appended, voice_overs: &HashMap<String, String>) {
    let src = at.start_ms;
    let out = a.output_duration_ms();
    let b_slices = if b.slices.is_empty() { vec![Slice::new(0.0, at.len_ms)] } else { b.slices.clone() };

    // detached audio: when either side has it, both do (the other side's clips reproduce what
    // its linked audio played)
    if a.is_audio_detached() || b.is_audio_detached() {
        let mut tracks = vec![];
        if at.has_microphone {
            tracks.push(AudioTrack::Microphone);
        }
        if at.has_system_audio {
            tracks.push(AudioTrack::SystemAudio);
        }
        let mut clips = a.audio_clips().unwrap_or_else(|| clips_from_slices(a, &tracks));
        let b_linked = Scene { slices: b_slices.clone(), ..b.clone() };
        for mut c in b.audio_clips().unwrap_or_else(|| clips_from_slices(&b_linked, &tracks)) {
            c.id = new_id();
            c.output_start_ms += out;
            c.source_start_ms += src;
            c.source_end_ms += src;
            clips.push(c);
        }
        a.set_audio_clips(Some(clips));
    }

    for mut s in b_slices {
        s.id = new_id();
        s.source_start_ms += src;
        s.source_end_ms += src;
        a.slices.push(s);
    }
    for z in &b.zooms {
        a.zooms.push(crate::v4::Zoom { id: new_id(), source_start_ms: z.source_start_ms + src, source_end_ms: z.source_end_ms + src, ..z.clone() });
    }
    for l in &b.layouts {
        a.layouts.push(Layout { id: new_id(), source_start_ms: l.source_start_ms + src, source_end_ms: l.source_end_ms + src, ..l.clone() });
    }
    for m in &b.masks {
        a.masks.push(crate::v4::Mask { id: new_id(), source_start_ms: m.source_start_ms + src, source_end_ms: m.source_end_ms + src, ..m.clone() });
    }
    for v in &b.voice_overs {
        let mut v = VoiceOver { id: new_id(), source_start_ms: v.source_start_ms + src, ..v.clone() };
        if let Some(n) = voice_overs.get(&v.file_name) {
            v.file_name = n.clone();
        }
        if let Some(n) = v.source.get("sfOriginal").and_then(|o| o.as_str()).and_then(|o| voice_overs.get(o)) {
            v.source["sfOriginal"] = n.clone().into();
        }
        a.voice_overs.push(v);
    }
    // a part recorded without a camera shows the screen only, wherever no layout says otherwise
    for gap in &at.camera_gaps {
        for [s, e] in uncovered(*gap, &a.layouts) {
            a.layouts.push(Layout::new(s, e, "screen-only"));
        }
    }
    a.layouts.sort_by(|x, y| x.source_start_ms.total_cmp(&y.source_start_ms));

    // titles and overlays
    let mut tracks = a.tracks();
    for mut t in b.tracks() {
        for c in &mut t.clips {
            c.id = new_id();
            c.output_start_ms += out;
            if let Some(l) = c.link.as_mut() {
                l.source_ms += src;
            }
        }
        if t.is_visual() {
            for c in t.clips {
                add_clip(&mut tracks, c);
            }
        } else {
            t.id = new_id();
            tracks.push(t);
        }
    }
    a.set_tracks(tracks);
    let mut media = a.media();
    for m in b.media() {
        if !media.iter().any(|x| x.id == m.id) {
            media.push(m);
        }
    }
    a.set_media(media);
}

/// The parts of `[start, end]` no layout covers.
fn uncovered([start, end]: [f64; 2], layouts: &[Layout]) -> Vec<[f64; 2]> {
    let mut covered: Vec<[f64; 2]> = layouts.iter().filter(|l| l.source_end_ms > start && l.source_start_ms < end).map(|l| [l.source_start_ms, l.source_end_ms]).collect();
    covered.sort_by(|x, y| x[0].total_cmp(&y[0]));
    let mut out = vec![];
    let mut at = start;
    for [s, e] in covered {
        if s > at + 1.0 {
            out.push([at, s]);
        }
        at = at.max(e);
    }
    if end > at + 1.0 {
        out.push([at, end]);
    }
    out
}

/// Add project `b` to the end of `a`, whose recording `sf_recording::merge::append_recording`
/// has already extended (`at`). Copies `b`'s voice-over and media files into `a`, joins the
/// microphone transcripts, moves `b`'s markers and timeline after `a`'s and reloads `a`'s
/// recording metadata. `a` is not saved; `b` is only read.
pub fn append_package(a: &mut ProjectPackage, b: &ProjectPackage, at: &Appended) -> Result<()> {
    let (Some(_), Some(b_scene)) = (a.project.scenes.first(), b.project.scenes.first()) else { return Ok(()) };

    // voice-over files (and their transcripts), renamed when the name is taken
    let mut renamed: HashMap<String, String> = HashMap::new();
    let files = b_scene.voice_overs.iter().flat_map(|v| [v.file_name.clone(), v.recorded_file().to_string()]);
    for name in files {
        if renamed.contains_key(&name) {
            continue;
        }
        let from = b.path.join(VoiceOver::DIR).join(&name);
        if !from.is_file() {
            continue;
        }
        let new = free_name(&a.path.join(VoiceOver::DIR), &name);
        copy(&from, &a.path.join(VoiceOver::DIR).join(&new))?;
        if let Some(mut t) = TranscriptFile::load_voice_over(&b.path, &name)? {
            t.location.file_name = Some(new.clone());
            t.save(&a.path)?;
        }
        renamed.insert(name, new);
    }

    // media bin files (named by their content, so one already there is the same file)
    for m in b_scene.media() {
        let (from, to) = (b.path.join(&m.path), a.path.join(&m.path));
        if from.is_file() && !to.exists() {
            copy(&from, &to)?;
        }
    }

    // captions: the other microphone's words, where its sound now plays in the joined file
    if let Some(shift) = at.microphone_shift_ms {
        if let Some(theirs) = TranscriptFile::load_microphone(&b.path, 0)? {
            let mut t = TranscriptFile::load_microphone(&a.path, 0)?.unwrap_or_else(|| TranscriptFile::microphone(0, theirs.generator.clone(), vec![]));
            // words past the end of this recording's part were cut from the joined file
            t.transcript.retain(|w| w.start_ms < shift);
            for w in &mut t.transcript {
                w.end_ms = w.end_ms.min(shift);
            }
            t.transcript.extend(theirs.transcript.into_iter().map(|mut w| {
                w.start_ms += shift;
                w.end_ms += shift;
                w
            }));
            for (i, w) in t.transcript.iter_mut().enumerate() {
                w.index = i as u32;
            }
            t.save(&a.path)?;
        }
    }

    // markers set while recording are wall-clock dates
    for m in &b.markers {
        let mut m = m.clone();
        if let Some(d) = m.get("date").and_then(|d| d.as_str()).and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok()) {
            let moved = d.with_timezone(&chrono::Utc) + chrono::Duration::milliseconds(at.unix_shift_ms.round() as i64);
            m["date"] = moved.to_rfc3339_opts(chrono::SecondsFormat::Millis, true).into();
        }
        if m.get("id").is_some() {
            m["id"] = new_id().into();
        }
        a.markers.push(m);
    }

    append_scene(&mut a.project.scenes[0], b_scene, at, &renamed);
    if at.crops_in_parts {
        a.project.config.crop.rect01 = crate::schema::common::Rect::UNIT;
    }
    a.recording = Some(RecordingMetadata::load(&a.recording_dir())?);
    Ok(())
}

/// `name`, or `stem-2.ext`, `stem-3.ext`, … when `dir` has a file by that name.
fn free_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = p.extension().and_then(|e| e.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    (2..).map(|n| format!("{stem}-{n}{ext}")).find(|n| !dir.join(n).exists()).unwrap_or_else(|| name.to_string())
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|source| Error::Io { path: dir.to_path_buf(), source })?;
    }
    std::fs::copy(from, to).map(|_| ()).map_err(|source| Error::Io { path: to.to_path_buf(), source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_clips::AudioClip;
    use crate::tracks::{Clip, Link, SceneTracksExt};
    use crate::v4::Zoom;
    use crate::SceneMusicExt;

    fn at(start: f64, len: f64) -> Appended {
        Appended { start_ms: start, len_ms: len, has_microphone: true, ..Default::default() }
    }

    #[test]
    fn the_other_timeline_follows_this_one() {
        // this project: 0–4 s and 6–10 s of a 10 s recording (8 s of video)
        let mut a = Scene::recording("A", 0);
        a.slices = vec![Slice::new(0.0, 4_000.0), Slice::new(6_000.0, 10_000.0)];
        a.zooms.push(Zoom::auto(1_000.0, 2_000.0, 2.0));
        // the other: 5 s, a zoom, a title at 1 s and a mask
        let mut b = Scene::recording("B", 0);
        b.slices = vec![Slice::new(0.0, 5_000.0)];
        b.zooms.push(Zoom::auto(500.0, 1_500.0, 2.0));
        b.masks.push(crate::v4::Mask::new(0.0, 1_000.0, "highlight", vec![]));
        let mut title = Clip::title(1_000.0, 2_000.0, "Part two");
        title.link = Some(Link { source_ms: 1_000.0 });
        let mut tracks = b.tracks();
        add_clip(&mut tracks, title);
        b.set_tracks(tracks);

        append_scene(&mut a, &b, &at(10_000.0, 5_000.0), &HashMap::new());
        let ranges: Vec<_> = a.slices.iter().map(|s| (s.source_start_ms, s.source_end_ms)).collect();
        assert_eq!(ranges, vec![(0.0, 4_000.0), (6_000.0, 10_000.0), (10_000.0, 15_000.0)]);
        assert_eq!((a.zooms[1].source_start_ms, a.zooms[1].source_end_ms), (10_500.0, 11_500.0));
        assert_eq!(a.masks[0].source_start_ms, 10_000.0);
        // the title sits 1 s into the other part on the output timeline, after this one's 8 s
        let clip = &a.tracks()[0].clips[0];
        assert_eq!((clip.output_start_ms, clip.link.map(|l| l.source_ms)), (9_000.0, Some(11_000.0)));
        // nothing keeps the other project's ids
        assert_ne!(a.slices[2].id, b.slices[0].id);
        assert_ne!(a.zooms[1].id, b.zooms[0].id);
    }

    #[test]
    fn a_project_without_clips_comes_in_whole() {
        let mut a = Scene::recording("A", 0);
        a.slices = vec![Slice::new(0.0, 3_000.0)];
        append_scene(&mut a, &Scene::recording("B", 0), &at(3_000.0, 2_000.0), &HashMap::new());
        assert_eq!((a.slices[1].source_start_ms, a.slices[1].source_end_ms), (3_000.0, 5_000.0));
    }

    #[test]
    fn detached_audio_on_either_side_detaches_both() {
        let mut a = Scene::recording("A", 0);
        a.slices = vec![Slice::new(0.0, 4_000.0)];
        let mut b = Scene::recording("B", 0);
        b.slices = vec![Slice::new(0.0, 2_000.0)];
        let mut moved = AudioClip::new(AudioTrack::Microphone, 500.0, 0.0, 2_000.0, 1.0);
        moved.layer = 1;
        b.set_audio_clips(Some(vec![moved]));
        append_scene(&mut a, &b, &at(4_000.0, 2_000.0), &HashMap::new());
        let clips = a.audio_clips().expect("detached");
        // this project's linked microphone became a clip; the other's clip keeps its offset
        assert_eq!(clips.len(), 2);
        assert_eq!((clips[0].output_start_ms, clips[0].source_start_ms), (0.0, 0.0));
        assert_eq!((clips[1].output_start_ms, clips[1].source_start_ms, clips[1].layer), (4_500.0, 4_000.0, 1));
        assert!(a.music_clips().is_none());
    }

    #[test]
    fn a_part_without_camera_shows_the_screen() {
        let mut a = Scene::recording("A", 0);
        a.slices = vec![Slice::new(0.0, 4_000.0)];
        a.layouts.push(Layout::new(1_000.0, 2_000.0, "camera-overlay"));
        let b = Scene::recording("B", 0);
        let at = Appended { camera_gaps: vec![[0.0, 4_000.0]], ..at(4_000.0, 3_000.0) };
        append_scene(&mut a, &b, &at, &HashMap::new());
        let l: Vec<_> = a.layouts.iter().map(|l| (l.source_start_ms, l.source_end_ms, l.kind.as_str())).collect();
        assert_eq!(l, vec![(0.0, 1_000.0, "screen-only"), (1_000.0, 2_000.0, "camera-overlay"), (2_000.0, 4_000.0, "screen-only")]);
    }

    #[test]
    fn voice_overs_follow_their_renamed_files() {
        let mut a = Scene::recording("A", 0);
        a.slices = vec![Slice::new(0.0, 1_000.0)];
        let mut b = Scene::recording("B", 0);
        let mut v = VoiceOver::microphone(200.0, 500.0, "voiceover-1-clean.wav");
        v.set_enhanced("voiceover-1-clean.wav", "clean");
        v.source["sfOriginal"] = "voiceover-1.wav".into();
        b.voice_overs.push(v);
        let names = HashMap::from([("voiceover-1.wav".to_string(), "voiceover-1-2.wav".to_string()), ("voiceover-1-clean.wav".to_string(), "voiceover-1-clean-2.wav".to_string())]);
        append_scene(&mut a, &b, &at(1_000.0, 1_000.0), &names);
        let v = &a.voice_overs[0];
        assert_eq!((v.source_start_ms, v.file_name.as_str(), v.recorded_file()), (1_200.0, "voiceover-1-clean-2.wav", "voiceover-1-2.wav"));
    }

    #[test]
    fn files_and_captions_are_copied_in() {
        use crate::transcript::{Generator, TranscriptItem};
        let dir = tempfile::tempdir().unwrap();
        let rec = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/recording");
        let make = |name: &str| {
            let mut p = ProjectPackage::create(dir.path().join(format!("{name}.screenforge")), name).unwrap();
            std::fs::create_dir_all(p.recording_dir()).unwrap();
            std::fs::copy(rec.join("metadata.json"), p.recording_dir().join("metadata.json")).unwrap();
            p.project.scenes[0].slices = vec![Slice::new(0.0, 3_000.0)];
            p
        };
        let (mut a, mut b) = (make("A"), make("B"));
        let gen = Generator { kind: "whisper".into(), model: None, language: None, prompt: None, extra: Default::default() };
        TranscriptFile::microphone(0, gen.clone(), vec![TranscriptItem::new(0, 100.0, 400.0, "first"), TranscriptItem::new(1, 2_900.0, 3_200.0, "edge"), TranscriptItem::new(2, 3_100.0, 3_400.0, "cut")]).save(&a.path).unwrap();
        TranscriptFile::microphone(0, gen.clone(), vec![TranscriptItem::new(0, 50.0, 300.0, "second")]).save(&b.path).unwrap();
        // both have a voice-over called voiceover-1.wav
        for p in [&a, &b] {
            std::fs::create_dir_all(p.path.join(VoiceOver::DIR)).unwrap();
            std::fs::write(p.path.join(VoiceOver::DIR).join("voiceover-1.wav"), p.project.name.as_bytes()).unwrap();
        }
        TranscriptFile::voice_over("voiceover-1.wav", gen, vec![TranscriptItem::new(0, 0.0, 100.0, "hello")]).save(&b.path).unwrap();
        b.project.scenes[0].voice_overs.push(VoiceOver::microphone(100.0, 500.0, "voiceover-1.wav"));
        b.markers.push(serde_json::json!({ "id": "m1", "date": "2026-01-01T00:00:01.000Z" }));
        b.save().unwrap();
        a.save().unwrap();

        let at = Appended { microphone_shift_ms: Some(3_000.0), unix_shift_ms: 60_000.0, ..at(3_000.0, 3_000.0) };
        append_package(&mut a, &b, &at).unwrap();
        let v = &a.project.scenes[0].voice_overs[0];
        assert_eq!(v.file_name, "voiceover-1-2.wav");
        assert_eq!(std::fs::read_to_string(a.path.join(VoiceOver::DIR).join("voiceover-1-2.wav")).unwrap(), "B");
        assert_eq!(std::fs::read_to_string(a.path.join(VoiceOver::DIR).join("voiceover-1.wav")).unwrap(), "A");
        assert!(TranscriptFile::load_voice_over(&a.path, "voiceover-1-2.wav").unwrap().is_some());
        let words: Vec<_> = TranscriptFile::load_microphone(&a.path, 0).unwrap().unwrap().transcript.into_iter().map(|w| (w.index, w.start_ms, w.end_ms, w.text)).collect();
        let w = |i, s, e, t: &str| (i, s, e, t.to_string());
        assert_eq!(words, vec![w(0, 100.0, 400.0, "first"), w(1, 2_900.0, 3_000.0, "edge"), w(2, 3_050.0, 3_300.0, "second")]);
        assert_eq!(a.markers[0]["date"], "2026-01-01T00:01:01.000Z");
        // the other package is untouched
        assert_eq!(ProjectPackage::open(&b.path).unwrap().project.scenes[0].voice_overs[0].file_name, "voiceover-1.wav");
    }
}
