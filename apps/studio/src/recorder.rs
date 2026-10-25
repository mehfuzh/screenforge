//! Recording flow driven by the picker: countdown → `sf_recording::Recorder` → project package.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Stroke, Ui};
use sf_recording::{CaptureTarget, Recorder, RecorderState, RecordingConfig};

use crate::picker::{InputDevice, Inputs, Mode, RecordRequest};
use crate::theme;

/// Displays only: cheap, and their names come from `NSScreen`, which needs the main thread.
pub fn display_inputs() -> Inputs {
    Inputs {
        displays: sf_recording::discover::displays().iter().map(|m| crate::picker::DisplayEntry { id: m.id, name: m.name.clone(), bounds: m.bounds_points, is_main: m.is_main }).collect(),
        // system audio comes from ScreenCaptureKit (macOS) or WASAPI loopback (Windows)
        system_audio_supported: cfg!(any(target_os = "macos", target_os = "windows")),
        ..Inputs::default()
    }
}

/// Cameras, microphones and windows. Slower (listing cameras runs ffmpeg), so the picker
/// loads them on a background thread; nothing here needs the main thread.
pub fn device_inputs() -> Inputs {
    Inputs {
        cameras: sf_recording::discover::cameras().iter().map(|c| InputDevice { id: c.id.clone(), name: c.name.clone() }).collect(),
        phones: sf_recording::device::devices().into_iter().map(|d| InputDevice { id: d.id, name: d.name }).collect(),
        microphones: sf_recording::discover::microphones().iter().map(|m| InputDevice { id: m.id.clone(), name: m.name.clone() }).collect(),
        windows: discover_windows(),
        ..Inputs::default()
    }
}

/// Recordable windows of other apps (on and off screen).
pub fn discover_windows() -> Vec<crate::picker::WindowEntry> {
    sf_recording::windows_with(false)
        .iter()
        .filter(|w| w.bounds.width >= 64.0 && w.bounds.height >= 64.0 && !w.app_name.is_empty() && !is_helper_app(&w.app_name, &w.bundle_id))
        .map(|w| crate::picker::WindowEntry { id: w.id, pid: w.pid, app: w.app_name.clone(), title: w.title.clone(), bounds: w.bounds, on_screen: w.is_on_screen })
        .collect()
}

/// Make sure recording can start. Screen Recording is required: if it is not granted we ask
/// macOS once (the system prompt) and otherwise return a message; the user then grants it in
/// System Settings (the error dialog offers to open it). Input Monitoring is optional: without
/// it the recording simply has no cursor/keystroke data, so we never block on it.
pub fn check_permissions(req: &mut RecordRequest) -> Option<String> {
    use sf_recording::permissions as p;
    #[cfg(target_os = "linux")]
    if let Some(why) = sf_recording::display::unsupported_reason() {
        return Some(why);
    }
    // an iPhone or iPad's screen comes through the camera APIs: camera access, no screen
    // recording, and no Mac pointer or keyboard to log
    if req.mode == Mode::Device {
        req.capture_input = false;
        req.system_audio = false;
        if matches!(p::request_camera(), sf_recording::PermissionStatus::Denied) {
            return Some("permission:device".into());
        }
    } else if !p::screen_recording().is_granted() && !p::request_screen_recording().is_granted() {
        return Some("permission:screen".into());
    }
    if req.mode == Mode::Device {
        if req.microphone.is_some() && matches!(p::microphone(), sf_recording::PermissionStatus::Denied) {
            req.microphone = None;
        }
        return None;
    }
    if !p::input_monitoring().is_granted() {
        let _ = p::request_input_monitoring();
        req.capture_input = p::input_monitoring().is_granted();
    }
    if req.microphone.is_some() && matches!(p::microphone(), sf_recording::PermissionStatus::Denied) {
        req.microphone = None;
    }
    if req.camera.is_some() && matches!(p::request_camera(), sf_recording::PermissionStatus::Denied) {
        return Some("permission:camera".into());
    }
    None
}

/// User-facing text for a `permission:*` message and the settings pane it refers to.
pub fn permission_help(msg: &str) -> Option<(&'static str, sf_recording::Permission)> {
    match msg {
        "permission:screen" => Some(("ScreenForge needs Screen Recording permission to capture the screen.\n\nEnable ScreenForge in System Settings → Privacy & Security → Screen Recording, then start the recording again. If you launched it from a terminal with cargo run, the permission applies to the terminal app instead.", sf_recording::Permission::ScreenRecording)),
        "permission:device" => Some(("ScreenForge needs Camera permission to record an iPhone or iPad: macOS treats a connected device's screen like a camera.\n\nEnable ScreenForge in System Settings → Privacy & Security → Camera, then try again.", sf_recording::Permission::Camera)),
        "permission:camera" => Some(("ScreenForge needs Camera permission to record your camera.\n\nEnable ScreenForge in System Settings → Privacy & Security → Camera, or choose \"Don't record camera\" in the toolbar.", sf_recording::Permission::Camera)),
        _ => None,
    }
}

/// System helpers and our own windows never make sense to record.
fn is_helper_app(name: &str, bundle_id: &str) -> bool {
    const NAMES: &[&str] = &["ScreenForge", "screenforge", "Window Server", "Dock", "Spotlight", "Control Center", "Control Centre", "Notification Center", "Notification Centre", "Screenshot", "loginwindow", "Wallpaper", "TextInputMenuAgent", "Open and Save Panel Service"];
    // Windows shell pieces that show up as top-level windows
    const WIN_NAMES: &[&str] = &["TextInputHost", "ShellExperienceHost", "StartMenuExperienceHost", "SearchHost", "SearchApp", "LockApp", "ScreenClippingHost", "PhoneExperienceHost", "SystemSettingsBroker"];
    NAMES.iter().any(|n| name == *n) || WIN_NAMES.iter().any(|n| name.eq_ignore_ascii_case(n)) || name.contains("Panel Service") || name.ends_with("Agent") || bundle_id.starts_with("com.apple.controlcenter") || bundle_id.starts_with("com.apple.notificationcenterui") || bundle_id == "com.apple.Spotlight"
}

/// Bring the app with `pid` to the front.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn activate_app(pid: i32) {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
        if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            #[allow(deprecated)]
            app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(w) = sf_recording::windows_with(false).into_iter().find(|w| w.pid == pid) {
            sf_recording::win::activate(w.id);
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = pid;
}

pub fn activate_and_locate(pid: i32, window_id: u32) -> Option<sf_project::schema::common::Rect> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        #[cfg(target_os = "macos")]
        activate_app(pid);
        #[cfg(target_os = "windows")]
        {
            let _ = pid;
            sf_recording::win::activate(window_id);
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
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
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

/// `HWND`s of our own windows; the recorder hides them from the capture.
#[cfg(target_os = "windows")]
pub fn own_window_ids() -> Vec<u32> {
    sf_recording::win::own_windows()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
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
    /// Live camera preview (when a camera is selected); its camera is the one recorded.
    pub camera: Option<crate::camera_bubble::CameraBubble>,
    /// Recent microphone levels for the HUD equalizer, and when the last one was taken.
    levels: std::collections::VecDeque<f32>,
    level_sampled_at: f64,
    /// Where the HUD pill was drawn last frame (window points).
    pub hud_rect: egui::Rect,
}

pub enum RecordingOutcome {
    /// Keep going.
    Continue,
    /// Recording finished and a project was created.
    Finished(PathBuf),
    /// Cancelled or failed; back to the picker.
    Cancelled(Option<String>),
}

/// On Wayland the window to record is chosen in the system's screen-sharing dialog instead
/// of ScreenForge's own window overlay.
pub fn picks_window_in_portal() -> bool {
    #[cfg(target_os = "linux")]
    return sf_recording::portal::is_wayland();
    #[cfg(not(target_os = "linux"))]
    false
}

impl RecordingScreen {
    pub fn new(req: RecordRequest) -> Self {
        // Windows file names cannot contain ':'
        let stamp = chrono::Local::now().format(if cfg!(windows) { "%Y-%m-%d %H.%M.%S" } else { "%Y-%m-%d %H:%M:%S" }).to_string();
        let name = format!("{} {stamp}", target_name(&req));
        let dir = recordings_folder().join(name);
        let total_ms = req.countdown_ms;
        let camera = req.camera.as_deref().map(crate::camera_bubble::CameraBubble::open);
        RecordingScreen { req, phase: Phase::Countdown { until: Instant::now() + std::time::Duration::from_millis(total_ms), total_ms }, output_dir: dir, error: None, camera, levels: Default::default(), level_sampled_at: 0.0, hud_rect: egui::Rect::NOTHING }
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
            (Mode::Window, None) if picks_window_in_portal() => CaptureTarget::PortalWindow,
            (Mode::Device, _) => CaptureTarget::Device { id: self.req.device.clone().context("no iPhone or iPad chosen")? },
            _ => CaptureTarget::Display { display_id: display.id },
        };
        let mut cfg = RecordingConfig::new(&self.output_dir, target);
        cfg.microphone = self.req.microphone.clone();
        cfg.capture_system_audio = self.req.system_audio;
        cfg.camera = self.req.camera.clone();
        cfg.exclude_window_ids = own_window_ids();
        // a picked window is recorded as the area around it, which moves with the window
        if self.req.mode == Mode::Window {
            cfg.follow_window = self.req.window_id.as_deref().and_then(|id| id.parse().ok());
        }
        cfg.capture_input = self.req.capture_input;
        cfg.capture_keystrokes = self.req.capture_input;
        let rec = match self.camera.as_ref().and_then(|b| b.capture.clone()) {
            Some(cam) => Recorder::start_with_camera(cfg, cam),
            None => Recorder::start(cfg),
        }
        .context("starting the recorder")?;
        self.phase = Phase::Recording(rec);
        Ok(())
    }

    /// Draw the countdown / HUD into the floating window. Returns what to do next.
    ///
    /// Styled like a tape recorder (bordered keys, a display window holding the REC light, the
    /// timer and, with a microphone, a level equalizer), drawn flat on the same frosted glass
    /// as the main toolbar so it follows the theme.
    pub fn ui(&mut self, ui: &mut Ui, ctx: &egui::Context) -> RecordingOutcome {
        let full = ui.max_rect();
        if let Some(b) = self.camera.as_mut() {
            b.show(ctx);
            if let Some(e) = b.error.take() {
                return RecordingOutcome::Cancelled(Some(format!("Camera: {e}")));
            }
        }
        let camera_opening = self.camera.as_ref().is_some_and(|b| b.is_opening());
        let pal = theme::p();
        let t = ui.input(|i| i.time);

        // microphone level history for the equalizer (newest last)
        let level = match &self.phase {
            Phase::Recording(rec) => rec.microphone_level(),
            _ => None,
        };
        if let Some(l) = level {
            if t - self.level_sampled_at >= 0.06 {
                self.level_sampled_at = t;
                self.levels.push_back(l);
                while self.levels.len() > EQ_BARS {
                    self.levels.pop_front();
                }
            }
        }
        let has_eq = self.req.microphone.is_some();
        ctx.request_repaint_after(std::time::Duration::from_millis(if has_eq { 50 } else { 100 }));

        // pill: [stop] [lcd] [keys…]
        let key = vec2(36.0, 40.0);
        let lcd_w = match self.phase {
            Phase::Recording(_) => if has_eq { 196.0 } else { 150.0 },
            _ => 230.0,
        };
        let keys_w = match self.phase {
            Phase::Recording(_) => 4.0 * key.x + 3.0 * 4.0,
            _ => 84.0,
        };
        let pill_w = 12.0 + 40.0 + 8.0 + lcd_w + 8.0 + keys_w + 12.0;
        let pill = egui::Rect::from_center_size(pos2(full.center().x, full.bottom() - 10.0 - 29.0), vec2(pill_w, 58.0));
        let painter = ui.painter().clone();
        self.hud_rect = pill;
        crate::vibrancy::set(Some([pill.left(), pill.top(), pill.width(), pill.height()]), full.height(), crate::picker::PILL_RADIUS);
        let tint = if crate::vibrancy::is_active() { pal.glass } else { pal.glass_solid };
        painter.rect(pill, CornerRadius::same(crate::picker::PILL_RADIUS as u8), tint, Stroke::new(1.0, pal.glass_border), egui::StrokeKind::Inside);
        let cy = pill.center().y;
        let stop_rect = egui::Rect::from_min_size(pos2(pill.left() + 12.0, cy - 20.0), vec2(40.0, 40.0));
        let lcd = egui::Rect::from_min_size(pos2(stop_rect.right() + 8.0, cy - 20.0), vec2(lcd_w, 40.0));
        let keys_left = lcd.right() + 8.0;
        lcd_window(&painter, lcd);

        match &mut self.phase {
            Phase::Countdown { until, total_ms } => {
                let left = until.saturating_duration_since(Instant::now());
                let secs = (left.as_secs_f64()).ceil() as u64;
                // the countdown can end before the camera is awake: hold at 0 until it is
                if (left.is_zero() || *total_ms == 0) && !camera_opening {
                    if let Err(e) = self.start_recorder() {
                        return RecordingOutcome::Cancelled(Some(format!("{e:#}")));
                    }
                    return RecordingOutcome::Continue;
                }
                // stop key stands idle; the LCD counts down
                tape_key(ui, stop_rect, "stop-idle", None, |p, r| paint_stop_glyph(p, r, pal.text_faint), "");
                rec_light(&painter, pos2(lcd.left() + 14.0, cy), (t * 2.0).sin() > 0.0);
                painter.text(pos2(lcd.left() + 46.0, cy), Align2::CENTER_CENTER, secs.to_string(), FontId::monospace(24.0), pal.accent_hover);
                let title = if left.is_zero() && camera_opening { "Starting camera…" } else { "Recording starts in" };
                painter.text(pos2(lcd.left() + 70.0, cy - 8.0), Align2::LEFT_CENTER, title, FontId::proportional(12.5), pal.on_glass);
                painter.text(pos2(lcd.left() + 70.0, cy + 9.0), Align2::LEFT_CENTER, match self.req.mode { Mode::Window => "Selected window", Mode::Area => "Selected area", _ => "Entire display" }, FontId::proportional(11.0), pal.text_dim);
                let cancel = egui::Rect::from_min_size(pos2(keys_left, cy - 20.0), vec2(keys_w, 40.0));
                let label = |p: &egui::Painter, r: egui::Rect| {
                    p.text(r.center(), Align2::CENTER_CENTER, "Cancel", FontId::proportional(13.0), pal.on_glass);
                };
                if tape_key(ui, cancel, "cancel-countdown", None, label, "Cancel (Esc)").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
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

                // stop: a red-faced key
                let mut finish = tape_key(ui, stop_rect, "stop", Some(pal.danger), |p, r| paint_stop_glyph(p, r, Color32::WHITE), "Finish recording").clicked();
                let mut restart = false;

                // LCD: REC light · timer · equalizer
                rec_light(&painter, pos2(lcd.left() + 14.0, cy - 7.0), !paused && (t * 2.0).sin() > -0.3);
                painter.text(pos2(lcd.left() + 14.0, cy + 9.0), Align2::CENTER_CENTER, if paused { "PAUSE" } else { "REC" }, FontId::monospace(8.5), if paused { pal.text_dim } else { pal.danger });
                painter.text(pos2(lcd.left() + 34.0, cy), Align2::LEFT_CENTER, crate::editor::fmt_clock_short(elapsed), FontId::monospace(20.0), if paused { pal.text_dim } else { pal.accent_hover });
                if has_eq {
                    let eq = egui::Rect::from_min_max(pos2(lcd.right() - 10.0 - EQ_BARS as f32 * 6.0, lcd.top() + 9.0), pos2(lcd.right() - 10.0, lcd.bottom() - 9.0));
                    paint_equalizer(&painter, eq, &self.levels, if paused { pal.text_faint } else { pal.meter });
                }

                // keys: pause · marker · restart · cancel
                let at = |i: usize| egui::Rect::from_min_size(pos2(keys_left + i as f32 * (key.x + 4.0), cy - 20.0), key);
                let (icon, tip) = if paused { (crate::widgets::Icon::Play, "Resume") } else { (crate::widgets::Icon::Pause, "Pause") };
                if tape_key(ui, at(0), "pause", None, |p, r| icon_glyph(p, r, icon, 14.0), tip).clicked() {
                    let r = if paused { rec.resume() } else { rec.pause() };
                    if let Err(e) = r {
                        self.error = Some(e.to_string());
                    }
                }
                if tape_key(ui, at(1), "marker", None, |p, r| icon_glyph(p, r, crate::widgets::Icon::Flag, 14.0), "Add marker (split point)").clicked() {
                    rec.add_marker();
                }
                if tape_key(ui, at(2), "restart", None, |p, r| icon_glyph(p, r, crate::widgets::Icon::Restart, 15.0), "Restart recording").clicked() {
                    restart = true;
                }
                if tape_key(ui, at(3), "cancel", None, |p, r| icon_glyph(p, r, crate::widgets::Icon::Close, 13.0), "Cancel recording").clicked() {
                    if let Phase::Recording(rec) = std::mem::replace(&mut self.phase, Phase::Finishing) {
                        rec.cancel();
                    }
                    return RecordingOutcome::Cancelled(None);
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    finish = true;
                }
                if restart {
                    if let Phase::Recording(rec) = std::mem::replace(&mut self.phase, Phase::Finishing) {
                        rec.cancel();
                    }
                    let _ = std::fs::remove_dir_all(&self.output_dir);
                    self.levels.clear();
                    self.phase = Phase::Countdown { until: Instant::now() + std::time::Duration::from_secs(3), total_ms: 3000 };
                    return RecordingOutcome::Continue;
                }
                if finish {
                    if let Phase::Recording(rec) = std::mem::replace(&mut self.phase, Phase::Finishing) {
                        let markers = rec.markers();
                        return match rec.finish() {
                            Ok(_meta) => match sf_session::create::create_project_from_recording(
                                &self.output_dir,
                                sf_session::create::CreateOptions { name: self.output_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Recording".into()), projects_folder: sf_session::create::default_projects_folder(), default_config: None, create_automatic_zooms: true, markers_ms: markers, device_mockup: self.req.mode == Mode::Device },
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
                    painter.text(pos2(pill.center().x, pill.top() - 16.0), Align2::CENTER_CENTER, e, FontId::proportional(12.0), pal.danger);
                }
                RecordingOutcome::Continue
            }
            Phase::Finishing => {
                painter.text(lcd.center(), Align2::CENTER_CENTER, "Finishing…", FontId::proportional(15.0), pal.on_glass);
                RecordingOutcome::Continue
            }
        }
    }
}

/// Bars in the HUD's microphone equalizer.
const EQ_BARS: usize = 7;

/// The display window: a faint glass tint inside a border.
fn lcd_window(p: &egui::Painter, r: egui::Rect) {
    p.rect(r, CornerRadius::same(2), theme::glass_alpha(14), Stroke::new(1.0, theme::p().glass_border), egui::StrokeKind::Inside);
}

/// The record indicator lamp.
fn rec_light(p: &egui::Painter, c: egui::Pos2, lit: bool) {
    let pal = theme::p();
    if lit {
        p.circle_filled(c, 7.0, pal.danger.gamma_multiply(0.25));
        p.circle_filled(c, 4.5, pal.danger);
    } else {
        p.circle_filled(c, 4.5, pal.danger.gamma_multiply(0.3));
    }
}

/// Level history as bars rising from the bottom of `r` (newest on the right).
fn paint_equalizer(p: &egui::Painter, r: egui::Rect, levels: &std::collections::VecDeque<f32>, color: Color32) {
    let bar_w = r.width() / EQ_BARS as f32;
    for i in 0..EQ_BARS {
        let l = levels.len().checked_sub(EQ_BARS - i).and_then(|k| levels.get(k)).copied().unwrap_or(0.0);
        let h = (r.height() * l.clamp(0.0, 1.0)).max(2.0);
        let x = r.left() + i as f32 * bar_w;
        p.rect_filled(egui::Rect::from_min_max(pos2(x + 1.0, r.bottom() - h), pos2(x + bar_w - 1.0, r.bottom())), CornerRadius::same(1), color);
    }
}

fn paint_stop_glyph(p: &egui::Painter, r: egui::Rect, color: Color32) {
    p.rect_filled(egui::Rect::from_center_size(r.center(), vec2(12.0, 12.0)), CornerRadius::same(2), color);
}

fn icon_glyph(p: &egui::Painter, r: egui::Rect, icon: crate::widgets::Icon, size: f32) {
    crate::widgets::paint_icon(p, egui::Rect::from_center_size(r.center(), vec2(size, size)), icon, theme::p().on_glass);
}

/// A flat, bordered tape-recorder key on the glass. `face` fills the key (e.g. red for
/// stop); `glyph` paints its label. Hover and press deepen the glass tint like the toolbar.
fn tape_key(ui: &mut Ui, rect: egui::Rect, id: &str, face: Option<Color32>, glyph: impl FnOnce(&egui::Painter, egui::Rect), tooltip: &str) -> egui::Response {
    let pal = theme::p();
    let resp = ui.interact(rect, ui.id().with(("tape-key", id)), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    let fill = match face {
        Some(c) if resp.is_pointer_button_down_on() => c.gamma_multiply(0.8),
        Some(c) => c,
        None if resp.is_pointer_button_down_on() => theme::glass_alpha(44),
        None if resp.hovered() => theme::glass_alpha(28),
        None => Color32::TRANSPARENT,
    };
    p.rect(rect, CornerRadius::same(2), fill, Stroke::new(1.0, pal.glass_border), egui::StrokeKind::Inside);
    glyph(p, rect);
    if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) }
}

fn target_name(req: &RecordRequest) -> &'static str {
    match req.mode {
        Mode::Display => "Display",
        Mode::Window => "Window",
        Mode::Area => "Area",
        Mode::Device => "iPhone",
    }
}


/// `~/Library/Application Support/ScreenForge/Recordings`
pub fn recordings_folder() -> PathBuf {
    directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.data_dir().join("Recordings")).unwrap_or_else(|| std::env::temp_dir().join("screenforge-recordings"))
}
