//! Screen Studio style widgets: section titles, segmented controls, labeled sliders with
//! "Reset", toggle switches, icon rail buttons and pill buttons.

use egui::{pos2, vec2, Color32, CornerRadius, Rect, Response, RichText, Sense, Stroke, Ui, Widget};

use crate::theme;

/// Section heading ("Background", "Padding" …).
pub fn title(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).size(16.0).color(theme::p().text));
    ui.add_space(4.0);
}

/// Sub heading / description text.
/// Tooltip for controls that are disabled because the feature is not built yet.
pub const ROADMAP: &str = "Not available yet · on the product roadmap";

/// Run `add` in a disabled scope (greyed out, no interaction) and attach the roadmap tooltip.
pub fn roadmap<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let out = ui.scope(|ui| {
        ui.disable();
        add(ui)
    });
    out.response.on_disabled_hover_text(ROADMAP).on_hover_text(ROADMAP);
    out.inner
}

pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(theme::p().text_dim));
}

/// A row of mutually exclusive buttons. Returns the newly selected index if it changed.
pub fn segmented(ui: &mut Ui, selected: usize, labels: &[&str]) -> Option<usize> {
    segmented_with_disabled(ui, selected, labels, &[])
}

/// `segmented` with some options greyed out (roadmap items).
pub fn segmented_with_disabled(ui: &mut Ui, selected: usize, labels: &[&str], disabled: &[usize]) -> Option<usize> {
    let mut out = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (i, l) in labels.iter().enumerate() {
            let sel = i == selected;
            let fill = if sel { theme::p().card_3 } else { theme::p().card_2 };
            let b = egui::Button::new(RichText::new(*l).color(if sel { theme::p().text } else { theme::p().text_dim })).fill(fill).corner_radius(CornerRadius::same(BUTTON_RADIUS)).min_size(vec2(0.0, 30.0));
            if disabled.contains(&i) {
                ui.add_enabled(false, b).on_disabled_hover_text(ROADMAP);
                continue;
            }
            if ui.add(b).clicked() && !sel {
                out = Some(i);
            }
        }
    });
    out
}

/// Small pill buttons (wallpaper categories). Returns the newly selected index.
pub fn pills(ui: &mut Ui, selected: usize, labels: &[&str]) -> Option<usize> {
    let mut out = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (i, l) in labels.iter().enumerate() {
            let sel = i == selected;
            // outlined chips; the selected one is filled with an accent border
            let stroke = if sel { Stroke::new(1.5, theme::p().accent_hover) } else { Stroke::new(1.0, theme::p().border) };
            let b = egui::Button::new(RichText::new(*l).size(13.0).color(if sel { theme::p().text } else { theme::p().text_dim }))
                .fill(if sel { theme::p().card_3 } else { Color32::TRANSPARENT })
                .stroke(stroke)
                .corner_radius(CornerRadius::same(BUTTON_RADIUS))
                .min_size(vec2(44.0, 30.0));
            if ui.add(b).clicked() && !sel {
                out = Some(i);
            }
        }
    });
    out
}

/// Result of [`labeled_slider`].
pub struct SliderResult {
    pub response: Response,
    pub reset: bool,
}

/// "Label\n[———●———]  Reset" slider as in the Screen Studio sidebar.
pub fn labeled_slider(ui: &mut Ui, label: &str, value: &mut f64, range: std::ops::RangeInclusive<f64>, default: Option<f64>, suffix: &str) -> SliderResult {
    ui.add_space(6.0);
    ui.label(RichText::new(label).size(15.0).color(theme::p().text));
    ui.add_space(2.0);
    let mut reset = false;
    let response = ui
        .horizontal(|ui| {
            let w = ui.available_width() - if default.is_some() { 64.0 } else { 0.0 };
            ui.spacing_mut().slider_width = w.max(80.0);
            let r = ui.add(egui::Slider::new(value, range).show_value(false).suffix(suffix));
            if let Some(d) = default {
                let changed = (*value - d).abs() > 1e-9;
                let btn = egui::Button::new(RichText::new("Reset").size(13.0).color(if changed { theme::p().text_dim } else { theme::p().text_faint })).frame(false);
                if ui.add_enabled(changed, btn).clicked() {
                    *value = d;
                    reset = true;
                }
            }
            r
        })
        .inner;
    ui.add_space(4.0);
    SliderResult { response, reset }
}

/// iOS style toggle with a title and description on the left.
pub fn toggle_row(ui: &mut Ui, title: &str, description: &str, on: &mut bool) -> bool {
    ui.add_space(6.0);
    let changed = ui
        .horizontal(|ui| {
            // keep text clear of the switch on the right (switch ~44 px + gap)
            let text_w = (ui.available_width() - 60.0).max(80.0);
            ui.vertical(|ui| {
                ui.set_max_width(text_w);
                ui.add(egui::Label::new(RichText::new(title).size(15.0).color(theme::p().text)).wrap());
                if !description.is_empty() {
                    ui.add(egui::Label::new(RichText::new(description).size(13.0).color(theme::p().text_dim)).wrap());
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.add(Toggle(on)).changed())
        })
        .inner
        .inner;
    ui.add_space(4.0);
    changed
}

pub struct Toggle<'a>(pub &'a mut bool);

impl Widget for Toggle<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let size = vec2(44.0, 26.0);
        let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
        if resp.clicked() {
            *self.0 = !*self.0;
            resp.mark_changed();
        }
        let t = ui.ctx().animate_bool(resp.id, *self.0);
        let fill = egui::lerp(egui::Rgba::from(theme::p().card_3)..=egui::Rgba::from(theme::p().accent), t);
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(13), Color32::from(fill));
        let knob_x = egui::lerp(rect.left() + 13.0..=rect.right() - 13.0, t);
        painter.circle_filled(pos2(knob_x, rect.center().y), 10.0, Color32::WHITE);
        resp
    }
}

/// Prominent accent button ("Export").
/// Button corner radius and height shared by every text button (toolbar guideline: square-ish).
pub const BUTTON_RADIUS: u8 = 4;
pub const BUTTON_H: f32 = 32.0;

/// Primary action: accent fill, white text.
pub fn accent_button(ui: &mut Ui, text: &str) -> Response {
    let fill = if ui.is_enabled() { theme::p().accent } else { theme::p().card_3 };
    ui.add(egui::Button::new(RichText::new(text).size(14.0).strong().color(Color32::WHITE)).fill(fill).corner_radius(CornerRadius::same(BUTTON_RADIUS)).min_size(vec2(0.0, BUTTON_H)))
}

/// Secondary action: card fill, normal text, same size as `accent_button`.
pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Button::new(RichText::new(text).size(14.0).color(theme::p().text)).fill(theme::p().card_2).corner_radius(CornerRadius::same(BUTTON_RADIUS)).min_size(vec2(0.0, BUTTON_H)))
}

/// Single-line text field: roomy (38 px) with 4 px corners like the buttons, padded text and a
/// quiet border that turns accent while focused.
pub fn text_field(ui: &mut Ui, text: &mut String, hint: &str) -> Response {
    let pal = theme::p();
    let id = ui.next_auto_id().with("text_field");
    let focused = ui.memory(|m| m.has_focus(id));
    egui::Frame::NONE
        .fill(pal.bg)
        .stroke(Stroke::new(1.0, if focused { pal.accent } else { pal.border }))
        .corner_radius(CornerRadius::same(BUTTON_RADIUS))
        .inner_margin(egui::Margin::symmetric(12, 0))
        .show(ui, |ui| {
            ui.add_sized(
                vec2(ui.available_width(), 36.0),
                egui::TextEdit::singleline(text)
                    .id(id)
                    .hint_text(RichText::new(hint).color(pal.text_faint))
                    .font(egui::FontId::proportional(14.0))
                    .text_color(pal.text)
                    .frame(egui::Frame::NONE)
                    .vertical_align(egui::Align::Center)
                    .margin(egui::Margin::ZERO)
                    .min_size(vec2(0.0, 36.0)),
            )
        })
        .inner
}

/// Paint a rounded card background behind `rect`.
pub fn card(painter: &egui::Painter, rect: Rect) {
    painter.rect(rect, CornerRadius::same(theme::RADIUS as u8 + 4), theme::p().card, Stroke::new(1.0, theme::p().border), egui::StrokeKind::Inside);
}


/// Simple vector icons painted with strokes (egui's bundled fonts lack most UI glyphs).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Display,
    Window,
    Area,
    Device,
    Crop,
    Aspect,
    Undo,
    Redo,
    ChevronDown,
    Close,
    Camera,
    Mic,
    Pause,
    Play,
    Flag,
    Restart,
    Zoom,
    Sparkle,
    Layout,
    Mask,
    General,
    Cursor,
    CameraLayout,
    CameraStyle,
    Captions,
    Audio,
    Keyboard,
    Animations,
    DeviceFrame,
    Folder,
    Trash,
    Gear,
    Command,
    Waveform,
    Scissors,
}

/// Microphone icon whose capsule fills from the bottom with the live input `level` (0..1).
pub fn paint_mic_level(p: &egui::Painter, rect: Rect, level: f32, color: Color32, fill: Color32) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) / 2.0;
    let capsule = Rect::from_min_max(pos2(c.x - 0.3 * r, c.y - 0.9 * r), pos2(c.x + 0.3 * r, c.y + 0.2 * r));
    let top = capsule.bottom() - capsule.height() * level.clamp(0.0, 1.0);
    p.with_clip_rect(Rect::from_min_max(pos2(capsule.left() - 2.0, top), pos2(capsule.right() + 2.0, capsule.bottom() + 2.0)))
        .rect_filled(capsule, 6.0, fill);
    paint_icon(p, rect, Icon::Mic, color);
}

pub fn paint_icon(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = Stroke::new(1.6, color);
    let c = rect.center();
    let r = rect.width().min(rect.height()) / 2.0;
    let rr = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(c.x + x0 * r, c.y + y0 * r), pos2(c.x + x1 * r, c.y + y1 * r));
    let line = |a: (f32, f32), b: (f32, f32)| p.line_segment([pos2(c.x + a.0 * r, c.y + a.1 * r), pos2(c.x + b.0 * r, c.y + b.1 * r)], s);
    match icon {
        Icon::Display => {
            p.rect_stroke(rr(-0.9, -0.7, 0.9, 0.45), 2.0, s, egui::StrokeKind::Middle);
            line((-0.35, 0.85), (0.35, 0.85));
            line((0.0, 0.45), (0.0, 0.85));
        }
        Icon::Window => {
            p.rect_stroke(rr(-0.9, -0.75, 0.9, 0.75), 3.0, s, egui::StrokeKind::Middle);
            line((-0.9, -0.3), (0.9, -0.3));
            p.circle_filled(pos2(c.x - 0.6 * r, c.y - 0.52 * r), 1.2, color);
            p.circle_filled(pos2(c.x - 0.35 * r, c.y - 0.52 * r), 1.2, color);
        }
        Icon::Area => {
            let d = 0.3;
            for (x, y) in [(-0.9, -0.9), (0.9, -0.9), (-0.9, 0.9), (0.9, 0.9)] {
                let sx = if x < 0.0 { 1.0 } else { -1.0 };
                let sy = if y < 0.0 { 1.0 } else { -1.0 };
                line((x, y), (x + sx * d, y));
                line((x, y), (x, y + sy * d));
            }
            line((-0.2, -0.9), (0.2, -0.9));
            line((-0.2, 0.9), (0.2, 0.9));
            line((-0.9, -0.2), (-0.9, 0.2));
            line((0.9, -0.2), (0.9, 0.2));
        }
        Icon::Device | Icon::DeviceFrame => {
            p.rect_stroke(rr(-0.5, -0.9, 0.5, 0.9), 4.0, s, egui::StrokeKind::Middle);
            line((-0.15, 0.65), (0.15, 0.65));
        }
        Icon::Crop => {
            line((-0.5, -0.9), (-0.5, 0.5));
            line((-0.5, 0.5), (0.9, 0.5));
            line((-0.9, -0.5), (0.5, -0.5));
            line((0.5, -0.5), (0.5, 0.9));
        }
        Icon::Aspect => {
            p.rect_stroke(rr(-0.9, -0.6, 0.9, 0.6), 2.0, s, egui::StrokeKind::Middle);
            line((0.2, -0.6), (0.2, 0.6));
        }
        Icon::Undo | Icon::Redo => {
            // curved arrow: a half-circle arc with an arrowhead; mirrored for redo
            let f = if icon == Icon::Undo { 1.0 } else { -1.0 };
            let (cx, cy, rad) = (0.05, 0.15, 0.55);
            let pts: Vec<egui::Pos2> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 16.0); // 180°..360°: upper half
                    pos2(c.x + f * (cx + rad * a.cos()) * r, c.y + (cy + rad * a.sin()) * r)
                })
                .collect();
            p.add(egui::Shape::line(pts, s));
            // tail going down on the far side, head on the near side pointing down-left
            line((f * (cx + rad), cy), (f * (cx + rad), cy + 0.45));
            let tip = (f * (cx - rad), cy);
            line(tip, (tip.0 - f * 0.28, tip.1 - 0.28));
            line(tip, (tip.0 + f * 0.28, tip.1 - 0.28));
        }
        Icon::ChevronDown => {
            line((-0.6, -0.3), (0.0, 0.3));
            line((0.0, 0.3), (0.6, -0.3));
        }
        Icon::Close => {
            line((-0.6, -0.6), (0.6, 0.6));
            line((-0.6, 0.6), (0.6, -0.6));
        }
        Icon::Camera => {
            p.rect_stroke(rr(-0.9, -0.55, 0.9, 0.65), 3.0, s, egui::StrokeKind::Middle);
            line((-0.35, -0.55), (-0.2, -0.85));
            line((-0.2, -0.85), (0.2, -0.85));
            line((0.2, -0.85), (0.35, -0.55));
            p.circle_stroke(pos2(c.x, c.y + 0.07 * r), 0.32 * r, s);
        }
        Icon::Mic => {
            p.rect_stroke(rr(-0.3, -0.9, 0.3, 0.2), 6.0, s, egui::StrokeKind::Middle);
            let pts: Vec<egui::Pos2> = (0..=12).map(|i| {
                let a = i as f32 / 12.0 * std::f32::consts::PI;
                pos2(c.x - 0.6 * r * a.cos(), c.y - 0.05 * r + 0.55 * r * a.sin())
            }).collect();
            p.add(egui::Shape::line(pts, s));
            line((0.0, 0.5), (0.0, 0.9));
            line((-0.35, 0.9), (0.35, 0.9));
        }
        Icon::Pause => {
            p.rect_filled(rr(-0.6, -0.7, -0.2, 0.7), 1.0, color);
            p.rect_filled(rr(0.2, -0.7, 0.6, 0.7), 1.0, color);
        }
        Icon::Play => {
            p.add(egui::Shape::convex_polygon(vec![pos2(c.x - 0.5 * r, c.y - 0.7 * r), pos2(c.x + 0.7 * r, c.y), pos2(c.x - 0.5 * r, c.y + 0.7 * r)], color, Stroke::NONE));
        }
        Icon::Flag => {
            line((-0.6, -0.8), (-0.6, 0.9));
            p.add(egui::Shape::convex_polygon(vec![pos2(c.x - 0.6 * r, c.y - 0.8 * r), pos2(c.x + 0.7 * r, c.y - 0.5 * r), pos2(c.x - 0.6 * r, c.y - 0.1 * r)], color, Stroke::NONE));
        }
        Icon::Restart => {
            // open circular arrow
            let pts: Vec<egui::Pos2> = (0..=28).map(|i| {
                let a = -0.5 * std::f32::consts::PI + 0.25 + i as f32 / 28.0 * (2.0 * std::f32::consts::PI - 0.9);
                pos2(c.x + 0.72 * r * a.cos(), c.y + 0.72 * r * a.sin())
            }).collect();
            p.add(egui::Shape::line(pts, s));
            line((0.05, -0.95), (0.35, -0.62));
            line((0.05, -0.3), (0.35, -0.62));
        }
        Icon::Zoom => {
            p.circle_stroke(pos2(c.x - 0.15 * r, c.y - 0.15 * r), 0.55 * r, s);
            line((0.25, 0.25), (0.8, 0.8));
            line((-0.45, -0.15), (0.15, -0.15));
        }
        Icon::Sparkle | Icon::Animations => {
            line((0.0, -0.9), (0.0, 0.9));
            line((-0.9, 0.0), (0.9, 0.0));
            line((-0.5, -0.5), (0.5, 0.5));
            line((-0.5, 0.5), (0.5, -0.5));
            p.circle_filled(c, 0.22 * r, color);
        }
        Icon::Layout | Icon::CameraLayout => {
            p.rect_stroke(rr(-0.9, -0.7, 0.9, 0.7), 2.0, s, egui::StrokeKind::Middle);
            p.rect_filled(rr(0.25, 0.1, 0.75, 0.55), 1.5, color);
        }
        Icon::Mask => {
            p.rect_stroke(rr(-0.9, -0.7, 0.9, 0.7), 2.0, s, egui::StrokeKind::Middle);
            for i in 0..4 {
                let x = -0.7 + i as f32 * 0.45;
                line((x, -0.4), (x + 0.3, 0.4));
            }
        }
        Icon::General => {
            p.rect_stroke(rr(-0.9, -0.75, 0.9, 0.75), 3.0, s, egui::StrokeKind::Middle);
            p.rect_stroke(rr(-0.5, -0.4, 0.5, 0.4), 2.0, s, egui::StrokeKind::Middle);
        }
        Icon::Cursor => {
            let pts = vec![pos2(c.x - 0.5 * r, c.y - 0.8 * r), pos2(c.x + 0.6 * r, c.y + 0.1 * r), pos2(c.x + 0.05 * r, c.y + 0.2 * r), pos2(c.x + 0.35 * r, c.y + 0.85 * r), pos2(c.x + 0.05 * r, c.y + 0.95 * r), pos2(c.x - 0.2 * r, c.y + 0.35 * r), pos2(c.x - 0.5 * r, c.y + 0.6 * r)];
            p.add(egui::Shape::closed_line(pts, s));
        }
        Icon::CameraStyle => {
            p.circle_stroke(c, 0.75 * r, s);
            p.circle_filled(pos2(c.x - 0.25 * r, c.y - 0.15 * r), 1.5, color);
            p.circle_filled(pos2(c.x + 0.25 * r, c.y - 0.15 * r), 1.5, color);
            let pts: Vec<egui::Pos2> = (0..=8).map(|i| { let t = -0.35 + 0.7 * i as f32 / 8.0; pos2(c.x + t * r, c.y + (0.25 + 0.2 * (1.0 - (t / 0.35).powi(2))) * r) }).collect();
            p.add(egui::Shape::line(pts, s));
        }
        Icon::Captions => {
            p.rect_stroke(rr(-0.9, -0.7, 0.9, 0.4), 4.0, s, egui::StrokeKind::Middle);
            line((-0.4, 0.4), (-0.3, 0.85));
            line((-0.3, 0.85), (0.1, 0.4));
            line((-0.5, -0.15), (0.5, -0.15));
        }
        Icon::Audio => {
            let pts = vec![pos2(c.x - 0.8 * r, c.y - 0.3 * r), pos2(c.x - 0.4 * r, c.y - 0.3 * r), pos2(c.x + 0.1 * r, c.y - 0.75 * r), pos2(c.x + 0.1 * r, c.y + 0.75 * r), pos2(c.x - 0.4 * r, c.y + 0.3 * r), pos2(c.x - 0.8 * r, c.y + 0.3 * r)];
            p.add(egui::Shape::closed_line(pts, s));
            line((0.4, -0.35), (0.4, 0.35));
            line((0.75, -0.6), (0.75, 0.6));
        }
        Icon::Keyboard | Icon::Command => {
            p.rect_stroke(rr(-0.9, -0.55, 0.9, 0.55), 3.0, s, egui::StrokeKind::Middle);
            for i in 0..4 {
                let x = -0.6 + i as f32 * 0.4;
                p.circle_filled(pos2(c.x + x * r, c.y - 0.2 * r), 1.2, color);
            }
            line((-0.45, 0.25), (0.45, 0.25));
        }
        Icon::Folder => {
            p.rect_stroke(rr(-0.9, -0.5, 0.9, 0.7), 2.5, s, egui::StrokeKind::Middle);
            line((-0.9, -0.5), (-0.6, -0.8));
            line((-0.6, -0.8), (-0.1, -0.8));
            line((-0.1, -0.8), (0.1, -0.5));
        }
        Icon::Trash => {
            line((-0.8, -0.6), (0.8, -0.6));
            line((-0.3, -0.85), (0.3, -0.85));
            p.rect_stroke(rr(-0.6, -0.6, 0.6, 0.85), 2.0, s, egui::StrokeKind::Middle);
            line((-0.2, -0.3), (-0.2, 0.55));
            line((0.2, -0.3), (0.2, 0.55));
        }
        Icon::Gear => {
            p.circle_stroke(c, 0.35 * r, s);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                p.line_segment([pos2(c.x + a.cos() * 0.55 * r, c.y + a.sin() * 0.55 * r), pos2(c.x + a.cos() * 0.9 * r, c.y + a.sin() * 0.9 * r)], Stroke::new(2.2, color));
            }
        }
        Icon::Waveform => {
            for (i, h) in [0.3, 0.7, 1.0, 0.5, 0.8, 0.35].iter().enumerate() {
                let x = -0.85 + i as f32 * 0.34;
                line((x, -h * 0.8), (x, h * 0.8));
            }
        }
        Icon::Scissors => {
            p.circle_stroke(pos2(c.x - 0.5 * r, c.y + 0.5 * r), 0.3 * r, s);
            p.circle_stroke(pos2(c.x + 0.5 * r, c.y + 0.5 * r), 0.3 * r, s);
            line((-0.3, 0.3), (0.6, -0.9));
            line((0.3, 0.3), (-0.6, -0.9));
        }
    }
}

/// Icon-only button (quiet).
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, tooltip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size + 12.0, size + 8.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(7), theme::p().card_2);
    }
    let color = if resp.hovered() { theme::p().text } else { theme::p().text_dim };
    paint_icon(ui.painter(), Rect::from_center_size(rect.center(), vec2(size, size)), icon, color);
    resp.on_hover_text(tooltip)
}

/// Rail button drawn with a vector icon.
pub fn rail_icon(ui: &mut Ui, icon: Icon, selected: bool, tooltip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
    let painter = ui.painter();
    if selected || resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(9), if selected { theme::p().card_3 } else { theme::p().card_2 });
    }
    let color = if selected { theme::p().accent_hover } else if resp.hovered() { theme::p().text } else { theme::p().text_dim };
    paint_icon(painter, Rect::from_center_size(rect.center(), vec2(18.0, 18.0)), icon, color);
    if selected {
        painter.circle_filled(pos2(rect.right() - 6.0, rect.top() + 7.0), 2.5, theme::p().accent_hover);
    }
    resp.on_hover_text(tooltip)
}

/// Text button with a vector icon on the left and an optional chevron on the right.
pub fn icon_text_button(ui: &mut Ui, icon: Option<Icon>, text: &str, chevron: bool) -> Response {
    let font = egui::FontId::proportional(14.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, theme::p().text);
    let w = galley.size().x + 24.0 + if icon.is_some() { 22.0 } else { 0.0 } + if chevron { 18.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(8), theme::p().card_2);
    }
    let mut x = rect.left() + 12.0;
    if let Some(ic) = icon {
        paint_icon(p, Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(15.0, 15.0)), ic, theme::p().text);
        x += 22.0;
    }
    p.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::p().text);
    if chevron {
        paint_icon(p, Rect::from_center_size(pos2(rect.right() - 14.0, rect.center().y), vec2(10.0, 10.0)), Icon::ChevronDown, theme::p().text_dim);
    }
    resp
}
