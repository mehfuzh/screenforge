//! Tools for the items on the timeline besides clips and zooms: masks, camera layouts, the
//! recorded audio once detached, background music and voice-overs. Times are output ms, as in
//! [`crate::tools`].

use serde_json::{json, Value};
use sf_engine::timeline::Timeline;
use sf_project::schema::common::Rect;
use sf_project::v4::{Layout, Mask};
use sf_project::{MusicClip, SceneAudioExt, SceneMusicExt, SceneTracksExt, Track, TrackKind};

use crate::tools::{clock, num, opt_num, Outcome, Workspace};

pub const LAYOUT_TYPES: [&str; 5] = ["screen-only", "camera-overlay", "split-screen", "fullscreen-camera", "cutout-camera"];

pub fn definitions() -> Vec<Value> {
    let area = json!({
        "type": "object",
        "description": "Part of the screen, 0–1 across the recording (x, y of the top-left corner)",
        "properties": { "x": { "type": "number" }, "y": { "type": "number" }, "width": { "type": "number" }, "height": { "type": "number" } },
        "required": ["x", "y", "width", "height"]
    });
    vec![
        json!({
            "name": "add_mask",
            "description": "Blur part of the screen (passwords, emails, private details) or highlight it (everything else is dimmed) over a range.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "start_ms": { "type": "number" },
                    "end_ms": { "type": "number" },
                    "kind": { "type": "string", "enum": ["blur", "highlight"] },
                    "areas": { "type": "array", "items": area, "minItems": 1 }
                },
                "required": ["start_ms", "end_ms", "kind", "areas"]
            }
        }),
        json!({
            "name": "update_mask",
            "description": "Change a mask from get_project: range, kind, areas (replaces them), blur 4–100, dim 0–1 (highlight), disabled.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "start_ms": { "type": "number" },
                    "end_ms": { "type": "number" },
                    "kind": { "type": "string", "enum": ["blur", "highlight"] },
                    "areas": { "type": "array", "items": area },
                    "blur": { "type": "number" },
                    "dim": { "type": "number" },
                    "disabled": { "type": "boolean" }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "add_layout",
            "description": "Use a different camera layout over a range (the rest of the video keeps /config/defaultLayout). Only matters when a camera was recorded.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "start_ms": { "type": "number" },
                    "end_ms": { "type": "number" },
                    "type": { "type": "string", "enum": LAYOUT_TYPES }
                },
                "required": ["start_ms", "end_ms", "type"]
            }
        }),
        json!({
            "name": "update_layout",
            "description": "Change a camera layout from get_project: range, type, disabled.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "start_ms": { "type": "number" },
                    "end_ms": { "type": "number" },
                    "type": { "type": "string", "enum": LAYOUT_TYPES },
                    "disabled": { "type": "boolean" }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "remove",
            "description": "Delete items by id (zooms, masks, camera layouts, voice-overs, music parts, audio clips, titles, overlays), or every item of one kind.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "ids": { "type": "array", "items": { "type": "string" } },
                    "all": { "type": "string", "enum": ["zooms", "masks", "layouts", "voice_overs", "music", "audio_clips", "titles", "overlays"] }
                }
            }
        }),
        json!({
            "name": "set_audio_detached",
            "description": "Detach the recorded audio (microphone, system audio) from the video so it becomes clips you can split, move, speed up, mute or delete on their own (update_audio, split with an id), or relink it (detached false: it follows the video clips again and its own edits are dropped).",
            "input_schema": { "type": "object", "properties": { "detached": { "type": "boolean" } }, "required": ["detached"] }
        }),
        json!({
            "name": "update_audio",
            "description": "Change an audio item from get_project (a detached audio clip, a music part or a voice-over): start_ms moves it on the timeline; volume (0–2, 1 = as recorded); muted; speed (audio clips, 0.25–16); boost_db (audio clips and voice-overs: 0, 6, 12 or 18 dB of gain with a limiter, for a quiet clip; that clip only); disabled (voice-overs); duration_ms, fade_in_ms, fade_out_ms and track (music parts; track from get_options music).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "start_ms": { "type": "number" },
                    "volume": { "type": "number" },
                    "muted": { "type": "boolean" },
                    "speed": { "type": "number" },
                    "disabled": { "type": "boolean" },
                    "boost_db": { "type": "number" },
                    "duration_ms": { "type": "number" },
                    "fade_in_ms": { "type": "number" },
                    "fade_out_ms": { "type": "number" },
                    "track": { "type": "string" }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "add_narration",
            "description": "Read text aloud with an AI voice (made on this computer) and add it as a voice-over take starting at start_ms; its words become captions. Write it as spoken sentences, and use one call per passage you place (one per section of the video); the result gives the take's end, so the next starts after it. Keep each passage within the part of the video it describes. On this computer, free: am_fenrir male, am_michael male, am_puck male, af_heart female, af_bella female, af_nicole female. ElevenLabs (studio quality, read online, about 8 credits per 1,000 characters from the user's credits; only when the user chose one in the Narration panel or asks for ElevenLabs, studio quality or a more realistic voice): el_eric male (use this one when asked for studio quality or a more realistic voice without naming a voice), el_brian male, el_george male British, el_daniel male British, el_sarah female, el_matilda female, el_alice female British, el_jessica female. Leave voice and speed out to use the ones the user chose in the Narration panel, and keep one voice for the whole video unless asked. speed 0.7–1.3 (1 = normal; ElevenLabs reads 0.7–1.2). The first on-device narration downloads the voice model (about 330 MB) and can take a minute. Lower the music (set_music volume 0.2–0.3) so the voice is heard.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "start_ms": { "type": "number" },
                    "voice": { "type": "string", "enum": crate::host::VOICES.iter().map(|(id, _)| *id).collect::<Vec<_>>() },
                    "speed": { "type": "number" }
                },
                "required": ["text", "start_ms"]
            }
        }),
        json!({
            "name": "set_music",
            "description": "Background music for the whole video: track (a name from get_options music, a file brought in with import_file or use_music, or null for no music), volume 0–1 (0.2–0.3 sits under a voice), muted. Choosing a track replaces the music parts' own tracks.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "track": { "type": ["string", "null"] },
                    "volume": { "type": "number" },
                    "muted": { "type": "boolean" }
                }
            }
        }),
        json!({
            "name": "add_music",
            "description": "Music over a range only (another part when there is music already). Without track it plays the project's music; with one, that track.",
            "input_schema": {
                "type": "object",
                "properties": { "start_ms": { "type": "number" }, "end_ms": { "type": "number" }, "track": { "type": "string" } },
                "required": ["start_ms", "end_ms"]
            }
        }),
    ]
}

fn kind_of(input: &Value) -> Result<Option<&'static str>, String> {
    match input.get("kind").and_then(Value::as_str) {
        None => Ok(None),
        Some("blur") => Ok(Some(Mask::SENSITIVE_DATA)),
        Some("highlight") => Ok(Some(Mask::HIGHLIGHT)),
        Some(k) => Err(format!("unknown mask kind {k} (blur or highlight)")),
    }
}

fn layout_type(input: &Value) -> Result<Option<String>, String> {
    match input.get("type").and_then(Value::as_str) {
        None => Ok(None),
        Some(t) if LAYOUT_TYPES.contains(&t) => Ok(Some(t.to_string())),
        Some(t) => Err(format!("unknown layout {t}")),
    }
}

impl Workspace {
    /// Mask areas: 0–1 of the screen to pixels of the cropped recording.
    fn areas(&self, input: &Value) -> Result<Option<Vec<Rect>>, String> {
        let Some(list) = input.get("areas").and_then(Value::as_array) else { return Ok(None) };
        let (w, h) = self.content_px;
        list.iter()
            .map(|a| {
                let (x, y) = (num(a, "x")?.clamp(0.0, 1.0), num(a, "y")?.clamp(0.0, 1.0));
                let (aw, ah) = (num(a, "width")?.clamp(0.01, 1.0 - x), num(a, "height")?.clamp(0.01, 1.0 - y));
                Ok(Rect { x: (x * w).round(), y: (y * h).round(), width: (aw * w).round().max(1.0), height: (ah * h).round().max(1.0) })
            })
            .collect::<Result<Vec<_>, String>>()
            .map(Some)
    }

    /// A new source range from `start_ms` / `end_ms` in `input`, defaulting to `current`.
    fn range_update(&self, input: &Value, current: (f64, f64)) -> Result<Option<(f64, f64)>, String> {
        match (opt_num(input, "start_ms")?, opt_num(input, "end_ms")?) {
            (None, None) => Ok(None),
            (a, b) => {
                let tl = self.timeline();
                let cur = Self::output_range(&tl, current.0, current.1).unwrap_or((0.0, 0.0));
                Self::source_range(&tl, a.unwrap_or(cur.0), b.unwrap_or(cur.1)).map(Some)
            }
        }
    }

    pub(crate) fn run_item(&mut self, name: &str, input: &Value) -> Option<Result<Outcome, String>> {
        Some(match name {
            "add_mask" => (|| {
                let (t0, t1) = (num(input, "start_ms")?, num(input, "end_ms")?);
                let (s0, s1) = Self::source_range(&self.timeline(), t0, t1)?;
                self.free(&self.mask_lane(), "mask", s0, s1, None)?;
                let kind = kind_of(input)?.unwrap_or(Mask::SENSITIVE_DATA);
                let areas = self.areas(input)?.filter(|a| !a.is_empty()).ok_or("`areas` needs at least one area")?;
                let m = Mask::new(s0, s1, kind, areas);
                let id = m.id.clone();
                self.project.scenes[0].masks.push(m);
                let what = if kind == Mask::HIGHLIGHT { "Highlight" } else { "Blur" };
                Ok(Outcome::edit(json!({ "id": id }), format!("{what} {} – {}", clock(t0), clock(t1))))
            })(),
            "update_mask" => (|| {
                let id = input.get("id").and_then(Value::as_str).ok_or("`id` is required")?;
                let m = self.project.scenes[0].masks.iter().find(|m| m.id == id).ok_or_else(|| format!("no mask {id}"))?;
                let range = self.range_update(input, (m.source_start_ms, m.source_end_ms))?;
                if let Some((s0, s1)) = range {
                    self.free(&self.mask_lane(), "mask", s0, s1, Some(id))?;
                }
                let (kind, areas) = (kind_of(input)?, self.areas(input)?);
                let m = self.project.scenes[0].masks.iter_mut().find(|m| m.id == id).expect("mask");
                if let Some((s0, s1)) = range {
                    (m.source_start_ms, m.source_end_ms) = (s0, s1);
                }
                if let Some(k) = kind {
                    m.kind = k.into();
                }
                if let Some(a) = areas.filter(|a| !a.is_empty()) {
                    m.bounds = a;
                }
                if let Some(b) = opt_num(input, "blur")? {
                    m.blur = b.clamp(4.0, 100.0);
                }
                if let Some(d) = opt_num(input, "dim")? {
                    m.highlight_mask_opacity = d.clamp(0.0, 1.0);
                }
                if let Some(d) = input.get("disabled").and_then(Value::as_bool) {
                    m.is_disabled = d;
                }
                Ok(Outcome::edit(json!({ "ok": true }), "Changed a mask"))
            })(),
            "add_layout" => (|| {
                let (t0, t1) = (num(input, "start_ms")?, num(input, "end_ms")?);
                let (s0, s1) = Self::source_range(&self.timeline(), t0, t1)?;
                self.free(&self.layout_lane(), "camera layout", s0, s1, None)?;
                let kind = layout_type(input)?.ok_or("`type` is required")?;
                let l = Layout::new(s0, s1, &kind);
                let id = l.id.clone();
                self.project.scenes[0].layouts.push(l);
                Ok(Outcome::edit(json!({ "id": id }), format!("Layout {kind} {} – {}", clock(t0), clock(t1))))
            })(),
            "update_layout" => (|| {
                let id = input.get("id").and_then(Value::as_str).ok_or("`id` is required")?;
                let l = self.project.scenes[0].layouts.iter().find(|l| l.id == id).ok_or_else(|| format!("no camera layout {id}"))?;
                let range = self.range_update(input, (l.source_start_ms, l.source_end_ms))?;
                if let Some((s0, s1)) = range {
                    self.free(&self.layout_lane(), "camera layout", s0, s1, Some(id))?;
                }
                let kind = layout_type(input)?;
                let l = self.project.scenes[0].layouts.iter_mut().find(|l| l.id == id).expect("layout");
                if let Some((s0, s1)) = range {
                    (l.source_start_ms, l.source_end_ms) = (s0, s1);
                }
                if let Some(k) = kind {
                    l.kind = k;
                }
                if let Some(d) = input.get("disabled").and_then(Value::as_bool) {
                    l.is_disabled = d;
                }
                Ok(Outcome::edit(json!({ "ok": true }), "Changed a camera layout"))
            })(),
            "remove" => self.remove(input),
            "set_audio_detached" => (|| {
                let detached = input.get("detached").and_then(Value::as_bool).ok_or("`detached` must be true or false")?;
                let scene = &mut self.project.scenes[0];
                if detached == scene.is_audio_detached() {
                    return Ok(Outcome::read(json!({ "ok": true, "note": "already so" }), if detached { "Audio is already detached" } else { "Audio already follows the video" }));
                }
                if detached {
                    if self.audio_tracks.is_empty() {
                        return Err("this recording has no microphone or system audio".into());
                    }
                    let clips = sf_project::audio_clips::clips_from_slices(scene, &self.audio_tracks);
                    scene.set_audio_clips(Some(clips));
                    Ok(Outcome::edit(json!({ "ok": true, "note": "call get_project for the audio clip ids" }), "Detached the audio"))
                } else {
                    scene.set_audio_clips(None);
                    Ok(Outcome::edit(json!({ "ok": true }), "Relinked the audio to the video"))
                }
            })(),
            "update_audio" => self.update_audio(input),
            "add_narration" => self.add_narration(input),
            "set_music" => (|| {
                let audio = &mut self.project.config.audio;
                let mut changed = vec![];
                match input.get("track") {
                    Some(Value::Null) => {
                        audio.background_audio_file_name = None;
                        self.project.scenes[0].set_music_clips(None);
                        return Ok(Outcome::edit(json!({ "ok": true }), "Removed the music"));
                    }
                    Some(Value::String(t)) => {
                        audio.background_audio_file_name = Some(t.clone());
                        audio.mute_background_audio = false;
                        // the 5 % default is near silent: start where it can be heard
                        if audio.background_audio_volume < 0.2 && input.get("volume").is_none() {
                            audio.background_audio_volume = 0.3;
                        }
                        changed.push(format!("Music: {}", t.rsplit('/').next().unwrap_or(t)));
                    }
                    _ => {}
                }
                if let Some(v) = opt_num(input, "volume")? {
                    audio.background_audio_volume = v.clamp(0.0, 1.0);
                    changed.push(format!("music volume {:.0}%", v.clamp(0.0, 1.0) * 100.0));
                }
                if let Some(m) = input.get("muted").and_then(Value::as_bool) {
                    audio.mute_background_audio = m;
                    changed.push(if m { "music muted".into() } else { "music on".into() });
                }
                if input.get("track").is_some_and(Value::is_string) {
                    if let Some(mut parts) = self.project.scenes[0].music_clips() {
                        parts.iter_mut().for_each(|p| p.file_name = None);
                        self.project.scenes[0].set_music_clips(Some(parts));
                    }
                }
                if changed.is_empty() {
                    return Err("give a track, volume or muted".into());
                }
                Ok(Outcome::edit(json!({ "ok": true }), changed.join(", ")))
            })(),
            "add_music" => (|| {
                let (t0, t1) = (num(input, "start_ms")?, num(input, "end_ms")?);
                let dur = self.timeline().duration_ms();
                let (t0, t1) = (t0.clamp(0.0, dur), t1.clamp(0.0, dur));
                if t1 - t0 < MusicClip::MIN_DURATION_MS {
                    return Err("the range is too short for music".into());
                }
                let track = input.get("track").and_then(Value::as_str).map(str::to_string);
                let mut clip = MusicClip::new(t0, t1 - t0);
                let tl = self.timeline();
                let mut parts = if self.project.config.audio.background_audio_file_name.is_none() {
                    // no music yet: this track becomes the project's music, playing only here
                    let t = track.clone().ok_or("there is no music yet: give a track (get_options music)")?;
                    self.project.config.audio.background_audio_file_name = Some(t);
                    if self.project.config.audio.background_audio_volume < 0.2 {
                        self.project.config.audio.background_audio_volume = 0.3;
                    }
                    vec![]
                } else {
                    clip.file_name = track;
                    sf_engine::lanes::music_clips(&self.project, &tl)
                };
                let id = clip.id.clone();
                parts.push(clip);
                self.project.scenes[0].set_music_clips(Some(parts));
                Ok(Outcome::edit(json!({ "id": id }), format!("Music {} – {}", clock(t0), clock(t1))))
            })(),
            _ => return None,
        })
    }

    fn remove(&mut self, input: &Value) -> Result<Outcome, String> {
        let tl = self.timeline();
        let mut music = sf_engine::lanes::music_clips(&self.project, &tl);
        let scene = &mut self.project.scenes[0];
        let mut audio = scene.audio_clips();
        let mut n = 0;
        let mut tracks = scene.tracks();
        let clip_count = |t: &[Track]| t.iter().map(|t| t.clips.len()).sum::<usize>();
        if let Some(all) = input.get("all").and_then(Value::as_str) {
            n = match all {
                "titles" | "overlays" => {
                    let text = all == "titles";
                    let before = clip_count(&tracks);
                    tracks.retain(|t| !t.is_visual() || (t.kind == TrackKind::Text) != text);
                    before - clip_count(&tracks)
                }
                "zooms" => std::mem::take(&mut scene.zooms).len(),
                "masks" => std::mem::take(&mut scene.masks).len(),
                "layouts" => std::mem::take(&mut scene.layouts).len(),
                "voice_overs" => std::mem::take(&mut scene.voice_overs).len(),
                "music" => std::mem::take(&mut music).len(),
                "audio_clips" => audio.as_mut().map(std::mem::take).map_or(0, |v| v.len()),
                k => return Err(format!("unknown kind {k}")),
            };
        }
        let ids: Vec<&str> = input.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        if !ids.is_empty() {
            let keep = |id: &str| !ids.contains(&id);
            let before = scene.zooms.len() + scene.masks.len() + scene.layouts.len() + scene.voice_overs.len() + music.len() + audio.as_ref().map_or(0, Vec::len);
            scene.zooms.retain(|z| keep(&z.id));
            scene.masks.retain(|m| keep(&m.id));
            scene.layouts.retain(|l| keep(&l.id));
            scene.voice_overs.retain(|v| keep(&v.id));
            music.retain(|m| keep(&m.id));
            if let Some(a) = audio.as_mut() {
                a.retain(|c| keep(&c.id));
                sf_project::audio_clips::compact_layers(a);
            }
            n += before - (scene.zooms.len() + scene.masks.len() + scene.layouts.len() + scene.voice_overs.len() + music.len() + audio.as_ref().map_or(0, Vec::len));
            for id in &ids {
                if sf_project::tracks::remove_clip(&mut tracks, id) {
                    n += 1;
                }
            }
        }
        scene.set_tracks(tracks);
        if n == 0 {
            return Err("nothing matched: check the ids with get_project".into());
        }
        if music.is_empty() {
            // the last music part went: no music at all
            self.project.config.audio.background_audio_file_name = None;
            scene.set_music_clips(None);
        } else if self.project.config.audio.background_audio_file_name.is_some() {
            scene.set_music_clips(Some(music));
        }
        if audio.is_some() {
            scene.set_audio_clips(audio);
        }
        Ok(Outcome::edit(json!({ "removed": n }), if n == 1 { "Removed 1 item".to_string() } else { format!("Removed {n} items") }))
    }

    fn update_audio(&mut self, input: &Value) -> Result<Outcome, String> {
        let id = input.get("id").and_then(Value::as_str).ok_or("`id` is required")?.to_string();
        let start = opt_num(input, "start_ms")?;
        let volume = opt_num(input, "volume")?.map(|v| v.clamp(0.0, 2.0));
        let muted = input.get("muted").and_then(Value::as_bool);
        let tl = self.timeline();
        let scene = &mut self.project.scenes[0];
        if let Some(mut clips) = scene.audio_clips().filter(|cs| cs.iter().any(|c| c.id == id)) {
            let c = clips.iter_mut().find(|c| c.id == id).expect("clip");
            if let Some(s) = start {
                c.output_start_ms = s.max(0.0);
            }
            if let Some(v) = volume {
                c.volume = v;
            }
            if let Some(m) = muted {
                c.muted = m;
            }
            if let Some(sp) = opt_num(input, "speed")? {
                c.speed = sp.clamp(0.25, 16.0);
            }
            if let Some(b) = opt_num(input, "boost_db")? {
                c.boost_db = Some(b.clamp(0.0, 24.0));
            }
            scene.set_audio_clips(Some(clips));
            return Ok(Outcome::edit(json!({ "ok": true }), "Changed an audio clip"));
        }
        if let Some(v) = scene.voice_overs.iter_mut().find(|v| v.id == id) {
            if let Some(s) = start {
                v.source_start_ms = tl.output_to_source(s.clamp(0.0, tl.duration_ms())).ok_or("start is outside the video")?;
            }
            if let Some(vol) = volume {
                v.volume = vol;
            }
            if let Some(d) = input.get("disabled").and_then(Value::as_bool).or(muted) {
                v.is_disabled = d;
            }
            if let Some(b) = opt_num(input, "boost_db")? {
                v.boost_db = b.clamp(0.0, 24.0);
            }
            return Ok(Outcome::edit(json!({ "ok": true }), "Changed a voice-over"));
        }
        let mut music = sf_engine::lanes::music_clips(&self.project, &tl);
        if let Some(m) = music.iter_mut().find(|m| m.id == id) {
            if let Some(s) = start {
                m.output_start_ms = s.max(0.0);
            }
            if let Some(d) = opt_num(input, "duration_ms")? {
                m.duration_ms = d.max(MusicClip::MIN_DURATION_MS);
            }
            if let Some(f) = opt_num(input, "fade_in_ms")? {
                m.fade_in_ms = f.max(0.0);
            }
            if let Some(f) = opt_num(input, "fade_out_ms")? {
                m.fade_out_ms = f.max(0.0);
            }
            if let Some(v) = volume {
                m.volume = Some(v.min(1.0));
            }
            if let Some(mu) = muted {
                m.muted = mu;
            }
            if let Some(t) = input.get("track").and_then(Value::as_str) {
                m.file_name = Some(t.to_string());
                m.muted = false;
            }
            self.project.scenes[0].set_music_clips(Some(music));
            return Ok(Outcome::edit(json!({ "ok": true }), "Changed the music"));
        }
        Err(format!("no audio clip, voice-over or music part {id}"))
    }

    /// `split` with an id: cut an audio item at `t` (output ms). The id of the right part.
    pub(crate) fn split_audio(&mut self, id: &str, t: f64) -> Result<Outcome, String> {
        let tl = self.timeline();
        let scene = &mut self.project.scenes[0];
        if let Some(mut clips) = scene.audio_clips().filter(|cs| cs.iter().any(|c| c.id == id)) {
            let right = sf_project::audio_clips::split_clip(&mut clips, id, t, false).ok_or_else(|| format!("{} is not inside that audio clip, or too close to its edge", clock(t)))?;
            scene.set_audio_clips(Some(clips));
            return Ok(Outcome::edit(json!({ "right": right }), format!("Split audio at {}", clock(t))));
        }
        if scene.voice_overs.iter().any(|v| v.id == id) {
            let right = sf_engine::audio::split_voice_over(scene, &tl, id, t).ok_or_else(|| format!("{} is not inside that voice-over", clock(t)))?;
            return Ok(Outcome::edit(json!({ "right": right }), format!("Split voice-over at {}", clock(t))));
        }
        let mut music = sf_engine::lanes::music_clips(&self.project, &tl);
        if let Some(i) = music.iter().position(|m| m.id == id) {
            let right = music[i].split_at_output(t).ok_or_else(|| format!("{} is not inside that music part, or too close to its edge", clock(t)))?;
            let rid = right.id.clone();
            music.insert(i + 1, right);
            self.project.scenes[0].set_music_clips(Some(music));
            return Ok(Outcome::edit(json!({ "right": rid }), format!("Split music at {}", clock(t))));
        }
        Err(format!("no audio clip, voice-over or music part {id}"))
    }

    fn add_narration(&mut self, input: &Value) -> Result<Outcome, String> {
        let text = input.get("text").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).ok_or("`text` is required")?;
        let t = num(input, "start_ms")?;
        // the voice and speed chosen in the Narration panel, unless asked for another
        let audio = &self.project.config.audio;
        let default_voice = audio.narration_voice.clone().filter(|v| crate::host::VOICES.iter().any(|(id, _)| id == v)).unwrap_or_else(|| crate::host::VOICES[0].0.to_string());
        let voice = input.get("voice").and_then(Value::as_str).unwrap_or(&default_voice).to_string();
        let voice = voice.as_str();
        if !crate::host::VOICES.iter().any(|(id, _)| *id == voice) {
            return Err(format!("unknown voice {voice}"));
        }
        let speed = opt_num(input, "speed")?.unwrap_or(audio.narration_speed());
        if !(0.5..=2.0).contains(&speed) {
            return Err("`speed` is 0.7–1.3 (1 = normal)".into());
        }
        let tl = self.timeline();
        if t < 0.0 || t >= tl.duration_ms() {
            return Err(format!("{} is not on the timeline (it is {} long)", clock(t), clock(tl.duration_ms())));
        }
        let source = tl.output_to_source(t).ok_or_else(|| format!("{} is not on the timeline", clock(t)))?;
        let spoken = self.host.as_deref().ok_or("not available here (no editor)")?.speak(text, voice, speed)?;
        let vo = sf_project::v4::VoiceOver::spoken(source, spoken.duration_ms, spoken.file_name, text, voice);
        let id = vo.id.clone();
        self.project.scenes[0].voice_overs.push(vo);
        let end = t + spoken.duration_ms;
        let mut result = json!({ "id": id, "startMs": t.round(), "endMs": end.round(), "voice": voice });
        if end > tl.duration_ms() {
            result["note"] = json!(format!("The narration runs {:.1} s past the end of the video and is cut there; shorten the text, speed it up, or start it earlier.", (end - tl.duration_ms()) / 1000.0));
        }
        Ok(Outcome::edit(result, format!("Narration {} – {}", clock(t), clock(end.min(tl.duration_ms())))))
    }

    /// Audio, music and voice-overs for `get_project`.
    pub(crate) fn audio_overview(&self, tl: &Timeline) -> Value {
        let ms = |v: f64| (v * 10.0).round() / 10.0;
        let scene = &self.project.scenes[0];
        let a = &self.project.config.audio;
        let clips: Vec<Value> = scene.audio_clips().unwrap_or_default().iter().map(|c| json!({ "id": c.id, "track": c.track, "startMs": ms(c.output_start_ms), "endMs": ms(c.output_end_ms()), "volume": c.volume, "muted": c.muted, "speed": c.speed, "boostDb": c.boost_db })).collect();
        let music: Vec<Value> = sf_engine::lanes::music_clips(&self.project, tl).iter().map(|m| json!({ "id": m.id, "startMs": ms(m.output_start_ms), "endMs": ms(m.output_end_ms()), "track": m.file_name, "volume": m.volume, "muted": m.muted, "fadeInMs": m.fade_in_ms, "fadeOutMs": m.fade_out_ms })).collect();
        let voice: Vec<Value> = sf_engine::audio::voice_over_spans(scene, tl)
            .iter()
            .map(|v| {
                let mut o = json!({ "id": v.id, "startMs": ms(v.output_start_ms), "endMs": ms(v.output_end_ms), "volume": v.volume, "boostDb": v.boost_db, "disabled": v.muted });
                // an AI voice's take: what it says, to change it (remove, then add_narration)
                if let Some(take) = scene.voice_overs.iter().find(|t| t.id == v.id).filter(|t| t.spoken_voice().is_some()) {
                    o["aiVoice"] = json!(take.spoken_voice());
                    o["text"] = take.source.get("text").cloned().unwrap_or(Value::Null);
                }
                o
            })
            .collect();
        json!({
            "recordedTracks": self.audio_tracks,
            "detached": scene.is_audio_detached(),
            "audioClips": clips,
            "music": { "track": a.background_audio_file_name, "volume": a.background_audio_volume, "muted": a.mute_background_audio, "parts": music },
            "voiceOvers": voice,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{Action, Host};
    use sf_project::audio_clips::AudioTrack;
    use sf_project::v4::{Slice, VoiceOver};
    use sf_project::Project;
    use std::sync::Arc;

    fn ws() -> Workspace {
        let mut p = Project::new("T");
        p.scenes[0].slices = vec![Slice::new(0.0, 10_000.0)];
        let mut w = Workspace::new(p);
        w.content_px = (2000.0, 1000.0);
        w.audio_tracks = vec![AudioTrack::Microphone];
        w
    }

    fn id(o: &Outcome, key: &str) -> String {
        serde_json::from_str::<Value>(&o.result).unwrap()[key].as_str().unwrap().to_string()
    }

    #[test]
    fn masks_use_screen_fractions_and_one_lane() {
        let mut w = ws();
        let o = w.run("add_mask", &json!({ "start_ms": 1000, "end_ms": 3000, "kind": "blur", "areas": [{ "x": 0.5, "y": 0.25, "width": 0.25, "height": 0.5 }] })).unwrap();
        let m = &w.project.scenes[0].masks[0];
        assert_eq!((m.bounds[0].x, m.bounds[0].y, m.bounds[0].width, m.bounds[0].height), (1000.0, 250.0, 500.0, 500.0));
        assert!(w.run("add_mask", &json!({ "start_ms": 2000, "end_ms": 4000, "kind": "blur", "areas": [{ "x": 0, "y": 0, "width": 1, "height": 1 }] })).is_err());
        w.run("update_mask", &json!({ "id": id(&o, "id"), "kind": "highlight", "dim": 0.8, "end_ms": 5000 })).unwrap();
        let m = &w.project.scenes[0].masks[0];
        assert_eq!((m.kind.as_str(), m.highlight_mask_opacity, m.source_end_ms), (Mask::HIGHLIGHT, 0.8, 5000.0));
    }

    #[test]
    fn layouts_over_a_range() {
        let mut w = ws();
        let o = w.run("add_layout", &json!({ "start_ms": 0, "end_ms": 4000, "type": "fullscreen-camera" })).unwrap();
        assert!(w.run("add_layout", &json!({ "start_ms": 3000, "end_ms": 6000, "type": "screen-only" })).is_err());
        assert!(w.run("add_layout", &json!({ "start_ms": 5000, "end_ms": 6000, "type": "nope" })).is_err());
        w.run("update_layout", &json!({ "id": id(&o, "id"), "type": "split-screen" })).unwrap();
        assert_eq!(w.project.scenes[0].layouts[0].kind, "split-screen");
    }

    /// A host that "speaks" by length: 100 ms per character.
    struct Voice;
    impl crate::Host for Voice {
        fn options(&self) -> Value {
            json!({})
        }
        fn apply_preset(&self, _: &str, _: &mut Project) -> Result<(), String> {
            Err("no".into())
        }
        fn search_photos(&self, _: &str) -> Result<Value, String> {
            Err("no".into())
        }
        fn download_photo(&self, _: &str) -> Result<Value, String> {
            Err("no".into())
        }
        fn import_file(&self, _: &str) -> Result<String, String> {
            Err("no".into())
        }
        fn screen_motion(&self) -> Result<Vec<(f64, f64, crate::motion::Motion)>, String> {
            Err("no".into())
        }
        fn screen_frames(&self, _: &[f64]) -> Result<Vec<Vec<u8>>, String> {
            Err("no".into())
        }
        fn speak(&self, text: &str, voice: &str, _: f64) -> Result<crate::host::Spoken, String> {
            Ok(crate::host::Spoken { file_name: format!("{voice}.wav"), duration_ms: text.len() as f64 * 100.0 })
        }
    }

    #[test]
    fn narration_is_placed_where_asked_and_listed_with_its_text() {
        let mut w = ws();
        // 2–4 s cut: output 5 s is source 7 s
        w.project.scenes[0].slices = vec![Slice::new(0.0, 2_000.0), Slice::new(4_000.0, 10_000.0)];
        assert!(w.run("add_narration", &json!({ "text": "Hi", "start_ms": 1000 })).is_err(), "no voice without an editor");
        w.host = Some(std::sync::Arc::new(Voice));
        let o = w.run("add_narration", &json!({ "text": "Hello there", "start_ms": 5000, "voice": "af_heart" })).unwrap();
        let v = &w.project.scenes[0].voice_overs[0];
        assert_eq!((v.source_start_ms, v.voiceover_duration_ms, v.file_name.as_str(), v.spoken_voice()), (7_000.0, 1_100.0, "af_heart.wav", Some("af_heart")));
        assert_eq!(serde_json::from_str::<Value>(&o.result).unwrap()["endMs"], 6100.0);
        let listed = &w.audio_overview(&w.timeline())["voiceOvers"][0];
        assert_eq!((listed["aiVoice"].as_str(), listed["text"].as_str()), (Some("af_heart"), Some("Hello there")));
        // past the end: placed, with a note to shorten it
        let o = w.run("add_narration", &json!({ "text": "x".repeat(40), "start_ms": 6000 })).unwrap();
        assert!(o.result.contains("past the end"), "{}", o.result);
        assert_eq!(w.project.scenes[0].voice_overs[1].spoken_voice(), Some("am_fenrir"));
        assert!(w.run("add_narration", &json!({ "text": "Hi", "start_ms": 1000, "voice": "nobody" })).is_err());
        assert!(w.run("add_narration", &json!({ "text": "Hi", "start_ms": 9000 })).is_err(), "the timeline is 8 s");
    }

    #[test]
    fn detached_audio_can_be_split_moved_and_relinked() {
        let mut w = ws();
        w.run("set_audio_detached", &json!({ "detached": true })).unwrap();
        let clip = w.project.scenes[0].audio_clips().unwrap()[0].id.clone();
        let right = id(&w.run("split", &json!({ "at_ms": 4000, "id": clip })).unwrap(), "right");
        w.run("update_audio", &json!({ "id": right, "volume": 0.5, "muted": true })).unwrap();
        let cs = w.project.scenes[0].audio_clips().unwrap();
        assert_eq!(cs.len(), 2);
        let r = cs.iter().find(|c| c.id == right).unwrap();
        assert!(r.muted && r.volume == 0.5);
        w.run("remove", &json!({ "ids": [clip] })).unwrap();
        assert_eq!(w.project.scenes[0].audio_clips().unwrap().len(), 1);
        w.run("set_audio_detached", &json!({ "detached": false })).unwrap();
        assert!(!w.project.scenes[0].is_audio_detached());
    }

    #[test]
    fn music_for_the_whole_video_and_for_a_range() {
        let mut w = ws();
        assert!(w.run("add_music", &json!({ "start_ms": 0, "end_ms": 4000 })).unwrap_err().contains("track"));
        w.run("add_music", &json!({ "start_ms": 0, "end_ms": 4000, "track": "calm/a.mp3" })).unwrap();
        assert_eq!(w.project.config.audio.background_audio_file_name.as_deref(), Some("calm/a.mp3"));
        let parts = w.project.scenes[0].music_clips().unwrap();
        assert_eq!((parts.len(), parts[0].duration_ms), (1, 4000.0));
        w.run("set_music", &json!({ "track": "electronic/b.mp3", "volume": 0.25 })).unwrap();
        assert_eq!(w.project.config.audio.background_audio_volume, 0.25);
        let part = parts[0].id.clone();
        w.run("update_audio", &json!({ "id": part, "fade_out_ms": 0, "start_ms": 1000 })).unwrap();
        assert_eq!(w.project.scenes[0].music_clips().unwrap()[0].output_start_ms, 1000.0);
        w.run("remove", &json!({ "all": "music" })).unwrap();
        assert!(w.project.config.audio.background_audio_file_name.is_none());
        // whole-video music shows as one part with the default id
        w.run("set_music", &json!({ "track": "calm/a.mp3" })).unwrap();
        let v: Value = serde_json::from_str(&w.run("get_project", &json!({})).unwrap().result).unwrap();
        assert_eq!(v["audio"]["music"]["parts"][0]["id"], "music");
        w.run("set_music", &json!({ "track": null })).unwrap();
        assert!(w.project.config.audio.background_audio_file_name.is_none());
    }

    #[test]
    fn voice_overs_move_and_split() {
        let mut w = ws();
        w.project.scenes[0].voice_overs.push(VoiceOver::microphone(1000.0, 3000.0, "take.m4a"));
        let vid = w.project.scenes[0].voice_overs[0].id.clone();
        w.run("update_audio", &json!({ "id": vid, "start_ms": 2000, "volume": 1.5, "boost_db": 12 })).unwrap();
        assert_eq!(w.project.scenes[0].voice_overs[0].source_start_ms, 2000.0);
        assert_eq!(w.project.scenes[0].voice_overs[0].boost(), 12.0);
        w.run("split", &json!({ "id": vid, "at_ms": 3000 })).unwrap();
        assert_eq!(w.project.scenes[0].voice_overs.len(), 2);
        assert!(w.run("update_audio", &json!({ "id": "missing" })).is_err());
    }

    struct MockHost;
    impl Host for MockHost {
        fn options(&self) -> Value {
            json!({ "music": [{ "track": "calm/a.mp3", "label": "A" }] })
        }
        fn apply_preset(&self, name: &str, p: &mut Project) -> Result<(), String> {
            if name != "Dark" {
                return Err("no preset".into());
            }
            p.config.output.padding_ratio01 = 0.2;
            Ok(())
        }
        fn search_photos(&self, _: &str) -> Result<Value, String> {
            Ok(json!([{ "id": "p1", "description": "mountains" }]))
        }
        fn download_photo(&self, id: &str) -> Result<Value, String> {
            Ok(json!({ "image": format!("unsplash-{id}.jpg"), "id": id, "author": "Ann", "credit": "Photo by Ann on Unsplash" }))
        }
        fn import_file(&self, path: &str) -> Result<String, String> {
            Ok(path.rsplit('/').next().unwrap().to_string())
        }
        fn search_music(&self, _: &str, instrumental: bool) -> Result<Value, String> {
            Ok(json!([{ "id": "7", "name": "Lights", "artist": "Ann", "seconds": 151, "instrumental": instrumental }]))
        }
        fn download_music(&self, id: &str) -> Result<Value, String> {
            Ok(json!({ "fileName": format!("jamendo_{id}.mp3"), "name": "Lights", "artist": "Ann", "credit": "Music: \"Lights\" by Ann" }))
        }
        fn import_url(&self, url: &str) -> Result<String, String> {
            Ok(url.rsplit('/').next().unwrap().to_string())
        }
        fn screen_motion(&self) -> Result<Vec<(f64, f64, crate::motion::Motion)>, String> {
            use crate::motion::Motion;
            Ok(vec![(0.0, 2000.0, Motion::Still), (2000.0, 6000.0, Motion::Small), (6000.0, 10_000.0, Motion::Large)])
        }
        fn screen_frames(&self, source_ms: &[f64]) -> Result<Vec<Vec<u8>>, String> {
            Ok(source_ms.iter().map(|t| format!("jpeg@{t}").into_bytes()).collect())
        }
    }

    #[test]
    fn app_tools_go_through_the_host() {
        let mut w = ws();
        assert!(w.run("search_photos", &json!({ "query": "x" })).is_err(), "no editor, no photos");
        w.host = Some(Arc::new(MockHost));
        let v: Value = serde_json::from_str(&w.run("get_options", &json!({})).unwrap().result).unwrap();
        assert_eq!(v["music"][0]["track"], "calm/a.mp3");
        assert!(v["settings"].as_str().unwrap().contains("/config/output/paddingRatio01"));
        assert!(v["gradients"].as_array().is_some_and(|g| !g.is_empty()));
        w.run("search_photos", &json!({ "query": "mountains" })).unwrap();
        let o = w.run("use_photo", &json!({ "id": "p1" })).unwrap();
        assert!(o.result.contains("Photo by Ann on Unsplash"), "the credit goes back to the agent");
        let bg = &w.project.config.styles.background;
        assert_eq!((bg.kind.as_str(), bg.unsplash.as_ref().unwrap()["image"].as_str()), ("unsplash", Some("unsplash-p1.jpg")));
        assert!(bg.unsplash.as_ref().unwrap().get("credit").is_none(), "the credit is not saved in the project");
        w.run("import_file", &json!({ "path": "/Users/me/song.mp3", "use_as": "music" })).unwrap();
        assert_eq!(w.project.config.audio.background_audio_file_name.as_deref(), Some("song.mp3"));
        let v: Value = serde_json::from_str(&w.run("search_music", &json!({ "query": "calm" })).unwrap().result).unwrap();
        assert_eq!(v["tracks"][0]["instrumental"], true, "instrumental unless asked otherwise");
        let o = w.run("use_music", &json!({ "id": "7", "volume": 0.25 })).unwrap();
        assert!(o.result.contains("by Ann"), "the credit goes back to the agent");
        assert_eq!(w.project.config.audio.background_audio_file_name.as_deref(), Some("jamendo_7.mp3"));
        assert_eq!(w.project.config.audio.background_audio_volume, 0.25);
        w.run("import_file", &json!({ "path": "https://cdn.example.com/beat.mp3", "use_as": "music" })).unwrap();
        assert_eq!(w.project.config.audio.background_audio_file_name.as_deref(), Some("beat.mp3"));
        assert!(w.run("import_file", &json!({ "path": "https://example.com/a.jpg", "use_as": "background" })).is_err());
        w.run("apply_preset", &json!({ "name": "Dark" })).unwrap();
        assert_eq!(w.project.config.output.padding_ratio01, 0.2);
        assert!(w.run("apply_preset", &json!({ "name": "Nope" })).is_err());
        // motion follows the timeline: after cutting the first 2 s the typing starts at 0
        w.run("cut", &json!({ "ranges": [{ "start_ms": 0, "end_ms": 2000 }] })).unwrap();
        let m: Value = serde_json::from_str(&w.run("get_motion", &json!({})).unwrap().result).unwrap();
        assert_eq!(m["sections"], json!([[0.0, 4000.0, "small"], [4000.0, 8000.0, "large"]]));
        // frames are asked for by source time and come back as images
        let o = w.run("view_frames", &json!({ "at_ms": [1000, 5000] })).unwrap();
        assert_eq!(o.images, vec![b"jpeg@3000".to_vec(), b"jpeg@7000".to_vec()]);
        assert!(w.run("view_frames", &json!({ "at_ms": [] })).is_err());
        w.run("transcribe", &json!({ "language": "en" })).unwrap();
        w.run("open_export", &json!({})).unwrap();
        assert_eq!(w.actions, vec![Action::Transcribe { language: Some("en".into()) }, Action::OpenExport]);
    }
}
