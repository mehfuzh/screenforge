//! First-launch permissions window: asks for Screen Recording (required), Input Monitoring
//! (cursor and clicks), Microphone, Camera and Accessibility (zooms follow typing), like Screen Studio's onboarding. Shown in its own
//! viewport centred on the screen because the picker window is only a thin bar.

use egui::{vec2, Align2, Color32, CornerRadius, FontId, RichText, Stroke};
use sf_recording::permissions as p;
use sf_recording::{Permission, PermissionStatus};

use crate::{theme, widgets};

pub struct PermissionsSetup {
    last_poll: f64,
    /// Fixed once so the window does not move after the first frame.
    pos: Option<egui::Pos2>,
    /// Window height, measured from the content so there is no empty space below it.
    height: f32,
    status: [PermissionStatus; 5],
    /// The Accessibility prompt was shown once; later clicks open System Settings.
    ax_prompted: bool,
    /// The app icon beside the title, loaded on first show.
    logo: Option<egui::TextureHandle>,
}

const WIDTH: f32 = 480.0;
/// First-frame guess; corrected to the measured content height right away.
const INITIAL_HEIGHT: f32 = 500.0;
const MARGIN: i8 = 22;
const ROW_H: f32 = 52.0;
/// Left edge of the row text, right of the icon.
const TEXT_X: f32 = 48.0;
const LOGO: f32 = 40.0;

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
        PermissionsSetup { last_poll: 0.0, pos: None, height: INITIAL_HEIGHT, status: Self::poll(), ax_prompted: false, logo: None }
    }

    fn poll() -> [PermissionStatus; 5] {
        // SF_SHOW_PERMISSIONS=denied: UI test for the not-granted layout
        if std::env::var("SF_SHOW_PERMISSIONS").as_deref() == Ok("denied") {
            return [PermissionStatus::Denied, PermissionStatus::Denied, PermissionStatus::NotDetermined, PermissionStatus::NotDetermined, PermissionStatus::NotDetermined];
        }
        [p::screen_recording(), p::input_monitoring(), p::microphone(), p::camera(), p::accessibility()]
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
            egui::pos2(((mon.x - WIDTH) / 2.0).max(0.0), ((mon.y - INITIAL_HEIGHT) / 2.0 - 40.0).max(0.0))
        });
        let builder = egui::ViewportBuilder::default()
            .with_title("ScreenForge permissions")
            .with_inner_size(vec2(WIDTH, self.height))
            .with_position(pos)
            .with_resizable(false)
            .with_decorations(true)
            .with_always_on_top()
            .with_active(true);
        let mut done = false;
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("permissions"), builder, |vctx, _| {
            let pal = theme::p();
            let mut content_bottom = 0.0;
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(pal.bg).inner_margin(egui::Margin::same(MARGIN))).show(vctx, |ui| {
                let logo = self.logo.get_or_insert_with(|| {
                    let img = image::load_from_memory(include_bytes!("../assets/icon-256.png")).expect("app icon").resize_exact(96, 96, image::imageops::FilterType::Lanczos3).to_rgba8();
                    let size = [img.width() as usize, img.height() as usize];
                    ui.ctx().load_texture("permissions-logo", egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), egui::TextureOptions::LINEAR)
                });
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new((logo.id(), vec2(LOGO, LOGO))));
                    ui.add_space(6.0);
                    ui.label(RichText::new("Welcome to ScreenForge").size(20.0).strong().color(pal.text));
                });
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new("Allow access so recordings capture your screen, cursor and voice.").size(13.0).color(pal.text_dim)).wrap());
                ui.add_space(14.0);
                let required = [
                    (Permission::ScreenRecording, widgets::Icon::Display, "Screen Recording", "Capture the display or a window.", self.status[0]),
                    (Permission::InputMonitoring, widgets::Icon::Cursor, "Input Monitoring", input_monitoring_desc(self.status[1]), self.status[1]),
                ];
                let optional = [
                    (Permission::Microphone, widgets::Icon::Mic, "Microphone", "Record your voice with the screen.", self.status[2]),
                    (Permission::Camera, widgets::Icon::Camera, "Camera", "Record yourself next to the screen.", self.status[3]),
                    (Permission::Accessibility, widgets::Icon::Keyboard, "Accessibility", "Automatic zooms follow the text you type.", self.status[4]),
                ];
                self.group(ui, "Required", &required);
                ui.add_space(12.0);
                self.group(ui, "Optional", &optional);
                ui.add_space(16.0);
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
                    ui.add_space(8.0);
                    ui.add(egui::Label::new(RichText::new("Already allowed in System Settings? macOS applies Screen Recording and Input Monitoring only after the app reopens.").size(11.5).color(pal.text_faint)).wrap());
                }
                content_bottom = ui.min_rect().bottom();
            });
            // fit the window to its content (the hint line comes and goes)
            let want = (content_bottom + MARGIN as f32).ceil();
            if (want - self.height).abs() > 1.0 {
                self.height = want;
                vctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(WIDTH, want)));
            }
            if vctx.input(|i| i.viewport().close_requested()) {
                done = true;
            }
            vctx.request_repaint_after(std::time::Duration::from_millis(500));
        });
        done
    }

    /// A captioned list of permission rows in one card, separated by hairlines.
    fn group(&mut self, ui: &mut egui::Ui, caption: &str, rows: &[(Permission, widgets::Icon, &str, &str, PermissionStatus)]) {
        let pal = theme::p();
        ui.label(RichText::new(caption).size(11.5).strong().color(pal.text_faint));
        ui.add_space(4.0);
        let width = ui.available_width();
        // lay the text out first: it wraps beside the button and the row grows to fit it
        let laid: Vec<_> = rows
            .iter()
            .map(|&(_, _, title, desc, status)| {
                let side = match status {
                    PermissionStatus::Denied => 108.0,
                    PermissionStatus::Granted | PermissionStatus::Unsupported => 84.0,
                    _ => 72.0,
                };
                let wrap = (width - TEXT_X - side - 24.0).max(80.0);
                let t = ui.fonts_mut(|f| f.layout(title.to_string(), FontId::proportional(13.5), pal.text, wrap));
                let d = ui.fonts_mut(|f| f.layout(desc.to_string(), FontId::proportional(11.5), pal.text_dim, wrap));
                let h = (t.size().y + 3.0 + d.size().y + 18.0).max(ROW_H);
                (t, d, h)
            })
            .collect();
        let total: f32 = laid.iter().map(|l| l.2).sum();
        let (card, _) = ui.allocate_exact_size(vec2(width, total), egui::Sense::hover());
        ui.painter().rect(card, CornerRadius::same(10), pal.card, Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
        let mut top = card.top();
        for (i, (&(perm, icon, _, _, status), (t, d, h))) in rows.iter().zip(laid).enumerate() {
            let rect = egui::Rect::from_min_size(egui::pos2(card.left(), top), vec2(card.width(), h));
            top += h;
            if i > 0 {
                ui.painter().hline(egui::Rangef::new(rect.left() + TEXT_X, rect.right()), rect.top(), Stroke::new(1.0, pal.border));
            }
            widgets::paint_icon(ui.painter(), egui::Rect::from_center_size(egui::pos2(rect.left() + 24.0, rect.center().y), vec2(18.0, 18.0)), icon, pal.text);
            let text_top = rect.center().y - (t.size().y + 3.0 + d.size().y) / 2.0;
            let d_top = text_top + t.size().y + 3.0;
            ui.painter().galley(egui::pos2(rect.left() + TEXT_X, text_top), t, pal.text);
            ui.painter().galley(egui::pos2(rect.left() + TEXT_X, d_top), d, pal.text_dim);
            let right = rect.right() - 12.0;
            match status {
                PermissionStatus::Granted => {
                    let g = ui.painter().text(egui::pos2(right, rect.center().y), Align2::RIGHT_CENTER, "Granted", FontId::proportional(12.5), pal.text_dim);
                    ui.painter().circle_filled(egui::pos2(g.left() - 9.0, rect.center().y), 4.0, pal.meter);
                }
                PermissionStatus::Unsupported => {
                    ui.painter().text(egui::pos2(right, rect.center().y), Align2::RIGHT_CENTER, "Unavailable", FontId::proportional(12.5), pal.text_faint);
                }
                _ => {
                    let label = if status == PermissionStatus::Denied { "Open Settings" } else { "Allow" };
                    let w = if status == PermissionStatus::Denied { 108.0 } else { 72.0 };
                    let btn_rect = egui::Rect::from_min_max(egui::pos2(right - w, rect.center().y - 13.0), egui::pos2(right, rect.center().y + 13.0));
                    let b = ui.put(btn_rect, egui::Button::new(RichText::new(label).size(12.0).color(Color32::WHITE)).fill(pal.accent).corner_radius(CornerRadius::same(widgets::BUTTON_RADIUS)).wrap_mode(egui::TextWrapMode::Extend));
                    if b.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        let after = match (perm, status) {
                            (Permission::ScreenRecording, PermissionStatus::NotDetermined) => p::request_screen_recording(),
                            (Permission::InputMonitoring, PermissionStatus::NotDetermined) => p::request_input_monitoring(),
                            (Permission::Microphone, PermissionStatus::NotDetermined) => p::request_microphone(),
                            (Permission::Camera, PermissionStatus::NotDetermined) => p::request_camera(),
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
        }
        // `ui.put` leaves the cursor at the last button; continue below the card
        ui.advance_cursor_after_rect(card);
    }
}

/// Row text for Input Monitoring. Once macOS has answered, the app has to be added by hand:
/// macOS 26 refuses to prompt for this permission and does not list the app on request.
fn input_monitoring_desc(status: PermissionStatus) -> &'static str {
    if status == PermissionStatus::Denied {
        "Click + in System Settings, add ScreenForge."
    } else {
        "Smooth cursor and automatic zooms from clicks and keys."
    }
}

/// The running `.app` bundle, if launched from one.
fn app_bundle() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(|p| p.to_path_buf())
}

/// Quit and start the app again (needed for macOS to apply newly granted permissions).
fn relaunch() {
    match app_bundle() {
        Some(app) => {
            let _ = std::process::Command::new("open").arg("-n").arg(&app).spawn();
        }
        None => {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(&exe).spawn();
            }
        }
    }
    std::process::exit(0);
}
