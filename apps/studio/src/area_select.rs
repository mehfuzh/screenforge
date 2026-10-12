//! Full-screen overlay to drag out a recording area (Screen Studio's "Area" mode).
//! Runs as an immediate child viewport covering the main display; returns the rectangle in
//! global screen points when the drag ends, or `None` when cancelled with Escape.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke};

#[allow(dead_code)]
pub struct AreaSelect {
    pub display_id: u32,
    pub screen: Rect,
    drag_from: Option<egui::Pos2>,
    current: Option<Rect>,
}

pub enum AreaOutcome {
    Pending,
    Cancelled,
    Done(Rect),
}

impl AreaSelect {
    pub fn new(display_id: u32, screen: Rect) -> Self {
        AreaSelect { display_id, screen, drag_from: None, current: None }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> AreaOutcome {
        let mut outcome = AreaOutcome::Pending;
        let builder = egui::ViewportBuilder::default()
            .with_title("Select area")
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_resizable(false)
            .with_position(self.screen.min)
            .with_inner_size(self.screen.size())
            .with_active(true);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("area-select"), builder, |vctx, _class| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(vctx, |ui| {
                let full = ui.max_rect();
                let resp = ui.allocate_rect(full, Sense::click_and_drag());
                let p = ui.painter();
                let dim = Color32::from_rgba_unmultiplied(0, 0, 0, 90);
                match self.current {
                    Some(sel) => {
                        // dim everything except the selection
                        p.rect_filled(Rect::from_min_max(full.min, pos2(full.max.x, sel.min.y)), 0.0, dim);
                        p.rect_filled(Rect::from_min_max(pos2(full.min.x, sel.max.y), full.max), 0.0, dim);
                        p.rect_filled(Rect::from_min_max(pos2(full.min.x, sel.min.y), pos2(sel.min.x, sel.max.y)), 0.0, dim);
                        p.rect_filled(Rect::from_min_max(pos2(sel.max.x, sel.min.y), pos2(full.max.x, sel.max.y)), 0.0, dim);
                        p.rect_stroke(sel, 0.0, Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Outside);
                        let label = format!("{} × {}", sel.width().round() as i64, sel.height().round() as i64);
                        let lr = Rect::from_center_size(pos2(sel.center().x, sel.max.y + 22.0), vec2(120.0, 26.0));
                        p.rect_filled(lr, CornerRadius::same(7), Color32::from_rgba_unmultiplied(0x1b, 0x1b, 0x27, 0xf0));
                        p.text(lr.center(), Align2::CENTER_CENTER, label, FontId::proportional(13.0), Color32::WHITE);
                    }
                    None => {
                        p.rect_filled(full, 0.0, dim);
                        let hint = Rect::from_center_size(pos2(full.center().x, full.min.y + 60.0), vec2(420.0, 40.0));
                        p.rect_filled(hint, CornerRadius::same(10), Color32::from_rgba_unmultiplied(0x1b, 0x1b, 0x27, 0xf0));
                        p.text(hint.center(), Align2::CENTER_CENTER, "Drag to select the area to record · Esc to cancel", FontId::proportional(14.0), Color32::WHITE);
                    }
                }
                if let Some(pos) = resp.interact_pointer_pos() {
                    if resp.drag_started() {
                        self.drag_from = Some(pos);
                    }
                    if let Some(from) = self.drag_from {
                        self.current = Some(Rect::from_two_pos(from, pos).intersect(full));
                    }
                }
                if resp.drag_stopped() {
                    if let Some(sel) = self.current {
                        if sel.width() >= 32.0 && sel.height() >= 32.0 {
                            // viewport coords → global points. macOS may place the window below the
                            // menu bar, so use where the viewport actually ended up.
                            let origin = ui.input(|i| i.viewport().outer_rect.map(|r| r.min)).unwrap_or(self.screen.min);
                            let global = sel.translate(origin.to_vec2());
                            outcome = AreaOutcome::Done(global);
                        } else {
                            self.current = None;
                            self.drag_from = None;
                        }
                    }
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) || ui.input(|i| i.viewport().close_requested()) {
                    outcome = AreaOutcome::Cancelled;
                }
                ui.ctx().request_repaint();
            });
        });
        outcome
    }
}
