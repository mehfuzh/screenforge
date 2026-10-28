use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, TextureId};
use sf_engine::stage::OutputSize;
use sf_render::{Compositor, Gpu};
use sf_session::Session;

use crate::audio::AudioPlayer;
use crate::export::ExportDialog;
use crate::sidebar::{self, Section};
use crate::theme;
use crate::timeline::TimelineWidget;
use crate::crop;
use crate::widgets;

/// A button of the preview's tool rail.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RailTool {
    TitleCard,
    AutoZooms,
    Mask,
    Title,
    Media,
    Transition,
    VoiceOver,
}

/// The rail's buttons: only things added at the playhead (settings live in the sidebar), in
/// groups (what goes on screen; editing the video; recording), most used first. The "More
/// tools" menu takes them from the bottom on a short window. A single zoom is added from the
/// zoom lane, its menu or the sidebar.
const RAIL_TOOLS: [RailTool; 6] = [RailTool::Title, RailTool::Media, RailTool::Transition, RailTool::Mask, RailTool::AutoZooms, RailTool::VoiceOver];
/// A divider follows these buttons on the rail.
const RAIL_GROUP_ENDS: [RailTool; 2] = [RailTool::Media, RailTool::AutoZooms];

/// What the Text button offers.
const TEXT_CHOICES: [RailTool; 2] = [RailTool::Title, RailTool::TitleCard];

impl RailTool {
    fn icon(self) -> widgets::Icon {
        match self {
            RailTool::AutoZooms => widgets::Icon::Sparkle,
            RailTool::Mask => widgets::Icon::Mask,
            RailTool::Title => widgets::Icon::Text,
            RailTool::TitleCard => widgets::Icon::TitleCard,
            RailTool::Media => widgets::Icon::Media,
            RailTool::Transition => widgets::Icon::Transition,
            RailTool::VoiceOver => widgets::Icon::Mic,
        }
    }

    /// Name in the "More tools" menu.
    fn label(self) -> &'static str {
        match self {
            RailTool::AutoZooms => "Automatic zooms",
            RailTool::Mask => "Blur mask",
            RailTool::Title => "Title over the video",
            RailTool::TitleCard => "Title card",
            RailTool::Media => "Image or video",
            RailTool::Transition => "Transition",
            RailTool::VoiceOver => "Voice-over",
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            RailTool::AutoZooms => "Automatic zooms from clicks",
            RailTool::Mask => "Add blur mask at playhead (M)",
            RailTool::Title => "A title over the video at the playhead (T)",
            RailTool::TitleCard => "A blank clip with a title, before the clip at the playhead",
            RailTool::Media => "Add image or video over the recording (I)",
            RailTool::Transition => "Add transition at the nearest cut",
            RailTool::VoiceOver => "Voice-over (R)",
        }
    }
}

pub struct Editor {
    /// Opened straight after recording: closing asks whether to keep it.
    pub fresh_recording: bool,
    pub path: PathBuf,
    pub session: Session,
    pub compositor: Compositor,
    pub texture_id: Option<TextureId>,
    texture_key: Option<(u32, u32)>,
    pub preview_size: OutputSize,
    pub playhead_ms: f64,
    pub playing: bool,
    last_tick: Option<Instant>,
    last_rendered_ms: Option<f64>,
    pub selected_slice: Option<usize>,
    /// Selected zoom item (id), shown in the sidebar inspector.
    pub selected_zoom: Option<String>,
    /// Selected mask (id): its regions are edited on the preview.
    pub selected_mask: Option<String>,
    /// Mask region being dragged on the preview.
    pub mask_drag: Option<crate::masks::RegionDrag>,
    /// Frames drawn since the project opened, until the window has been grown to fit the
    /// whole tool rail (once; see `fit_window_to_rail`).
    rail_fit_frames: u8,
    /// Selected title or overlay (clip id in `sfTracks`).
    pub selected_clip: Option<String>,
    /// Drags and typing in progress on titles and overlays.
    pub layers: crate::layers::LayerUi,
    /// Selected detached audio clip (id).
    pub selected_audio: Option<String>,
    /// Selected background music clip (id); ⌫ removes it, or the music with its last clip.
    pub selected_music: Option<String>,
    /// Selected voice-over (id).
    pub selected_voice_over: Option<String>,
    /// A voice-over is being recorded: the video plays muted from where it started.
    pub voice_rec: Option<crate::voice_over::VoiceRecorder>,
    /// Microphone for voice-overs; `None` records from the system default.
    pub voice_mic: Option<sf_recording::discover::AudioDevice>,
    /// Microphones in the voice-over menu, listed when it is first shown and again on opening it.
    pub voice_mics: Option<Vec<sf_recording::discover::AudioDevice>>,
    /// The voice-over bar is shown over the preview (microphone, Record / Stop).
    pub voice_hud: bool,
    /// Live level of the voice-over microphone while the bar waits to record.
    voice_meter: Option<crate::mic_meter::MicMeter>,
    /// Recent microphone levels for the bar's equalizer (one every 50 ms, oldest first).
    voice_levels: std::collections::VecDeque<f32>,
    voice_levels_at: Option<Instant>,
    /// Background → Unsplash search and downloads.
    pub unsplash: crate::unsplash::Browser,
    /// Audio → Find music (Jamendo search and link import).
    pub music: crate::music::Browser,
    pub dirty: bool,
    /// Time of the last change; unsaved changes are written shortly after editing stops.
    last_edit: Option<Instant>,
    pub timeline: TimelineWidget,
    pub export: ExportDialog,
    pub undo: Vec<sf_project::Project>,
    pub redo: Vec<sf_project::Project>,
    pub status: Option<(String, Instant)>,
    pub section: Section,
    /// Right-hand sidebar (icon rail + inspector) is shown; toggled from the title bar or ⌘\.
    pub sidebar_open: bool,
    pub audio: AudioPlayer,
    pub preview_quality: PreviewQuality,
    pub preview_scale: f32,
    /// Title being edited (click the title to rename the project).
    pub title_edit: Option<String>,
    /// Set after a rename so the app can update its recent-projects list.
    pub renamed_to: Option<PathBuf>,
    /// The Trash button was pressed: the app asks for confirmation, then moves the project to
    /// the Trash and goes back to the toolbar.
    pub remove_requested: bool,
    /// The project was autosaved since the QuickLook preview was last written; leaving the
    /// editor writes a fresh one so the recent-projects thumbnail shows the edits.
    preview_stale: bool,
    /// Captions panel: Whisper transcription running in the background.
    pub transcribe: crate::captions::TranscribeJob,
    /// Narration panel: the AI voice's downloads, previews and readings.
    pub narration: crate::narration::NarrationJob,
    /// Crop tool open: the preview shows the whole recording with the crop frame over it.
    pub crop: Option<crate::crop::CropEditor>,
    /// "Save preset as…", "Rename preset" or "Delete preset" dialog.
    preset_dialog: Option<PresetDialog>,
    /// Agent page of the sidebar: chat that edits the project.
    pub agent: crate::agent_panel::AgentPanel,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PreviewQuality {
    Full,
    Performance,
}

impl Editor {
    /// The slow part of opening a project (package, cursor track, sprites); safe to run on a
    /// background thread so the "Opening …" screen keeps animating.
    pub fn load_session(path: &Path, gpu: Gpu) -> Result<Session> {
        let mut session = Session::open(path, gpu)?;
        session.decode_max_width = Some(1440);
        Ok(session)
    }

    /// Finish opening on the UI thread with a session from `load_session`.
    pub fn with_session(path: &Path, mut session: Session, gpu: Gpu) -> Self {
        // older projects (and Screen Studio ones) can have audio items overlapping on a lane:
        // give each its own before anything is drawn (saved with the next edit)
        let timeline = session.timeline.clone();
        sf_engine::lanes::settle(&mut session.pkg.project, &timeline);
        let preview_size = session.output_size(1080);
        let compositor = Compositor::new(gpu);
        let audio = AudioPlayer::new();
        audio.load(&session);
        let session_crop = session.pkg.project.config.crop.rect01;
        let start_ms = session.pkg.project.data.get("last-playback-position-source-time-ms").and_then(|v| v.as_f64()).and_then(|src| session.timeline.source_to_output(src)).unwrap_or(0.0);
        Editor {
            fresh_recording: false,
            path: path.to_path_buf(),
            session,
            compositor,
            texture_id: None,
            texture_key: None,
            preview_size,
            playhead_ms: start_ms,
            playing: false,
            last_tick: None,
            last_rendered_ms: None,
            selected_slice: None,
            selected_zoom: None,
            selected_mask: None,
            selected_clip: None,
            rail_fit_frames: 0,
            layers: Default::default(),
            mask_drag: None,
            selected_audio: None,
            selected_music: None,
            selected_voice_over: None,
            voice_rec: None,
            voice_mic: None,
            voice_mics: None,
            voice_hud: false,
            voice_meter: None,
            voice_levels: Default::default(),
            voice_levels_at: None,
            unsplash: Default::default(),
            music: Default::default(),
            dirty: false,
            last_edit: None,
            timeline: TimelineWidget::default(),
            export: ExportDialog::new(),
            undo: vec![],
            redo: vec![],
            status: None,
            section: match std::env::var("SF_START_SECTION").as_deref() {
                Ok("audio") => Section::Audio,
                Ok("narration") => Section::Narration,
                Ok("captions") => Section::Captions,
                Ok("cursor") => Section::Cursor,
                Ok("animations") => Section::Animations,
                Ok("shortcuts") => Section::Shortcuts,
                Ok("camera-layout") => Section::CameraLayout,
                Ok("camera-style") => Section::CameraStyle,
                Ok("agent") => Section::Agent,
                Ok("device") => Section::DeviceFrame,
                Ok("general") => Section::General,
                // the agent is the first thing to see (it offers the upgrade without Pro)
                _ => Section::Agent,
            },
            sidebar_open: true,
            audio,
            preview_quality: PreviewQuality::Full,
            preview_scale: 1.0,
            title_edit: None,
            renamed_to: None,
            remove_requested: false,
            preview_stale: false,
            transcribe: Default::default(),
            narration: Default::default(),
            // SF_START_CROP opens the crop tool straight away (for screenshots)
            crop: std::env::var_os("SF_START_CROP").map(|_| crop::CropEditor::new(session_crop)),
            preset_dialog: None,
            agent: Default::default(),
        }
    }

    pub fn duration_ms(&self) -> f64 {
        self.session.duration_ms()
    }

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), Instant::now()));
    }

    /// Re-render the preview on the next frame (after a live edit).
    pub fn invalidate_preview(&mut self) {
        self.last_rendered_ms = None;
        self.mark_dirty();
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.last_edit = Some(Instant::now());
    }

    /// Snapshot for undo, then apply an edit to the project and rebuild derived state.
    pub fn edit(&mut self, f: impl FnOnce(&mut sf_project::Project)) {
        self.undo.push(self.session.pkg.project.clone());
        self.redo.clear();
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        f(&mut self.session.pkg.project);
        self.after_edit();
    }

    /// Push one undo entry for a finished live edit (a drag that changed the project as it went):
    /// restore the snapshot from before it and re-apply the final state through `edit`.
    pub fn commit_live_edit(&mut self, snapshot: sf_project::Project) {
        let current = self.session.pkg.project.clone();
        if current == snapshot {
            return;
        }
        self.session.pkg.project = snapshot;
        self.edit(move |p| *p = current);
    }

    /// Replace the project with the agent's edit while its turn runs: no undo entry (the
    /// turn becomes one with `commit_live_edit` when it ends).
    pub fn apply_agent_project(&mut self, project: sf_project::Project) {
        self.session.pkg.project = project;
        self.after_edit();
    }

    /// Bring the sidebar's inspector into view: the Agent page has none, so it moves to the
    /// General page, and a hidden sidebar opens.
    pub fn show_inspector(&mut self) {
        if self.section == Section::Agent {
            self.section = Section::General;
        }
        self.sidebar_open = true;
    }

    /// Select a title or overlay (and nothing else), with its settings in the sidebar.
    pub fn select_clip(&mut self, id: String) {
        if self.selected_clip.as_deref() != Some(id.as_str()) {
            self.show_inspector();
        }
        self.selected_clip = Some(id);
        self.selected_mask = None;
        self.selected_zoom = None;
        self.selected_slice = None;
        self.selected_audio = None;
        self.selected_music = None;
        self.selected_voice_over = None;
    }

    /// Select clip `index` of the main track (and nothing else).
    pub fn select_slice(&mut self, index: usize) {
        self.selected_slice = Some(index);
        self.selected_clip = None;
        self.selected_mask = None;
        self.selected_zoom = None;
        self.selected_audio = None;
        self.selected_music = None;
        self.selected_voice_over = None;
    }

    /// Select a mask (and nothing else).
    pub fn select_mask(&mut self, id: String) {
        self.selected_mask = Some(id);
        self.selected_clip = None;
        self.selected_zoom = None;
        self.selected_slice = None;
        self.selected_audio = None;
        self.selected_music = None;
        self.selected_voice_over = None;
    }

    /// `edit` for a speed change: the playhead stays on the same source frame (and so the same
    /// spot on the timeline, since clips keep their width), instead of on the same output time,
    /// which a faster or slower clip moves to a different frame.
    pub fn edit_speed(&mut self, f: impl FnOnce(&mut sf_project::Project)) {
        let src = self.session.timeline.output_to_source(self.playhead_ms);
        self.edit(f);
        if let Some(ms) = src.and_then(|s| self.session.timeline.source_to_output(s)) {
            self.playhead_ms = ms.clamp(0.0, self.duration_ms());
            if self.playing {
                self.audio.play_from(self.playhead_ms);
            } else {
                self.audio.seek(self.playhead_ms);
            }
        }
    }

    /// A transcript was (re)generated: reload the captions and redraw the preview.
    pub fn captions_changed(&mut self) {
        self.session.reload_captions();
        self.last_rendered_ms = None;
    }

    fn after_edit(&mut self) {
        self.session.rebuild();
        // keep the shared audio lanes valid: nothing overlaps on a lane, no empty lanes
        let timeline = self.session.timeline.clone();
        sf_engine::lanes::settle(&mut self.session.pkg.project, &timeline);
        self.preview_size = self.session.output_size(1080);
        self.mark_dirty();
        self.last_rendered_ms = None;
        self.playhead_ms = self.playhead_ms.min(self.duration_ms());
        self.audio.update_timeline(&self.session);
    }

    /// Decode the audio tracks again (after mute changes).
    pub fn reload_audio(&mut self) {
        self.audio.load(&self.session);
    }

    /// Set every clip (and detached audio) to `speed`. Clips keep their width on the timeline
    /// and show the speed as a label; the playhead crosses them faster.
    pub fn set_recording_speed(&mut self, speed: f64) {
        use sf_project::SceneAudioExt;
        let speed = speed.clamp(0.25, 16.0);
        self.edit_speed(move |p| {
            let scene = &mut p.scenes[0];
            let n = scene.slices.len();
            let mut slices = scene.slices.clone();
            for i in 0..n {
                slices = sf_engine::timeline::edit::set_speed(&slices, i, speed);
            }
            scene.slices = slices;
            if let Some(mut cs) = scene.audio_clips() {
                for c in cs.iter_mut() {
                    let ratio = c.speed / speed; // keep clips at the same relative place
                    c.output_start_ms *= ratio;
                    c.speed = speed;
                }
                scene.set_audio_clips(Some(cs));
            }
        });
        self.set_status(&format!("Speed {speed}× for the whole recording"));
    }

    /// The speed shared by all clips, or None when they differ.
    pub fn recording_speed(&self) -> Option<f64> {
        let slices = &self.session.pkg.project.scenes[0].slices;
        let first = slices.first().map(|s| 1.0 / s.time_scale.max(1e-6))?;
        slices.iter().all(|s| (1.0 / s.time_scale.max(1e-6) - first).abs() < 1e-3).then_some(first)
    }

    /// Change one audio clip's speed (independent of the video clip speeds).
    pub fn set_audio_clip_speed(&mut self, id: &str, speed: f64) {
        use sf_project::SceneAudioExt;
        let idc = id.to_string();
        self.edit(move |p| {
            let scene = &mut p.scenes[0];
            if let Some(mut cs) = scene.audio_clips() {
                if let Some(c) = cs.iter_mut().find(|c| c.id == idc) {
                    c.speed = speed.clamp(0.25, 16.0);
                }
                scene.set_audio_clips(Some(cs));
            }
        });
        self.set_status(&format!("Audio clip speed {speed}×"));
    }

    /// Split a detached audio clip at output time `at_ms`; with `next_layer` the right-hand
    /// part goes one lane down so the pieces can overlap.
    pub fn split_audio_clip(&mut self, id: &str, at_ms: f64, next_layer: bool) {
        use sf_project::SceneAudioExt;
        let idc = id.to_string();
        let mut done = false;
        self.edit(|p| {
            let scene = &mut p.scenes[0];
            if let Some(mut cs) = scene.audio_clips() {
                done = sf_project::audio_clips::split_clip(&mut cs, &idc, at_ms, next_layer).is_some();
                scene.set_audio_clips(Some(cs));
            }
        });
        self.set_status(match (done, next_layer) {
            (true, true) => "Split audio — right part moved to the layer below",
            (true, false) => "Split audio",
            (false, _) => "Too close to the clip edge to split",
        });
    }

    /// The selected item on the shared audio lanes, as its lane key (`sf_engine::lanes::key`).
    pub fn selected_lane_key(&self) -> Option<String> {
        use sf_engine::lanes::{key, Kind};
        self.selected_audio.as_deref().map(|id| key(Kind::Recorded, id)).or_else(|| self.selected_voice_over.as_deref().map(|id| key(Kind::Voice, id))).or_else(|| self.selected_music.as_deref().map(|id| key(Kind::Music, id)))
    }

    /// Move the audio item `key` (recorded clip, voice-over or music clip) up (`delta` < 0) or
    /// down (`delta` > 0) by whole lanes; one lane past the bottom one creates a new lane, and
    /// lanes left empty collapse.
    pub fn move_lane_item(&mut self, key: &str, delta: i32) {
        let (timeline, key) = (self.session.timeline.clone(), key.to_string());
        let mut layer = None;
        self.edit(|p| layer = sf_engine::lanes::move_layer(p, &timeline, &key, delta));
        match layer {
            Some(l) => self.set_status(&format!("Moved to layer {}", l + 1)),
            None => self.set_status(if delta > 0 { "No room on the layer below" } else { "No room on the layer above" }),
        }
    }

    /// Select a background music clip (and nothing else).
    pub fn select_music(&mut self, id: String) {
        self.selected_clip = None;
        self.selected_music = Some(id);
        self.selected_zoom = None;
        self.selected_slice = None;
        self.selected_mask = None;
        self.selected_audio = None;
        self.selected_voice_over = None;
    }

    /// Remove the background music (track and clips).
    pub fn remove_music(&mut self) {
        use sf_project::SceneMusicExt;
        self.selected_music = None;
        self.edit(|p| {
            p.config.audio.background_audio_file_name = None;
            p.scenes[0].set_music_clips(None);
        });
        self.reload_audio();
    }

    /// Change one music clip through `edit` (the whole-video music becomes a clip first).
    pub fn edit_music_clip(&mut self, id: &str, f: impl FnOnce(&mut sf_project::MusicClip)) {
        use sf_project::SceneMusicExt;
        let mut clips = sf_engine::lanes::music_clips(&self.session.pkg.project, &self.session.timeline);
        let Some(c) = clips.iter_mut().find(|c| c.id == id) else { return };
        f(c);
        self.edit(move |p| p.scenes[0].set_music_clips(Some(clips)));
    }

    /// Split music clip `id` at output time `t`; the right-hand part is selected.
    pub fn split_music(&mut self, id: &str, t: f64) {
        use sf_project::SceneMusicExt;
        let mut clips = sf_engine::lanes::music_clips(&self.session.pkg.project, &self.session.timeline);
        let Some(i) = clips.iter().position(|c| c.id == id) else { return };
        match clips[i].split_at_output(t) {
            Some(right) => {
                let rid = right.id.clone();
                clips.insert(i + 1, right);
                self.edit(move |p| p.scenes[0].set_music_clips(Some(clips)));
                self.select_music(rid);
                self.set_status("Split music");
            }
            None => self.set_status("Move the playhead inside the music clip to split it"),
        }
    }

    /// Push volume/mute changes to the audio player without re-decoding.
    pub fn refresh_audio_levels(&mut self) {
        self.audio.update_timeline(&self.session);
    }

    /// Start playback so a newly chosen background track is heard right away.
    pub fn preview_background_audio(&mut self) {
        // music clips keep their place when the track changes: with none under the playhead,
        // play from the next clip (or the first one) instead of silence
        let clips = sf_engine::lanes::music_clips(&self.session.pkg.project, &self.session.timeline);
        let t = self.playhead_ms;
        let jump = (!clips.iter().any(|c| c.contains_output(t))).then(|| clips.iter().map(|c| c.output_start_ms).filter(|s| *s >= t).fold(None, |m: Option<f64>, s| Some(m.map_or(s, |m| m.min(s)))).or_else(|| clips.iter().map(|c| c.output_start_ms).reduce(f64::min))).flatten();
        if let Some(start) = jump {
            self.pause();
            self.seek(start);
        }
        if !self.playing {
            self.toggle_play();
        } else {
            self.audio.play_from(self.playhead_ms);
        }
        let name = self.session.pkg.project.config.audio.background_audio_file_name.clone().unwrap_or_default();
        let label = sf_session::assets::background_audio_label(&name);
        match jump {
            Some(start) => self.set_status(&format!("Background music: {label} (its clip starts at {})", fmt_clock_short(start))),
            None => self.set_status(&format!("Background music: {label}")),
        }
    }

    /// Start recording a voice-over at the playhead: the video plays without sound (so the
    /// microphone hears only you) until it ends or the recording is stopped.
    pub fn start_voice_over(&mut self) {
        if self.voice_rec.is_some() || self.export.is_busy() {
            return;
        }
        if self.playhead_ms >= self.duration_ms() - 500.0 {
            self.seek(0.0);
        }
        let src = self.session.timeline.output_to_source(self.playhead_ms).unwrap_or(0.0);
        let name = match &self.voice_mic {
            Some(m) => m.name.clone(),
            None => crate::voice_over::default_label(self.voice_mics.get_or_insert_with(crate::voice_over::microphones)),
        };
        // the bar's level meter lets go of the microphone before the take opens it
        self.voice_meter = None;
        match crate::voice_over::VoiceRecorder::start(src, self.voice_mic.as_ref(), name) {
            Ok(rec) => {
                self.voice_hud = true;
                self.set_status(format!("Recording voice-over from {}", rec.device_name));
                self.voice_rec = Some(rec);
                self.audio.set_volume(0.0);
                self.pause();
                self.toggle_play();
            }
            Err(e) => self.set_status(format!("Could not start the microphone: {e}")),
        }
    }

    /// Microphone rows for the voice-over menus: the system default, then each microphone.
    pub fn voice_mic_rows(&mut self, ui: &mut egui::Ui) {
        let mics = self.voice_mics.get_or_insert_with(crate::voice_over::microphones).clone();
        if crate::widgets::menu_item(ui, self.voice_mic.is_none(), crate::voice_over::default_label(&mics)).clicked() {
            self.voice_mic = None;
        }
        // Linux lists the system default as its own entry: that is the row above
        for m in mics.iter().filter(|m| m.id != "default") {
            if crate::widgets::menu_item(ui, self.voice_mic.as_ref().is_some_and(|v| v.id == m.id), &m.name).clicked() {
                self.voice_mic = Some(m.clone());
            }
        }
    }

    /// Show the voice-over bar over the preview, with the microphones listed again in case one
    /// was plugged in.
    pub fn open_voice_over_hud(&mut self) {
        self.voice_hud = true;
        self.voice_mics = Some(crate::voice_over::microphones());
    }

    /// Stop the voice-over recording and place the take on the timeline.
    pub fn stop_voice_over(&mut self) {
        let Some(rec) = self.voice_rec.take() else { return };
        self.pause();
        self.audio.set_volume(1.0);
        let (start, device) = (rec.source_start_ms, rec.device_name.clone());
        let dir = self.session.pkg.path.join(sf_project::v4::VoiceOver::DIR);
        match rec.finish(&dir) {
            Ok(Some(take)) => {
                let (silent, clipped) = (take.silent, take.clipped);
                let vo = sf_project::v4::VoiceOver::microphone(start, take.duration_ms, take.file_name);
                let id = vo.id.clone();
                self.edit(move |p| p.scenes[0].voice_overs.push(vo));
                self.audio.load_voice_overs(&self.session);
                self.select_voice_over(id);
                if silent {
                    // kept so it can be checked, but say why it will play back as nothing
                    self.set_status(format!("Voice-over is silent: {device} sent no voice (muted, input volume too low, or in use by a call?). Choose another microphone in Audio, or raise its input volume in the system sound settings."));
                } else if clipped {
                    // the peaks are gone from the file: boosting or filtering cannot bring them back
                    self.set_status(format!("Added voice-over, but it is distorted: {device} was too loud and cut off the peaks. Lower its input volume in the system sound settings (or move back from it) and record again."));
                } else {
                    self.set_status("Added voice-over");
                }
            }
            Ok(None) => self.set_status("Voice-over too short: discarded"),
            Err(e) => self.set_status(format!("Could not save the voice-over: {e}")),
        }
    }

    /// Put takes read by the AI voice on the timeline in one edit (see
    /// [`crate::narration::place`]) and select the take when there is one.
    fn place_narration(&mut self, made: Vec<crate::narration::Made>) {
        let scene = &self.session.pkg.project.scenes[0];
        // a take deleted while it was being read is not brought back
        let (made, gone): (Vec<_>, Vec<_>) = made.into_iter().partition(|m| m.replaces.as_ref().is_none_or(|id| scene.voice_overs.iter().any(|v| &v.id == id)));
        if made.is_empty() {
            if !gone.is_empty() {
                self.set_status("The take was deleted while it was being read");
            }
            return;
        }
        let (n, replaced) = (made.len(), made[0].replaces.is_some());
        let mut ids = vec![];
        let mut project = self.session.pkg.project.clone();
        ids.extend(crate::narration::place(&mut project, made));
        self.edit(move |p| *p = project);
        if let [id] = ids.as_slice() {
            self.select_voice_over(id.clone());
        }
        self.set_status(match (n, replaced) {
            (1, false) => "Added the narration".to_string(),
            (1, true) => "Read the take again".to_string(),
            (n, _) => format!("Read {n} narration takes again"),
        });
    }

    /// Cut voice-over `id` at output time `t`; the right-hand part is selected.
    pub fn split_voice_over(&mut self, id: &str, t: f64) {
        let mut scene = self.session.pkg.project.scenes[0].clone();
        match sf_engine::audio::split_voice_over(&mut scene, &self.session.timeline, id, t) {
            Some(right) => {
                self.edit(move |p| p.scenes[0] = scene);
                self.select_voice_over(right);
                self.set_status("Split voice-over");
            }
            None => self.set_status("Move the playhead inside the voice-over to split it"),
        }
    }

    /// Select a voice-over (and nothing else).
    pub fn select_voice_over(&mut self, id: String) {
        self.selected_clip = None;
        self.selected_voice_over = Some(id);
        self.selected_zoom = None;
        self.selected_slice = None;
        self.selected_mask = None;
        self.selected_audio = None;
        self.selected_music = None;
    }

    /// Change one voice-over through `edit` (no-op when it is gone).
    pub fn edit_voice_over(&mut self, id: &str, f: impl FnOnce(&mut sf_project::v4::VoiceOver)) {
        let id = id.to_string();
        self.edit(move |p| {
            if let Some(v) = p.scenes[0].voice_overs.iter_mut().find(|v| v.id == id) {
                f(v);
            }
        });
    }

    pub fn undo(&mut self) {
        if let Some(p) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.session.pkg.project, p));
            self.after_edit();
        }
    }

    pub fn redo(&mut self) {
        if let Some(p) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.session.pkg.project, p));
            self.after_edit();
        }
    }

    /// Write unsaved changes before the editor goes away (back to projects, another project,
    /// quit). Skipped when the package is gone, e.g. it was just moved to the Trash.
    pub fn save_if_dirty(&mut self) {
        if (self.dirty || self.preview_stale) && self.path.exists() {
            self.save();
        }
    }

    /// Autosave once editing has paused (no drag in progress), so changes survive a crash or a
    /// force quit too. Only the project files are written; the QuickLook preview is left for
    /// explicit saves and leaving the editor (`preview_stale`).
    fn autosave(&mut self, ctx: &egui::Context) {
        const DELAY: std::time::Duration = std::time::Duration::from_millis(1500);
        let Some(t) = self.last_edit.filter(|_| self.dirty) else { return };
        let since = t.elapsed();
        if since < DELAY || ctx.input(|i| i.pointer.any_down()) || !self.path.exists() {
            ctx.request_repaint_after(DELAY.saturating_sub(since).max(std::time::Duration::from_millis(100)));
            return;
        }
        self.write_project(false);
    }

    pub fn save(&mut self) {
        self.write_project(true);
    }

    fn write_project(&mut self, explicit: bool) {
        let src = self.session.timeline.output_to_source(self.playhead_ms).unwrap_or(0.0);
        self.session.pkg.project.data.insert("last-playback-position-source-time-ms".into(), serde_json::json!(src));
        match self.session.pkg.save() {
            Ok(()) => {
                self.dirty = false;
                if explicit {
                    sf_session::preview::write_quicklook_preview_async(&self.session.pkg.path);
                    self.preview_stale = false;
                    self.set_status("Saved");
                } else {
                    self.preview_stale = true;
                }
            }
            Err(e) => self.set_status(format!("Save failed: {e}")),
        }
    }

    fn render_preview(&mut self, rs: &egui_wgpu::RenderState) {
        if self.last_rendered_ms == Some(self.playhead_ms) {
            return;
        }
        let height = match self.preview_quality {
            PreviewQuality::Full => 1080,
            PreviewQuality::Performance => 720,
        };
        self.preview_size = if self.crop.is_some() { self.session.source_size(height) } else { self.session.output_size(height) };
        // while a sped-up clip plays, decode only the frames that will be shown
        self.session.decode_speed = if self.playing { self.session.timeline.speed_at_output(self.playhead_ms) } else { 1.0 };
        let rendered = if self.crop.is_some() {
            self.session.source_frame_desc_at(self.playhead_ms, self.preview_size).map(|desc| self.compositor.render(&desc))
        } else {
            // through the session, so a transition finds the frame it blends from
            self.session.render(&mut self.compositor, self.playhead_ms, self.preview_size)
        };
        match rendered {
            Ok(()) => {
                self.last_rendered_ms = Some(self.playhead_ms);
            }
            Err(e) => {
                self.set_status(format!("Preview error: {e}"));
                self.last_rendered_ms = Some(self.playhead_ms);
            }
        }
        if let Some(t) = self.compositor.target() {
            let key = (t.width, t.height);
            let mut renderer = rs.renderer.write();
            match self.texture_id {
                Some(_) if self.texture_key == Some(key) => {}
                Some(id) => {
                    renderer.update_egui_texture_from_wgpu_texture(&rs.device, &t.view, wgpu::FilterMode::Linear, id);
                    self.texture_key = Some(key);
                }
                None => {
                    self.texture_id = Some(renderer.register_native_texture(&rs.device, &t.view, wgpu::FilterMode::Linear));
                    self.texture_key = Some(key);
                }
            }
        }
    }

    /// Stop playback, audio included (scrubbing or jumping must not leave the music running
    /// and skipping along with the playhead).
    pub fn pause(&mut self) {
        self.playing = false;
        self.audio.pause();
    }

    pub fn toggle_play(&mut self) {
        if self.voice_rec.is_some() && self.playing {
            self.stop_voice_over();
            return;
        }
        if self.playing {
            self.playing = false;
            self.audio.pause();
        } else {
            if self.playhead_ms >= self.duration_ms() - 1.0 {
                self.playhead_ms = 0.0;
            }
            self.playing = true;
            self.last_tick = None;
            self.audio.play_from(self.playhead_ms);
        }
    }

    pub fn seek(&mut self, ms: f64) {
        self.playhead_ms = ms.clamp(0.0, self.duration_ms());
        self.audio.seek(self.playhead_ms);
    }

    fn tick(&mut self, ctx: &egui::Context) {
        self.autosave(ctx);
        // audio decodes in the background: keep drawing until the waveform can show
        if self.audio.is_loading() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.playing {
            if self.audio.has_audio() && self.audio.available {
                self.playhead_ms = self.audio.position_ms();
                if !self.audio.is_playing() {
                    self.playing = false;
                }
            } else {
                let now = Instant::now();
                if let Some(prev) = self.last_tick {
                    self.playhead_ms += now.duration_since(prev).as_secs_f64() * 1000.0;
                }
                self.last_tick = Some(now);
            }
            if self.playhead_ms >= self.duration_ms() {
                self.playhead_ms = self.duration_ms();
                self.playing = false;
                self.audio.pause();
            }
            ctx.request_repaint();
        }
        // the video reached its end (or playback stopped some other way): keep the take
        if self.voice_rec.is_some() && !self.playing {
            self.stop_voice_over();
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // the export dialog is modal: its own keys (Esc) are handled in ExportDialog::poll
        // only typing in a text field (agent message, title, search) holds the keys back: a
        // clicked button or slider also takes focus, and must not swallow Space and the rest
        if ctx.egui_wants_keyboard_input() || self.export.open || self.export.is_busy() {
            return;
        }
        // while a voice-over records, Space and Esc stop it and nothing else edits the project
        if self.voice_rec.is_some() {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space) || i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                self.stop_voice_over();
            }
            return;
        }
        if self.voice_hud && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.voice_hud = false;
        }
        let frame = 1000.0 / 60.0;
        let (mut split, mut del, mut undo, mut redo, mut save, mut export, mut prev_b, mut next_b, mut toggle) = (false, false, false, false, false, false, false, false, false);
        let mut toggle_sidebar = false;
        let mut section: Option<Section> = None;
        let mut layer_delta = 0i32;
        let mut record = false;
        let mut agent = false;
        let mut tool: Option<RailTool> = None;
        ctx.input_mut(|i| {
            agent = i.consume_key(egui::Modifiers::COMMAND, egui::Key::L);
            if i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowDown) { layer_delta = 1; }
            if i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowUp) { layer_delta = -1; }
            toggle = i.consume_key(egui::Modifiers::NONE, egui::Key::Space);
            record = i.consume_key(egui::Modifiers::NONE, egui::Key::R);
            // the rail's tools: T title, I image or video, M mask
            for (key, t) in [(egui::Key::T, RailTool::Title), (egui::Key::I, RailTool::Media), (egui::Key::M, RailTool::Mask)] {
                if i.consume_key(egui::Modifiers::NONE, key) {
                    tool = Some(t);
                }
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) { self.playhead_ms = (self.playhead_ms - frame).max(0.0); }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) { self.playhead_ms = (self.playhead_ms + frame).min(self.duration_ms()); }
            if i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowLeft) { self.playhead_ms = (self.playhead_ms - 1000.0).max(0.0); }
            if i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowRight) { self.playhead_ms = (self.playhead_ms + 1000.0).min(self.duration_ms()); }
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::ArrowLeft) || i.consume_key(egui::Modifiers::NONE, egui::Key::Home) { self.playhead_ms = 0.0; }
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::ArrowRight) || i.consume_key(egui::Modifiers::NONE, egui::Key::End) { self.playhead_ms = self.duration_ms(); }
            split = i.consume_key(egui::Modifiers::NONE, egui::Key::S) || i.consume_key(egui::Modifiers::NONE, egui::Key::C) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::B);
            del = i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace);
            undo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
            redo = i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z);
            save = i.consume_key(egui::Modifiers::COMMAND, egui::Key::S);
            export = i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::E) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::E);
            prev_b = i.consume_key(egui::Modifiers::NONE, egui::Key::OpenBracket);
            next_b = i.consume_key(egui::Modifiers::NONE, egui::Key::CloseBracket);
            toggle_sidebar = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Backslash);
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Equals) { self.timeline.px_per_sec = (self.timeline.px_per_sec * 1.25).min(400.0); }
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Minus) { self.timeline.px_per_sec = (self.timeline.px_per_sec / 1.25).max(5.0); }
            for (n, key) in [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5, egui::Key::Num7, egui::Key::Num8, egui::Key::Num9, egui::Key::Num0].iter().enumerate() {
                if i.consume_key(egui::Modifiers::COMMAND, *key) {
                    section = Section::ALL.get(n).copied();
                }
            }
        });
        if toggle { self.toggle_play(); }
        if record && self.voice_rec.is_none() { self.start_voice_over(); }
        if let Some(t) = tool { self.run_rail_tool(t); }
        if split { self.split_at_playhead(); }
        if del { self.delete_selected(); }
        if undo { self.undo(); }
        if redo { self.redo(); }
        if save { self.save(); }
        if export { self.export.open = true; }
        if prev_b { crate::timeline::jump_boundary(self, false); }
        if next_b { crate::timeline::jump_boundary(self, true); }
        if toggle_sidebar { self.sidebar_open = !self.sidebar_open; }
        if layer_delta != 0 {
            if let Some(key) = self.selected_lane_key() {
                self.move_lane_item(&key, layer_delta);
            }
        }
        if let Some(s) = section {
            self.section = s;
            self.sidebar_open = true;
        }
        if agent {
            self.section = Section::Agent;
            self.sidebar_open = true;
            self.agent.focus_input = true;
        }
        if !self.playing {
            self.audio.seek(self.playhead_ms);
        }
    }

    pub fn split_at_playhead(&mut self) {
        let t = self.playhead_ms;
        // a selected voice-over under the playhead is cut instead of the video
        if let Some(id) = self.selected_voice_over.clone() {
            if self.session.voice_over_spans().iter().any(|v| v.id == id && v.contains_output(t)) {
                self.split_voice_over(&id, t);
                return;
            }
        }
        if let Some(id) = self.selected_music.clone() {
            if sf_engine::lanes::music_clips(&self.session.pkg.project, &self.session.timeline).iter().any(|m| m.id == id && m.contains_output(t)) {
                self.split_music(&id, t);
                return;
            }
        }
        // Split the selected audio clip only when the playhead is over it; otherwise cut the video.
        let audio_hit = self.selected_audio.clone().filter(|id| {
            use sf_project::SceneAudioExt;
            self.session.pkg.project.scenes[0].audio_clips().is_some_and(|cs| cs.iter().any(|c| &c.id == id && c.contains_output(t)))
        });
        if let Some(id) = audio_hit {
            self.split_audio_clip(&id, t, false);
            return;
        }
        self.edit(|p| {
            let s = &mut p.scenes[0];
            s.slices = sf_engine::timeline::edit::split_at_output(&s.slices, t);
        });
        self.set_status("Split");
    }

    pub fn delete_selected(&mut self) {
        if let Some(id) = self.selected_music.take() {
            use sf_project::SceneMusicExt;
            let mut clips = sf_engine::lanes::music_clips(&self.session.pkg.project, &self.session.timeline);
            clips.retain(|c| c.id != id);
            if clips.is_empty() {
                self.remove_music();
                self.set_status("Removed background music");
            } else {
                self.edit(move |p| p.scenes[0].set_music_clips(Some(clips)));
                self.set_status("Removed music clip");
            }
            return;
        }
        if let Some(id) = self.selected_voice_over.take() {
            self.edit(move |p| p.scenes[0].voice_overs.retain(|v| v.id != id));
            self.set_status("Removed voice-over");
            return;
        }
        if let Some(id) = self.selected_audio.take() {
            self.edit(move |p| {
                use sf_project::SceneAudioExt;
                let scene = &mut p.scenes[0];
                if let Some(mut clips) = scene.audio_clips() {
                    clips.retain(|c| c.id != id);
                    sf_project::audio_clips::compact_layers(&mut clips);
                    scene.set_audio_clips(Some(clips));
                }
            });
            self.set_status("Removed audio clip");
            return;
        }
        if let Some(id) = self.selected_zoom.take() {
            self.edit(move |p| p.scenes[0].zooms.retain(|z| z.id != id));
            self.set_status("Removed zoom");
            return;
        }
        if let Some(id) = self.selected_mask.take() {
            self.edit(move |p| p.scenes[0].masks.retain(|m| m.id != id));
            self.set_status("Removed mask");
            return;
        }
        if let Some(id) = self.selected_clip.clone() {
            crate::layers::remove(self, &id);
            return;
        }
        if let Some(i) = self.selected_slice.take() {
            let slices = &self.session.pkg.project.scenes[0].slices;
            if slices.get(i).is_some_and(|s| s.is_card()) {
                crate::layers::remove_card(self, i);
            } else if slices.iter().filter(|s| !s.is_card()).count() > 1 {
                self.edit(|p| {
                    let s = &mut p.scenes[0];
                    s.slices = sf_engine::timeline::edit::delete(&s.slices, i);
                });
                self.set_status("Deleted clip");
            }
        }
    }

    /// Once, a few frames after the project opens (when the window has its editor size and
    /// the lanes are laid out): when the rail's buttons do not all fit beside the preview and
    /// the screen has room, make the window taller by what is missing and centre it again.
    /// The user's own resizing is left alone afterwards.
    fn fit_window_to_rail(&mut self, ctx: &egui::Context, stage_h: f32, rail_h: f32) {
        const SETTLE_FRAMES: u8 = 6;
        if self.rail_fit_frames > SETTLE_FRAMES {
            return;
        }
        self.rail_fit_frames += 1;
        if self.rail_fit_frames <= SETTLE_FRAMES || stage_h >= rail_h {
            return;
        }
        let (inner, mon) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().monitor_size));
        let (Some(inner), Some(mon)) = (inner, mon) else { return };
        // the menu bar and a little air above and below
        let h = (inner.height() + rail_h - stage_h).min(mon.y - 80.0);
        if h > inner.height() + 1.0 {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize([inner.width(), h].into()));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition([inner.left(), ((mon.y - h) / 2.0 - 10.0).max(25.0)].into()));
        }
    }

    /// Tooltip of a rail button (the voice-over one says what a click does now).
    fn rail_tooltip(&self, t: RailTool) -> &'static str {
        match t {
            RailTool::VoiceOver if self.voice_rec.is_some() => "Stop voice-over (Space)",
            RailTool::VoiceOver => "Voice-over (R)",
            t => t.tooltip(),
        }
    }

    fn run_rail_tool(&mut self, t: RailTool) {
        match t {
            RailTool::AutoZooms => {
                let zooms = self.session.auto_zooms();
                self.edit(move |p| p.scenes[0].zooms = zooms);
                self.set_status("Generated automatic zooms");
            }
            RailTool::Mask => crate::masks::add_at_playhead(self),
            RailTool::Title => crate::layers::add_title(self),
            RailTool::TitleCard => {
                let index = self.session.timeline.span_at_output(self.playhead_ms).map(|s| s.index).unwrap_or(0);
                crate::layers::add_card(self, index);
            }
            RailTool::Media => crate::layers::add_media(self),
            RailTool::Transition => crate::layers::add_transition(self),
            RailTool::VoiceOver => {
                if self.voice_rec.is_some() {
                    self.stop_voice_over();
                } else if self.voice_hud {
                    self.voice_hud = false;
                } else {
                    self.open_voice_over_hud();
                }
            }
        }
    }

    /// The rail's buttons in `slots` places: every tool when they fit, else the first ones and
    /// a "More tools" arrow last, whose menu lists the rest.
    fn rail_buttons(&mut self, ui: &mut egui::Ui, slots: usize) {
        let shown = if slots >= RAIL_TOOLS.len() { RAIL_TOOLS.len() } else { slots.saturating_sub(1) };
        let mut run = None;
        // the Text button offers both kinds of title, as editors' Text tools do
        let choices = |t: RailTool| if t == RailTool::Title { &TEXT_CHOICES[..] } else { std::slice::from_ref(&RAIL_TOOLS[RAIL_TOOLS.iter().position(|r| *r == t).unwrap_or(0)]) };
        for t in &RAIL_TOOLS[..shown] {
            let on = *t == RailTool::VoiceOver && (self.voice_rec.is_some() || self.voice_hud);
            let tip = if *t == RailTool::Title { "Text (T)" } else { self.rail_tooltip(*t) };
            let b = widgets::rail_icon(ui, t.icon(), on, tip);
            // a thin divider in the gap after the last button of a group (not after the last shown)
            if RAIL_GROUP_ENDS.contains(t) && RAIL_TOOLS[..shown].last() != Some(t) {
                let y = b.rect.bottom() + ui.spacing().item_spacing.y / 2.0;
                ui.painter().line_segment([pos2(b.rect.left() + 8.0, y), pos2(b.rect.right() - 8.0, y)], Stroke::new(1.0, theme::p().border));
            }
            if *t == RailTool::Title {
                egui::Popup::menu(&b).show(|ui| {
                    for k in &TEXT_CHOICES {
                        if widgets::icon_text_button(ui, Some(k.icon()), k.label(), false).on_hover_text(self.rail_tooltip(*k)).clicked() {
                            run = Some(*k);
                        }
                    }
                });
            } else if b.clicked() {
                run = Some(*t);
            }
        }
        if shown < RAIL_TOOLS.len() {
            let more = widgets::rail_icon(ui, widgets::Icon::ChevronDown, false, "More tools");
            egui::Popup::menu(&more).show(|ui| {
                for k in RAIL_TOOLS[shown..].iter().flat_map(|t| choices(*t)) {
                    if widgets::icon_text_button(ui, Some(k.icon()), k.label(), false).on_hover_text(self.rail_tooltip(*k)).clicked() {
                        run = Some(*k);
                    }
                }
            });
        }
        if let Some(t) = run {
            self.run_rail_tool(t);
        }
    }

    /// Returns true when the user wants to go back to the picker.
    pub fn ui(&mut self, root: &mut egui::Ui, rs: &egui_wgpu::RenderState) -> bool {
        let ctx = &root.ctx().clone();
        let mut go_home = false;
        self.tick(ctx);
        self.shortcuts(ctx);
        let mut agent = std::mem::take(&mut self.agent);
        agent.poll(self);
        self.agent = agent;
        if self.transcribe.poll() {
            self.captions_changed();
        }
        if self.transcribe.is_running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        if let Some(made) = self.narration.poll() {
            self.place_narration(made);
        }
        if self.narration.working().is_some() || self.narration.is_playing() {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }

        // ── title bar ──
        egui::Panel::top("titlebar").exact_size(56.0).frame(egui::Frame::default().fill(theme::p().bg).inner_margin(egui::Margin::symmetric(16, 0))).show(root, |ui| {
            let rect = ui.max_rect();
            let name = self.session.pkg.project.name.clone();
            if let Some(buf) = self.title_edit.as_mut() {
                let edit_rect = Rect::from_center_size(rect.center(), vec2(420.0, 30.0));
                let resp = ui.put(edit_rect, egui::TextEdit::singleline(buf).font(FontId::proportional(15.0)).horizontal_align(egui::Align::Center).desired_width(420.0));
                resp.request_focus();
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if escape {
                    self.title_edit = None;
                } else if enter || (resp.lost_focus() && !escape) {
                    let new_name = self.title_edit.take().unwrap_or_default();
                    self.rename_project(&new_name);
                }
            } else {
                let painter = ui.painter();
                let galley = painter.layout_no_wrap(name.clone(), FontId::proportional(15.0), theme::p().text);
                let ext = painter.layout_no_wrap(format!(".{}", sf_project::PACKAGE_EXTENSION), FontId::proportional(15.0), theme::p().text_faint);
                let total = galley.size().x + ext.size().x;
                let x0 = rect.center().x - total / 2.0;
                let title_rect = Rect::from_min_size(pos2(x0 - 8.0, rect.center().y - 14.0), vec2(total + 16.0, 28.0));
                let resp = ui.interact(title_rect, ui.id().with("title"), Sense::click()).on_hover_cursor(egui::CursorIcon::Text).on_hover_text("Click to rename the project");
                if resp.hovered() {
                    painter.rect_filled(title_rect, CornerRadius::same(6), theme::p().card_2);
                }
                painter.galley(pos2(x0, rect.center().y - galley.size().y / 2.0), galley, theme::p().text);
                painter.galley(pos2(x0 + total - ext.size().x, rect.center().y - ext.size().y / 2.0), ext, theme::p().text_faint);
                if self.dirty {
                    painter.circle_filled(pos2(x0 + total + 10.0, rect.center().y), 3.0, theme::p().accent_hover);
                }
                if resp.clicked() {
                    self.title_edit = Some(name.clone());
                }
            }
            ui.horizontal_centered(|ui| {
                if cfg!(target_os = "macos") {
                    ui.add_space(72.0); // traffic lights float over the toolbar
                }
                if widgets::icon_button_tinted(ui, widgets::Icon::Folder, 18.0, "Projects", theme::p().accent).clicked() {
                    go_home = true;
                }
                if widgets::icon_button_tinted(ui, widgets::Icon::Trash, 18.0, "Move project to Trash", theme::p().danger).clicked() && !self.export.is_busy() {
                    self.remove_requested = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let tip = if self.sidebar_open { "Hide sidebar (⌘\\)" } else { "Show sidebar (⌘\\)" };
                    if widgets::icon_button(ui, widgets::Icon::Sidebar, 18.0, tip).clicked() {
                        self.sidebar_open = !self.sidebar_open;
                    }
                    ui.add_space(6.0);
                    if widgets::accent_icon_button(ui, widgets::Icon::Export, "Export (⌘E)").clicked() {
                        self.export.open = true;
                    }
                    ui.add_space(6.0);
                    let presets = widgets::icon_text_button(ui, Some(widgets::Icon::Sparkle), "Presets", true);
                    egui::Popup::menu(&presets).show(|ui| self.presets_menu(ui));
                    ui.add_space(6.0);
                    let can_redo = !self.redo.is_empty();
                    let can_undo = !self.undo.is_empty();
                    let redo = widgets::icon_button(ui, widgets::Icon::Redo, 18.0, "Redo (⌘⇧Z)");
                    if redo.clicked() && can_redo {
                        self.redo();
                    }
                    let undo = widgets::icon_button(ui, widgets::Icon::Undo, 18.0, "Undo (⌘Z)");
                    if undo.clicked() && can_undo {
                        self.undo();
                    }
                    if let Some((version, url)) = crate::update::available() {
                        if widgets::accent_badge(ui, &format!("Update v{version}")).on_hover_text(format!("ScreenForge v{version} is out: open the release page to download it")).clicked() {
                            crate::os::open(url);
                        }
                    }
                    ui.add_space(6.0);
                    widgets::icon_button(ui, widgets::Icon::Command, 16.0, "Shortcuts: Space play · S split · R record voice-over · T title · I image or video · M mask · ⌥↑↓ audio layer · ⌫ delete · ⌘Z undo · ⌘S save · ⌘⇧E export · [ ] jump cuts · ⌘1…⌘0 sidebar · ⌘L agent · ⌘\\ toggle sidebar");
                    if let Some((s, t)) = &self.status {
                        if t.elapsed().as_secs() < 4 {
                            ui.label(RichText::new(s).color(theme::p().text_dim));
                        }
                    }
                });
            });
        });

        // ── right: icon rail + sidebar card (collapsible; dragging it shut also hides it) ──
        let mut sidebar_open = self.sidebar_open;
        egui::Panel::right("sidebar").resizable(true).default_size(430.0).min_size(360.0).frame(egui::Frame::default().fill(theme::p().bg).inner_margin(egui::Margin { left: 0, right: 12, top: 8, bottom: 12 })).show_collapsible(root, &mut sidebar_open, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.add_space(8.0);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    for s in Section::ALL {
                        if widgets::rail_icon(ui, s.icon(), self.section == s, s.title()).clicked() {
                            self.section = s;
                        }
                    }
                });
                let card_rect = ui.available_rect_before_wrap();
                widgets::card(ui.painter(), card_rect);
                let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(card_rect.shrink2(vec2(18.0, 10.0))).layout(egui::Layout::top_down(egui::Align::Min)));
                // nothing in the inspector may paint past the card (the child would otherwise
                // inherit the whole panel's clip rect)
                inner.set_clip_rect(card_rect.shrink2(vec2(6.0, 0.0)).intersect(ui.clip_rect()));
                sidebar::ui(self, &mut inner, self.section);
            });
        });
        self.sidebar_open = sidebar_open;

        // ── bottom: transport + timeline ──
        egui::Panel::bottom("timeline").resizable(true).default_size(280.0).min_size(200.0).frame(egui::Frame::default().fill(theme::p().bg).inner_margin(egui::Margin { left: 12, right: 12, top: 4, bottom: 10 })).show(root, |ui| {
            self.transport_ui(ui);
            ui.add_space(4.0);
            let mut tl = std::mem::take(&mut self.timeline);
            tl.ui(self, ui);
            self.timeline = tl;
        });

        // ── centre: preview toolbar + stage ──
        egui::CentralPanel::default().frame(egui::Frame::default().fill(theme::p().bg).inner_margin(egui::Margin { left: 16, right: 8, top: 4, bottom: 8 })).show(root, |ui| {
            self.preview_toolbar(ui);
            ui.add_space(6.0);
            self.render_preview(rs);
            let stage = ui.available_rect_before_wrap();
            let aspect = self.preview_size.width as f32 / self.preview_size.height.max(1) as f32;
            let mut size = vec2(stage.width(), stage.width() / aspect) * self.preview_scale;
            if size.y > stage.height() {
                size = vec2(stage.height() * aspect, stage.height()) * self.preview_scale;
            }
            let img_rect = Rect::from_center_size(stage.center(), size);
            if let Some(id) = self.texture_id {
                let img = egui::Image::from_texture(egui::load::SizedTexture::new(id, size)).corner_radius(4.0);
                let resp = ui.put(img_rect, img);
                if let Some(mut crop) = self.crop.take() {
                    let action = crop.ui(ui, img_rect);
                    self.crop = Some(crop);
                    match action {
                        Some(crate::crop::Action::Apply) => self.close_crop(true),
                        Some(crate::crop::Action::Cancel) => self.close_crop(false),
                        None => {}
                    }
                } else {
                    if crate::masks::ui(self, ui, img_rect) {
                        self.invalidate_preview();
                    }
                    if crate::layers::preview_ui(self, ui, img_rect) {
                        self.invalidate_preview();
                    }
                    if resp.clicked() {
                        self.toggle_play();
                    }
                    paint_zoom_indicator(ui.painter(), img_rect, self.session.zoom_state());
                }
            } else {
                ui.painter().text(stage.center(), Align2::CENTER_CENTER, "Rendering…", FontId::proportional(14.0), theme::p().text_dim);
            }
            // floating tool rail (left): one column of the buttons that fit beside the preview;
            // on a short window the last place is a "More tools" arrow with the rest in a menu,
            // and with no room for a single button the rail hides
            let (icon, gap, pad) = (40.0, 8.0, 10.0);
            let slots = ((stage.height() - 8.0 - 2.0 * pad + gap) / (icon + gap)).floor().max(0.0) as usize;
            let slots = slots.min(RAIL_TOOLS.len());
            self.fit_window_to_rail(ui.ctx(), stage.height(), RAIL_TOOLS.len() as f32 * (icon + gap) - gap + 2.0 * pad + 8.0);
            if slots > 0 {
                let h = slots as f32 * (icon + gap) - gap + 2.0 * pad;
                let rail = Rect::from_min_size(pos2(stage.left() + 4.0, (stage.center().y - h / 2.0).max(stage.top() + 4.0)), vec2(56.0, h));
                ui.painter().rect(rail, CornerRadius::same(14), theme::p().card, Stroke::new(1.0, theme::p().border), egui::StrokeKind::Inside);
                let mut rail_ui = ui.new_child(egui::UiBuilder::new().max_rect(rail.shrink2(vec2(8.0, pad))).layout(egui::Layout::top_down(egui::Align::Center)));
                rail_ui.spacing_mut().item_spacing.y = gap;
                self.rail_buttons(&mut rail_ui, slots);
            }
            if self.voice_hud || self.voice_rec.is_some() {
                self.voice_over_hud(ui, stage);
            } else {
                self.voice_meter = None;
                self.voice_levels.clear();
            }
        });

        self.export_modal(ctx);
        self.preset_modal(ctx);

        go_home
    }

    /// Voice-over bar over the top of the preview. Waiting: the microphone's live level, a menu
    /// to change it, Record and close. Recording: pulsing dot, elapsed time, level and Stop.
    fn voice_over_hud(&mut self, ui: &mut egui::Ui, stage: Rect) {
        let pal = theme::p();
        let font = FontId::proportional(13.0);
        let recording = self.voice_rec.is_some();
        if !recording {
            // a meter on the chosen microphone shows it hears you before you record
            let id = self.voice_mic.as_ref().map_or("default", |m| m.id.as_str()).to_string();
            if self.voice_meter.as_ref().is_none_or(|m| m.device_name != id) {
                self.voice_meter = crate::mic_meter::MicMeter::start(&id);
            }
        }
        let mic_name = match (&self.voice_rec, &self.voice_mic) {
            (Some(rec), _) => rec.device_name.clone(),
            (None, Some(m)) => m.name.clone(),
            (None, None) => crate::voice_over::default_label(self.voice_mics.get_or_insert_with(crate::voice_over::microphones)),
        };
        let level = match &self.voice_rec {
            Some(rec) => rec.level(),
            None => self.voice_meter.as_ref().map_or(0.0, |m| m.level()),
        };
        if self.voice_levels_at.is_none_or(|t| t.elapsed().as_millis() >= 50) {
            self.voice_levels_at = Some(Instant::now());
            self.voice_levels.push_back(level);
            if self.voice_levels.len() > 32 {
                self.voice_levels.pop_front();
            }
        }
        let elapsed = self.voice_rec.as_ref().map_or(0.0, |r| r.elapsed_ms());
        // nothing heard after a couple of seconds: the mic is muted, too quiet or held by a call
        let no_sound = elapsed > 2000.0 && self.voice_rec.as_ref().is_some_and(|r| r.is_silent());
        // the microphone cut off the peaks: the take is distorting
        let too_loud = self.voice_rec.as_ref().is_some_and(|r| r.is_clipping());

        // the bar can be dragged anywhere on the preview; where it was left is kept (as a
        // fraction of the preview, so a resized window keeps it in the same spot) across runs
        let pos_id = egui::Id::new("voice-over-hud-pos");
        let size = vec2(512.0, 44.0);
        let saved: Option<egui::Vec2> = ui.ctx().data_mut(|d| d.get_persisted(pos_id));
        let centre = saved.map_or(pos2(stage.center().x, stage.top() + 30.0), |f| stage.min + f * stage.size());
        let inside = |c: egui::Pos2| {
            let (half, room) = (size / 2.0 + vec2(4.0, 4.0), stage.size() / 2.0);
            let clamp = |v: f32, lo: f32, hi: f32| if lo < hi { v.clamp(lo, hi) } else { (lo + hi) / 2.0 };
            pos2(clamp(c.x, stage.center().x - room.x + half.x, stage.center().x + room.x - half.x), clamp(c.y, stage.center().y - room.y + half.y, stage.center().y + room.y - half.y))
        };
        let mut bar = Rect::from_center_size(inside(centre), size);
        // registered before the controls, so they stay clickable and the empty parts drag
        let drag = ui.interact(bar, ui.id().with("voice-over-drag"), Sense::click_and_drag()).on_hover_text("Drag to move · double-click to put back");
        if drag.dragged() {
            bar = Rect::from_center_size(inside(bar.center() + drag.drag_delta()), size);
            let f = (bar.center() - stage.min) / stage.size();
            ui.ctx().data_mut(|d| d.insert_persisted(pos_id, f));
        }
        if drag.double_clicked() {
            ui.ctx().data_mut(|d| d.remove::<egui::Vec2>(pos_id));
        }
        if drag.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if drag.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        ui.painter().rect(bar, CornerRadius::same(10), pal.glass_solid, Stroke::new(1.0, pal.glass_border), egui::StrokeKind::Inside);
        let cy = bar.center().y;
        // grip: two columns of dots on the left edge
        for (dx, dy) in [(0.0, -5.0), (0.0, 0.0), (0.0, 5.0), (4.0, -5.0), (4.0, 0.0), (4.0, 5.0)] {
            ui.painter().circle_filled(pos2(bar.left() + 10.0 + dx, cy + dy), 1.2, pal.text_dim);
        }
        let mut x = bar.left() + 24.0;
        if recording {
            // the dot pulses once a second
            let pulse = 0.55 + 0.45 * ((elapsed / 1000.0 * std::f64::consts::TAU).cos() as f32 * 0.5 + 0.5);
            ui.painter().circle_filled(pos2(x + 6.0, cy), 6.0, pal.danger.gamma_multiply(pulse));
            ui.painter().text(pos2(x + 20.0, cy), Align2::LEFT_CENTER, fmt_clock_short(elapsed), FontId::monospace(13.0), pal.on_glass);
            x += 68.0;
        }
        let levels: Vec<f32> = self.voice_levels.iter().copied().collect();
        widgets::paint_level_bars(ui.painter(), Rect::from_min_size(pos2(x, cy - 11.0), vec2(70.0, 22.0)), &levels, if too_loud { pal.danger } else { pal.meter }, theme::glass_alpha(60));
        x += 80.0;

        // right side: Record / Stop, and close while waiting
        let close = Rect::from_center_size(pos2(bar.right() - 22.0, cy), vec2(28.0, 28.0));
        let button = Rect::from_min_size(pos2(if recording { bar.right() - 92.0 } else { close.left() - 88.0 }, cy - 14.0), vec2(80.0, 28.0));

        // middle: the microphone, a menu while waiting; its name or the warning while recording
        let name_rect = Rect::from_min_max(pos2(x, cy - 14.0), pos2(button.left() - 8.0, cy + 14.0));
        let fit = |text: String, max_width: f32, color: Color32| {
            let mut job = egui::text::LayoutJob::simple_singleline(text, font.clone(), color);
            job.wrap = egui::text::TextWrapping { max_width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            ui.fonts_mut(|f| f.layout_job(job))
        };
        if recording {
            let (text, color) = if too_loud {
                ("Too loud: lower the input volume".to_string(), pal.danger)
            } else if no_sound {
                (format!("No sound from {mic_name}"), pal.danger)
            } else {
                (mic_name, pal.on_glass)
            };
            let galley = fit(text, name_rect.width(), color);
            ui.painter().galley(pos2(name_rect.left(), cy - galley.size().y / 2.0), galley, color);
        } else {
            let resp = ui.interact(name_rect, ui.id().with("voice-over-mic"), Sense::click()).on_hover_text("Microphone");
            let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
            if resp.hovered() || open {
                ui.painter().rect_filled(name_rect, CornerRadius::same(2), theme::glass_alpha(36));
            }
            let galley = fit(mic_name, name_rect.width() - 34.0, pal.on_glass);
            ui.painter().galley(pos2(name_rect.left() + 8.0, cy - galley.size().y / 2.0), galley, pal.on_glass);
            widgets::paint_icon(ui.painter(), Rect::from_center_size(pos2(name_rect.right() - 14.0, cy + 1.0), vec2(10.0, 10.0)), widgets::Icon::ChevronDown, pal.text_dim);
            if resp.clicked() {
                // listed again on opening (a microphone may have been plugged in)
                self.voice_mics = Some(crate::voice_over::microphones());
            }
            crate::picker::bar_menu(&resp, egui::RectAlign::BOTTOM_START, |ui| {
                ui.set_min_width(name_rect.width().max(260.0));
                self.voice_mic_rows(ui);
            });
        }

        let (tip, label) = if recording { ("Stop and keep the take (Space)", "Stop") } else { ("Record from the playhead (R)", "Record") };
        let resp = ui.interact(button, ui.id().with("voice-over-record"), Sense::click()).on_hover_text(tip);
        let p = ui.painter();
        p.rect_filled(button, CornerRadius::same(2), theme::glass_alpha(if resp.hovered() { 48 } else { 28 }));
        let icon = if recording { widgets::Icon::Stop } else { widgets::Icon::Record };
        widgets::paint_icon(p, Rect::from_center_size(pos2(button.left() + 17.0, cy), vec2(16.0, 16.0)), icon, pal.danger);
        p.text(pos2(button.left() + 31.0, cy), Align2::LEFT_CENTER, label, font.clone(), pal.on_glass);
        if resp.clicked() {
            if recording {
                self.stop_voice_over();
            } else {
                self.start_voice_over();
            }
        }
        if !recording {
            let resp = ui.interact(close, ui.id().with("voice-over-close"), Sense::click()).on_hover_text("Close (Esc)");
            if resp.hovered() {
                ui.painter().rect_filled(close, CornerRadius::same(2), theme::glass_alpha(36));
            }
            widgets::paint_icon(ui.painter(), close.shrink(8.0), widgets::Icon::Close, pal.on_glass);
            if resp.clicked() {
                self.voice_hud = false;
            }
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }

    /// Name prompt for "Save preset as…" / "Rename preset", and the delete confirmation.
    fn preset_modal(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.preset_dialog.take() else { return };
        let pal = theme::p();
        let frame = egui::Frame::NONE.fill(pal.bg).stroke(egui::Stroke::new(1.0, pal.border)).corner_radius(theme::RADIUS).inner_margin(egui::Margin::symmetric(22, 18)).shadow(ctx.global_style().visuals.window_shadow);
        let mut done = false;
        let resp = egui::Modal::new(egui::Id::new("preset_dialog")).frame(frame).show(ctx, |ui| {
            ui.set_width(360.0);
            let (title, verb) = match &dlg.action {
                PresetAction::SaveAs => ("Save preset as", "Save"),
                PresetAction::Rename(_) => ("Rename preset", "Rename"),
                PresetAction::Delete(_) => ("Delete preset", "Delete"),
            };
            ui.label(RichText::new(title).size(17.0).color(pal.text));
            ui.add_space(10.0);
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if let PresetAction::Delete(path) = &dlg.action {
                let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                crate::widgets::hint(ui, &format!("Delete “{name}”? The preset is removed from the presets folder; this cannot be undone."));
            } else {
                crate::widgets::hint(ui, "Presets keep every setting except the crop and are listed in the Presets menu of every project.");
                ui.add_space(6.0);
                let r = crate::widgets::text_field(ui, &mut dlg.name, "Preset name");
                if !dlg.focused {
                    r.request_focus();
                    dlg.focused = true;
                }
            }
            if let Some(e) = &dlg.error {
                ui.add_space(4.0);
                ui.label(RichText::new(e).size(13.0).color(pal.danger));
            }
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let confirm = crate::widgets::accent_button(ui, verb).clicked() || enter;
                let cancel = crate::widgets::secondary_button(ui, "Cancel").clicked() || escape;
                if confirm {
                    match self.run_preset_action(&dlg) {
                        Ok(msg) => {
                            self.set_status(msg);
                            done = true;
                        }
                        Err(e) => dlg.error = Some(e),
                    }
                } else if cancel {
                    done = true;
                }
            });
        });
        if resp.backdrop_response.clicked() {
            done = true;
        }
        if !done {
            self.preset_dialog = Some(dlg);
        }
    }

    /// Perform a preset dialog's action; the message is shown in the status line.
    fn run_preset_action(&mut self, dlg: &PresetDialog) -> std::result::Result<String, String> {
        let folder = sf_project::Preset::default_folder().ok_or("no home folder")?;
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        match &dlg.action {
            PresetAction::SaveAs => {
                let name = sf_project::preset::sanitize_preset_name(&dlg.name);
                if name.is_empty() {
                    return Err("Give the preset a name.".into());
                }
                if sf_project::Preset::exists(&folder, &name) {
                    return Err(format!("A preset called “{name}” already exists."));
                }
                let assets = self.session.pkg.path.join("assets");
                sf_project::Preset::create(sf_project::Preset::path_for(&folder, &name), &name, &self.session.pkg.project.config, assets.is_dir().then_some(assets.as_path())).map_err(|e| e.to_string())?;
                Ok(format!("Saved preset {name}"))
            }
            PresetAction::Rename(path) => {
                let mut p = sf_project::Preset::open(path).map_err(|e| e.to_string())?;
                let name = sf_project::preset::sanitize_preset_name(&dlg.name);
                if name.is_empty() {
                    return Err("Give the preset a name.".into());
                }
                if !name.eq_ignore_ascii_case(&p.name()) && sf_project::Preset::exists(&folder, &name) {
                    return Err(format!("A preset called “{name}” already exists."));
                }
                p.rename(&name).map_err(|e| e.to_string())?;
                Ok(format!("Renamed preset to {name}"))
            }
            PresetAction::Delete(path) => {
                let p = sf_project::Preset::open(path).map_err(|e| e.to_string())?;
                let name = p.name();
                p.delete().map_err(|e| e.to_string())?;
                Ok(format!("Deleted preset {name}"))
            }
        }
    }

    /// Export dialog over the dimmed editor, like Screen Studio's: settings first, then the
    /// progress and result of the running export in the same dialog.
    fn export_modal(&mut self, ctx: &egui::Context) {
        let mut ex = std::mem::take(&mut self.export);
        ex.poll(ctx);
        if ex.open || ex.is_busy() {
            // opening the dialog (button or ⌘E) stops playback behind it
            if self.playing {
                self.pause();
            }
            let pal = theme::p();
            let frame = egui::Frame::NONE.fill(pal.bg).stroke(egui::Stroke::new(1.0, pal.border)).corner_radius(theme::RADIUS).inner_margin(egui::Margin::symmetric(22, 18)).shadow(ctx.global_style().visuals.window_shadow);
            // progress gets its own (smaller) modal: an egui area keeps its last size per id
            let busy = ex.is_busy();
            let id = egui::Id::new(if busy { "export_progress" } else { "export_dialog" });
            let resp = egui::Modal::new(id).frame(frame).show(ctx, |ui| {
                if busy {
                    ui.set_width(crate::export::PROGRESS_W);
                    ex.compact_ui(ui);
                } else {
                    ui.set_width(crate::export::DIALOG_W);
                    ex.settings_ui(self, ui);
                }
            });
            // clicking the backdrop dismisses the settings (never a running export)
            if resp.backdrop_response.clicked() && !ex.is_busy() {
                ex.open = false;
            }
        }
        self.export = ex;
    }

    /// Rename the project: updates `project.name`, renames the package folder on disk and
    /// saves. Like Screen Studio, the file name follows the project name.
    pub fn rename_project(&mut self, new_name: &str) {
        let new_name = sf_session::create::sanitize_name(new_name.trim());
        if new_name.is_empty() || new_name == self.session.pkg.project.name {
            return;
        }
        let old_path = self.session.pkg.path.clone();
        let parent = old_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let new_path = parent.join(format!("{new_name}.{}", sf_project::PACKAGE_EXTENSION));
        if new_path.exists() {
            self.set_status("A project with that name already exists");
            return;
        }
        if let Err(e) = std::fs::rename(&old_path, &new_path) {
            self.set_status(format!("Rename failed: {e}"));
            return;
        }
        if let Err(e) = self.session.relocate(&new_path) {
            self.set_status(format!("Renamed, but reopening failed: {e}"));
        }
        self.path = new_path.clone();
        self.session.pkg.project.name = new_name.clone();
        self.save();
        self.set_status(format!("Renamed to {new_name}"));
        self.renamed_to = Some(new_path);
    }

    fn presets_menu(&mut self, ui: &mut egui::Ui) {
        const MENU_W: f32 = 232.0;
        ui.set_width(MENU_W);
        ui.spacing_mut().item_spacing.y = 2.0;
        let folder = sf_project::Preset::default_folder();
        let presets = folder.as_ref().map(|f| sf_project::Preset::list(f)).unwrap_or_default();
        if presets.is_empty() {
            ui.label(RichText::new("No presets yet").color(theme::p().text_dim));
        }
        for p in presets {
            // a plain list row: the name applies the preset; rename / delete icons show on hover
            match preset_row(ui, &p.name(), MENU_W) {
                PresetRowAction::None => {}
                PresetRowAction::Apply => {
                    let dir = self.session.pkg.path.clone();
                    let pr = p.clone();
                    self.edit(move |proj| {
                        let _ = pr.apply(proj, &dir);
                    });
                    self.set_status(format!("Applied preset {}", p.name()));
                    ui.close();
                }
                PresetRowAction::Rename => {
                    self.preset_dialog = Some(PresetDialog::new(PresetAction::Rename(p.path.clone()), p.name()));
                    ui.close();
                }
                PresetRowAction::Delete => {
                    self.preset_dialog = Some(PresetDialog::new(PresetAction::Delete(p.path.clone()), p.name()));
                    ui.close();
                }
            }
        }
        ui.separator();
        if ui.button("Save preset as…").clicked() {
            self.preset_dialog = Some(PresetDialog::new(PresetAction::SaveAs, format!("Preset {}", chrono_like_stamp())));
            ui.close();
        }
        if ui.button("Open presets folder").clicked() {
            if let Some(folder) = sf_project::Preset::default_folder() {
                let _ = std::fs::create_dir_all(&folder);
                crate::os::open(folder);
            }
            ui.close();
        }
    }

    /// Open the crop tool on the current crop (playback stops; the preview shows the whole recording).
    fn open_crop(&mut self) {
        self.pause();
        self.crop = Some(crop::CropEditor::new(self.session.pkg.project.config.crop.rect01));
        self.last_rendered_ms = None;
    }

    /// Close the crop tool, keeping the new crop (one undo step) or discarding it.
    fn close_crop(&mut self, apply: bool) {
        let Some(c) = self.crop.take() else { return };
        self.last_rendered_ms = None;
        let r = crop::clamp(c.draft);
        if apply && r != self.session.pkg.project.config.crop.rect01 {
            self.edit(move |p| p.config.crop.rect01 = r);
            self.set_status(if crop::is_full(r) { "Crop removed" } else { "Cropped" });
        }
    }

    fn preview_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let aspect = self.session.pkg.project.config.output.aspect_ratio;
            let b = widgets::icon_text_button(ui, Some(widgets::Icon::Aspect), aspect.label().split(' ').next().unwrap_or("Auto"), true);
            egui::Popup::menu(&b).show(|ui| {
                ui.set_min_width(220.0);
                for (label, preset) in sf_project::schema::common::AspectRatio::PRESETS {
                    let value = preset.map(|(w, h)| sf_project::schema::common::AspectRatio::ratio(w, h)).unwrap_or(sf_project::schema::common::AspectRatio::AUTO);
                    if crate::widgets::menu_item(ui, aspect == value, *label).clicked() {
                        self.edit(move |p| p.config.output.aspect_ratio = value);
                        ui.close();
                    }
                }
            });
            if widgets::icon_text_button(ui, Some(widgets::Icon::Crop), "Crop", false).on_hover_text("Crop the recording").clicked() {
                if self.crop.is_some() {
                    self.close_crop(true);
                } else {
                    self.open_crop();
                }
            }
            if self.crop.is_some() {
                if widgets::secondary_button(ui, "Reset").clicked() {
                    if let Some(c) = &mut self.crop {
                        c.draft = sf_project::schema::common::Rect::UNIT;
                    }
                }
                if widgets::secondary_button(ui, "Cancel").on_hover_text("Esc").clicked() {
                    self.close_crop(false);
                }
                if widgets::accent_button(ui, "Done").on_hover_text("Return").clicked() {
                    self.close_crop(true);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // the same margin as before Auto on the left
                ui.add_space(8.0);
                if widgets::toolbar_icon_button(ui, widgets::Icon::Zoom, "Preview size +").clicked() {
                    self.preview_scale = (self.preview_scale * 1.15).min(3.0);
                }
                if widgets::toolbar_icon_button(ui, widgets::Icon::ZoomOut, "Preview size −").clicked() {
                    self.preview_scale = (self.preview_scale / 1.15).max(0.5);
                }
                let q = match self.preview_quality {
                    PreviewQuality::Full => "Full",
                    PreviewQuality::Performance => "Performance",
                };
                let b = widgets::icon_text_button(ui, Some(widgets::Icon::Gear), q, true).on_hover_text("Preview quality");
                egui::Popup::menu(&b).show(|ui| {
                    ui.label(RichText::new("Preview quality").small().color(theme::p().text_dim));
                    if crate::widgets::menu_item(ui, self.preview_quality == PreviewQuality::Full, "Full").clicked() {
                        self.preview_quality = PreviewQuality::Full;
                        self.invalidate_preview();
                    }
                    if crate::widgets::menu_item(ui, self.preview_quality == PreviewQuality::Performance, "Performance").clicked() {
                        self.preview_quality = PreviewQuality::Performance;
                        self.invalidate_preview();
                    }
                });
            });
        });
    }

    fn transport_ui(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        let mut left = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        if widgets::icon_button(&mut left, widgets::Icon::Scissors, 20.0, "Split at playhead (S)").clicked() {
            self.split_at_playhead();
        }
        let can_delete = self.selected_zoom.is_some() || self.selected_mask.is_some() || self.selected_audio.is_some() || self.selected_voice_over.is_some() || self.selected_music.is_some() || (self.selected_slice.is_some() && self.session.pkg.project.scenes[0].slices.len() > 1);
        if widgets::icon_button(&mut left, widgets::Icon::Trash, 16.0, "Delete selection (⌫)").clicked() && can_delete {
            self.delete_selected();
        }

        // centre transport
        let center = Rect::from_center_size(rect.center(), vec2(420.0, rect.height()));
        let mut mid = ui.new_child(egui::UiBuilder::new().max_rect(center).layout(egui::Layout::left_to_right(egui::Align::Center)));
        mid.spacing_mut().item_spacing.x = 18.0;
        mid.label(RichText::new(fmt_clock(self.playhead_ms)).size(15.0).color(theme::p().text_dim));
        if mid.add(egui::Button::new(RichText::new("⏮").size(18.0)).frame(false)).clicked() {
            self.seek(0.0);
        }
        let (pr, presp) = mid.allocate_exact_size(vec2(36.0, 36.0), Sense::click());
        let presp = presp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let p = mid.painter();
        p.circle_stroke(pr.center(), 16.0, Stroke::new(1.5, theme::p().text));
        // the play triangle sits a touch right of centre so it looks centred
        let icon = if self.playing { widgets::Icon::Pause } else { widgets::Icon::Play };
        widgets::paint_icon(p, Rect::from_center_size(pr.center() + vec2(if self.playing { 0.0 } else { 1.0 }, 0.0), vec2(13.0, 13.0)), icon, theme::p().text);
        if presp.clicked() {
            self.toggle_play();
        }
        if mid.add(egui::Button::new(RichText::new("⏭").size(18.0)).frame(false)).clicked() {
            let d = self.duration_ms();
            self.seek(d);
        }
        mid.label(RichText::new(fmt_clock(self.duration_ms())).size(15.0).color(theme::p().text_dim));

        // right: waveform toggle, speed, timeline zoom
        let mut right = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
        right.spacing_mut().slider_width = 90.0;
        right.add(egui::Slider::new(&mut self.timeline.px_per_sec, 5.0..=400.0).logarithmic(true).show_value(false));
        right.label(RichText::new("1×").color(theme::p().text_dim));
        widgets::icon_button(&mut right, widgets::Icon::Waveform, 16.0, "Audio waveform");
        let _ = Color32::WHITE;
    }
}

pub fn fmt_ms(ms: f64) -> String {
    let total = (ms / 1000.0).max(0.0);
    let m = (total / 60.0).floor() as u32;
    let s = total - m as f64 * 60.0;
    format!("{m:02}:{s:05.2}")
}

/// `0:39.06` like Screen Studio's transport.
/// Badge in the preview's top-left corner while the camera is zoomed in: the zoom level and a
/// mini-map of which part of the screen is visible.
fn paint_zoom_indicator(p: &egui::Painter, img_rect: Rect, (level, vis): (f64, [f64; 4])) {
    if level < 1.02 {
        return;
    }
    let pal = theme::p();
    let badge = Rect::from_min_size(img_rect.min + vec2(10.0, 10.0), vec2(104.0, 36.0));
    p.rect(badge, CornerRadius::same(2), pal.glass_solid, Stroke::new(1.0, pal.glass_border), egui::StrokeKind::Inside);
    // mini-map: full screen outline + visible part
    let map = Rect::from_min_size(badge.min + vec2(8.0, 7.0), vec2(36.0, 22.0));
    p.rect_stroke(map, CornerRadius::same(1), Stroke::new(1.0, pal.text_faint), egui::StrokeKind::Inside);
    let v = Rect::from_min_size(
        pos2(map.left() + vis[0] as f32 * map.width(), map.top() + vis[1] as f32 * map.height()),
        vec2((vis[2] as f32 * map.width()).max(2.0), (vis[3] as f32 * map.height()).max(2.0)),
    );
    p.rect(v, CornerRadius::same(1), pal.accent.gamma_multiply(0.35), Stroke::new(1.0, pal.accent_hover), egui::StrokeKind::Inside);
    p.text(pos2(map.right() + 10.0, badge.center().y), Align2::LEFT_CENTER, format!("{level:.1}×"), FontId::monospace(13.0), pal.on_glass);
}

/// `m:ss` without fractions, for the recording HUD.
pub fn fmt_clock_short(ms: f64) -> String {
    let total = (ms / 1000.0).max(0.0).floor() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

pub fn fmt_clock(ms: f64) -> String {
    let total = (ms / 1000.0).max(0.0);
    let m = (total / 60.0).floor() as u32;
    let s = total - m as f64 * 60.0;
    format!("{m}:{s:05.2}")
}

enum PresetRowAction {
    None,
    Apply,
    Rename,
    Delete,
}

/// One preset in the Presets menu: a full-width list row (name on the left) that applies the
/// preset when clicked, with pencil and trash icons appearing at the right edge on hover.
fn preset_row(ui: &mut egui::Ui, name: &str, width: f32) -> PresetRowAction {
    use crate::widgets::{paint_icon, Icon};
    let (rect, row) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click());
    let pal = theme::p();
    let icon = 14.0;
    let delete_rect = Rect::from_center_size(pos2(rect.right() - 14.0, rect.center().y), vec2(icon + 8.0, icon + 8.0));
    let rename_rect = delete_rect.translate(vec2(-(icon + 12.0), 0.0));
    let delete = ui.interact(delete_rect, row.id.with("delete"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let rename = ui.interact(rename_rect, row.id.with("rename"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = row.hovered() || delete.hovered() || rename.hovered();
    if hovered {
        ui.painter().rect_filled(rect, CornerRadius::same(4), pal.card_2);
    }
    let text_w = if hovered { rename_rect.left() - rect.left() - 16.0 } else { rect.width() - 16.0 };
    let galley = ui.painter().layout(name.to_string(), FontId::proportional(14.0), pal.text, text_w.max(20.0));
    let galley = if galley.rows.len() > 1 { ui.painter().layout_no_wrap(format!("{}…", name.chars().take(28).collect::<String>()), FontId::proportional(14.0), pal.text) } else { galley };
    ui.painter().galley(pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, pal.text);
    if hovered {
        for (r, ic, resp) in [(rename_rect, Icon::Pencil, &rename), (delete_rect, Icon::Trash, &delete)] {
            let color = if resp.hovered() { pal.text } else { pal.text_dim };
            paint_icon(ui.painter(), Rect::from_center_size(r.center(), vec2(icon, icon)), ic, color);
        }
    }
    let _ = rename.clone().on_hover_text("Rename this preset");
    let _ = delete.clone().on_hover_text("Delete this preset");
    if delete.clicked() {
        PresetRowAction::Delete
    } else if rename.clicked() {
        PresetRowAction::Rename
    } else if row.clicked() {
        PresetRowAction::Apply
    } else {
        PresetRowAction::None
    }
}

/// What the preset dialog does when confirmed.
enum PresetAction {
    SaveAs,
    Rename(PathBuf),
    Delete(PathBuf),
}

struct PresetDialog {
    action: PresetAction,
    name: String,
    error: Option<String>,
    focused: bool,
}

impl PresetDialog {
    fn new(action: PresetAction, name: String) -> Self {
        PresetDialog { action, name, error: None, focused: false }
    }
}

fn chrono_like_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{}", secs % 100_000)
}
