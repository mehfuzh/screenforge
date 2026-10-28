//! Titles, overlay media and transitions in the editor: adding them from the tool rail, their
//! lanes on the timeline, the inspector of the selected title or overlay, the transition of the
//! selected clip, and moving a title or overlay on the preview.
//!
//! Titles and overlays live in the scene's `sfTracks` (see `sf_project::tracks`); the session
//! keeps a copy with linked clips moved to where their moment plays, which is what the lanes
//! draw. Edits write the start back together with the link, so a clip stays pinned to the
//! moment it was moved to.

use egui::{pos2, vec2, CursorIcon, Pos2, Rect, RichText, Sense, Stroke};
use sf_engine::tracks::clip_start;
use sf_project::tracks::{self, Clip, ClipAnim, Link, MediaItem, MediaKind, SceneTracksExt};
use sf_project::v4::Transition;
use sf_project::Project;

use crate::editor::Editor;
use crate::sidebar::cfg_slider;
use crate::{theme, widgets};

/// Height of one title or overlay lane on the timeline, and the gap between lanes.
pub const LANE_H: f32 = 40.0;
const LANE_GAP: f32 = 4.0;
const HANDLE_W: f32 = 6.0;
/// How long a new image overlay shows.
const IMAGE_MS: f64 = 4000.0;

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    Move,
    TrimStart,
    TrimEnd,
}

/// A title or overlay being moved or trimmed on the timeline.
pub struct LaneDrag {
    id: String,
    kind: DragKind,
    /// Pointer's distance from the clip start when the move began.
    grab_ms: f64,
    /// The clip as drawn when the drag began (start resolved from its link).
    clip: Clip,
    /// Length of the clip's video, so trimming stops at its ends.
    media_ms: Option<f64>,
    snapshot: Project,
}

/// A title or overlay being moved on the preview.
pub struct PreviewDrag {
    id: String,
    from: Pos2,
    /// Its rect when the drag began (output px).
    rect: sf_project::schema::common::Rect,
    clip: Clip,
    snapshot: Project,
}

#[derive(Default)]
pub struct LayerUi {
    pub lane_drag: Option<LaneDrag>,
    pub preview_drag: Option<PreviewDrag>,
    /// The project before typing into a title began; one undo step when the field loses focus.
    text_snapshot: Option<Project>,
    /// The title whose words are being typed on the preview, and whether its field has had
    /// focus yet (it is focused once, on the first frame).
    editing: Option<(String, bool)>,
    /// The project before that typing began.
    inline_snapshot: Option<Project>,
}

// ───────────── editing helpers ─────────────

/// Apply `f` to clip `id` as one undoable edit.
fn edit_clip(ed: &mut Editor, id: &str, f: impl FnOnce(&mut Clip) + 'static) {
    let id = id.to_string();
    ed.edit(move |p| {
        let mut t = p.scenes[0].tracks();
        if let Some(c) = tracks::find_clip_mut(&mut t, &id) {
            f(c);
        }
        p.scenes[0].set_tracks(t);
    });
}

/// Apply `f` to clip `id` without an undo step (a drag or typing in progress).
fn live_clip(ed: &mut Editor, id: &str, f: impl FnOnce(&mut Clip)) {
    let mut t = ed.session.pkg.project.scenes[0].tracks();
    if let Some(c) = tracks::find_clip_mut(&mut t, id) {
        f(c);
    }
    ed.session.pkg.project.scenes[0].set_tracks(t);
    ed.session.rebuild();
    ed.invalidate_preview();
}

/// The clip with `id` as the session draws it (linked start resolved).
fn drawn_clip(ed: &Editor, id: &str) -> Option<Clip> {
    ed.session.visuals.tracks.iter().flat_map(|t| &t.clips).find(|c| c.id == id).cloned()
}

/// Move a clip to output time `start`, re-pinning it to the recording there when it is pinned
/// (a pinned clip moved onto a title card is unpinned: the card has no recording).
fn set_start(c: &mut Clip, start: f64, timeline: &sf_engine::timeline::Timeline) {
    c.output_start_ms = start;
    if timeline.span_at_output(start).is_some_and(|s| s.card) {
        c.link = None;
    } else if c.link.is_some() {
        if let Some(src) = timeline.output_to_source(start) {
            c.link = Some(Link { source_ms: src });
        }
    }
}

fn add(ed: &mut Editor, clip: Clip, media: Option<MediaItem>) {
    let id = clip.id.clone();
    ed.edit(move |p| {
        let scene = &mut p.scenes[0];
        if let Some(m) = media {
            scene.add_media(m);
        }
        let mut t = scene.tracks();
        tracks::add_clip(&mut t, clip);
        scene.set_tracks(t);
    });
    ed.select_clip(id);
}

/// A new clip at the playhead, at most `ms` long, pinned to the recording there (on a title
/// card, which has no recording, it stays at its time).
fn at_playhead(ed: &Editor, mut clip: Clip, ms: f64) -> Clip {
    let t = ed.playhead_ms.min((ed.duration_ms() - Clip::MIN_DURATION_MS).max(0.0));
    clip.output_start_ms = t;
    clip.set_output_duration(ms.min(ed.duration_ms() - t));
    let on_card = ed.session.timeline.span_at_output(t).is_some_and(|s| s.card);
    clip.link = if on_card { None } else { ed.session.timeline.output_to_source(t).map(|s| Link { source_ms: s }) };
    clip
}

/// Add a title at the playhead and select it.
pub fn add_title(ed: &mut Editor) {
    let clip = at_playhead(ed, Clip::title(0.0, Clip::TITLE_MS, "Your *title* here"), Clip::TITLE_MS);
    add(ed, clip, None);
    ed.set_status("Added a title: write it in the sidebar, drag it on the preview");
}

/// Copy an image or video into the project's media folder, a video with its length (the
/// tool rail and the agent).
pub fn import(package: &std::path::Path, path: &std::path::Path) -> Result<MediaItem, String> {
    match MediaItem::kind_of(path) {
        Some(MediaKind::Image) | Some(MediaKind::Video) => {}
        _ => return Err(format!("{} is not an image or a video", path.display())),
    }
    let probe = (MediaItem::kind_of(path) == Some(MediaKind::Video)).then(|| sf_media::probe::probe(path)).transpose().map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let duration = probe.as_ref().and_then(|p| p.duration_ms());
    let has_audio = probe.as_ref().is_some_and(|p| p.audio().is_some());
    tracks::import_media(package, path, duration, has_audio).map_err(|e| format!("could not add {}: {e}", path.display()))
}

/// Pick an image or video and show it over the recording at the playhead.
pub fn add_media(ed: &mut Editor) {
    let Some(path) = rfd::FileDialog::new().add_filter("Images and videos", &["png", "jpg", "jpeg", "webp", "gif", "mp4", "mov", "m4v", "webm"]).pick_file() else { return };
    let item = match import(&ed.session.pkg.path, &path) {
        Ok(i) => i,
        Err(e) => return ed.set_status(e),
    };
    let ms = item.duration_ms.unwrap_or(IMAGE_MS);
    let clip = at_playhead(ed, Clip::overlay(&item, 0.0, ms), ms);
    add(ed, clip, Some(item));
    ed.set_status("Added over the recording: place it in the sidebar or drag it on the preview");
}

/// Put a transition at the cut nearest the playhead (from black at the start of the video),
/// or select the clip after that cut when it has one already.
pub fn add_transition(ed: &mut Editor) {
    let t = ed.playhead_ms;
    let Some(index) = ed.session.timeline.spans().iter().min_by(|a, b| (a.output_start_ms - t).abs().total_cmp(&(b.output_start_ms - t).abs())).map(|s| s.index) else { return };
    ed.select_slice(index);
    ed.show_inspector();
    if ed.session.pkg.project.scenes[0].slices[index].transition.is_some() {
        return ed.set_status("This cut has a transition: change it in the sidebar");
    }
    let kind = if index == 0 { "fade-black" } else { "crossfade" };
    ed.edit(move |p| p.scenes[0].slices[index].transition = Some(Transition::new(kind)));
    ed.set_status(if index == 0 { "The video now fades in from black" } else { "Added a crossfade at the cut" });
}

// ───────────── title cards ─────────────

/// "Add title card before / after" for the menu of slice `index`.
pub fn card_menu(ed: &mut Editor, ui: &mut egui::Ui, index: usize) {
    for (label, at) in [("Add title card before", index), ("Add title card after", index + 1)] {
        if ui.button(label).clicked() {
            add_card(ed, at);
            ui.close();
        }
    }
}

/// Insert a title card before slice `index`, with a title on it and a crossfade into the clip
/// after it, and select the title so its words can be written.
pub fn add_card(ed: &mut Editor, index: usize) {
    let text = "Your *title* here".to_string();
    let mut id = None;
    let slot = &mut id;
    ed.edit(move |p| *slot = Some(sf_engine::tracks::add_title_card(&mut p.scenes[0], index, &text, sf_project::v4::Card::DEFAULT_MS)));
    if let Some(start) = ed.session.timeline.spans().get(index).map(|s| s.output_start_ms) {
        ed.seek(start + 1200.0);
    }
    if let Some(id) = id {
        ed.select_clip(id);
    }
    ed.set_status("Added a title card: write its words in the sidebar");
}

/// Remove the title card at slice `index` with the titles made for it.
pub fn remove_card(ed: &mut Editor, index: usize) {
    let Some(span) = ed.session.timeline.spans().get(index).copied().filter(|s| s.card) else { return };
    ed.selected_slice = None;
    ed.edit(move |p| {
        let scene = &mut p.scenes[0];
        // titles lying within the card go with it
        let mut t = scene.tracks();
        let inside: Vec<String> = t.iter().flat_map(|tr| &tr.clips).filter(|c| c.link.is_none() && c.output_start_ms >= span.output_start_ms - 0.5 && c.output_end_ms() <= span.output_end_ms + 0.5).map(|c| c.id.clone()).collect();
        for id in &inside {
            tracks::remove_clip(&mut t, id);
        }
        scene.set_tracks(t);
        sf_engine::tracks::remove_card(scene, index);
    });
    ed.set_status("Removed the title card");
}

/// The selected title card's length and background, in the sidebar.
pub fn card_item_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    let Some(index) = ed.selected_slice else { return };
    let Some(card) = ed.session.pkg.project.scenes[0].slices.get(index).and_then(|s| s.card.clone()) else { return };
    widgets::title(ui, "Title card");
    widgets::hint(ui, "A clip with only a background and its titles. Click its title on the timeline to write the words.");
    let mut ms = card.duration_ms / 1000.0;
    let r = widgets::labeled_slider(ui, "Length", &mut ms, 1.0..=10.0, Some(sf_project::v4::Card::DEFAULT_MS / 1000.0), " s");
    if r.response.drag_stopped() || r.response.lost_focus() || r.reset || (r.response.changed() && !r.response.dragged()) {
        let ms = if r.reset { sf_project::v4::Card::DEFAULT_MS } else { ms * 1000.0 };
        ed.edit(move |p| sf_engine::tracks::set_card_duration(&mut p.scenes[0], index, ms));
    }
    let choices = [(Some(sf_project::v4::Card::DEFAULT_FILL), "Night"), (Some("#000000"), "Black"), (Some("#ffffff"), "White"), (None, "Wallpaper")];
    let labels: Vec<&str> = choices.iter().map(|c| c.1).collect();
    let cur = choices.iter().position(|c| c.0 == card.background.as_deref()).unwrap_or(0);
    widgets::title(ui, "Fill");
    widgets::hint(ui, "Wallpaper uses the project's background (wallpaper, gradient or colour).");
    if let Some(i) = widgets::segmented(ui, cur, &labels) {
        let bg = choices[i].0.map(str::to_string);
        ed.edit(move |p| {
            if let Some(c) = p.scenes[0].slices.get_mut(index).and_then(|s| s.card.as_mut()) {
                c.background = bg;
            }
        });
    }
    // one backdrop for the intro and the video: the card's fill behind the recording too
    if let Some(fill) = card.background.clone() {
        let bg = &ed.session.pkg.project.config.styles.background;
        let same = bg.kind == "color" && bg.color.eq_ignore_ascii_case(&fill);
        ui.add_space(4.0);
        if same {
            widgets::hint(ui, "The video uses this fill behind the recording too.");
        } else if widgets::secondary_button(ui, "Use behind the whole video").on_hover_text("Set the project's background to this fill, so the card and the video share it").clicked() {
            ed.edit(move |p| {
                let bg = &mut p.config.styles.background;
                bg.kind = "color".into();
                bg.color = fill;
            });
            ed.set_status("The video's background now matches the title card");
        }
    }
}

// ───────────── timeline lanes ─────────────

/// Height the title and overlay lanes take on the timeline (0 without any).
pub fn lanes_height(ed: &Editor) -> f32 {
    let n = ed.session.visuals.tracks.len() as f32;
    if n == 0.0 {
        0.0
    } else {
        n * LANE_H + (n - 1.0) * LANE_GAP
    }
}

/// Label of a clip block: the title's words, or the media's file name.
fn label(ed: &Editor, c: &Clip) -> String {
    if let Some(t) = &c.text {
        return t.content.replace('*', "").replace('\n', " ");
    }
    let media = ed.session.pkg.project.scenes[0].media();
    let name = c.media.as_ref().and_then(|id| media.iter().find(|m| &m.id == id)).map(|m| {
        // `media/<sha8>-<name>` → `<name>`
        let file = m.path.rsplit('/').next().unwrap_or(&m.path);
        file.split_once('-').map_or(file, |(_, name)| name).to_string()
    });
    name.unwrap_or_else(|| "Media".into())
}

/// Lanes of titles and overlays from `top`, with their blocks: click selects, drag moves,
/// edges trim, right-click for the menu.
#[allow(clippy::too_many_arguments)]
pub fn lanes_ui(ed: &mut Editor, ui: &mut egui::Ui, rect: Rect, top: f32, x_of: impl Fn(f64) -> f32 + Copy, ms_of: impl Fn(f32) -> f64 + Copy, seek_to: &mut Option<f64>, ctx_ms: &mut f64) {
    drive_lane_drag(ed, ui, ms_of);
    let pal = theme::p();
    let painter = ui.painter_at(rect);
    let timeline = ed.session.timeline.clone();
    let lanes = ed.session.visuals.tracks.clone();
    for (i, track) in lanes.iter().enumerate() {
        let lt = top + i as f32 * (LANE_H + LANE_GAP);
        let lane = Rect::from_min_max(pos2(rect.left(), lt), pos2(rect.right(), lt + LANE_H));
        painter.rect(lane.shrink2(vec2(1.5, 0.0)), 10.0, pal.track_empty, Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
        let bg = ui.interact(lane, ui.id().with(("layer-lane", &track.id)), Sense::click());
        if let Some(p) = bg.interact_pointer_pos().filter(|_| bg.clicked()) {
            *seek_to = Some(ms_of(p.x));
        }
        for c in &track.clips {
            // a clip pinned to a moment that is cut is not shown
            let Some(start) = clip_start(c, &timeline) else { continue };
            let end = start + c.output_duration_ms();
            let (x0, x1) = (x_of(start), x_of(end));
            let block = Rect::from_min_max(pos2(x0 + 1.5, lt + 3.0), pos2((x1 - 1.5).max(x0 + 4.0), lt + LANE_H - 3.0));
            let visible = block.intersect(rect);
            if visible.width() <= 0.0 {
                continue;
            }
            let selected = ed.selected_clip.as_deref() == Some(c.id.as_str());
            let color = if c.text.is_some() { pal.title } else { pal.overlay };
            let color = if track.hidden { color.gamma_multiply(0.4) } else { color };
            let fill = color.gamma_multiply(if selected { 0.42 } else { 0.26 });
            painter.rect(block, 2.0, fill, if selected { Stroke::new(2.0, color) } else { Stroke::new(1.0, color.gamma_multiply(0.75)) }, egui::StrokeKind::Inside);
            if visible.width() > 40.0 {
                let icon = Rect::from_center_size(pos2(visible.left() + 14.0, block.center().y), vec2(14.0, 14.0));
                widgets::paint_icon(&painter, icon, if c.text.is_some() { widgets::Icon::Text } else { widgets::Icon::Media }, pal.text);
                painter.with_clip_rect(visible.shrink(2.0)).text(pos2(visible.left() + 28.0, block.center().y), egui::Align2::LEFT_CENTER, label(ed, c), egui::FontId::proportional(12.0), pal.text);
            }
            let item = ui.interact(visible, ui.id().with(("layer-clip", &c.id)), Sense::click_and_drag()).on_hover_cursor(CursorIcon::Grab);
            if let Some(p) = item.interact_pointer_pos() {
                if item.clicked() || item.secondary_clicked() || item.drag_started() {
                    ed.select_clip(c.id.clone());
                }
                if item.clicked() {
                    *seek_to = Some(ms_of(p.x));
                }
                if item.secondary_clicked() {
                    *ctx_ms = ms_of(p.x);
                }
                if item.drag_started() {
                    start_lane_drag(ed, c, DragKind::Move, ms_of(p.x) - start);
                }
            }
            let (id, at) = (c.id.clone(), *ctx_ms);
            item.context_menu(|ui| clip_menu(ed, ui, &id, at));
            if block.width() > HANDLE_W * 3.0 {
                for (kind, hr) in [(DragKind::TrimStart, Rect::from_min_max(block.left_top(), pos2(block.left() + HANDLE_W, block.bottom()))), (DragKind::TrimEnd, Rect::from_min_max(pos2(block.right() - HANDLE_W, block.top()), block.right_bottom()))] {
                    let hr = hr.intersect(rect);
                    if hr.width() <= 0.0 {
                        continue;
                    }
                    let h = ui.interact(hr, ui.id().with(("layer-trim", &c.id, kind == DragKind::TrimStart)), Sense::drag()).on_hover_cursor(CursorIcon::ResizeHorizontal);
                    if h.hovered() || h.dragged() {
                        painter.rect_filled(hr, 2.0, pal.accent.gamma_multiply(0.7));
                    }
                    if h.drag_started() {
                        ed.select_clip(c.id.clone());
                        start_lane_drag(ed, c, kind, 0.0);
                    }
                }
            }
        }
    }
}

fn start_lane_drag(ed: &mut Editor, c: &Clip, kind: DragKind, grab_ms: f64) {
    let media = ed.session.pkg.project.scenes[0].media();
    let media_ms = c.media.as_ref().and_then(|id| media.iter().find(|m| &m.id == id)).filter(|m| m.kind == MediaKind::Video).and_then(|m| m.duration_ms);
    let mut clip = c.clone();
    clip.output_start_ms = clip_start(c, &ed.session.timeline).unwrap_or(c.output_start_ms);
    ed.layers.lane_drag = Some(LaneDrag { id: c.id.clone(), kind, grab_ms, clip, media_ms, snapshot: ed.session.pkg.project.clone() });
}

/// Follow the pointer with the clip being dragged; one undo step when the button is released.
fn drive_lane_drag(ed: &mut Editor, ui: &egui::Ui, ms_of: impl Fn(f32) -> f64) {
    let Some(drag) = ed.layers.lane_drag.take() else { return };
    let (down, pos) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos()));
    if !down {
        ed.commit_live_edit(drag.snapshot);
        return;
    }
    if let Some(p) = pos {
        let at = ms_of(p.x);
        let c0 = &drag.clip;
        let (start0, end0, speed) = (c0.output_start_ms, c0.output_end_ms(), c0.speed.max(0.01));
        let duration = ed.duration_ms();
        let min = Clip::MIN_DURATION_MS;
        let (mut start, mut src_start, mut src_end) = (start0, c0.source_start_ms, c0.source_end_ms);
        match drag.kind {
            DragKind::Move => start = (at - drag.grab_ms).clamp(0.0, (duration - c0.output_duration_ms()).max(0.0)),
            DragKind::TrimStart => {
                // a video cannot start before its first frame
                let earliest = if drag.media_ms.is_some() { start0 - c0.source_start_ms / speed } else { 0.0 };
                start = at.clamp(earliest.max(0.0), end0 - min);
                if drag.media_ms.is_some() {
                    src_start = c0.source_start_ms + (start - start0) * speed;
                } else {
                    (src_start, src_end) = (0.0, end0 - start);
                }
            }
            DragKind::TrimEnd => {
                let latest = drag.media_ms.map(|m| start0 + (m - c0.source_start_ms) / speed).unwrap_or(f64::INFINITY);
                let end = at.clamp(start0 + min, latest.min(duration));
                src_end = c0.source_start_ms + (end - start0) * speed;
            }
        }
        // clips on one lane never overlap: a move or trim stops where it would
        let end = start + (src_end - src_start) / speed;
        let lane = ed.session.visuals.tracks.iter().find(|t| t.clips.iter().any(|c| c.id == drag.id));
        let timeline = ed.session.timeline.clone();
        let free = lane.is_none_or(|t| t.clips.iter().filter(|c| c.id != drag.id).filter_map(|c| clip_start(c, &timeline).map(|s| (s, s + c.output_duration_ms()))).all(|(s, e)| end <= s + 0.5 || start >= e - 0.5));
        if free {
            live_clip(ed, &drag.id, |c| {
                set_start(c, start, &timeline);
                c.source_start_ms = src_start;
                c.source_end_ms = src_end;
            });
        }
    }
    ed.layers.lane_drag = Some(drag);
    ui.ctx().request_repaint();
}

fn clip_menu(ed: &mut Editor, ui: &mut egui::Ui, id: &str, at_ms: f64) {
    ui.set_min_width(170.0);
    let Some(c) = drawn_clip(ed, id) else { return };
    if ui.button("Split here").clicked() {
        let id = id.to_string();
        ed.edit(move |p| {
            let mut t = p.scenes[0].tracks();
            tracks::split_clip(&mut t, &id, at_ms);
            p.scenes[0].set_tracks(t);
        });
        ui.close();
    }
    let pinned = c.link.is_some();
    if ui.button(if pinned { "Unpin from the recording" } else { "Pin to the recording" }).on_hover_text("A pinned clip moves with its moment when earlier footage is cut").clicked() {
        let timeline = ed.session.timeline.clone();
        let start = clip_start(&c, &timeline).unwrap_or(c.output_start_ms);
        edit_clip(ed, id, move |c| {
            c.output_start_ms = start;
            c.link = if pinned { None } else { timeline.output_to_source(start).map(|s| Link { source_ms: s }) };
        });
        ui.close();
    }
    ui.separator();
    if ui.button("Delete").clicked() {
        remove(ed, id);
        ui.close();
    }
}

/// Delete clip `id` (the Delete key and the menus).
pub fn remove(ed: &mut Editor, id: &str) {
    let id = id.to_string();
    ed.selected_clip = None;
    ed.edit(move |p| {
        let mut t = p.scenes[0].tracks();
        tracks::remove_clip(&mut t, &id);
        p.scenes[0].set_tracks(t);
    });
    ed.set_status("Removed");
}

/// The transition into the clip starting at `x` on the timeline: a small badge on its edge.
pub fn paint_transition_badge(painter: &egui::Painter, x: f32, clip_top: f32) {
    let pal = theme::p();
    let r = Rect::from_center_size(pos2(x, clip_top + 12.0), vec2(20.0, 16.0));
    painter.rect(r, 2.0, pal.card, Stroke::new(1.0, pal.accent), egui::StrokeKind::Inside);
    widgets::paint_icon(painter, r.shrink(2.0), widgets::Icon::Transition, pal.accent);
}

/// Transition entries for the clip menu of slice `index`.
pub fn transition_menu(ed: &mut Editor, ui: &mut egui::Ui, index: usize) {
    let cur = ed.session.pkg.project.scenes[0].slices.get(index).and_then(|s| s.transition.clone());
    let label = if index == 0 { "Fade in" } else { "Transition" };
    ui.menu_button(label, |ui| {
        for (kind, name) in transition_choices(index) {
            let on = cur.as_ref().map(|t| t.kind.as_str()) == kind;
            if crate::widgets::menu_item(ui, on, name).clicked() {
                let k = kind.map(str::to_string);
                ed.edit(move |p| p.scenes[0].slices[index].transition = k.map(|k| Transition::new(&k)));
                ui.close();
            }
        }
    });
}

fn transition_choices(index: usize) -> Vec<(Option<&'static str>, &'static str)> {
    if index == 0 {
        vec![(None, "None"), (Some("fade-black"), "From black")]
    } else {
        vec![(None, "Cut"), (Some("crossfade"), "Crossfade"), (Some("fade-black"), "Dip to black"), (Some("blur"), "Blur")]
    }
}

// ───────────── sidebar ─────────────

/// The transition into the selected clip, in the sidebar.
pub fn transition_item_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    let Some(index) = ed.selected_slice else { return };
    let Some(slice) = ed.session.pkg.project.scenes[0].slices.get(index).cloned() else { return };
    widgets::title(ui, if index == 0 { "Fade in" } else { "Transition into this clip" });
    widgets::hint(ui, if index == 0 { "How the video starts." } else { "How the clip before hands over to this one. The last frame before the cut blends into this clip, so the video keeps its length." });
    let choices = transition_choices(index);
    let labels: Vec<&str> = choices.iter().map(|c| c.1).collect();
    let cur = choices.iter().position(|c| c.0 == slice.transition.as_ref().map(|t| t.kind.as_str())).unwrap_or(0);
    if let Some(i) = widgets::segmented(ui, cur, &labels) {
        let kind = choices[i].0.map(str::to_string);
        ed.edit(move |p| p.scenes[0].slices[index].transition = kind.map(|k| Transition::new(&k)));
    }
    if slice.transition.is_some() {
        cfg_slider(
            ed,
            ui,
            "Duration",
            move |p| p.scenes[0].slices.get(index).and_then(|s| s.transition.as_ref()).map(|t| t.duration_ms / 1000.0).unwrap_or(0.5),
            move |p, v| {
                if let Some(t) = p.scenes[0].slices.get_mut(index).and_then(|s| s.transition.as_mut()) {
                    t.duration_ms = v * 1000.0;
                }
            },
            0.1..=2.0,
            Some(Transition::DEFAULT_MS / 1000.0),
            " s",
        );
    }
}

/// Slider for a field of the selected clip (one undo step per change, like the other sliders).
#[allow(clippy::too_many_arguments)]
fn clip_slider(ed: &mut Editor, ui: &mut egui::Ui, id: &str, label: &str, get: fn(&Clip) -> f64, set: fn(&mut Clip, f64), range: std::ops::RangeInclusive<f64>, default: f64, suffix: &str) {
    let (gid, sid) = (id.to_string(), id.to_string());
    cfg_slider(
        ed,
        ui,
        label,
        move |p| p.scenes[0].tracks().iter().flat_map(|t| &t.clips).find(|c| c.id == gid).map(get).unwrap_or(default),
        move |p, v| {
            let mut t = p.scenes[0].tracks();
            if let Some(c) = tracks::find_clip_mut(&mut t, &sid) {
                set(c, v);
            }
            p.scenes[0].set_tracks(t);
        },
        range,
        Some(default),
        suffix,
    );
}

/// A choice among `(value, label)` pairs; returns the picked value when it changes.
fn pick<'a>(ui: &mut egui::Ui, title: &str, choices: &[(&'a str, &str)], cur: &str) -> Option<&'a str> {
    widgets::title(ui, title);
    let labels: Vec<&str> = choices.iter().map(|c| c.1).collect();
    let i = choices.iter().position(|c| c.0 == cur).unwrap_or(0);
    widgets::segmented(ui, i, &labels).map(|i| choices[i].0)
}

const ANIMS: [(&str, &str); 10] = [("none", "None"), ("fade", "Fade"), ("blur", "Blur"), ("words", "Words"), ("rise", "Rise"), ("zoom", "Zoom"), ("slide-left", "Slide left"), ("slide-right", "Slide right"), ("slide-up", "Slide up"), ("slide-down", "Slide down")];

fn anim_combo(ui: &mut egui::Ui, salt: &str, cur: &Option<ClipAnim>, title: bool) -> Option<Option<ClipAnim>> {
    let kind = cur.as_ref().map(|a| a.kind.as_str()).unwrap_or("none");
    let mut out = None;
    egui::ComboBox::from_id_salt(salt).selected_text(ANIMS.iter().find(|a| a.0 == kind).map(|a| a.1).unwrap_or("None")).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
        // word by word is for titles, and only as an entrance
        for (k, name) in ANIMS.iter().filter(|a| a.0 != "words" || (title && salt.ends_with("in"))) {
            if crate::widgets::menu_item(ui, kind == *k, *name).clicked() && kind != *k {
                let ms = cur.as_ref().map(|a| a.duration_ms).unwrap_or(ClipAnim::DEFAULT_MS);
                out = Some((*k != "none").then(|| ClipAnim { kind: k.to_string(), duration_ms: ms }));
            }
        }
    });
    out
}

/// Inspector of the selected title or overlay.
pub fn clip_item_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    let Some(id) = ed.selected_clip.clone() else { return };
    let Some(c) = ed.session.pkg.project.scenes[0].tracks().iter().flat_map(|t| &t.clips).find(|c| c.id == id).cloned() else {
        ed.selected_clip = None;
        return;
    };
    let is_title = c.text.is_some();
    widgets::title(ui, if is_title { "Title" } else { "Over the recording" });
    widgets::hint(ui, "Drag it on the preview to move it.");

    if let Some(style) = &c.text {
        // typing edits live; the whole change is one undo step once the field is left
        let mut content = style.content.clone();
        let mut subtitle = style.subtitle.clone();
        let pal = theme::p();
        ui.label(RichText::new("Words").size(15.0).color(pal.text));
        let a = widgets::text_area(ui, &mut content, "Wrap words in * to colour them", 2);
        ui.label(RichText::new("Subtitle").size(15.0).color(pal.text));
        ui.add_space(6.0);
        let b = widgets::text_area(ui, &mut subtitle, "Optional smaller line", 1);
        if (a.changed() || b.changed()) && ed.layers.text_snapshot.is_none() {
            ed.layers.text_snapshot = Some(ed.session.pkg.project.clone());
        }
        if a.changed() || b.changed() {
            live_clip(ed, &id, |c| {
                if let Some(t) = c.text.as_mut() {
                    t.content = content.clone();
                    t.subtitle = subtitle.clone();
                }
            });
        }
        if !a.has_focus() && !b.has_focus() {
            if let Some(s) = ed.layers.text_snapshot.take() {
                ed.commit_live_edit(s);
            }
        }
        widgets::hint(ui, "Words between * are drawn in the accent colour, like *this*.");

        let style = style.clone();
        let set_style = |ed: &mut Editor, f: Box<dyn FnOnce(&mut sf_project::TextStyle)>| {
            edit_clip(ed, &id, move |c| {
                if let Some(t) = c.text.as_mut() {
                    f(t)
                }
            })
        };
        if let Some(v) = pick(ui, "Font", &[("sans-serif", "Sans"), ("heavy", "Bold"), ("serif", "Serif"), ("mono", "Mono")], &style.font) {
            set_style(ed, Box::new(move |t| t.font = v.into()));
        }
        clip_slider(ed, ui, &id, "Size", |c| c.text.as_ref().map(|t| t.size_ratio * 100.0).unwrap_or(6.0), |c, v| if let Some(t) = c.text.as_mut() { t.size_ratio = v / 100.0 }, 2.0..=20.0, sf_project::TextStyle::DEFAULT_SIZE * 100.0, "");
        if let Some(v) = pick(ui, "Text colour", &[("#ffffff", "White"), ("#0f0f14", "Black")], &style.color) {
            set_style(ed, Box::new(move |t| t.color = v.into()));
        }
        let accents = [(sf_project::TextStyle::DEFAULT_ACCENT, "Blue"), ("#c084fc", "Purple"), ("#fa4a32", "Red"), ("#ffd60a", "Yellow"), ("#32d74b", "Green")];
        if let Some(v) = pick(ui, "Accent colour", &accents, &style.accent_color) {
            set_style(ed, Box::new(move |t| t.accent_color = v.into()));
        }
        if let Some(v) = pick(ui, "Align", &[("left", "Left"), ("center", "Center"), ("right", "Right")], &style.align) {
            // keep the block on its side of the frame
            let x = match v {
                "center" => 0.5,
                "right" => 0.94,
                _ => 0.06,
            };
            set_style(ed, Box::new(move |t| {
                t.align = v.into();
                t.position01.x = x;
            }));
        }
        let rows = [(0.15, "Top"), (0.5, "Middle"), (0.82, "Bottom")];
        let row = rows.iter().position(|r| (r.0 - style.position01.y).abs() < 0.02);
        widgets::title(ui, "Position");
        if let Some(i) = widgets::segmented(ui, row.unwrap_or(usize::MAX), &["Top", "Middle", "Bottom"]) {
            let y = rows[i].0;
            set_style(ed, Box::new(move |t| t.position01.y = y));
        }
        let mut shadow = style.shadow;
        if widgets::toggle_row(ui, "Shadow", "A soft shadow under the letters, for busy footage.", &mut shadow) {
            set_style(ed, Box::new(move |t| t.shadow = shadow));
        }
    } else {
        let placements = [("full", "Full"), ("center", "Center"), ("pip-br", "Corner"), ("custom", "Free")];
        let cur = c.placement.as_deref().unwrap_or("center");
        let shown = if cur.starts_with("pip-") { "pip-br" } else { cur };
        if let Some(v) = pick(ui, "Placement", &placements, shown) {
            let rect = ed.session.visuals.drawn.get(&id).copied();
            let (w, h) = (ed.preview_size.width.max(1) as f64, ed.preview_size.height.max(1) as f64);
            edit_clip(ed, &id, move |c| {
                c.placement = Some(v.into());
                // free placement starts where the overlay is now
                if v == "custom" && c.rect01.is_none() {
                    c.rect01 = rect.map(|r| sf_project::schema::common::Rect { x: r.x / w, y: r.y / h, width: r.width / w, height: r.height / h });
                }
            });
        }
        if cur.starts_with("pip-") {
            if let Some(v) = pick(ui, "Corner", &[("pip-tl", "Top left"), ("pip-tr", "Top right"), ("pip-bl", "Bottom left"), ("pip-br", "Bottom right")], cur) {
                edit_clip(ed, &id, move |c| c.placement = Some(v.into()));
            }
        }
        if cur != "full" {
            clip_slider(ed, ui, &id, "Rounded corners", |c| c.corner_ratio * 100.0, |c, v| c.corner_ratio = v / 100.0, 0.0..=20.0, 3.0, "");
            clip_slider(ed, ui, &id, "Shadow", |c| c.shadow, |c, v| c.shadow = v, 0.0..=1.0, 0.5, "");
        }
    }

    widgets::title(ui, "Behind it");
    widgets::hint(ui, "Blur and darken the recording while this shows, like a camera focusing on it.");
    clip_slider(ed, ui, &id, "Blur the recording", |c| c.backdrop_blur, |c, v| c.backdrop_blur = v, 0.0..=1.0, 0.0, "");
    clip_slider(ed, ui, &id, "Darken the recording", |c| c.backdrop_dim, |c, v| c.backdrop_dim = v, 0.0..=1.0, 0.0, "");

    widgets::title(ui, "Comes in with");
    if let Some(a) = anim_combo(ui, &format!("{id}-in"), &c.anim_in, is_title) {
        edit_clip(ed, &id, move |c| c.anim_in = a);
    }
    widgets::title(ui, "Leaves with");
    if let Some(a) = anim_combo(ui, &format!("{id}-out"), &c.anim_out, is_title) {
        edit_clip(ed, &id, move |c| c.anim_out = a);
    }
    if c.anim_in.is_some() || c.anim_out.is_some() {
        clip_slider(ed, ui, &id, "Animation length", |c| c.anim_in.as_ref().or(c.anim_out.as_ref()).map(|a| a.duration_ms / 1000.0).unwrap_or(0.45), |c, v| {
            for a in [c.anim_in.as_mut(), c.anim_out.as_mut()].into_iter().flatten() {
                a.duration_ms = v * 1000.0;
            }
        }, 0.1..=2.0, ClipAnim::DEFAULT_MS / 1000.0, " s");
    }
    if c.media.is_none() || c.text.is_some() || ed.session.pkg.project.scenes[0].media().iter().any(|m| Some(&m.id) == c.media.as_ref() && m.kind == MediaKind::Image) {
        clip_slider(ed, ui, &id, "Shows for", |c| c.output_duration_ms() / 1000.0, |c, v| c.set_output_duration(v * 1000.0), 0.5..=30.0, Clip::TITLE_MS / 1000.0, " s");
    }
    let mut pinned = c.link.is_some();
    if widgets::toggle_row(ui, "Pinned to the recording", "Moves with its moment when earlier footage is cut.", &mut pinned) {
        let timeline = ed.session.timeline.clone();
        let start = clip_start(&c, &timeline).unwrap_or(c.output_start_ms);
        edit_clip(ed, &id, move |c| {
            c.output_start_ms = start;
            c.link = if pinned { timeline.output_to_source(start).map(|s| Link { source_ms: s }) } else { None };
        });
    }
    ui.add_space(8.0);
    if widgets::secondary_button(ui, "Delete").clicked() {
        remove(ed, &id);
    }
}

// ───────────── preview ─────────────

/// Titles and overlays on the preview: a click picks one (its settings open in the sidebar), a
/// click on the picked title or a double-click edits its words in place, and dragging the
/// picked one moves it. True when the preview needs a new frame.
pub fn preview_ui(ed: &mut Editor, ui: &mut egui::Ui, img: Rect) -> bool {
    let (w, h) = (ed.preview_size.width.max(1) as f64, ed.preview_size.height.max(1) as f64);
    let k = img.width() / w as f32;
    let on_screen = |r: &sf_project::schema::common::Rect| Rect::from_min_size(img.min + vec2(r.x as f32, r.y as f32) * k, vec2(r.width as f32, r.height as f32) * k);
    if ed.layers.editing.is_some() {
        return inline_edit(ed, ui, img, k);
    }
    // smaller ones last, so they are on top and stay clickable over a bigger one
    let mut drawn: Vec<(String, sf_project::schema::common::Rect)> = ed.session.visuals.drawn.iter().map(|(id, r)| (id.clone(), *r)).collect();
    drawn.sort_by(|a, b| (b.1.width * b.1.height).total_cmp(&(a.1.width * a.1.height)));
    let titles: std::collections::HashSet<String> = ed.session.visuals.tracks.iter().flat_map(|t| &t.clips).filter(|c| c.text.is_some()).map(|c| c.id.clone()).collect();
    for (id, r) in &drawn {
        let frame = on_screen(r);
        let selected = ed.selected_clip.as_deref() == Some(id.as_str());
        let is_title = titles.contains(id);
        let resp = ui.interact(frame, ui.id().with(("layer-hit", id)), Sense::click_and_drag());
        let resp = if selected { resp.on_hover_cursor(CursorIcon::Move) } else { resp.on_hover_cursor(CursorIcon::PointingHand) };
        if selected {
            ui.painter().rect_stroke(frame.expand(3.0), 2.0, Stroke::new(1.5, theme::p().accent), egui::StrokeKind::Outside);
        } else if resp.hovered() {
            ui.painter().rect_stroke(frame.expand(3.0), 2.0, Stroke::new(1.0, theme::p().accent.gamma_multiply(0.5)), egui::StrokeKind::Outside);
        }
        if resp.double_clicked() || (resp.clicked() && selected && is_title) {
            if is_title {
                ed.select_clip(id.clone());
                ed.layers.editing = Some((id.clone(), false));
                ed.layers.inline_snapshot = Some(ed.session.pkg.project.clone());
                return true;
            }
        } else if resp.clicked() {
            ed.select_clip(id.clone());
        }
        if resp.drag_started() {
            ed.select_clip(id.clone());
            if let (Some(p), Some(c)) = (resp.interact_pointer_pos(), ed.session.pkg.project.scenes[0].tracks().iter().flat_map(|t| &t.clips).find(|c| &c.id == id).cloned()) {
                ed.layers.preview_drag = Some(PreviewDrag { id: id.clone(), from: p, rect: *r, clip: c, snapshot: ed.session.pkg.project.clone() });
            }
        }
    }
    drive_preview_drag(ed, ui, k, w, h)
}

/// Follow the pointer with the title or overlay being dragged on the preview; one undo step
/// when the button is released.
fn drive_preview_drag(ed: &mut Editor, ui: &egui::Ui, k: f32, w: f64, h: f64) -> bool {
    let Some(drag) = ed.layers.preview_drag.take() else { return false };
    let (down, pos) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos()));
    if !down {
        ed.commit_live_edit(drag.snapshot);
        return true;
    }
    if let Some(p) = pos {
        let (dx, dy) = (((p.x - drag.from.x) / k) as f64 / w, ((p.y - drag.from.y) / k) as f64 / h);
        let (c0, r0) = (drag.clip.clone(), drag.rect);
        live_clip(ed, &drag.id, |c| {
            if let (Some(t), Some(t0)) = (c.text.as_mut(), c0.text.as_ref()) {
                t.position01.x = (t0.position01.x + dx).clamp(0.0, 1.0);
                t.position01.y = (t0.position01.y + dy).clamp(0.0, 1.0);
            } else {
                // dragging frees an overlay from its preset placement
                let base = if c0.placement.as_deref() == Some("custom") { c0.rect01 } else { None }.unwrap_or(sf_project::schema::common::Rect { x: r0.x / w, y: r0.y / h, width: r0.width / w, height: r0.height / h });
                c.placement = Some("custom".into());
                c.rect01 = Some(sf_project::schema::common::Rect { x: base.x + dx, y: base.y + dy, ..base });
            }
        });
    }
    ed.layers.preview_drag = Some(drag);
    ui.ctx().request_repaint();
    true
}

/// Typing a title's words in place: a text box over the title on the preview. Esc, ⌘↩ or a
/// click elsewhere finishes, as one undo step.
fn inline_edit(ed: &mut Editor, ui: &mut egui::Ui, img: Rect, k: f32) -> bool {
    let Some((id, focused)) = ed.layers.editing.clone() else { return false };
    let style = ed.session.pkg.project.scenes[0].tracks().iter().flat_map(|t| &t.clips).find(|c| c.id == id).and_then(|c| c.text.clone());
    let (Some(style), Some(r)) = (style, ed.session.visuals.drawn.get(&id).copied()) else {
        // the title is gone or no longer at the playhead
        finish_inline(ed);
        return true;
    };
    let pal = theme::p();
    let frame = Rect::from_min_size(img.min + vec2(r.x as f32, r.y as f32) * k, vec2(r.width as f32, r.height as f32) * k).expand(8.0).intersect(img.expand(40.0));
    ui.painter().rect(frame, 2.0, pal.bg.gamma_multiply(0.92), Stroke::new(1.5, pal.accent), egui::StrokeKind::Outside);
    let size = (style.size_ratio * ed.preview_size.height as f64) as f32 * k;
    let mut text = style.content.clone();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(frame.shrink(6.0)));
    let edit = child.add(
        egui::TextEdit::multiline(&mut text)
            .id(ui.id().with(("title-inline", &id)))
            .font(egui::FontId::proportional(size.clamp(12.0, 40.0)))
            .text_color(pal.text)
            .frame(egui::Frame::NONE)
            .desired_width(frame.width() - 12.0)
            .desired_rows(1)
            .horizontal_align(match style.align.as_str() {
                "center" => egui::Align::Center,
                "right" => egui::Align::Max,
                _ => egui::Align::Min,
            }),
    );
    if !focused {
        edit.request_focus();
        ed.layers.editing = Some((id.clone(), true));
    }
    if edit.changed() {
        let words = text.clone();
        live_clip(ed, &id, move |c| {
            if let Some(t) = c.text.as_mut() {
                t.content = words;
            }
        });
    }
    let (esc, submit) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.modifiers.command && i.key_pressed(egui::Key::Enter)));
    if esc || submit || (focused && !edit.has_focus()) {
        finish_inline(ed);
    }
    true
}

fn finish_inline(ed: &mut Editor) {
    ed.layers.editing = None;
    if let Some(s) = ed.layers.inline_snapshot.take() {
        ed.commit_live_edit(s);
    }
}
