//! Full-screen overlay to pick a window by clicking on it (like the macOS screenshot window
//! picker). Hovering highlights the frontmost window under the pointer and shows a bar on it
//! with aspect ratios and a Record button; picking a ratio resizes the window (and keeps it
//! selected), a click anywhere else on the window records it, Escape cancels.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke};

use crate::picker::WindowEntry;

pub struct WindowSelect {
    pub screen: Rect,
    /// On-screen windows, front to back, in global points.
    windows: Vec<WindowEntry>,
    /// Window kept selected after an aspect ratio was picked (bounds updated to the new frame).
    locked: Option<WindowEntry>,
    /// Frame the locked window had before any resize ("Original" puts it back).
    original: Option<sf_project::schema::common::Rect>,
    aspect: usize,
    note: Option<&'static str>,
}

/// Aspect ratios offered on the window bar (width / height); `None` keeps the window's own.
const ASPECTS: [(&str, Option<f64>); 5] = [("Original", None), ("16:9", Some(16.0 / 9.0)), ("4:3", Some(4.0 / 3.0)), ("1:1", Some(1.0)), ("9:16", Some(9.0 / 16.0))];

pub enum WindowOutcome {
    Pending,
    Cancelled,
    Done(WindowEntry),
}

impl WindowSelect {
    pub fn new(screen: Rect) -> Self {
        let windows = crate::recorder::discover_windows().into_iter().filter(|w| w.on_screen).collect();
        WindowSelect { screen, windows, locked: None, original: None, aspect: 0, note: None }
    }

    fn hit(&self, global: egui::Pos2) -> Option<&WindowEntry> {
        self.windows.iter().find(|w| {
            let r = Rect::from_min_size(pos2(w.bounds.x as f32, w.bounds.y as f32), vec2(w.bounds.width as f32, w.bounds.height as f32));
            r.contains(global)
        })
    }

    /// Resize the locked window to aspect `i`.
    fn set_aspect(&mut self, i: usize) {
        let Some(w) = self.locked.as_mut() else { return };
        let original = *self.original.get_or_insert(w.bounds);
        // the menu bar (taller with a notch) is not usable space
        let screen = sf_project::schema::common::Rect { x: self.screen.min.x as f64, y: self.screen.min.y as f64 + 38.0, width: self.screen.width() as f64, height: self.screen.height() as f64 - 38.0 };
        let target = match ASPECTS[i].1 {
            Some(ratio) => sf_recording::window_frame::fit_aspect(w.bounds, ratio, screen),
            None => original,
        };
        if sf_recording::window_frame::set_window_frame(w.pid, w.bounds, target) {
            w.bounds = target;
            self.aspect = i;
            self.note = None;
        } else if !sf_recording::caret::accessibility_trusted() {
            let _ = sf_recording::permissions::request_accessibility();
            self.note = Some("Allow ScreenForge in Accessibility settings to resize windows");
        } else {
            self.note = Some("This window can't be resized");
        }
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
                let pointer = ui.input(|i| i.pointer.hover_pos());
                let hover = self.locked.clone().or_else(|| pointer.and_then(|p| self.hit(p + origin.to_vec2()).cloned()));
                let p = ui.painter();
                let dim = Color32::from_rgba_unmultiplied(0, 0, 0, 90);
                let mut chips: Vec<Rect> = Vec::new();
                let mut record_btn = Rect::NOTHING;
                let mut sel = Rect::NOTHING;
                match &hover {
                    Some(w) => {
                        sel = Rect::from_min_size(pos2(w.bounds.x as f32, w.bounds.y as f32), vec2(w.bounds.width as f32, w.bounds.height as f32)).translate(-origin.to_vec2()).intersect(full);
                        let pal = crate::theme::p();
                        p.rect_filled(Rect::from_min_max(full.min, pos2(full.max.x, sel.min.y)), 0.0, dim);
                        p.rect_filled(Rect::from_min_max(pos2(full.min.x, sel.max.y), full.max), 0.0, dim);
                        p.rect_filled(Rect::from_min_max(pos2(full.min.x, sel.min.y), pos2(sel.min.x, sel.max.y)), 0.0, dim);
                        p.rect_filled(Rect::from_min_max(pos2(sel.max.x, sel.min.y), pos2(full.max.x, sel.max.y)), 0.0, dim);
                        p.rect_filled(sel, CornerRadius::same(8), pal.accent.gamma_multiply(0.16));
                        p.rect_stroke(sel, CornerRadius::same(8), Stroke::new(3.0, pal.accent), egui::StrokeKind::Outside);
                        let label = if w.title.is_empty() { w.app.clone() } else { format!("{}  ·  {}", w.app, w.title) };
                        let label = format!("{label}  ·  {} × {}", w.bounds.width.round() as i64, w.bounds.height.round() as i64);
                        let lw = (label.len() as f32 * 7.5 + 30.0).clamp(120.0, sel.width().max(160.0));
                        let lr = Rect::from_center_size(pos2(sel.center().x, (sel.max.y + 22.0).min(full.max.y - 16.0)), vec2(lw, 26.0));
                        p.rect_filled(lr, CornerRadius::same(7), Color32::from_rgba_unmultiplied(0x1b, 0x1b, 0x27, 0xf0));
                        p.text(lr.center(), Align2::CENTER_CENTER, label, FontId::proportional(13.0), Color32::WHITE);
                        // the bar in the middle of the window, styled like the recording bar:
                        // aspect ratios, then the primary Record button (clicking anywhere else
                        // on the window also records)
                        let (chip_w, gap, pad, btn_w, h) = (60.0, 4.0, 8.0, 156.0, 32.0);
                        let bar_w = pad * 2.0 + ASPECTS.len() as f32 * (chip_w + gap) + 8.0 + btn_w;
                        let mut bar = Rect::from_center_size(sel.center(), vec2(bar_w, h + pad * 2.0));
                        bar = bar.translate(vec2((full.min.x - bar.min.x).max(0.0) - (bar.max.x - full.max.x).max(0.0), 0.0));
                        p.rect(bar, CornerRadius::same(10), pal.glass_solid, Stroke::new(1.0, pal.glass_border), egui::StrokeKind::Inside);
                        let mut x = bar.min.x + pad;
                        for (i, (name, _)) in ASPECTS.iter().enumerate() {
                            let r = Rect::from_min_size(pos2(x, bar.min.y + pad), vec2(chip_w, h));
                            let over = pointer.is_some_and(|pt| r.contains(pt));
                            if i == self.aspect || over {
                                p.rect_filled(r, CornerRadius::same(2), crate::theme::glass_alpha(if i == self.aspect { 44 } else { 24 }));
                            }
                            p.text(r.center(), Align2::CENTER_CENTER, *name, FontId::proportional(13.0), if i == self.aspect { pal.on_glass } else { pal.text_dim });
                            chips.push(r);
                            x += chip_w + gap;
                        }
                        record_btn = Rect::from_min_size(pos2(x + 8.0, bar.min.y + pad), vec2(btn_w, h));
                        let over = pointer.is_some_and(|pt| record_btn.contains(pt));
                        p.rect_filled(record_btn, CornerRadius::same(2), if over { pal.accent_hover } else { pal.accent });
                        p.circle_filled(pos2(record_btn.min.x + 20.0, record_btn.center().y), 6.0, pal.clip_text);
                        p.text(pos2(record_btn.min.x + 34.0, record_btn.center().y), Align2::LEFT_CENTER, "Record window", FontId::proportional(14.0), pal.clip_text);
                        if let Some(note) = self.note {
                            let nr = Rect::from_center_size(pos2(bar.center().x, bar.max.y + 22.0), vec2(note.len() as f32 * 7.0 + 30.0, 26.0));
                            p.rect_filled(nr, CornerRadius::same(7), Color32::from_rgba_unmultiplied(0x1b, 0x1b, 0x27, 0xf0));
                            p.text(nr.center(), Align2::CENTER_CENTER, note, FontId::proportional(13.0), Color32::WHITE);
                        }
                    }
                    None => {
                        p.rect_filled(full, 0.0, dim);
                        let hint = Rect::from_center_size(pos2(full.center().x, full.min.y + 60.0), vec2(400.0, 40.0));
                        p.rect_filled(hint, CornerRadius::same(10), Color32::from_rgba_unmultiplied(0x1b, 0x1b, 0x27, 0xf0));
                        p.text(hint.center(), Align2::CENTER_CENTER, "Click a window to record it · Esc to cancel", FontId::proportional(14.0), Color32::WHITE);
                    }
                }
                if resp.clicked() {
                    let at = resp.interact_pointer_pos().or(pointer);
                    if let Some(i) = at.and_then(|pt| chips.iter().position(|r| r.contains(pt))) {
                        if self.locked.is_none() {
                            self.locked = hover.clone();
                        }
                        self.set_aspect(i);
                    } else if at.is_some_and(|pt| record_btn.contains(pt) || sel.contains(pt)) {
                        if let Some(w) = self.locked.clone().or(hover) {
                            outcome = WindowOutcome::Done(w);
                        }
                    } else {
                        // a click outside the selected window lets go of it
                        self.locked = None;
                        self.original = None;
                        self.aspect = 0;
                        self.note = None;
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
