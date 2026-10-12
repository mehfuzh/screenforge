//! Full-screen overlay to pick a window by clicking on it (like the macOS screenshot window
//! picker). Hovering highlights the frontmost window under the pointer; click records it,
//! Escape cancels.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke};

use crate::picker::WindowEntry;

pub struct WindowSelect {
    pub screen: Rect,
    /// On-screen windows, front to back, in global points.
    windows: Vec<WindowEntry>,
}

pub enum WindowOutcome {
    Pending,
    Cancelled,
    Done(WindowEntry),
}

impl WindowSelect {
    pub fn new(screen: Rect) -> Self {
        let windows = crate::recorder::discover_inputs().windows.into_iter().filter(|w| w.on_screen).collect();
        WindowSelect { screen, windows }
    }

    fn hit(&self, global: egui::Pos2) -> Option<&WindowEntry> {
        self.windows.iter().find(|w| {
            let r = Rect::from_min_size(pos2(w.bounds.x as f32, w.bounds.y as f32), vec2(w.bounds.width as f32, w.bounds.height as f32));
            r.contains(global)
        })
    }

    pub fn show(&mut self, ctx: &egui::Context) -> WindowOutcome {
        let mut outcome = WindowOutcome::Pending;
        let builder = egui::ViewportBuilder::default()
            .with_title("Select window")
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_resizable(false)
            .with_position(self.screen.min)
            .with_inner_size(self.screen.size())
            .with_active(true);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("window-select"), builder, |vctx, _class| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(vctx, |ui| {
                let full = ui.max_rect();
                let resp = ui.allocate_rect(full, Sense::click());
                let origin = ui.input(|i| i.viewport().outer_rect.map(|r| r.min)).unwrap_or(self.screen.min);
                let hover = ui.input(|i| i.pointer.hover_pos()).and_then(|p| self.hit(p + origin.to_vec2()).cloned());
                let p = ui.painter();
                let dim = Color32::from_rgba_unmultiplied(0, 0, 0, 90);
                match &hover {
                    Some(w) => {
                        let sel = Rect::from_min_size(pos2(w.bounds.x as f32, w.bounds.y as f32), vec2(w.bounds.width as f32, w.bounds.height as f32)).translate(-origin.to_vec2()).intersect(full);
                        p.rect_filled(Rect::from_min_max(full.min, pos2(full.max.x, sel.min.y)), 0.0, dim);
                        p.rect_filled(Rect::from_min_max(pos2(full.min.x, sel.max.y), full.max), 0.0, dim);
                        p.rect_filled(Rect::from_min_max(pos2(full.min.x, sel.min.y), pos2(sel.min.x, sel.max.y)), 0.0, dim);
                        p.rect_filled(Rect::from_min_max(pos2(sel.max.x, sel.min.y), pos2(full.max.x, sel.max.y)), 0.0, dim);
                        p.rect_filled(sel, CornerRadius::same(8), Color32::from_rgba_unmultiplied(0x5b, 0x8c, 0xff, 40));
                        p.rect_stroke(sel, CornerRadius::same(8), Stroke::new(3.0, Color32::from_rgb(0x5b, 0x8c, 0xff)), egui::StrokeKind::Outside);
                        let label = if w.title.is_empty() { w.app.clone() } else { format!("{}  ·  {}", w.app, w.title) };
                        let lw = (label.len() as f32 * 7.5 + 30.0).clamp(120.0, sel.width().max(160.0));
                        let lr = Rect::from_center_size(pos2(sel.center().x, (sel.max.y + 22.0).min(full.max.y - 16.0)), vec2(lw, 26.0));
                        p.rect_filled(lr, CornerRadius::same(7), Color32::from_rgba_unmultiplied(0x1b, 0x1b, 0x27, 0xf0));
                        p.text(lr.center(), Align2::CENTER_CENTER, label, FontId::proportional(13.0), Color32::WHITE);
                    }
                    None => {
                        p.rect_filled(full, 0.0, dim);
                        let hint = Rect::from_center_size(pos2(full.center().x, full.min.y + 60.0), vec2(400.0, 40.0));
                        p.rect_filled(hint, CornerRadius::same(10), Color32::from_rgba_unmultiplied(0x1b, 0x1b, 0x27, 0xf0));
                        p.text(hint.center(), Align2::CENTER_CENTER, "Click a window to record it · Esc to cancel", FontId::proportional(14.0), Color32::WHITE);
                    }
                }
                if resp.clicked() {
                    if let Some(w) = hover {
                        outcome = WindowOutcome::Done(w);
                    }
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) || ui.input(|i| i.viewport().close_requested()) {
                    outcome = WindowOutcome::Cancelled;
                }
                ui.ctx().request_repaint();
            });
        });
        outcome
    }
}
