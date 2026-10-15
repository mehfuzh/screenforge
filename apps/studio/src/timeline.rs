//! Timeline: slices (clips) at their source length, zoom track, playhead, scrubbing.
//!
//! Positions are output time, drawn on the timeline's view axis (`Timeline::output_to_view`):
//! a sped-up clip keeps its width and the playhead crosses it faster.
//!
//! Editing surface: clip edges trim (ripple), zoom items select/move/resize, context menus,
//! snapping to the playhead and other item edges (hold Shift to disable).

use egui::{pos2, vec2, Color32, CursorIcon, Rect, Sense, Stroke};
use sf_engine::timeline::{edit, Timeline};
use sf_project::v4::Zoom;
use sf_project::Project;

use sf_project::audio_clips::{AudioClip, AudioTrack, SceneAudioExt};

use crate::editor::Editor;
use crate::theme;

const RULER_H: f32 = 26.0;
const MARKER_H: f32 = 22.0;
const CLIP_H: f32 = 62.0;
const AUDIO_H: f32 = 44.0;
const ZOOM_H: f32 = 48.0;
const MUSIC_H: f32 = 36.0;
const GAP: f32 = 8.0;
const HANDLE_W: f32 = 6.0;
const SNAP_PX: f32 = 6.0;
/// Minimum clip / zoom length (source ms).
const MIN_MS: f64 = 100.0;
const SNAP_LINE: Color32 = Color32::from_rgb(0xff, 0xd1, 0x5c);

pub const SPEEDS: [f64; 11] = [0.5, 0.75, 1.0, 1.2, 1.4, 1.6, 1.8, 2.0, 3.0, 4.0, 8.0];
pub const ZOOM_LEVELS: [f64; 6] = [1.25, 1.5, 2.0, 2.5, 3.0, 4.0];

/// Screen x ↔ view ms (`Timeline::output_to_view`) for the current zoom and scroll.
#[derive(Clone, Copy)]
struct ViewMap {
    left: f32,
    px_per_sec: f32,
    scroll_x: f32,
}

impl ViewMap {
    fn x(&self, view_ms: f64) -> f32 {
        self.left + 12.0 + (view_ms / 1000.0) as f32 * self.px_per_sec - self.scroll_x
    }
    fn view_ms(&self, x: f32) -> f64 {
        ((x - self.left - 12.0 + self.scroll_x) / self.px_per_sec * 1000.0) as f64
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    /// Trim a clip edge; `left` = source start.
    ClipEdge { index: usize, left: bool },
    /// Move a whole zoom item (keeps its duration).
    ZoomMove,
    /// Resize a zoom item edge.
    ZoomEdge { left: bool },
}

/// An in-progress drag; the project is mutated live and one undo entry is pushed on release.
struct Drag {
    kind: DragKind,
    zoom_id: String,
    start_x: f32,
    /// Original source range of the dragged item.
    src0: f64,
    src1: f64,
    /// Original output range (clips only).
    out0: f64,
    out1: f64,
    /// Allowed source range for the item (neighbours / recording).
    lo: f64,
    hi: f64,
    /// Snap targets on the output axis (ms), excluding the playhead (added live).
    targets: Vec<f64>,
    snapshot: Project,
}

#[derive(Clone, Copy, PartialEq)]
enum AudioDragKind {
    Move,
    Left,
    Right,
}

struct AudioDrag {
    id: String,
    kind: AudioDragKind,
    start_x: f32,
    orig: AudioClip,
    snapshot: Project,
}

pub struct TimelineWidget {
    pub px_per_sec: f32,
    pub scroll_x: f32,
    drag: Option<Drag>,
    audio_drag: Option<AudioDrag>,
    /// Cached waveform peaks (track, peaks, recording duration ms).
    peaks: Option<(AudioTrack, Vec<f32>, f64)>,
    peaks_gen: u64,
    /// Output ms under the pointer when a context menu was opened.
    ctx_ms: f64,
    /// Screen x of the active snap guide.
    snap_x: Option<f32>,
    /// Keep the playhead in view while playing; off once the user scrolls or zooms, back on
    /// when playback starts again.
    follow: bool,
    was_playing: bool,
}

impl Default for TimelineWidget {
    fn default() -> Self {
        TimelineWidget { px_per_sec: 40.0, scroll_x: 0.0, drag: None, audio_drag: None, peaks: None, peaks_gen: u64::MAX, ctx_ms: 0.0, snap_x: None, follow: true, was_playing: false }
    }
}

impl TimelineWidget {
    pub fn ui(&mut self, ed: &mut Editor, ui: &mut egui::Ui) {
        let avail = ui.available_size();
        // lanes stack: an optional lane (audio, zooms) is only laid out while it has content, so
        // deleting its last item collapses it and the lanes below move up
        let has_zooms = !ed.session.pkg.project.scenes[0].zooms.is_empty();
        let show_audio_lane = audio_lane_visible(ed);
        let show_music_lane = music_lane_visible(ed);
        let zoom_track_h = if has_zooms { ZOOM_H + GAP } else { 0.0 };
        let audio_track_h = if show_audio_lane { AUDIO_H + GAP } else { 0.0 };
        let music_track_h = if show_music_lane { MUSIC_H + GAP } else { 0.0 };
        let (rect, resp) = ui.allocate_exact_size(vec2(avail.x, avail.y.max(RULER_H + MARKER_H + CLIP_H + audio_track_h + music_track_h + zoom_track_h + GAP * 2.0)), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::p().bg);

        let view_len = ed.session.timeline.view_duration_ms().max(1.0);
        // auto-fit zoom the first time
        if self.px_per_sec == 40.0 && self.scroll_x == 0.0 {
            let fit = (rect.width() - 24.0) / (view_len as f32 / 1000.0);
            if fit.is_finite() && fit > 0.0 {
                self.px_per_sec = fit.clamp(5.0, 400.0);
            }
        }

        // wheel / trackpad: vertical scroll and pinch zoom the timeline around the pointer,
        // horizontal scroll (or shift+wheel) pans. Pointer-in-rect rather than `resp.hovered()`:
        // the clip, zoom and audio blocks are widgets on top of `resp`, so it isn't hovered
        // over them and scrolling would stop there.
        if ed.playing && !self.was_playing {
            self.follow = true;
        }
        self.was_playing = ed.playing;
        if ui.rect_contains_pointer(rect) {
            let (d, pinch, shift, pointer) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.shift, i.pointer.hover_pos()));
            let mut factor = pinch;
            let mut pan = 0.0;
            if shift {
                pan = d.y + d.x;
            } else if d.y.abs() > d.x.abs() {
                factor *= (1.0 + d.y * 0.004).clamp(0.5, 2.0);
            } else {
                pan = d.x;
            }
            if (factor - 1.0).abs() > 1e-4 {
                let old = self.px_per_sec;
                let new = (old * factor).clamp(5.0, 400.0);
                if let Some(p) = pointer {
                    // keep the time under the pointer fixed
                    let local = p.x - rect.left() - 12.0;
                    let ms = (local + self.scroll_x) / old * 1000.0;
                    self.scroll_x = ms / 1000.0 * new - local;
                }
                self.px_per_sec = new;
                self.follow = false;
            }
            if pan != 0.0 {
                self.scroll_x -= pan;
                self.follow = false;
            }
        }
        let content_w = view_len as f32 / 1000.0 * self.px_per_sec + 24.0;
        self.scroll_x = self.scroll_x.clamp(0.0, (content_w - rect.width()).max(0.0));
        let map = ViewMap { left: rect.left(), px_per_sec: self.px_per_sec, scroll_x: self.scroll_x };

        // live drag update (before drawing so this frame reflects it)
        self.snap_x = None;
        if let Some(mut drag) = self.drag.take() {
            let (down, pos, shift) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos(), i.modifiers.shift));
            if let Some(p) = pos {
                let view = ed.session.timeline.clone();
                let x_of = |ms: f64| map.x(view.output_to_view(ms));
                let ms_of = |x: f32| view.view_to_output(map.view_ms(x));
                self.snap_x = apply_drag(&mut drag, ed, p.x, shift, map, x_of, ms_of);
            }
            if down {
                self.drag = Some(drag);
                ui.ctx().request_repaint();
            } else {
                commit_drag(ed, drag.snapshot);
                self.snap_x = None;
            }
        }
        if let Some(drag) = self.audio_drag.take() {
            let (down, pos, shift) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos(), i.modifiers.shift));
            if let Some(p) = pos {
                let view = ed.session.timeline.clone();
                let x_of = |ms: f64| map.x(view.output_to_view(ms));
                let ms_of = |x: f32| view.view_to_output(map.view_ms(x));
                self.apply_audio_drag(&drag, ed, p.x, shift, x_of, ms_of);
            }
            if down {
                self.audio_drag = Some(drag);
                ui.ctx().request_repaint();
            } else {
                commit_drag(ed, drag.snapshot);
                self.snap_x = None;
            }
        }
        let duration = ed.duration_ms().max(1.0);
        // x ↔ output ms through this frame's slices (after any live drag above)
        let view = ed.session.timeline.clone();
        let x_of = |ms: f64| map.x(view.output_to_view(ms));
        let ms_of = |x: f32| view.view_to_output(map.view_ms(x));

        // ruler: "0:05" labels with a dot, and a dot halfway between. Output time, so marks
        // crowd together over sped-up clips; labels and dots that would overlap are skipped.
        let ruler = Rect::from_min_size(rect.min, vec2(rect.width(), RULER_H));
        let step_s = nice_step(self.px_per_sec);
        let (mut last_label, mut last_dot) = (f32::MIN, f32::MIN);
        let mut t = 0.0f64;
        while t <= duration / 1000.0 + step_s {
            let x = x_of(t * 1000.0);
            if x > rect.right() + 40.0 {
                break;
            }
            if x >= rect.left() - 40.0 {
                if t > 0.0 && x - last_label >= 56.0 {
                    painter.text(pos2(x, ruler.top() + 8.0), egui::Align2::CENTER_CENTER, fmt_ruler(t), egui::FontId::proportional(13.0), theme::p().ruler_text);
                    last_label = x;
                }
                if x - last_dot >= 8.0 {
                    painter.circle_filled(pos2(x, ruler.bottom() - 3.0), 2.0, theme::p().ruler_text);
                    last_dot = x;
                }
                let xh = x_of((t + step_s / 2.0) * 1000.0);
                if xh - last_dot >= 8.0 {
                    painter.circle_filled(pos2(xh, ruler.bottom() - 3.0), 2.0, theme::p().ruler_text.gamma_multiply(0.6));
                    last_dot = xh;
                }
            }
            t += step_s;
        }

        // marker row: scissors badges at cuts
        let marker_top = ruler.bottom() + 2.0;
        let clip_top = marker_top + MARKER_H + GAP;
        let spans: Vec<_> = ed.session.timeline.spans().to_vec();
        let zooms: Vec<Zoom> = ed.session.pkg.project.scenes[0].zooms.clone();
        let src_duration = ed.session.source.duration_ms;
        let mut seek_to: Option<f64> = None;
        let base_id = ui.id().with("timeline");
        let rec_tracks = crate::audio::AudioPlayer::available_tracks(&ed.session);
        let audio_in_clips = !rec_tracks.is_empty() && !ed.session.pkg.project.scenes[0].is_audio_detached();
        if audio_in_clips {
            self.ensure_peaks(ed, &rec_tracks[0]);
        }
        let main_vol = {
            let a = &ed.session.pkg.project.config.audio;
            match rec_tracks.first() {
                Some(AudioTrack::Microphone) if a.mute_microphone => 0.0,
                Some(AudioTrack::Microphone) => a.volume as f32,
                Some(AudioTrack::SystemAudio) if a.mute_system_audio => 0.0,
                _ => 1.0,
            }
        };
        for span in &spans {
            let x0 = x_of(span.output_start_ms);
            let x1 = x_of(span.output_end_ms);
            let full = Rect::from_min_max(pos2(x0, clip_top), pos2(x1, clip_top + CLIP_H));
            let r = full.intersect(rect);
            if r.width() <= 0.0 {
                continue;
            }
            let selected = ed.selected_slice == Some(span.index);
            let block = Rect::from_min_max(pos2(x0 + 1.5, clip_top), pos2(x1 - 1.5, clip_top + CLIP_H));
            glass_block(&painter, block, 8.0, if selected { theme::p().clip_selected } else { theme::p().clip }, selected);
            // faint vertical "beat" lines like Screen Studio's clip texture
            let mut lx = x0 + 1.5 + 120.0;
            while lx < x1 - 10.0 {
                if lx > rect.left() && lx < rect.right() {
                    painter.line_segment([pos2(lx, clip_top + 10.0), pos2(lx, clip_top + CLIP_H - 10.0)], Stroke::new(1.0, theme::p().clip.gamma_multiply(0.18)));
                }
                lx += 120.0;
            }
            // linked audio: its waveform is drawn inside the video clip, centred
            if audio_in_clips && main_vol > 0.0 {
                if let Some((_, peaks, dur)) = &self.peaks {
                    if *dur > 0.0 && !peaks.is_empty() {
                        let mid = clip_top + CLIP_H - 18.0;
                        let half = 11.0;
                        let mut x = r.left().max(block.left() + 4.0);
                        while x < r.right().min(block.right() - 4.0) {
                            if let Some(src) = ed.session.timeline.output_to_source(ms_of(x)) {
                                let b = ((src / dur) * peaks.len() as f64) as usize;
                                let a = (peaks[b.min(peaks.len() - 1)] * main_vol).min(1.0);
                                painter.line_segment([pos2(x, mid - a * half), pos2(x, mid + a * half)], Stroke::new(1.5, theme::p().clip.gamma_multiply(0.7)));
                            }
                            x += 2.0;
                        }
                    }
                }
            }
            let speed = 1.0 / span.time_scale;
            let secs = span.output_duration_ms() / 1000.0;
            let dur = if secs >= 10.0 { format!("{secs:.0}s") } else { format!("{secs:.1}s") };
            let label = format!("{dur}  ⏱ {}×", trim_float(speed));
            if r.width() > 60.0 {
                if audio_in_clips {
                    // label on top, waveform underneath
                    painter.text(pos2(r.center().x, r.top() + 16.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(14.0), theme::p().text);
                } else {
                    painter.text(pos2(r.center().x, r.top() + 16.0), egui::Align2::CENTER_CENTER, "Clip", egui::FontId::proportional(12.0), theme::p().text_dim);
                    painter.text(pos2(r.center().x, r.bottom() - 18.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(15.0), theme::p().text);
                }
            }
            // cut marker badge at the start of every clip but the first
            if span.index > 0 && x0 >= rect.left() && x0 <= rect.right() {
                let c = pos2(x0, marker_top + 10.0);
                painter.circle_filled(c, 11.0, theme::p().marker);
                painter.text(c, egui::Align2::CENTER_CENTER, "✂", egui::FontId::proportional(12.0), theme::p().clip_text);
                painter.line_segment([pos2(x0, marker_top + 21.0), pos2(x0, clip_top)], Stroke::new(1.0, theme::p().marker));
            }

            let index = span.index;
            let body_id = base_id.with(("clip", index));
            let body = ui.interact(r, body_id, Sense::click_and_drag());
            if let Some(p) = body.interact_pointer_pos() {
                if body.clicked() {
                    ed.selected_slice = Some(index);
                    ed.selected_zoom = None;
                    ed.selected_audio = None;
                    seek_to = Some(ms_of(p.x));
                }
                if body.drag_started() || body.dragged() {
                    seek_to = Some(ms_of(p.x));
                }
                if body.secondary_clicked() {
                    ed.selected_slice = Some(index);
                    ed.selected_zoom = None;
                    ed.selected_audio = None;
                    self.ctx_ms = ms_of(p.x);
                }
            }
            let ctx_ms = self.ctx_ms;
            body.context_menu(|ui| clip_menu(ed, ui, index, ctx_ms));

            // trim handles
            if full.width() > HANDLE_W * 3.0 {
                for is_left in [true, false] {
                    let hr = if is_left { Rect::from_min_max(pos2(x0, clip_top), pos2(x0 + HANDLE_W, clip_top + CLIP_H)) } else { Rect::from_min_max(pos2(x1 - HANDLE_W, clip_top), pos2(x1, clip_top + CLIP_H)) };
                    let hr = hr.intersect(rect);
                    if hr.width() <= 0.0 {
                        continue;
                    }
                    let h = ui.interact(hr, body_id.with(is_left), Sense::drag()).on_hover_cursor(CursorIcon::ResizeHorizontal);
                    if h.hovered() || h.dragged() {
                        painter.rect_filled(hr, 2.0, theme::p().accent.gamma_multiply(0.7));
                    }
                    if h.drag_started() {
                        if let Some(p) = h.interact_pointer_pos() {
                            ed.selected_slice = Some(index);
                            ed.selected_zoom = None;
                            let slices = &ed.session.pkg.project.scenes[0].slices;
                            let (lo, hi) = clip_bounds(slices, index, src_duration);
                            let mut targets: Vec<f64> = spans.iter().filter(|s| s.index <= index).map(|s| s.output_start_ms).collect();
                            let limit = if is_left { span.output_start_ms } else { span.output_end_ms };
                            if !is_left {
                                targets.push(span.output_end_ms);
                            }
                            for z in &zooms {
                                for e in [z.source_start_ms, z.source_end_ms] {
                                    if let Some(o) = ed.session.timeline.source_to_output(e) {
                                        if o < limit {
                                            targets.push(o);
                                        }
                                    }
                                }
                            }
                            self.drag = Some(Drag {
                                kind: DragKind::ClipEdge { index, left: is_left },
                                zoom_id: String::new(),
                                start_x: p.x,
                                src0: span.source_start_ms,
                                src1: span.source_end_ms,
                                out0: span.output_start_ms,
                                out1: span.output_end_ms,
                                lo,
                                hi,
                                targets,
                                snapshot: ed.session.pkg.project.clone(),
                            });
                        }
                    }
                }
            }
        }

        // audio track: its own lane only while detached clips remain; linked audio is drawn
        // inside the video clips above
        let audio_top = clip_top + CLIP_H + GAP;
        if show_audio_lane {
            self.audio_track_ui(ed, ui, rect, audio_top, x_of, ms_of, &mut seek_to);
        }

        // background music: a lane of its own, whether or not audio was recorded
        let music_top = if show_audio_lane { audio_top + AUDIO_H + GAP } else { audio_top };
        if show_music_lane {
            self.music_track_ui(ed, ui, rect, music_top, x_of, ms_of, &mut seek_to);
        }

        // zoom track (only shown once the project has zooms; add them from the tool rail,
        // the sidebar, or the empty-space context menu below the other lanes)
        let zoom_top = if show_music_lane { music_top + MUSIC_H + GAP } else { music_top };
        let track_rect = if has_zooms { Rect::from_min_max(pos2(rect.left(), zoom_top), pos2(rect.right(), zoom_top + ZOOM_H)) } else { Rect::from_min_max(pos2(rect.left(), zoom_top), pos2(rect.right(), rect.bottom().max(zoom_top))) };
        if has_zooms {
            painter.rect(track_rect.shrink2(vec2(1.5, 0.0)), 10.0, theme::p().track_empty, Stroke::new(1.0, theme::p().border), egui::StrokeKind::Inside);
        }
        let track = ui.interact(track_rect, base_id.with("zoomtrack"), Sense::click_and_drag());
        if let Some(p) = track.interact_pointer_pos() {
            if track.double_clicked() {
                let src = ed.session.timeline.output_to_source(ms_of(p.x)).unwrap_or(0.0);
                add_zoom_at(ed, src);
            } else if track.clicked() || track.drag_started() || track.dragged() {
                seek_to = Some(ms_of(p.x));
            }
            if track.secondary_clicked() {
                self.ctx_ms = ms_of(p.x);
            }
        }
        let ctx_ms = self.ctx_ms;
        track.context_menu(|ui| zoom_track_menu(ed, ui, ctx_ms));

        // zoom items (source time; mapped through the slices, so cut parts are hidden)
        let clip_boundaries: Vec<f64> = spans.iter().map(|s| s.output_start_ms).chain(std::iter::once(duration)).collect();
        for z in &zooms {
            let segments = visible_segments(&ed.session.timeline, z.source_start_ms, z.source_end_ms);
            if segments.is_empty() {
                continue;
            }
            let selected = ed.selected_zoom.as_deref() == Some(z.id.as_str());
            let color = if selected { theme::p().zoom_selected } else { theme::p().zoom };
            let color = if z.is_disabled { color.gamma_multiply(0.45) } else { color };
            let mut min_x = f32::MAX;
            let mut max_x = f32::MIN;
            for (o0, o1) in &segments {
                let r = Rect::from_min_max(pos2(x_of(*o0) + 1.5, zoom_top + 4.0), pos2(x_of(*o1) - 1.5, zoom_top + ZOOM_H - 4.0));
                min_x = min_x.min(r.left());
                max_x = max_x.max(r.right());
                glass_block(&painter, r, 8.0, color, selected);
            }
            let item_full = Rect::from_min_max(pos2(min_x, zoom_top), pos2(max_x, zoom_top + ZOOM_H));
            zoom_label(&painter, item_full.shrink2(vec2(6.0, 4.0)).intersect(rect), z);
            let item_rect = item_full.intersect(rect);
            if item_rect.width() <= 0.0 {
                continue;
            }
            let item_id = base_id.with(("zoom", &z.id));
            let item = ui.interact(item_rect, item_id, Sense::click_and_drag()).on_hover_cursor(CursorIcon::Grab);
            if let Some(p) = item.interact_pointer_pos() {
                if item.clicked() {
                    ed.selected_zoom = Some(z.id.clone());
                    ed.selected_slice = None;
                    seek_to = Some(ms_of(p.x));
                }
                if item.secondary_clicked() {
                    ed.selected_zoom = Some(z.id.clone());
                    ed.selected_slice = None;
                    self.ctx_ms = ms_of(p.x);
                }
                if item.drag_started() {
                    ed.selected_zoom = Some(z.id.clone());
                    ed.selected_slice = None;
                    self.drag = Some(zoom_drag(ed, z, &zooms, DragKind::ZoomMove, p.x, &clip_boundaries, src_duration));
                }
            }
            let zid = z.id.clone();
            item.context_menu(|ui| zoom_menu(ed, ui, &zid));

            if item_full.width() > HANDLE_W * 3.0 {
                for is_left in [true, false] {
                    let hr = if is_left { Rect::from_min_max(item_full.left_top(), pos2(item_full.left() + HANDLE_W, item_full.bottom())) } else { Rect::from_min_max(pos2(item_full.right() - HANDLE_W, item_full.top()), item_full.right_bottom()) };
                    let hr = hr.intersect(rect);
                    if hr.width() <= 0.0 {
                        continue;
                    }
                    let h = ui.interact(hr, item_id.with(is_left), Sense::drag()).on_hover_cursor(CursorIcon::ResizeHorizontal);
                    if h.hovered() || h.dragged() {
                        painter.rect_filled(hr, 2.0, theme::p().accent.gamma_multiply(0.7));
                    }
                    if h.drag_started() {
                        if let Some(p) = h.interact_pointer_pos() {
                            ed.selected_zoom = Some(z.id.clone());
                            ed.selected_slice = None;
                            self.drag = Some(zoom_drag(ed, z, &zooms, DragKind::ZoomEdge { left: is_left }, p.x, &clip_boundaries, src_duration));
                        }
                    }
                }
            }
        }

        // scrubbing on empty space / ruler
        if resp.clicked() || resp.drag_started() || resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                seek_to = Some(ms_of(p.x));
            }
        }
        if let Some(ms) = seek_to {
            if self.drag.is_none() {
                ed.pause();
                ed.seek(ms);
            }
        }

        // one selection at a time: picking a clip, zoom or audio clip deselects the music
        if ed.selected_slice.is_some() || ed.selected_zoom.is_some() || ed.selected_audio.is_some() || !show_music_lane {
            ed.selected_music = false;
        }

        // snap guide
        if let Some(x) = self.snap_x {
            if x >= rect.left() && x <= rect.right() {
                painter.line_segment([pos2(x, ruler.bottom()), pos2(x, rect.bottom())], Stroke::new(1.0, SNAP_LINE));
            }
        }

        // playhead
        let px = x_of(ed.playhead_ms);
        if px >= rect.left() && px <= rect.right() {
            painter.line_segment([pos2(px, ruler.bottom() - 2.0), pos2(px, rect.bottom())], Stroke::new(2.0, theme::p().playhead));
            let tri = [pos2(px - 6.0, ruler.bottom() - 10.0), pos2(px + 6.0, ruler.bottom() - 10.0), pos2(px, ruler.bottom() - 1.0)];
            painter.add(egui::Shape::convex_polygon(tri.to_vec(), theme::p().playhead, Stroke::NONE));
        }
        // follow playhead while playing
        if ed.playing && self.follow && (px > rect.right() - 40.0 || px < rect.left()) {
            self.scroll_x = (view.output_to_view(ed.playhead_ms) as f32 / 1000.0 * self.px_per_sec - rect.width() * 0.2).max(0.0);
        }
    }
}

impl TimelineWidget {
    #[allow(clippy::too_many_arguments)]
    /// Refresh the cached waveform of `track` when the audio was reloaded.
    fn ensure_peaks(&mut self, ed: &Editor, track: &AudioTrack) {
        let gen = ed.audio.generation.load(std::sync::atomic::Ordering::Relaxed);
        if self.peaks_gen != gen || self.peaks.as_ref().map(|(t, _, _)| t != track).unwrap_or(true) {
            if let (Some(p), Some(d)) = (ed.audio.peaks(track, 6000), ed.audio.track_duration_ms(track)) {
                self.peaks = Some((track.clone(), p, d));
                self.peaks_gen = gen;
            }
        }
    }

    /// Background music lane: one block over the whole video (the track loops), dimmed while
    /// muted. Click scrubs; right-click mutes, sets the volume or removes the music.
    #[allow(clippy::too_many_arguments)]
    fn music_track_ui(&mut self, ed: &mut Editor, ui: &mut egui::Ui, rect: Rect, top: f32, x_of: impl Fn(f64) -> f32 + Copy, ms_of: impl Fn(f32) -> f64 + Copy, seek_to: &mut Option<f64>) {
        let painter = ui.painter_at(rect);
        let track_rect = Rect::from_min_max(pos2(rect.left(), top), pos2(rect.right(), top + MUSIC_H));
        painter.rect(track_rect.shrink2(vec2(1.5, 0.0)), 10.0, theme::p().track_empty, Stroke::new(1.0, theme::p().border), egui::StrokeKind::Inside);
        let a = ed.session.pkg.project.config.audio.clone();
        let name = a.background_audio_file_name.as_deref().map(sf_session::assets::background_audio_label).unwrap_or_default();
        let x0 = x_of(0.0);
        let x1 = x_of(ed.duration_ms());
        let block = Rect::from_min_max(pos2(x0 + 1.5, top + 3.0), pos2(x1 - 1.5, top + MUSIC_H - 3.0));
        let selected = ed.selected_music;
        if block.intersect(rect).width() > 0.0 {
            let color = if a.mute_background_audio { theme::p().accent.gamma_multiply(0.4) } else { theme::p().accent };
            glass_block(&painter, block, 8.0, color, selected);
            let label = if a.mute_background_audio { format!("Music · {name} · muted") } else { format!("Music · {name} · {:.0}%", a.background_audio_volume * 100.0) };
            let lx = block.left().max(rect.left()) + 12.0;
            let text_rect = Rect::from_min_max(pos2(lx, block.top()), pos2(block.right().min(rect.right()) - 8.0, block.bottom()));
            painter.with_clip_rect(text_rect).text(pos2(lx, block.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(13.0), theme::p().text);
        }
        let lane = ui.interact(track_rect, ui.id().with("musictrack"), Sense::click_and_drag());
        if let Some(p) = lane.interact_pointer_pos() {
            // clicking (or right-clicking) the block selects the music, so ⌫ removes it
            if (lane.clicked() || lane.secondary_clicked()) && p.x >= block.left() && p.x <= block.right() {
                ed.selected_music = true;
                ed.selected_slice = None;
                ed.selected_zoom = None;
                ed.selected_audio = None;
            }
            if lane.clicked() || lane.drag_started() || lane.dragged() {
                *seek_to = Some(ms_of(p.x));
            }
        }
        lane.context_menu(|ui| music_menu(ed, ui));
    }

    #[allow(clippy::too_many_arguments)]
    fn audio_track_ui(&mut self, ed: &mut Editor, ui: &mut egui::Ui, rect: Rect, top: f32, x_of: impl Fn(f64) -> f32 + Copy, ms_of: impl Fn(f32) -> f64 + Copy, seek_to: &mut Option<f64>) {
        let painter = ui.painter_at(rect);
        let track_rect = Rect::from_min_max(pos2(rect.left(), top), pos2(rect.right(), top + AUDIO_H));
        painter.rect(track_rect.shrink2(vec2(1.5, 0.0)), 10.0, theme::p().track_empty, Stroke::new(1.0, theme::p().border), egui::StrokeKind::Inside);
        let tracks = crate::audio::AudioPlayer::available_tracks(&ed.session);
        let base_id = ui.id().with("audiotrack");
        if tracks.is_empty() {
            return;
        }
        let main_track = tracks[0].clone();
        self.ensure_peaks(ed, &main_track);
        let cfg = ed.session.pkg.project.config.clone();
        let spans = sf_engine::audio::audio_spans(&ed.session.pkg.project.scenes[0], &tracks, cfg.audio.volume, cfg.audio.mute_microphone, cfg.audio.mute_system_audio);
        let detached = ed.session.pkg.project.scenes[0].is_audio_detached();

        // empty-area interaction (scrub + context menu)
        let track = ui.interact(track_rect, base_id, Sense::click_and_drag());
        if let Some(p) = track.interact_pointer_pos() {
            if track.clicked() || track.drag_started() || track.dragged() {
                *seek_to = Some(ms_of(p.x));
            }
            if track.secondary_clicked() {
                self.ctx_ms = ms_of(p.x);
            }
        }
        track.context_menu(|ui| {
            if detached {
                if ui.button("Relink audio to video").clicked() {
                    ed.selected_audio = None;
                    ed.edit(|p| p.scenes[0].set_audio_clips(None));
                    ui.close();
                }
            } else if ui.button("Detach audio from video").clicked() {
                let clips = sf_project::audio_clips::clips_from_slices(&ed.session.pkg.project.scenes[0], &tracks);
                ed.edit(move |p| p.scenes[0].set_audio_clips(Some(clips)));
                ed.set_status("Audio detached — split, move and trim it independently");
                ui.close();
            }
        });

        for span in &spans {
            let x0 = x_of(span.output_start_ms);
            let x1 = x_of(span.output_end_ms);
            let full = Rect::from_min_max(pos2(x0 + 1.5, top + 3.0), pos2(x1 - 1.5, top + AUDIO_H - 3.0));
            let r = full.intersect(rect);
            if r.width() <= 0.0 {
                continue;
            }
            let selected = detached && ed.selected_audio.as_deref() == Some(span.id.as_str());
            let is_sys = span.track == AudioTrack::SystemAudio;
            let base = if is_sys { theme::p().meter } else { theme::p().accent_hover };
            glass_block(&painter, full, 8.0, if span.muted { base.gamma_multiply(0.4) } else { base }, selected);
            // waveform
            if let Some((t, peaks, dur)) = &self.peaks {
                if *t == span.track && *dur > 0.0 && !span.muted {
                    let mid = full.center().y;
                    let half = (full.height() / 2.0 - 3.0).max(1.0);
                    let step = 2.0;
                    let mut x = r.left().max(full.left());
                    let mut pts_top = Vec::new();
                    let mut pts_bot = Vec::new();
                    while x < r.right().min(full.right()) {
                        let src = span.output_to_source(ms_of(x));
                        let b = ((src / dur) * peaks.len() as f64) as usize;
                        // shows what you hear: microphone mode is baked into the peaks, volume scales them
                        let a = (peaks.get(b.min(peaks.len() - 1)).copied().unwrap_or(0.0) * span.volume as f32).min(1.0);
                        pts_top.push(pos2(x, mid - a * half));
                        pts_bot.push(pos2(x, mid + a * half));
                        x += step;
                    }
                    if pts_top.len() > 1 {
                        let mut poly = pts_top.clone();
                        poly.extend(pts_bot.iter().rev());
                        painter.add(egui::Shape::convex_polygon(vec![], Color32::TRANSPARENT, Stroke::NONE));
                        for (a, b) in pts_top.iter().zip(pts_bot.iter()) {
                            painter.line_segment([*a, *b], Stroke::new(1.5, base.gamma_multiply(0.75)));
                        }
                    }
                }
            }
            if r.width() > 70.0 {
                let base_label = if span.muted { "Muted" } else if is_sys { "System audio" } else { "Audio" };
                let label = if (span.speed - 1.0).abs() > 1e-6 { format!("{base_label} · {}×", trim_float(span.speed)) } else { base_label.to_string() };
                painter.text(pos2(r.left() + 8.0, full.top() + 9.0), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(10.5), theme::p().text_dim);
            }
            // In linked mode a click/drag on an audio block detaches the audio automatically and
            // continues with the matching clip, so the user can just select and delete.
            let id = if detached {
                span.id.clone()
            } else {
                let probe = ui.interact(r, base_id.with(("linked", &span.id, span.track == AudioTrack::SystemAudio)), Sense::click_and_drag());
                if !(probe.clicked() || probe.drag_started() || probe.secondary_clicked()) {
                    continue;
                }
                let tracks_c = tracks.clone();
                ed.edit(move |p| {
                    let clips = sf_project::audio_clips::clips_from_slices(&p.scenes[0], &tracks_c);
                    p.scenes[0].set_audio_clips(Some(clips));
                });
                ed.set_status("Audio detached from video — split, move, trim or delete it");
                let Some(c) = ed.session.pkg.project.scenes[0].audio_clips().and_then(|cs| cs.into_iter().find(|c| c.track == span.track && (c.output_start_ms - span.output_start_ms).abs() < 1.0)) else { continue };
                ed.selected_audio = Some(c.id.clone());
                ed.selected_zoom = None;
                ed.selected_slice = None;
                if let Some(pp) = probe.interact_pointer_pos() {
                    *seek_to = Some(ms_of(pp.x));
                }
                ui.ctx().request_repaint();
                continue;
            };
            // detached: select / move / trim / menu
            let item = ui.interact(r, base_id.with(("clip", &id)), Sense::click_and_drag()).on_hover_cursor(CursorIcon::Grab);
            if let Some(p) = item.interact_pointer_pos() {
                if item.clicked() {
                    ed.selected_audio = Some(id.clone());
                    ed.selected_zoom = None;
                    ed.selected_slice = None;
                    *seek_to = Some(ms_of(p.x));
                }
                if item.secondary_clicked() {
                    ed.selected_audio = Some(id.clone());
                    self.ctx_ms = ms_of(p.x);
                }
                if item.drag_started() {
                    ed.selected_audio = Some(id.clone());
                    if let Some(c) = ed.session.pkg.project.scenes[0].audio_clips().and_then(|cs| cs.into_iter().find(|c| c.id == id)) {
                        self.audio_drag = Some(AudioDrag { id: id.clone(), kind: AudioDragKind::Move, start_x: p.x, orig: c, snapshot: ed.session.pkg.project.clone() });
                    }
                }
            }
            let ctx_ms = self.ctx_ms;
            let menu_id = id.clone();
            item.context_menu(|ui| audio_menu(ed, ui, &menu_id, ctx_ms));
            if full.width() > HANDLE_W * 3.0 {
                for is_left in [true, false] {
                    let hr = if is_left { Rect::from_min_max(full.left_top(), pos2(full.left() + HANDLE_W, full.bottom())) } else { Rect::from_min_max(pos2(full.right() - HANDLE_W, full.top()), full.right_bottom()) };
                    let hr = hr.intersect(rect);
                    if hr.width() <= 0.0 {
                        continue;
                    }
                    let h = ui.interact(hr, base_id.with(("h", &id, is_left)), Sense::drag()).on_hover_cursor(CursorIcon::ResizeHorizontal);
                    if h.hovered() || h.dragged() {
                        painter.rect_filled(hr, 2.0, theme::p().accent.gamma_multiply(0.7));
                    }
                    if h.drag_started() {
                        if let Some(p) = h.interact_pointer_pos() {
                            if let Some(c) = ed.session.pkg.project.scenes[0].audio_clips().and_then(|cs| cs.into_iter().find(|c| c.id == id)) {
                                ed.selected_audio = Some(id.clone());
                                self.audio_drag = Some(AudioDrag { id: id.clone(), kind: if is_left { AudioDragKind::Left } else { AudioDragKind::Right }, start_x: p.x, orig: c, snapshot: ed.session.pkg.project.clone() });
                            }
                        }
                    }
                }
            }
        }

    }

    fn apply_audio_drag(&mut self, drag: &AudioDrag, ed: &mut Editor, x: f32, shift: bool, x_of: impl Fn(f64) -> f32 + Copy, ms_of: impl Fn(f32) -> f64) {
        // edges follow the pointer on screen (a pixel is not the same time everywhere)
        let dx = x - drag.start_x;
        let moved = |ms: f64| ms_of(x_of(ms) + dx);
        let src_duration = ed.session.source.duration_ms;
        let scene = &ed.session.pkg.project.scenes[0];
        let Some(mut clips) = scene.audio_clips() else { return };
        let Some(i) = clips.iter().position(|c| c.id == drag.id) else { return };
        let spans: Vec<f64> = ed.session.timeline.spans().iter().map(|s| s.output_start_ms).chain(std::iter::once(ed.duration_ms())).collect();
        let others: Vec<f64> = clips.iter().filter(|c| c.id != drag.id).flat_map(|c| [c.output_start_ms, c.output_end_ms()]).collect();
        let targets: Vec<f64> = spans.into_iter().chain(others).collect();
        let o = &drag.orig;
        self.snap_x = None;
        match drag.kind {
            AudioDragKind::Move => {
                let mut start = moved(o.output_start_ms).max(0.0);
                if !shift {
                    if let Some(sn) = snap(&targets, ed.playhead_ms, start, &x_of) {
                        start = sn;
                        self.snap_x = Some(x_of(sn));
                    } else if let Some(sn) = snap(&targets, ed.playhead_ms, start + o.output_duration_ms(), &x_of) {
                        start = sn - o.output_duration_ms();
                        self.snap_x = Some(x_of(sn));
                    }
                }
                clips[i].output_start_ms = start.max(0.0);
            }
            AudioDragKind::Left => {
                let mut start = moved(o.output_start_ms);
                if !shift {
                    if let Some(sn) = snap(&targets, ed.playhead_ms, start, &x_of) {
                        start = sn;
                        self.snap_x = Some(x_of(sn));
                    }
                }
                let max_start = o.output_end_ms() - AudioClip::MIN_DURATION_MS / o.speed;
                let start = start.clamp((o.output_start_ms - o.source_start_ms / o.speed).max(0.0), max_start);
                let delta_src = (start - o.output_start_ms) * o.speed;
                clips[i].output_start_ms = start;
                clips[i].source_start_ms = (o.source_start_ms + delta_src).max(0.0);
            }
            AudioDragKind::Right => {
                let mut end = moved(o.output_end_ms());
                if !shift {
                    if let Some(sn) = snap(&targets, ed.playhead_ms, end, &x_of) {
                        end = sn;
                        self.snap_x = Some(x_of(sn));
                    }
                }
                let max_end = o.output_start_ms + (src_duration - o.source_start_ms) / o.speed;
                let end = end.clamp(o.output_start_ms + AudioClip::MIN_DURATION_MS / o.speed, max_end);
                clips[i].source_end_ms = o.source_start_ms + (end - o.output_start_ms) * o.speed;
            }
        }
        ed.session.pkg.project.scenes[0].set_audio_clips(Some(clips));
        ed.session.rebuild();
        ed.audio.update_timeline(&ed.session);
        ed.invalidate_preview();
    }
}

fn audio_menu(ed: &mut Editor, ui: &mut egui::Ui, id: &str, at_ms: f64) {
    let Some(clips) = ed.session.pkg.project.scenes[0].audio_clips() else { return };
    let Some(clip) = clips.iter().find(|c| c.id == id).cloned() else { return };
    let idc = id.to_string();
    if ui.button("Split audio here").clicked() {
        ed.edit(move |p| {
            let scene = &mut p.scenes[0];
            if let Some(mut cs) = scene.audio_clips() {
                if let Some(i) = cs.iter().position(|c| c.id == idc) {
                    if let Some(r) = cs[i].split_at_output(at_ms) {
                        cs.insert(i + 1, r);
                    }
                }
                scene.set_audio_clips(Some(cs));
            }
        });
        ui.close();
    }
    ui.menu_button(format!("Speed ({}×)", clip.speed), |ui| {
        for sp in SPEEDS {
            if ui.selectable_label((sp - clip.speed).abs() < 1e-6, format!("{sp}×")).clicked() {
                ed.set_audio_clip_speed(id, sp);
                ui.close();
            }
        }
    });
    let idc = id.to_string();
    if ui.button(if clip.muted { "Unmute" } else { "Mute" }).clicked() {
        ed.edit(move |p| {
            let scene = &mut p.scenes[0];
            if let Some(mut cs) = scene.audio_clips() {
                if let Some(c) = cs.iter_mut().find(|c| c.id == idc) {
                    c.muted = !c.muted;
                }
                scene.set_audio_clips(Some(cs));
            }
        });
        ui.close();
    }
    ui.menu_button("Volume", |ui| {
        for v in [0.0, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0] {
            let idc = id.to_string();
            if ui.selectable_label((clip.volume - v).abs() < 1e-6, format!("{:.0}%", v * 100.0)).clicked() {
                ed.edit(move |p| {
                    let scene = &mut p.scenes[0];
                    if let Some(mut cs) = scene.audio_clips() {
                        if let Some(c) = cs.iter_mut().find(|c| c.id == idc) {
                            c.volume = v;
                        }
                        scene.set_audio_clips(Some(cs));
                    }
                });
                ui.close();
            }
        }
    });
    let idc = id.to_string();
    if ui.button("Delete audio clip").clicked() {
        ed.selected_audio = None;
        ed.edit(move |p| {
            let scene = &mut p.scenes[0];
            if let Some(mut cs) = scene.audio_clips() {
                cs.retain(|c| c.id != idc);
                scene.set_audio_clips(Some(cs));
            }
        });
        ui.close();
    }
    ui.separator();
    if ui.button("Relink audio to video").clicked() {
        ed.selected_audio = None;
        ed.edit(|p| p.scenes[0].set_audio_clips(None));
        ui.close();
    }
}

/// The recorded-audio lane has something to show: detached clips that are left.
fn audio_lane_visible(ed: &Editor) -> bool {
    !crate::audio::AudioPlayer::available_tracks(&ed.session).is_empty() && ed.session.pkg.project.scenes[0].audio_clips().is_some_and(|cs| !cs.is_empty())
}

/// A background track is chosen and found (shown even while muted).
fn music_lane_visible(ed: &Editor) -> bool {
    ed.session.pkg.project.config.audio.background_audio_file_name.is_some() && ed.session.background_audio_path().is_some()
}

fn music_menu(ed: &mut Editor, ui: &mut egui::Ui) {
    let a = ed.session.pkg.project.config.audio.clone();
    if ui.button(if a.mute_background_audio { "Unmute music" } else { "Mute music" }).clicked() {
        ed.edit(|p| p.config.audio.mute_background_audio = !p.config.audio.mute_background_audio);
        ed.reload_audio();
        ui.close();
    }
    ui.menu_button("Volume", |ui| {
        for v in [0.05, 0.1, 0.2, 0.3, 0.5, 0.75, 1.0] {
            if ui.selectable_label((a.background_audio_volume - v).abs() < 1e-6, format!("{:.0}%", v * 100.0)).clicked() {
                ed.edit(move |p| {
                    p.config.audio.background_audio_volume = v;
                    p.config.audio.mute_background_audio = false;
                });
                ed.reload_audio();
                ui.close();
            }
        }
    });
    ui.separator();
    if ui.button("Remove music").clicked() {
        ed.selected_music = false;
        ed.edit(|p| p.config.audio.background_audio_file_name = None);
        ed.reload_audio();
        ui.close();
    }
}

// ───────────── drag helpers ─────────────

/// Allowed source range for trimming slice `index`: the previous slice's end (if it precedes
/// this one in source time) up to the next slice's start, inside the recording.
fn clip_bounds(slices: &[sf_project::v4::Slice], index: usize, src_duration: f64) -> (f64, f64) {
    let s = &slices[index];
    let mut lo = 0.0f64;
    if index > 0 {
        let prev = &slices[index - 1];
        if prev.source_end_ms <= s.source_start_ms + 1.0 {
            lo = prev.source_end_ms;
        }
    }
    let mut hi = src_duration.max(s.source_end_ms);
    if index + 1 < slices.len() {
        let next = &slices[index + 1];
        if next.source_start_ms >= s.source_end_ms - 1.0 {
            hi = next.source_start_ms;
        }
    }
    (lo, hi)
}

/// Allowed source range for zoom `z` so it does not overlap the other zooms.
fn zoom_bounds(z: &Zoom, zooms: &[Zoom], src_duration: f64) -> (f64, f64) {
    let mut lo = 0.0f64;
    let mut hi = src_duration.max(z.source_end_ms);
    for o in zooms {
        if o.id == z.id {
            continue;
        }
        if o.source_end_ms <= z.source_start_ms + 1.0 {
            lo = lo.max(o.source_end_ms);
        } else if o.source_start_ms >= z.source_end_ms - 1.0 {
            hi = hi.min(o.source_start_ms);
        }
    }
    (lo, hi)
}

fn zoom_drag(ed: &Editor, z: &Zoom, zooms: &[Zoom], kind: DragKind, start_x: f32, clip_boundaries: &[f64], src_duration: f64) -> Drag {
    let (lo, hi) = zoom_bounds(z, zooms, src_duration);
    let mut targets = clip_boundaries.to_vec();
    for o in zooms {
        if o.id == z.id {
            continue;
        }
        for e in [o.source_start_ms, o.source_end_ms] {
            if let Some(out) = ed.session.timeline.source_to_output(e) {
                targets.push(out);
            }
        }
    }
    Drag {
        kind,
        zoom_id: z.id.clone(),
        start_x,
        src0: z.source_start_ms,
        src1: z.source_end_ms,
        out0: 0.0,
        out1: 0.0,
        lo,
        hi,
        targets,
        snapshot: ed.session.pkg.project.clone(),
    }
}

/// Nearest snap target (output ms) within `SNAP_PX` of output time `v`.
fn snap(targets: &[f64], playhead: f64, v: f64, x_of: impl Fn(f64) -> f32) -> Option<f64> {
    let vx = x_of(v);
    targets
        .iter()
        .copied()
        .chain(std::iter::once(playhead))
        .map(|t| (t, (x_of(t) - vx).abs()))
        .filter(|(_, d)| *d <= SNAP_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(t, _)| t)
}

/// Apply the pointer position to the dragged item (live, no undo). Returns the snap guide x.
fn apply_drag(drag: &mut Drag, ed: &mut Editor, x: f32, shift: bool, map: ViewMap, x_of: impl Fn(f64) -> f32 + Copy, ms_of: impl Fn(f32) -> f64 + Copy) -> Option<f32> {
    let playhead = ed.playhead_ms;
    let mut snap_x = None;
    let tl = Timeline::from_scene(&drag.snapshot.scenes[0]);
    match drag.kind {
        DragKind::ClipEdge { index, left } => {
            // the clip is drawn at its source length, so pointer travel on the view axis is
            // source time; work in the pre-drag layout, which the targets come from
            let edge = tl.output_to_view(if left { drag.out0 } else { drag.out1 });
            let mut ds = map.view_ms(x) - map.view_ms(drag.start_x);
            if !shift {
                let vx = map.x(edge + ds);
                let best = drag.targets.iter().copied().chain(std::iter::once(playhead)).map(|t| tl.output_to_view(t)).map(|v| (v, (map.x(v) - vx).abs())).filter(|(_, d)| *d <= SNAP_PX).min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((v, _)) = best {
                    ds = v - edge;
                    snap_x = Some(map.x(v));
                }
            }
            let p = &mut ed.session.pkg.project;
            let slices = &p.scenes[0].slices;
            if index >= slices.len() {
                return None;
            }
            let (ns, ne) = if left { ((drag.src0 + ds).clamp(drag.lo, drag.src1 - MIN_MS), drag.src1) } else { (drag.src0, (drag.src1 + ds).clamp(drag.src0 + MIN_MS, drag.hi)) };
            let (ns, ne) = (ns.round(), ne.round());
            let cur = &slices[index];
            if cur.source_start_ms != ns || cur.source_end_ms != ne {
                p.scenes[0].slices = edit::trim(slices, index, ns, ne);
                ed.session.rebuild();
                ed.invalidate_preview();
            }
        }
        DragKind::ZoomMove | DragKind::ZoomEdge { .. } => {
            let o2s = |x: f32| tl.output_to_source(ms_of(x)).unwrap_or(0.0);
            let ds = o2s(x) - o2s(drag.start_x);
            let dur = drag.src1 - drag.src0;
            let (mut ns, mut ne) = match drag.kind {
                DragKind::ZoomMove => (drag.src0 + ds, drag.src0 + ds + dur),
                DragKind::ZoomEdge { left: true } => (drag.src0 + ds, drag.src1),
                _ => (drag.src0, drag.src1 + ds),
            };
            if !shift {
                // snap whichever moving edge is closest to a target
                let mut best: Option<(f32, f64, bool)> = None; // (px distance, target, is_start)
                let moving: &[(f64, bool)] = match drag.kind {
                    DragKind::ZoomMove => &[(ns, true), (ne, false)],
                    DragKind::ZoomEdge { left: true } => &[(ns, true)],
                    _ => &[(ne, false)],
                };
                for (v, is_start) in moving.iter().copied() {
                    if let Some(vo) = tl.source_to_output(v) {
                        if let Some(t) = snap(&drag.targets, playhead, vo, x_of) {
                            let dist = (x_of(t) - x_of(vo)).abs();
                            if best.map(|b| dist < b.0).unwrap_or(true) {
                                best = Some((dist, t, is_start));
                            }
                        }
                    }
                }
                if let Some((_, t, is_start)) = best {
                    if let Some(ts) = tl.output_to_source(t) {
                        match drag.kind {
                            DragKind::ZoomMove => {
                                if is_start {
                                    ns = ts;
                                    ne = ts + dur;
                                } else {
                                    ne = ts;
                                    ns = ts - dur;
                                }
                            }
                            DragKind::ZoomEdge { left: true } => ns = ts,
                            _ => ne = ts,
                        }
                        snap_x = Some(x_of(t));
                    }
                }
            }
            // clamp inside the recording and away from neighbouring zooms
            match drag.kind {
                DragKind::ZoomMove => {
                    let max_start = drag.hi - dur;
                    if max_start >= drag.lo {
                        ns = ns.clamp(drag.lo, max_start);
                    } else {
                        ns = drag.src0;
                    }
                    ne = ns + dur;
                }
                DragKind::ZoomEdge { left: true } => ns = ns.clamp(drag.lo, drag.src1 - MIN_MS),
                _ => ne = ne.clamp(drag.src0 + MIN_MS, drag.hi),
            }
            let (ns, ne) = (ns.round(), ne.round());
            let id = drag.zoom_id.clone();
            let p = &mut ed.session.pkg.project;
            if let Some(z) = p.scenes[0].zooms.iter_mut().find(|z| z.id == id) {
                if z.source_start_ms != ns || z.source_end_ms != ne {
                    z.source_start_ms = ns;
                    z.source_end_ms = ne;
                    ed.session.rebuild();
                    ed.invalidate_preview();
                }
            }
        }
    }
    snap_x
}

/// Push one undo entry for a finished live drag: restore the pre-drag snapshot and re-apply
/// the final state through `Editor::edit`.
fn commit_drag(ed: &mut Editor, snapshot: Project) {
    let current = ed.session.pkg.project.clone();
    if current == snapshot {
        return;
    }
    ed.session.pkg.project = snapshot;
    ed.edit(move |p| *p = current);
}

/// Output-time segments of source range `[s0, s1)` that are not cut.
fn visible_segments(tl: &Timeline, s0: f64, s1: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for span in tl.spans() {
        let a = s0.max(span.source_start_ms);
        let b = s1.min(span.source_end_ms);
        if b > a {
            out.push((span.source_to_output(a), span.source_to_output(b)));
        }
    }
    out
}

// ───────────── zoom edits ─────────────

/// Insert a 3 s automatic zoom at source time `src`, clamped into free space.
fn add_zoom_at(ed: &mut Editor, src: f64) {
    let src_duration = ed.session.source.duration_ms;
    let zooms = &ed.session.pkg.project.scenes[0].zooms;
    if zooms.iter().any(|z| src >= z.source_start_ms && src < z.source_end_ms) {
        ed.set_status("There is already a zoom here");
        return;
    }
    let next_start = zooms.iter().map(|z| z.source_start_ms).filter(|s| *s >= src).fold(f64::INFINITY, f64::min);
    let end = (src + 3000.0).min(src_duration).min(next_start);
    if end - src < MIN_MS {
        ed.set_status("Not enough room for a zoom here");
        return;
    }
    ed.edit(move |p| p.scenes[0].zooms.push(Zoom::auto(src, end, Zoom::DEFAULT_LEVEL)));
    ed.set_status("Added zoom");
}

fn zoom_index(ed: &Editor, id: &str) -> Option<usize> {
    ed.session.pkg.project.scenes[0].zooms.iter().position(|z| z.id == id)
}

/// Zoom item label: painted magnifier + "Zoom 2×" ("Manual 2×", "2× off"), dropping the word
/// and then the icon as the block narrows, and clipped to the block so it never spills out.
fn zoom_label(painter: &egui::Painter, inner: Rect, z: &Zoom) {
    if inner.width() <= 0.0 {
        return;
    }
    let level = format!("{}×", trim_float(z.zoom));
    let long = if z.is_disabled { format!("{level} off") } else if z.is_auto() { format!("Zoom {level}") } else { format!("Manual {level}") };
    let font = egui::FontId::proportional(13.0);
    let (icon, gap) = (12.0, 4.0);
    let color = theme::p().text;
    let painter = painter.with_clip_rect(inner);
    for (text, with_icon) in [(long, true), (level.clone(), true), (level, false)] {
        let galley = painter.layout_no_wrap(text, font.clone(), color);
        let w = galley.size().x + if with_icon { icon + gap } else { 0.0 };
        if w > inner.width() {
            continue;
        }
        let mut x = inner.center().x - w / 2.0;
        if with_icon {
            let r = Rect::from_center_size(pos2(x + icon / 2.0, inner.center().y), vec2(icon, icon));
            crate::widgets::paint_icon(&painter, r, crate::widgets::Icon::Zoom, color);
            x += icon + gap;
        }
        painter.galley(pos2(x, inner.center().y - galley.size().y / 2.0), galley, color);
        return;
    }
}

/// Translucent timeline block: a light tint of `color` with a thin outline in the same colour
/// (stronger when selected), so the lanes read as glass over the theme background.
fn glass_block(painter: &egui::Painter, r: Rect, radius: f32, color: Color32, selected: bool) {
    let fill = color.gamma_multiply(if selected { 0.42 } else { 0.26 });
    let stroke = if selected { Stroke::new(2.0, color) } else { Stroke::new(1.0, color.gamma_multiply(0.75)) };
    painter.rect(r, radius, fill, stroke, egui::StrokeKind::Inside);
}

// ───────────── context menus ─────────────

fn clip_menu(ed: &mut Editor, ui: &mut egui::Ui, index: usize, at_ms: f64) {
    let slices = ed.session.pkg.project.scenes[0].slices.clone();
    let Some(slice) = slices.get(index) else { return };
    ui.set_min_width(160.0);
    let rec_tracks = crate::audio::AudioPlayer::available_tracks(&ed.session);
    if !rec_tracks.is_empty() && !ed.session.pkg.project.scenes[0].is_audio_detached() {
        if ui.button("Detach audio").on_hover_text("Move the audio to its own track to split, move, trim or speed it up separately").clicked() {
            let clips = sf_project::audio_clips::clips_from_slices(&ed.session.pkg.project.scenes[0], &rec_tracks);
            ed.edit(move |p| p.scenes[0].set_audio_clips(Some(clips)));
            ed.set_status("Audio detached: edit it on its own track");
            ui.close();
        }
        ui.separator();
    } else if !rec_tracks.is_empty() {
        // also reachable here because the audio lane disappears once its last clip is deleted
        if ui.button("Relink audio to video").clicked() {
            ed.selected_audio = None;
            ed.edit(|p| p.scenes[0].set_audio_clips(None));
            ui.close();
        }
        ui.separator();
    }
    if ui.button("Split here").clicked() {
        ed.edit(move |p| p.scenes[0].slices = edit::split_at_output(&p.scenes[0].slices, at_ms));
        ed.set_status("Split");
    }
    if ui.add_enabled(slices.len() > 1, egui::Button::new("Delete")).clicked() {
        ed.selected_slice = None;
        ed.edit(move |p| p.scenes[0].slices = edit::delete(&p.scenes[0].slices, index));
        ed.set_status("Deleted clip");
    }
    let speed = 1.0 / slice.time_scale;
    ui.menu_button(format!("Speed ({speed}×)"), |ui| {
        for s in SPEEDS {
            let sel = (speed - s).abs() < 1e-6;
            if ui.selectable_label(sel, format!("{s}×")).clicked() {
                if !sel {
                    ed.edit_speed(move |p| p.scenes[0].slices = edit::set_speed(&p.scenes[0].slices, index, s));
                }
                ui.close();
            }
        }
    });
    let mut hide = slice.hide_cursor;
    if ui.checkbox(&mut hide, "Hide cursor").changed() {
        ed.edit(move |p| p.scenes[0].slices[index].hide_cursor = hide);
    }
    let mut no_smooth = slice.disable_smooth_mouse_movement;
    if ui.checkbox(&mut no_smooth, "Disable smooth cursor").changed() {
        ed.edit(move |p| p.scenes[0].slices[index].disable_smooth_mouse_movement = no_smooth);
    }
    ui.separator();
    let contiguous = slices.get(index + 1).map(|n| (slice.source_end_ms - n.source_start_ms).abs() < 1.0).unwrap_or(false);
    if ui.add_enabled(contiguous, egui::Button::new("Merge with next")).clicked() {
        ed.edit(move |p| p.scenes[0].slices = edit::merge_with_next(&p.scenes[0].slices, index));
        ed.set_status("Merged clips");
    }
    let (lo, hi) = clip_bounds(&slices, index, ed.session.source.duration_ms);
    let can_reset = (slice.source_start_ms - lo).abs() > 0.5 || (slice.source_end_ms - hi).abs() > 0.5;
    if ui.add_enabled(can_reset, egui::Button::new("Reset trim")).on_hover_text("Extend the clip back to the neighbouring clips / recording bounds").clicked() {
        ed.edit(move |p| p.scenes[0].slices = edit::trim(&p.scenes[0].slices, index, lo, hi));
        ed.set_status("Trim reset");
    }
}

fn zoom_menu(ed: &mut Editor, ui: &mut egui::Ui, id: &str) {
    let Some(i) = zoom_index(ed, id) else { return };
    let z = ed.session.pkg.project.scenes[0].zooms[i].clone();
    ui.set_min_width(160.0);
    if ui.button("Remove").clicked() {
        ed.selected_zoom = None;
        ed.edit(move |p| { p.scenes[0].zooms.remove(i); });
        ed.set_status("Removed zoom");
    }
    if ui.button(if z.is_disabled { "Enable" } else { "Disable" }).clicked() {
        let v = !z.is_disabled;
        ed.edit(move |p| p.scenes[0].zooms[i].is_disabled = v);
    }
    if ui.button("Duplicate").clicked() {
        let src_duration = ed.session.source.duration_ms;
        let zooms = &ed.session.pkg.project.scenes[0].zooms;
        let next_start = zooms.iter().map(|o| o.source_start_ms).filter(|s| *s >= z.source_end_ms - 1.0).fold(f64::INFINITY, f64::min);
        let start = z.source_end_ms;
        let end = (start + z.duration_ms()).min(src_duration).min(next_start);
        if end - start >= MIN_MS {
            let mut dup = z.clone();
            dup.id = sf_project::ids::new_id();
            dup.source_start_ms = start;
            dup.source_end_ms = end;
            let dup_id = dup.id.clone();
            ed.edit(move |p| p.scenes[0].zooms.insert(i + 1, dup));
            ed.selected_zoom = Some(dup_id);
        } else {
            ed.set_status("No room after this zoom to duplicate it");
        }
    }
    ui.menu_button(format!("Zoom level ({:.2}×)", z.zoom), |ui| {
        for l in ZOOM_LEVELS {
            let sel = (z.zoom - l).abs() < 1e-6;
            if ui.selectable_label(sel, format!("{l}×")).clicked() {
                if !sel {
                    ed.edit(move |p| p.scenes[0].zooms[i].zoom = l);
                }
                ui.close();
            }
        }
    });
    let src = ed.session.timeline.output_to_source(ed.playhead_ms).unwrap_or(-1.0);
    let can_split = src > z.source_start_ms + MIN_MS && src < z.source_end_ms - MIN_MS;
    if ui.add_enabled(can_split, egui::Button::new("Split zoom at playhead")).clicked() {
        let src = src.round();
        let mut right = z.clone();
        right.id = sf_project::ids::new_id();
        right.source_start_ms = src;
        ed.edit(move |p| {
            p.scenes[0].zooms[i].source_end_ms = src;
            p.scenes[0].zooms.insert(i + 1, right);
        });
        ed.set_status("Split zoom");
    }
}

fn zoom_track_menu(ed: &mut Editor, ui: &mut egui::Ui, at_ms: f64) {
    ui.set_min_width(160.0);
    if ui.button("Add zoom here").clicked() {
        let src = ed.session.timeline.output_to_source(at_ms).unwrap_or(0.0);
        add_zoom_at(ed, src);
    }
    if ui.button("Auto zooms").on_hover_text("Replace all zooms with automatic ones generated from clicks").clicked() {
        let zooms = ed.session.auto_zooms();
        ed.selected_zoom = None;
        ed.edit(move |p| p.scenes[0].zooms = zooms);
        ed.set_status("Generated automatic zooms");
    }
    let n = ed.session.pkg.project.scenes[0].zooms.len();
    if ui.add_enabled(n > 0, egui::Button::new("Remove all zooms")).clicked() {
        ed.selected_zoom = None;
        ed.edit(|p| p.scenes[0].zooms.clear());
        ed.set_status("Removed all zooms");
    }
}

// ───────────── keyboard ─────────────

/// Move the playhead to the previous / next clip boundary (`[` / `]`).
pub fn jump_boundary(ed: &mut Editor, forward: bool) {
    let t = ed.playhead_ms;
    let spans = ed.session.timeline.spans();
    let boundaries = spans.iter().map(|s| s.output_start_ms).chain(std::iter::once(ed.session.timeline.duration_ms()));
    let target = if forward { boundaries.filter(|b| *b > t + 1.0).fold(None, |m: Option<f64>, b| Some(m.map_or(b, |m| m.min(b)))) } else { boundaries.filter(|b| *b < t - 1.0).fold(None, |m: Option<f64>, b| Some(m.map_or(b, |m| m.max(b)))) };
    if let Some(b) = target {
        ed.pause();
        ed.seek(b);
    }
}

fn fmt_ruler(secs: f64) -> String {
    let m = (secs / 60.0).floor() as u32;
    let s = secs - m as f64 * 60.0;
    if secs.fract().abs() < 1e-6 { format!("{m}:{:02}", s as u32) } else { format!("{m}:{s:04.1}") }
}

fn trim_float(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 { format!("{}", v.round() as i64) } else { format!("{v:.2}").trim_end_matches('0').trim_end_matches('.').to_string() }
}

fn nice_step(px_per_sec: f32) -> f64 {
    let min_px = 70.0;
    for s in [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0] {
        if s as f32 * px_per_sec >= min_px {
            return s;
        }
    }
    600.0
}
