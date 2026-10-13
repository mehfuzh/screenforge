//! Live camera preview (a rounded square) shown while a recording with a camera counts down and runs, like Loom
//! and Screen Studio. It is a window of our own, so the screen capture leaves it out (the
//! camera is recorded on its own channel and composited in the editor). The camera it opens
//! is handed to the recorder, so recording starts without waiting for the camera to wake up.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

use egui::{pos2, vec2, Color32, Pos2, Stroke};
use sf_recording::camera::CameraCapture;

use crate::{theme, widgets};

/// Bubble side in points.
const SIZE: f32 = 180.0;
/// Corner radius as a ratio of the side: the editor's default camera roundness
/// (`styles.cameraBorderRadiusRatio01`), so the preview looks like the finished video.
const CORNER_RATIO: f32 = 0.15;
/// Longest side of the preview texture (the camera delivers up to 1280 px).
const PREVIEW_SIDE: u32 = 360;

pub struct CameraBubble {
    opening: Option<Receiver<Result<Arc<CameraCapture>, String>>>,
    pub capture: Option<Arc<CameraCapture>>,
    pub error: Option<String>,
    texture: Option<egui::TextureHandle>,
    last_frame: u64,
    pos: Option<Pos2>,
}

impl CameraBubble {
    /// Start opening camera `id` in the background.
    pub fn open(id: &str) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let id = id.to_string();
        std::thread::spawn(move || {
            let r = CameraCapture::open(&id, sf_recording::log::RecLog::null()).map_err(|e| e.to_string());
            let _ = tx.send(r);
        });
        CameraBubble { opening: Some(rx), capture: None, error: None, texture: None, last_frame: 0, pos: None }
    }

    /// Still waiting for the camera's first frame.
    pub fn is_opening(&self) -> bool {
        self.opening.is_some()
    }

    /// Draw the bubble in its own always-on-top window (bottom-left of the screen, draggable).
    pub fn show(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.opening {
            match rx.try_recv() {
                Ok(Ok(c)) => {
                    self.capture = Some(c);
                    self.opening = None;
                }
                Ok(Err(e)) => {
                    self.error = Some(e);
                    self.opening = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.error = Some("the camera could not be opened".into());
                    self.opening = None;
                }
            }
        }
        if let Some(c) = &self.capture {
            let n = c.frames_received();
            if n != self.last_frame {
                self.last_frame = n;
                if let Some(frame) = c.latest() {
                    let (w, h, rgba) = frame.to_rgba(PREVIEW_SIDE, true);
                    let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                    match &mut self.texture {
                        Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                        None => self.texture = Some(ctx.load_texture("camera-bubble", img, egui::TextureOptions::LINEAR)),
                    }
                }
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(33));

        let pos = *self.pos.get_or_insert_with(|| {
            let mon = ctx.input(|i| i.viewport().monitor_size).unwrap_or(vec2(1440.0, 900.0));
            pos2(40.0, (mon.y - SIZE - 140.0).max(0.0))
        });
        let builder = egui::ViewportBuilder::default()
            .with_title("Camera")
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_resizable(false)
            .with_active(false)
            .with_position(pos)
            .with_inner_size(vec2(SIZE, SIZE));
        let texture = self.texture.clone();
        let opening = self.is_opening();
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("camera-bubble"), builder, |vctx, _| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(vctx, |ui| {
                let rect = ui.max_rect();
                let resp = ui.interact(rect, ui.id().with("bubble-drag"), egui::Sense::drag());
                if resp.drag_started() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                let pal = theme::p();
                let square = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(rect.width().min(rect.height()) - 4.0));
                let radius = egui::CornerRadius::same((square.width() * CORNER_RATIO).round() as u8);
                let p = ui.painter();
                match &texture {
                    Some(t) => {
                        p.add(egui::epaint::RectShape::filled(square, radius, Color32::WHITE).with_texture(t.id(), centre_square_uv(t.size())));
                    }
                    None => {
                        p.rect_filled(square, radius, pal.glass_solid);
                        let tint = if opening { pal.text_dim } else { pal.danger };
                        widgets::paint_icon(p, egui::Rect::from_center_size(square.center(), vec2(36.0, 36.0)), widgets::Icon::Camera, tint);
                    }
                }
                p.rect_stroke(square, radius, Stroke::new(2.0, pal.glass_border), egui::StrokeKind::Inside);
            });
        });
    }
}

/// UV rect of the centre square of a `[width, height]` texture (the bubble crops to a square).
fn centre_square_uv([w, h]: [usize; 2]) -> egui::Rect {
    let aspect = w as f32 / h.max(1) as f32;
    let (hu, hv) = if aspect >= 1.0 { (0.5 / aspect, 0.5) } else { (0.5, 0.5 * aspect) };
    egui::Rect::from_min_max(pos2(0.5 - hu, 0.5 - hv), pos2(0.5 + hu, 0.5 + hv))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bubble_crops_the_centre_square() {
        // landscape: the middle 9/16 of the width, the full height
        let uv = centre_square_uv([16, 9]);
        assert!((uv.width() - 9.0 / 16.0).abs() < 1e-6 && (uv.height() - 1.0).abs() < 1e-6);
        assert!((uv.center().x - 0.5).abs() < 1e-6);
        // portrait: the full width, the middle of the height
        let uv = centre_square_uv([9, 16]);
        assert!((uv.width() - 1.0).abs() < 1e-6 && (uv.height() - 9.0 / 16.0).abs() < 1e-6);
    }
}
