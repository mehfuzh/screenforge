//! Recording flow driven by the picker: countdown → `sf_recording::Recorder` → project package.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, RichText, Stroke, Ui};
use sf_recording::{CaptureTarget, Recorder, RecorderState, RecordingConfig};

use crate::picker::{InputDevice, Inputs, Mode, RecordRequest};
use crate::theme;

pub fn discover_inputs() -> Inputs {
    let d = sf_recording::discover();
    Inputs {
        cameras: d.cameras.iter().map(|c| InputDevice { id: c.id.clone(), name: c.name.clone() }).collect(),
        microphones: d.microphones.iter().map(|m| InputDevice { id: m.id.clone(), name: m.name.clone() }).collect(),
        displays: d.displays.iter().map(|m| crate::picker::DisplayEntry { id: m.id, name: m.name.clone(), bounds: m.bounds_points, is_main: m.is_main }).collect(),
        windows: sf_recording::windows_with(false)
            .iter()
            .filter(|w| w.bounds.width >= 64.0 && w.bounds.height >= 64.0 && !w.app_name.is_empty() && !is_helper_app(&w.app_name, &w.bundle_id))
            .map(|w| crate::picker::WindowEntry { id: w.id, pid: w.pid, app: w.app_name.clone(), title: w.title.clone(), bounds: w.bounds, on_screen: w.is_on_screen })
            .collect(),
        system_audio_supported: true,
    }
}

/// Make sure recording can start. Screen Recording is required: if it is not granted we ask
/// macOS once (the system prompt) and otherwise return a message; the user then grants it in
/// System Settings (the error dialog offers to open it). Input Monitoring is optional: without
/// it the recording simply has no cursor/keystroke data, so we never block on it.
pub fn check_permissions(req: &mut RecordRequest) -> Option<String> {
    use sf_recording::permissions as p;
    if !p::screen_recording().is_granted() && !p::request_screen_recording().is_granted() {
        return Some("permission:screen".into());
    }
    if !p::input_monitoring().is_granted() {
        let _ = p::request_input_monitoring();
        req.capture_input = p::input_monitoring().is_granted();
    }
    if req.microphone.is_some() && matches!(p::microphone(), sf_recording::PermissionStatus::Denied) {
        req.microphone = None;
    }
    None
}

/// User-facing text for a `permission:*` message and the settings pane it refers to.
pub fn permission_help(msg: &str) -> Option<(&'static str, sf_recording::Permission)> {
    match msg {
        "permission:screen" => Some(("ScreenForge needs Screen Recording permission to capture the screen.\n\nEnable ScreenForge in System Settings → Privacy & Security → Screen Recording, then start the recording again. If you launched it from a terminal with cargo run, the permission applies to the terminal app instead.", sf_recording::Permission::ScreenRecording)),
        _ => None,
    }
}

/// System helpers and our own windows never make sense to record.
fn is_helper_app(name: &str, bundle_id: &str) -> bool {
    const NAMES: &[&str] = &["ScreenForge", "screenforge", "Window Server", "Dock", "Spotlight", "Control Center", "Control Centre", "Notification Center", "Notification Centre", "Screenshot", "loginwindow", "Wallpaper", "TextInputMenuAgent", "Open and Save Panel Service"];
    NAMES.iter().any(|n| name == *n) || name.contains("Panel Service") || name.ends_with("Agent") || bundle_id.starts_with("com.apple.controlcenter") || bundle_id.starts_with("com.apple.notificationcenterui") || bundle_id == "com.apple.Spotlight"
}

/// Bring an application to the front (window picking), then re-read the window's bounds.
pub fn activate_and_locate(pid: i32, window_id: u32) -> Option<sf_project::schema::common::Rect> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
        if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            #[allow(deprecated)]
            app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
        }
        // give the Space switch / un-minimise a moment, then read the fresh bounds
        for _ in 0..8 {
            std::thread::sleep(std::time::Duration::from_millis(120));
            if let Some(w) = sf_recording::windows_with(true).into_iter().find(|w| w.id == window_id) {
                return Some(w.bounds);
            }
        }
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, window_id);
        None
    }
}

/// Window numbers of our own windows, so they are excluded from the capture.
#[cfg(target_os = "macos")]
pub fn own_window_ids() -> Vec<u32> {
    use objc2_app_kit::NSApplication;
    let Some(mtm) = objc2::MainThreadMarker::new() else { return vec![] };
    let app = NSApplication::sharedApplication(mtm);
    app.windows().iter().map(|w| w.windowNumber() as u32).collect()
}

#[cfg(not(target_os = "macos"))]
pub fn own_window_ids() -> Vec<u32> {
    vec![]
}

pub enum Phase {
    Countdown { until: Instant, total_ms: u64 },
    Recording(Recorder),
    Finishing,
}

/// State shown in the floating window while recording.
pub struct RecordingScreen {
    pub req: RecordRequest,
    pub phase: Phase,
    pub output_dir: PathBuf,
    pub error: Option<String>,
    /// The countdown is shown over the middle of the selected window/area (moved to the
    /// bottom of the screen once recording starts).
    pub hud_centred: bool,
}

pub enum RecordingOutcome {
    /// Keep going.
    Continue,
    /// Recording finished and a project was created.
    Finished(PathBuf),
    /// Cancelled or failed; back to the picker.
    Cancelled(Option<String>),
}

impl RecordingScreen {
    pub fn new(req: RecordRequest) -> Self {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let name = format!("{} {stamp}", target_name(&req));
        let dir = recordings_folder().join(name);
        let total_ms = req.countdown_ms;
        RecordingScreen { req, phase: Phase::Countdown { until: Instant::now() + std::time::Duration::from_millis(total_ms), total_ms }, output_dir: dir, error: None, hud_centred: false }
    }

    fn start_recorder(&mut self) -> Result<()> {
        let d = sf_recording::discover();
        let display = self.req.display_id.as_ref().and_then(|id| d.displays.iter().find(|x| x.id.to_string() == *id)).or_else(|| d.main_display()).context("no display found")?;
        let target = match (self.req.mode, self.req.area) {
            (Mode::Area | Mode::Window, Some(rect)) => {
                // clamp to the display and keep even dimensions
                let b = display.bounds_points;
                let x0 = rect.x.max(b.x);
                let y0 = rect.y.max(b.y);
                let x1 = (rect.x + rect.width).min(b.x + b.width);
                let y1 = (rect.y + rect.height).min(b.y + b.height);
                let w = ((x1 - x0).max(32.0) / 2.0).floor() * 2.0;
                let h = ((y1 - y0).max(32.0) / 2.0).floor() * 2.0;
                CaptureTarget::Area { display_id: display.id, rect_points: sf_project::schema::common::Rect { x: x0.round(), y: y0.round(), width: w, height: h } }
            }
            _ => CaptureTarget::Display { display_id: display.id },
        };
        let mut cfg = RecordingConfig::new(&self.output_dir, target);
        cfg.microphone = self.req.microphone.clone();
        cfg.capture_system_audio = self.req.system_audio;
        cfg.camera = self.req.camera.clone();
        cfg.exclude_window_ids = own_window_ids();
        cfg.capture_input = self.req.capture_input;
        cfg.capture_keystrokes = self.req.capture_input;
        let rec = Recorder::start(cfg).context("starting the recorder")?;
        self.phase = Phase::Recording(rec);
        Ok(())
    }

    /// Film-leader countdown: a dial with a sweeping hand and a big number, drawn over the
    /// middle of what is about to be recorded.
    fn leader_ui(&mut self, ui: &mut Ui, ctx: &egui::Context, until: Instant, total_ms: u64) -> RecordingOutcome {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() || total_ms == 0 {
            if let Err(e) = self.start_recorder() {
                return RecordingOutcome::Cancelled(Some(format!("{e:#}")));
            }
            return RecordingOutcome::Continue;
        }
        ctx.request_repaint();
        let pal = theme::p();
        let full = ui.max_rect();
        let p = ui.painter().clone();
        let c = pos2(full.center().x, full.top() + 130.0);
        let r = 112.0;
        let secs = left.as_secs_f64().ceil() as u64;
        // fraction of the current second already gone: the hand sweeps once per number
        let frac = 1.0 - (left.as_secs_f64() - (secs as f64 - 1.0)) as f32;
        let ink = pal.on_glass;

        p.circle_filled(c, r, pal.glass_solid);
        // swept sector, clockwise from 12 o'clock
        let tau = std::f32::consts::TAU;
        let at = |a: f32, rad: f32| pos2(c.x + rad * (a - tau / 4.0).cos(), c.y + rad * (a - tau / 4.0).sin());
        let steps = 64;
        let end = frac.clamp(0.0, 1.0) * tau;
        for i in 0..steps {
            let a0 = end * i as f32 / steps as f32;
            let a1 = end * (i + 1) as f32 / steps as f32;
            p.add(egui::Shape::convex_polygon(vec![c, at(a0, r - 4.0), at(a1, r - 4.0)], ink.gamma_multiply(0.14), Stroke::NONE));
        }
        // crosshair and rings, like a film leader
        let faint = Stroke::new(1.0, ink.gamma_multiply(0.35));
        p.line_segment([pos2(c.x - r, c.y), pos2(c.x + r, c.y)], faint);
        p.line_segment([pos2(c.x, c.y - r), pos2(c.x, c.y + r)], faint);
        p.circle_stroke(c, r * 0.72, faint);
        p.circle_stroke(c, r, Stroke::new(3.0, ink.gamma_multiply(0.9)));
        p.line_segment([c, at(end, r - 4.0)], Stroke::new(2.5, pal.danger));
        p.text(c, Align2::CENTER_CENTER, secs.to_string(), FontId::proportional(104.0), ink);

        let what = match self.req.mode { Mode::Window => "Recording window", Mode::Area => "Recording area", _ => "Recording screen" };
        let caption = egui::Rect::from_center_size(pos2(c.x, c.y + r + 26.0), vec2(220.0, 26.0));
        p.rect_filled(caption, CornerRadius::same(10), pal.glass_solid);
        p.text(caption.center(), Align2::CENTER_CENTER, format!("{what} · Esc to cancel"), FontId::proportional(12.5), ink);
        let cancel = egui::Rect::from_center_size(pos2(c.x, c.y + r + 56.0), vec2(84.0, 26.0));
        if ui.put(cancel, egui::Button::new(RichText::new("Cancel").size(13.0)).corner_radius(CornerRadius::same(4)).fill(pal.card_3)).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            return RecordingOutcome::Cancelled(None);
        }
        RecordingOutcome::Continue
    }

    /// Draw the countdown / HUD into the floating window. Returns what to do next.
    pub fn ui(&mut self, ui: &mut Ui, ctx: &egui::Context) -> RecordingOutcome {
        let full = ui.max_rect();
        if self.hud_centred {
            if let Phase::Countdown { until, total_ms } = self.phase {
                return self.leader_ui(ui, ctx, until, total_ms);
            }
        }
        let recording = matches!(self.phase, Phase::Recording(_));
        let pill_w = if recording { 330.0 } else { 420.0 };
        let pill = egui::Rect::from_center_size(pos2(full.center().x, full.bottom() - 10.0 - 29.0), vec2(pill_w, 58.0));
        let painter = ui.painter().clone();
        painter.rect(pill, CornerRadius::same(10), theme::p().glass_solid, Stroke::new(1.0, theme::p().glass_border), egui::StrokeKind::Inside);
        ctx.request_repaint_after(std::time::Duration::from_millis(100));

        match &mut self.phase {
            Phase::Countdown { until, total_ms } => {
                let left = until.saturating_duration_since(Instant::now());
                let secs = (left.as_secs_f64()).ceil() as u64;
                if left.is_zero() || *total_ms == 0 {
                    if let Err(e) = self.start_recorder() {
                        return RecordingOutcome::Cancelled(Some(format!("{e:#}")));
                    }
                    return RecordingOutcome::Continue;
                }
                // record badge · big number · label · cancel
                let badge = pos2(pill.left() + 34.0, pill.center().y);
                painter.circle_stroke(badge, 13.0, Stroke::new(2.0, theme::p().danger));
                painter.circle_filled(badge, 7.0, theme::p().danger);
                painter.text(pos2(pill.left() + 92.0, pill.center().y), Align2::CENTER_CENTER, secs.to_string(), FontId::proportional(30.0), theme::p().on_glass);
                painter.text(pos2(pill.left() + 124.0, pill.center().y - 9.0), Align2::LEFT_CENTER, "Recording starts in", FontId::proportional(13.0), theme::p().text);
                painter.text(pos2(pill.left() + 124.0, pill.center().y + 9.0), Align2::LEFT_CENTER, match self.req.mode { Mode::Window => "Selected window", Mode::Area => "Selected area", _ => "Entire display" }, FontId::proportional(11.5), theme::p().text_dim);
                let cancel = egui::Rect::from_center_size(pos2(pill.right() - 56.0, pill.center().y), vec2(84.0, 32.0));
                if ui.put(cancel, egui::Button::new(RichText::new("Cancel").size(13.5)).corner_radius(CornerRadius::same(9)).fill(theme::p().card_3)).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    return RecordingOutcome::Cancelled(None);
                }
                RecordingOutcome::Continue
            }
            Phase::Recording(rec) => {
                let state = rec.state();
                if let RecorderState::Failed(msg) = &state {
                    let msg = msg.clone();
                    return RecordingOutcome::Cancelled(Some(msg));
                }
                let elapsed = rec.elapsed_ms();
                let paused = matches!(state, RecorderState::Paused);
                let t = ui.input(|i| i.time);
                let cy = pill.center().y;

                // stop button: red disc with a white square
                let stop_c = pos2(pill.left() + 32.0, cy);
                let stop_rect = egui::Rect::from_center_size(stop_c, vec2(40.0, 40.0));
                let stop = ui.interact(stop_rect, ui.id().with("stop"), egui::Sense::click()).on_hover_text("Finish recording");
                let pulse = if paused { 1.0 } else { 0.9 + 0.1 * ((t * 2.0).sin() as f32) };
                painter.circle_filled(stop_c, if stop.hovered() { 19.0 } else { 17.0 }, theme::p().danger.gamma_multiply(pulse));
                painter.rect_filled(egui::Rect::from_center_size(stop_c, vec2(13.0, 13.0)), CornerRadius::same(3), Color32::WHITE);
                let mut finish = stop.clicked();
                let mut restart = false;

                // timer + state
                painter.text(pos2(pill.left() + 64.0, cy - 1.0), Align2::LEFT_CENTER, crate::editor::fmt_clock_short(elapsed), FontId::monospace(20.0), if paused { theme::p().text_dim } else { theme::p().text });
                if paused {
                    painter.text(pos2(pill.left() + 64.0, cy + 15.0), Align2::LEFT_CENTER, "Paused", FontId::proportional(10.5), theme::p().text_dim);
                }

                // controls: pause · marker · restart · cancel
                let mut row = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(pos2(pill.left() + 140.0, pill.top()), pill.right_bottom())).layout(egui::Layout::right_to_left(egui::Align::Center)));
                row.add_space(10.0);
                if crate::widgets::icon_button(&mut row, crate::widgets::Icon::Close, 14.0, "Cancel recording").clicked() {
                    if let Phase::Recording(rec) = std::mem::replace(&mut self.phase, Phase::Finishing) {
                        rec.cancel();
                    }
                    return RecordingOutcome::Cancelled(None);
                }
                row.add_space(2.0);
                if crate::widgets::icon_button(&mut row, crate::widgets::Icon::Restart, 15.0, "Restart recording").clicked() {
                    restart = true;
                }
                row.add_space(2.0);
                if crate::widgets::icon_button(&mut row, crate::widgets::Icon::Flag, 14.0, "Add marker (split point)").clicked() {
                    rec.add_marker();
                }
                row.add_space(2.0);
                let (icon, tip) = if paused { (crate::widgets::Icon::Play, "Resume") } else { (crate::widgets::Icon::Pause, "Pause") };
                if crate::widgets::icon_button(&mut row, icon, 14.0, tip).clicked() {
                    let r = if paused { rec.resume() } else { rec.pause() };
                    if let Err(e) = r {
                        self.error = Some(e.to_string());
                    }
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    finish = true;
                }
                if restart {
                    if let Phase::Recording(rec) = std::mem::replace(&mut self.phase, Phase::Finishing) {
                        rec.cancel();
                    }
                    let _ = std::fs::remove_dir_all(&self.output_dir);
                    self.phase = Phase::Countdown { until: Instant::now() + std::time::Duration::from_secs(3), total_ms: 3000 };
                    return RecordingOutcome::Continue;
                }
                if finish {
                    if let Phase::Recording(rec) = std::mem::replace(&mut self.phase, Phase::Finishing) {
                        let markers = rec.markers();
                        return match rec.finish() {
                            Ok(_meta) => match sf_session::create::create_project_from_recording(
                                &self.output_dir,
                                sf_session::create::CreateOptions { name: self.output_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Recording".into()), projects_folder: sf_session::create::default_projects_folder(), default_config: None, create_automatic_zooms: true, markers_ms: markers },
                            ) {
                                Ok(pkg) => {
                                    sf_session::preview::write_quicklook_preview_async(&pkg.path);
                                    RecordingOutcome::Finished(pkg.path)
                                }
                                Err(e) => RecordingOutcome::Cancelled(Some(format!("Could not create the project: {e:#}"))),
                            },
                            Err(e) => RecordingOutcome::Cancelled(Some(format!("Finishing the recording failed: {e:#}"))),
                        };
                    }
                }
                if let Some(e) = &self.error {
                    painter.text(pos2(pill.center().x, pill.top() - 16.0), Align2::CENTER_CENTER, e, FontId::proportional(12.0), theme::p().danger);
                }
                RecordingOutcome::Continue
            }
            Phase::Finishing => {
                painter.text(pill.center(), Align2::CENTER_CENTER, "Finishing…", FontId::proportional(16.0), theme::p().text);
                RecordingOutcome::Continue
            }
        }
    }
}

fn target_name(req: &RecordRequest) -> &'static str {
    match req.mode {
        Mode::Display => "Display",
        Mode::Window => "Window",
        Mode::Area => "Area",
        Mode::Device => "Device",
    }
}


/// `~/Library/Application Support/ScreenForge/Recordings`
pub fn recordings_folder() -> PathBuf {
    directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.data_dir().join("Recordings")).unwrap_or_else(|| std::env::temp_dir().join("screenforge-recordings"))
}
