//! First-launch permissions window: asks for Screen Recording (required), Input Monitoring
//! (cursor and clicks), Microphone and Accessibility (zooms follow typing), like Screen Studio's onboarding. Shown in its own
//! viewport centred on the screen because the picker window is only a thin bar.

use egui::{vec2, Align2, Color32, CornerRadius, FontId, RichText, Stroke};
use sf_recording::permissions as p;
use sf_recording::{Permission, PermissionStatus};

use crate::{theme, widgets};

pub struct PermissionsSetup {
    last_poll: f64,
    /// Fixed once so the window does not move after the first frame.
    pos: Option<egui::Pos2>,
    status: [PermissionStatus; 4],
    /// The Accessibility prompt was shown once; later clicks open System Settings.
    ax_prompted: bool,
}

const SIZE: egui::Vec2 = vec2(460.0, 660.0);

impl PermissionsSetup {
    /// True when a recording could not work yet.
    pub fn needed() -> bool {
        !p::screen_recording().is_granted() || !p::input_monitoring().is_granted()
    }

    /// Create the window and fire the system prompts that can be shown without user action.
    pub fn start() -> Self {
        if !p::screen_recording().is_granted() {
            let _ = p::request_screen_recording();
        }
        PermissionsSetup { last_poll: 0.0, pos: None, status: Self::poll(), ax_prompted: false }
    }

    fn poll() -> [PermissionStatus; 4] {
        // SF_SHOW_PERMISSIONS=denied: UI test for the not-granted layout
        if std::env::var("SF_SHOW_PERMISSIONS").as_deref() == Ok("denied") {
            return [PermissionStatus::Denied, PermissionStatus::Denied, PermissionStatus::NotDetermined, PermissionStatus::NotDetermined];
        }
        [p::screen_recording(), p::input_monitoring(), p::microphone(), p::accessibility()]
    }

    /// Returns true when the window should close.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let now = ctx.input(|i| i.time);
        if now - self.last_poll > 0.7 {
            self.last_poll = now;
            self.status = Self::poll();
        }
        let pos = *self.pos.get_or_insert_with(|| {
            let mon = ctx.input(|i| i.viewport().monitor_size).unwrap_or(vec2(1920.0, 1080.0));
            egui::pos2(((mon.x - SIZE.x) / 2.0).max(0.0), ((mon.y - SIZE.y) / 2.0 - 40.0).max(0.0))
        });
        let builder = egui::ViewportBuilder::default()
            .with_title("ScreenForge permissions")
            .with_inner_size(SIZE)
            .with_position(pos)
            .with_resizable(false)
            .with_decorations(true)
            .with_always_on_top()
            .with_active(true);
        let mut done = false;
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("permissions"), builder, |vctx, _| {
            let pal = theme::p();
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(pal.bg).inner_margin(egui::Margin::same(24))).show(vctx, |ui| {
                ui.label(RichText::new("Welcome to ScreenForge").size(20.0).strong().color(pal.text));
                ui.add_space(4.0);
                ui.label(RichText::new("Grant these permissions so recordings capture the screen, the cursor and your voice. macOS may ask you to quit and reopen the app after enabling Screen Recording.").size(13.0).color(pal.text_dim));
                ui.add_space(16.0);
                let rows = [
                    (Permission::ScreenRecording, widgets::Icon::Display, "Screen Recording", "Required to capture the display or a window.", self.status[0]),
                    (Permission::InputMonitoring, widgets::Icon::Cursor, "Input Monitoring", "Cursor, clicks and keystrokes: smooth cursor and automatic zooms.", self.status[1]),
                    (Permission::Microphone, widgets::Icon::Mic, "Microphone", "Optional. Record your voice with the screen.", self.status[2]),
                    (Permission::Accessibility, widgets::Icon::Keyboard, "Accessibility", "Optional. Automatic zooms follow the text you type.", self.status[3]),
                ];
                for (perm, icon, title, desc, status) in rows {
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 84.0), egui::Sense::hover());
                    ui.painter().rect(rect, CornerRadius::same(12), pal.card, Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
                    widgets::paint_icon(ui.painter(), egui::Rect::from_center_size(egui::pos2(rect.left() + 30.0, rect.center().y), vec2(22.0, 22.0)), icon, pal.text);
                    ui.painter().text(egui::pos2(rect.left() + 58.0, rect.top() + 26.0), Align2::LEFT_CENTER, title, FontId::proportional(14.5), pal.text);
                    let desc_rect = egui::Rect::from_min_max(egui::pos2(rect.left() + 58.0, rect.top() + 40.0), egui::pos2(rect.right() - 150.0, rect.bottom() - 8.0));
                    let galley = ui.fonts_mut(|f| f.layout(desc.to_string(), FontId::proportional(11.5), pal.text_dim, desc_rect.width()));
                    ui.painter().galley(desc_rect.min, galley, pal.text_dim);
                    let btn_rect = egui::Rect::from_center_size(egui::pos2(rect.right() - 74.0, rect.center().y), vec2(128.0, 30.0));
                    match status {
                        PermissionStatus::Granted => {
                            ui.painter().circle_filled(egui::pos2(btn_rect.center().x - 26.0, btn_rect.center().y), 6.0, Color32::from_rgb(0x3f, 0xc1, 0x7a));
                            ui.painter().text(egui::pos2(btn_rect.center().x - 14.0, btn_rect.center().y), Align2::LEFT_CENTER, "Granted", FontId::proportional(13.0), pal.text);
                        }
                        PermissionStatus::Unsupported => {
                            ui.painter().text(btn_rect.center(), Align2::CENTER_CENTER, "Unavailable", FontId::proportional(13.0), pal.text_faint);
                        }
                        _ => {
                            let label = if status == PermissionStatus::Denied { "Open Settings" } else { "Grant" };
                            let b = ui.put(btn_rect, egui::Button::new(RichText::new(label).size(12.5).color(Color32::WHITE)).fill(pal.accent).corner_radius(CornerRadius::same(widgets::BUTTON_RADIUS)).wrap_mode(egui::TextWrapMode::Extend));
                            if b.clicked() {
                                let after = match (perm, status) {
                                    (Permission::ScreenRecording, PermissionStatus::NotDetermined) => p::request_screen_recording(),
                                    (Permission::InputMonitoring, PermissionStatus::NotDetermined) => p::request_input_monitoring(),
                                    (Permission::Microphone, PermissionStatus::NotDetermined) => p::request_microphone(),
                                    // first click: macOS's own prompt (it links to System Settings)
                                    (Permission::Accessibility, _) if !self.ax_prompted => {
                                        self.ax_prompted = true;
                                        let _ = p::request_accessibility();
                                        PermissionStatus::Granted
                                    }
                                    _ => status,
                                };
                                if !after.is_granted() {
                                    let _ = p::open_settings(perm);
                                }
                                self.status = Self::poll();
                            }
                        }
                    }
                    // `ui.put` leaves the cursor at the button's bottom; continue below the card
                    ui.advance_cursor_after_rect(rect);
                    ui.add_space(10.0);
                }
                ui.add_space(6.0);
                let all = self.status[0].is_granted() && self.status[1].is_granted();
                ui.horizontal(|ui| {
                    let label = if all { "Continue" } else { "Continue anyway" };
                    if widgets::accent_button(ui, label).clicked() {
                        done = true;
                    }
                    if !all && widgets::secondary_button(ui, "Quit and reopen").clicked() {
                        relaunch();
                    }
                });
                if !all {
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(RichText::new("Already enabled ScreenForge in System Settings? macOS applies Screen Recording and Input Monitoring only after the app is reopened: use \"Quit and reopen\". Without them, recording fails and the cursor is not captured.").size(11.5).color(pal.text_faint)).wrap());
                }
            });
            if vctx.input(|i| i.viewport().close_requested()) {
                done = true;
            }
            vctx.request_repaint_after(std::time::Duration::from_millis(500));
        });
        done
    }
}

/// Quit and start the app again (needed for macOS to apply newly granted permissions).
fn relaunch() {
    if let Ok(exe) = std::env::current_exe() {
        let app = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(|p| p.to_path_buf());
        match app {
            Some(app) => {
                let _ = std::process::Command::new("open").arg("-n").arg(&app).spawn();
            }
            None => {
                let _ = std::process::Command::new(&exe).spawn();
            }
        }
    }
    std::process::exit(0);
}
