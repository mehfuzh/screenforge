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
use crate::widgets;

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
    /// Selected detached audio clip (id).
    pub selected_audio: Option<String>,
    /// The background music block is selected (⌫ removes the music).
    pub selected_music: bool,
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
    pub fn with_session(path: &Path, session: Session, gpu: Gpu) -> Self {
        let preview_size = session.output_size(1080);
        let compositor = Compositor::new(gpu);
        let audio = AudioPlayer::new();
        audio.load(&session);
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
            selected_audio: None,
            selected_music: false,
            dirty: false,
            last_edit: None,
            timeline: TimelineWidget::default(),
            export: ExportDialog::default(),
            undo: vec![],
            redo: vec![],
            status: None,
            section: match std::env::var("SF_START_SECTION").as_deref() {
                Ok("audio") => Section::Audio,
                Ok("cursor") => Section::Cursor,
                Ok("animations") => Section::Animations,
                _ => Section::General,
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

    /// A transcript was (re)generated: reload the captions and redraw the preview.
    pub fn captions_changed(&mut self) {
        self.session.reload_captions();
        self.last_rendered_ms = None;
    }

    fn after_edit(&mut self) {
        self.session.rebuild();
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
        self.edit(move |p| {
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

    /// Push volume/mute changes to the audio player without re-decoding.
    pub fn refresh_audio_levels(&mut self) {
        self.audio.update_timeline(&self.session);
    }

    /// Start playback so a newly chosen background track is heard right away.
    pub fn preview_background_audio(&mut self) {
        if !self.playing {
            self.toggle_play();
        } else {
            self.audio.play_from(self.playhead_ms);
        }
        let name = self.session.pkg.project.config.audio.background_audio_file_name.clone().unwrap_or_default();
        let label = sf_session::assets::background_audio_label(&name);
        self.set_status(&format!("Background music: {label}"));
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
        if self.preview_size.height != height {
            self.preview_size = self.session.output_size(height);
        }
        match self.session.frame_desc_at(self.playhead_ms, self.preview_size) {
            Ok(desc) => {
                self.compositor.render(&desc);
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
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // the export dialog is modal: its own keys (Esc) are handled in ExportDialog::poll
        if ctx.memory(|m| m.focused().is_some()) || self.export.open || self.export.is_busy() {
            return;
        }
        let frame = 1000.0 / 60.0;
        let (mut split, mut del, mut undo, mut redo, mut save, mut export, mut prev_b, mut next_b, mut toggle) = (false, false, false, false, false, false, false, false, false);
        let mut toggle_sidebar = false;
        let mut section: Option<Section> = None;
        ctx.input_mut(|i| {
            toggle = i.consume_key(egui::Modifiers::NONE, egui::Key::Space);
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
        if split { self.split_at_playhead(); }
        if del { self.delete_selected(); }
        if undo { self.undo(); }
        if redo { self.redo(); }
        if save { self.save(); }
        if export { self.export.open = true; }
        if prev_b { crate::timeline::jump_boundary(self, false); }
        if next_b { crate::timeline::jump_boundary(self, true); }
        if toggle_sidebar { self.sidebar_open = !self.sidebar_open; }
        if let Some(s) = section {
            self.section = s;
            self.sidebar_open = true;
        }
        if !self.playing {
            self.audio.seek(self.playhead_ms);
        }
    }

    pub fn split_at_playhead(&mut self) {
        let t = self.playhead_ms;
        // Split the selected audio clip only when the playhead is over it; otherwise cut the video.
        let audio_hit = self.selected_audio.clone().filter(|id| {
            use sf_project::SceneAudioExt;
            self.session.pkg.project.scenes[0].audio_clips().is_some_and(|cs| cs.iter().any(|c| &c.id == id && c.contains_output(t)))
        });
        if let Some(id) = audio_hit {
            self.edit(move |p| {
                use sf_project::SceneAudioExt;
                let scene = &mut p.scenes[0];
                if let Some(mut clips) = scene.audio_clips() {
                    if let Some(i) = clips.iter().position(|c| c.id == id) {
                        if let Some(right) = clips[i].split_at_output(t) {
                            clips.insert(i + 1, right);
                        }
                    }
                    scene.set_audio_clips(Some(clips));
                }
            });
            self.set_status("Split audio");
            return;
        }
        self.edit(|p| {
            let s = &mut p.scenes[0];
            s.slices = sf_engine::timeline::edit::split_at_output(&s.slices, t);
        });
        self.set_status("Split");
    }

    pub fn delete_selected(&mut self) {
        if std::mem::take(&mut self.selected_music) {
            self.edit(|p| p.config.audio.background_audio_file_name = None);
            self.reload_audio();
            self.set_status("Removed background music");
            return;
        }
        if let Some(id) = self.selected_audio.take() {
            self.edit(move |p| {
                use sf_project::SceneAudioExt;
                let scene = &mut p.scenes[0];
                if let Some(mut clips) = scene.audio_clips() {
                    clips.retain(|c| c.id != id);
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
        if let Some(i) = self.selected_slice.take() {
            if self.session.pkg.project.scenes[0].slices.len() > 1 {
                self.edit(|p| {
                    let s = &mut p.scenes[0];
                    s.slices = sf_engine::timeline::edit::delete(&s.slices, i);
                });
                self.set_status("Deleted clip");
            }
        }
    }

    /// Returns true when the user wants to go back to the picker.
    pub fn ui(&mut self, root: &mut egui::Ui, rs: &egui_wgpu::RenderState) -> bool {
        let ctx = &root.ctx().clone();
        let mut go_home = false;
        self.tick(ctx);
        self.shortcuts(ctx);
        if self.transcribe.poll() {
            self.captions_changed();
        }
        if self.transcribe.is_running() {
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
                let ext = painter.layout_no_wrap(".screenstudio".into(), FontId::proportional(15.0), theme::p().text_faint);
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
                ui.add_space(72.0); // traffic lights
                if widgets::icon_button(ui, widgets::Icon::Folder, 18.0, "Projects").clicked() {
                    go_home = true;
                }
                if widgets::icon_button(ui, widgets::Icon::Trash, 18.0, "Move project to Trash").clicked() && !self.export.is_busy() {
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
                    ui.add_space(6.0);
                    widgets::icon_button(ui, widgets::Icon::Command, 16.0, "Shortcuts: Space play · S split · ⌫ delete · ⌘Z undo · ⌘S save · ⌘⇧E export · [ ] jump cuts · ⌘1…⌘0 sidebar · ⌘\\ toggle sidebar");
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
                        if s.is_roadmap() {
                            widgets::roadmap(ui, |ui| widgets::rail_icon(ui, s.icon(), false, s.title()));
                            continue;
                        }
                        if widgets::rail_icon(ui, s.icon(), self.section == s, s.title()).clicked() {
                            self.section = s;
                        }
                    }
                });
                let card_rect = ui.available_rect_before_wrap();
                widgets::card(ui.painter(), card_rect);
                let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(card_rect.shrink2(vec2(18.0, 10.0))).layout(egui::Layout::top_down(egui::Align::Min)));
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
                if resp.clicked() {
                    self.toggle_play();
                }
                paint_zoom_indicator(ui.painter(), img_rect, self.session.zoom_state());
            } else {
                ui.painter().text(stage.center(), Align2::CENTER_CENTER, "Rendering…", FontId::proportional(14.0), theme::p().text_dim);
            }
            // floating tool rail (left)
            let rail = Rect::from_min_size(pos2(stage.left() + 4.0, stage.center().y - 110.0), vec2(56.0, 220.0));
            ui.painter().rect(rail, CornerRadius::same(14), theme::p().card, Stroke::new(1.0, theme::p().border), egui::StrokeKind::Inside);
            let mut rail_ui = ui.new_child(egui::UiBuilder::new().max_rect(rail.shrink2(vec2(8.0, 10.0))).layout(egui::Layout::top_down(egui::Align::Center)));
            rail_ui.spacing_mut().item_spacing.y = 8.0;
            if widgets::rail_icon(&mut rail_ui, widgets::Icon::Zoom, false, "Add zoom at playhead (3 s)").clicked() {
                let t = self.playhead_ms;
                let src = self.session.timeline.output_to_source(t).unwrap_or(0.0);
                self.edit(move |p| p.scenes[0].zooms.push(sf_project::v4::Zoom::auto(src, src + 3000.0, sf_project::v4::Zoom::DEFAULT_LEVEL)));
            }
            if widgets::rail_icon(&mut rail_ui, widgets::Icon::Sparkle, false, "Automatic zooms from clicks").clicked() {
                let zooms = self.session.auto_zooms();
                self.edit(move |p| p.scenes[0].zooms = zooms);
                self.set_status("Generated automatic zooms");
            }
            if widgets::rail_icon(&mut rail_ui, widgets::Icon::Layout, false, "Camera layout").clicked() {
                self.section = Section::CameraLayout;
            }
            widgets::roadmap(&mut rail_ui, |ui| widgets::rail_icon(ui, widgets::Icon::Mask, false, "Mask"));
        });

        self.export_modal(ctx);

        go_home
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
        ui.set_min_width(260.0);
        let folder = sf_project::Preset::default_folder();
        let presets = folder.as_ref().map(|f| sf_project::Preset::list(f)).unwrap_or_default();
        if presets.is_empty() {
            ui.label(RichText::new("No presets yet").color(theme::p().text_dim));
        }
        for p in presets {
            if ui.button(p.name()).clicked() {
                let dir = self.session.pkg.path.clone();
                let pr = p.clone();
                self.edit(move |proj| {
                    let _ = pr.apply(proj, &dir);
                });
                self.set_status(format!("Applied preset {}", p.name()));
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Create new preset…").clicked() {
            if let Some(folder) = folder {
                let _ = std::fs::create_dir_all(&folder);
                let name = format!("Preset {}", chrono_like_stamp());
                let out = folder.join(format!("{name}.screenstudiopreset"));
                let assets = self.session.pkg.path.join("assets");
                match sf_project::Preset::create(&out, &name, &self.session.pkg.project.config, assets.is_dir().then_some(assets.as_path())) {
                    Ok(_) => self.set_status(format!("Saved {name}")),
                    Err(e) => self.set_status(format!("Preset failed: {e}")),
                }
            }
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

    fn preview_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let aspect = self.session.pkg.project.config.output.aspect_ratio;
            let b = widgets::icon_text_button(ui, Some(widgets::Icon::Aspect), aspect.label().split(' ').next().unwrap_or("Auto"), true);
            egui::Popup::menu(&b).show(|ui| {
                ui.set_min_width(220.0);
                for (label, preset) in sf_project::schema::common::AspectRatio::PRESETS {
                    let value = preset.map(|(w, h)| sf_project::schema::common::AspectRatio::ratio(w, h)).unwrap_or(sf_project::schema::common::AspectRatio::AUTO);
                    if ui.selectable_label(aspect == value, *label).clicked() {
                        self.edit(move |p| p.config.output.aspect_ratio = value);
                        ui.close();
                    }
                }
            });
            widgets::roadmap(ui, |ui| widgets::icon_text_button(ui, Some(widgets::Icon::Crop), "Crop", false));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, widgets::Icon::Zoom, 16.0, "Preview size +").clicked() {
                    self.preview_scale = (self.preview_scale * 1.15).min(3.0);
                }
                if ui.add(egui::Button::new(RichText::new("−").size(16.0)).frame(false)).on_hover_text("Preview size −").clicked() {
                    self.preview_scale = (self.preview_scale / 1.15).max(0.5);
                }
                let q = match self.preview_quality {
                    PreviewQuality::Full => "Full",
                    PreviewQuality::Performance => "Performance",
                };
                let b = widgets::icon_text_button(ui, Some(widgets::Icon::Gear), &format!("Preview quality  {q}"), true);
                egui::Popup::menu(&b).show(|ui| {
                    if ui.selectable_label(self.preview_quality == PreviewQuality::Full, "Full").clicked() {
                        self.preview_quality = PreviewQuality::Full;
                        self.invalidate_preview();
                    }
                    if ui.selectable_label(self.preview_quality == PreviewQuality::Performance, "Performance").clicked() {
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
        let can_delete = self.selected_zoom.is_some() || self.selected_audio.is_some() || (self.selected_slice.is_some() && self.session.pkg.project.scenes[0].slices.len() > 1);
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
        let p = mid.painter();
        p.circle_stroke(pr.center(), 16.0, Stroke::new(1.5, theme::p().text));
        p.text(pr.center() + vec2(if self.playing { 0.0 } else { 1.0 }, 0.0), Align2::CENTER_CENTER, if self.playing { "⏸" } else { "▶" }, FontId::proportional(14.0), theme::p().text);
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

fn chrono_like_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{}", secs % 100_000)
}
