//! Mask editing on the preview: while a mask is selected and the playhead is inside it, its
//! regions are drawn as frames over the screen and can be moved and resized with the same
//! handles as the crop tool. Regions are stored as pixel rects in cropped-recording space.

use egui::{Pos2, Rect, Sense};
use sf_project::schema::common::Rect as Rect01;
use sf_project::v4::Mask;
use sf_project::Project;

use crate::crop::{self, Handle};
use crate::editor::Editor;
use crate::theme;

/// A region drag in progress: (mask id, region index, handle, region at the start in fractions of
/// the screen, pointer at the start, project before the drag).
pub struct RegionDrag {
    id: String,
    index: usize,
    handle: Handle,
    start: Rect01,
    from: Pos2,
    snapshot: Project,
}

/// New mask regions: centred, this part of the screen.
const NEW_REGION01: Rect01 = Rect01 { x: 0.35, y: 0.4, width: 0.3, height: 0.2 };

/// Pixel size of the cropped recording (the space mask bounds live in).
pub fn content_px(ed: &Editor) -> (f64, f64) {
    let (w, h) = ed.session.source.content_size_px(&ed.session.pkg.project.config.crop.rect01);
    (w as f64, h as f64)
}

fn to01(b: &Rect01, (cw, ch): (f64, f64)) -> Rect01 {
    Rect01 { x: b.x / cw, y: b.y / ch, width: b.width / cw, height: b.height / ch }
}

fn from01(r: Rect01, (cw, ch): (f64, f64)) -> Rect01 {
    Rect01 { x: (r.x * cw).round(), y: (r.y * ch).round(), width: (r.width * cw).round(), height: (r.height * ch).round() }
}

/// A region for a new mask or an added area, in cropped-recording pixels.
pub fn new_region(ed: &Editor) -> Rect01 {
    from01(NEW_REGION01, content_px(ed))
}

/// Add a blur mask at the playhead and select it.
pub fn add_at_playhead(ed: &mut Editor) {
    let Some(src) = ed.session.timeline.output_to_source(ed.playhead_ms) else { return };
    // masks share one lane, so a new one fits between its neighbours
    let masks = &ed.session.pkg.project.scenes[0].masks;
    if masks.iter().any(|m| src >= m.source_start_ms && src < m.source_end_ms) {
        ed.set_status("There is already a mask here");
        return;
    }
    let next_start = masks.iter().map(|m| m.source_start_ms).filter(|s| *s >= src).fold(f64::INFINITY, f64::min);
    let end = (src + Mask::INIT_DURATION_MS).min(ed.session.source.duration_ms).min(next_start);
    if end - src < Mask::MIN_DURATION_MS {
        ed.set_status("Not enough room for a mask here");
        return;
    }
    let mask = Mask::new(src, end, Mask::SENSITIVE_DATA, vec![new_region(ed)]);
    let id = mask.id.clone();
    ed.pause();
    ed.edit(move |p| p.scenes[0].masks.push(mask));
    ed.select_mask(id);
    ed.set_status("Added blur mask: drag it on the preview");
}

/// The selected mask, when the playhead is inside it.
fn active(ed: &Editor) -> Option<Mask> {
    let id = ed.selected_mask.as_deref()?;
    let src = ed.session.timeline.output_to_source(ed.playhead_ms)?;
    ed.session.pkg.project.scenes[0].masks.iter().find(|m| m.id == id && src >= m.source_start_ms && src < m.source_end_ms).cloned()
}

/// Draw and edit the selected mask's regions over `img` (the preview image). Returns true when
/// the preview must be rendered again.
pub fn ui(ed: &mut Editor, ui: &mut egui::Ui, img: Rect) -> bool {
    let Some(mask) = active(ed) else {
        ed.mask_drag = None;
        return false;
    };
    let Some(screen_px) = ed.session.last_screen_px else { return false };
    // the screen in preview points (it moves and grows with zooms)
    let k = img.width() / ed.preview_size.width.max(1) as f32;
    let screen = Rect::from_min_size(img.min + egui::vec2(screen_px.x as f32, screen_px.y as f32) * k, egui::vec2(screen_px.width as f32, screen_px.height as f32) * k);
    let content = content_px(ed);
    let frames: Vec<Rect> = mask.bounds.iter().map(|b| crop::to_screen(to01(b, content), screen)).collect();

    let resp = ui.interact(img, ui.id().with("mask_regions"), Sense::drag());
    // the last region is drawn on top, so it is hit first
    let hit_at = |p: Pos2| frames.iter().enumerate().rev().find_map(|(i, f)| crop::hit(*f, p).map(|h| (i, h)));
    let hovered = resp.hover_pos().and_then(hit_at);
    if resp.drag_started() {
        if let Some((index, handle)) = resp.interact_pointer_pos().and_then(hit_at) {
            ed.mask_drag = Some(RegionDrag { id: mask.id.clone(), index, handle, start: to01(&mask.bounds[index], content), from: resp.interact_pointer_pos().unwrap_or_default(), snapshot: ed.session.pkg.project.clone() });
        }
    }
    let mut changed = false;
    if let (Some(d), Some(p)) = (&ed.mask_drag, resp.interact_pointer_pos()) {
        let delta = p - d.from;
        let r = from01(crop::drag(d.handle, d.start, [(delta.x / screen.width()) as f64, (delta.y / screen.height()) as f64]), content);
        let (id, index) = (d.id.clone(), d.index);
        if let Some(b) = ed.session.pkg.project.scenes[0].masks.iter_mut().find(|m| m.id == id).and_then(|m| m.bounds.get_mut(index)) {
            if *b != r {
                *b = r;
                changed = true;
            }
        }
    }
    if resp.drag_stopped() {
        if let Some(d) = ed.mask_drag.take() {
            ed.commit_live_edit(d.snapshot);
        }
    }
    if let Some(h) = ed.mask_drag.as_ref().map(|d| d.handle).or(hovered.map(|h| h.1)) {
        ui.ctx().set_cursor_icon(crop::cursor_for(h));
    }

    // paint where the regions are now, after this frame's drag
    let bounds = ed.session.pkg.project.scenes[0].masks.iter().find(|m| m.id == mask.id).map(|m| m.bounds.clone()).unwrap_or_default();
    let p = ui.painter_at(img);
    let color = theme::p().mask_selected;
    for (i, b) in bounds.iter().enumerate() {
        let f = &crop::to_screen(to01(b, content), screen);
        let dragging = ed.mask_drag.as_ref().is_some_and(|d| d.index == i);
        crop::paint_frame(&p, *f, color, dragging);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_round_trip_through_screen_fractions() {
        let content = (1368.0, 1020.0);
        let b = Rect01 { x: 100.0, y: 250.0, width: 400.0, height: 120.0 };
        assert_eq!(from01(to01(&b, content), content), b);
        // the default region sits in the middle of the cropped recording
        let r = from01(NEW_REGION01, content);
        assert!((r.x + r.width / 2.0 - content.0 / 2.0).abs() <= 1.0 && (r.y + r.height / 2.0 - content.1 / 2.0).abs() <= 1.0);
    }

    #[test]
    fn dragging_a_region_stays_on_the_screen() {
        let content = (1000.0, 500.0);
        let start = to01(&Rect01 { x: 800.0, y: 400.0, width: 150.0, height: 80.0 }, content);
        let moved = from01(crop::drag(Handle(0, 0), start, [0.5, 0.5]), content);
        assert_eq!((moved.x + moved.width, moved.y + moved.height), (1000.0, 500.0));
        assert_eq!((moved.width, moved.height), (150.0, 80.0));
    }
}
