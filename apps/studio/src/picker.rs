//! Recording picker: the floating toolbar Screen Studio shows at launch.
//!
//! ```text
//! ( × │ [Display] [Window] [Area] [Device] │ No camera  No microphone  No system audio │ 📁 ⚙▾ )
//! ```
//! The window is frameless and transparent; the pill is painted by us.

use std::path::PathBuf;

use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, RichText, Sense, Stroke, Ui};

use crate::app::Prefs;
use crate::theme;
use crate::widgets;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Display,
    Window,
    Area,
    Device,
}

#[derive(Debug, Clone, Default)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct WindowEntry {
    pub id: u32,
    pub pid: i32,
    pub app: String,
    pub title: String,
    /// Global screen points, top-left origin.
    pub bounds: sf_project::schema::common::Rect,
    pub on_screen: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DisplayEntry {
    pub id: u32,
    pub name: String,
    pub bounds: sf_project::schema::common::Rect,
    pub is_main: bool,
}

/// Capture sources available on this machine (filled by the recorder crate).
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct Inputs {
    pub cameras: Vec<InputDevice>,
    /// iPhones and iPads connected by cable.
    pub phones: Vec<InputDevice>,
    pub microphones: Vec<InputDevice>,
    pub displays: Vec<DisplayEntry>,
    pub windows: Vec<WindowEntry>,
    pub system_audio_supported: bool,
}

impl Inputs {
    /// Take the device lists (cameras, microphones, windows) from a background discovery.
    pub fn merge_devices(&mut self, d: Inputs) {
        self.cameras = d.cameras;
        self.phones = d.phones;
        self.microphones = d.microphones;
        self.windows = d.windows;
    }
}

/// Theme preference stored in prefs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// ScreenForge dark: the website's dark colours (the default).
    #[default]
    #[serde(rename = "screenforge")]
    ScreenForge,
    /// ScreenForge light: the website's light colours.
    #[serde(rename = "screenforge-light")]
    ScreenForgeLight,
    /// ScreenForge light or dark, with the system.
    System,
    /// Solarized Light.
    Light,
    /// Catppuccin Mocha.
    Dark,
    /// Dark, with the Osaka Jade palette.
    #[serde(rename = "osaka-jade")]
    OsakaJade,
}

impl Appearance {
    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Appearance::System => egui::ThemePreference::System,
            Appearance::Light | Appearance::ScreenForgeLight => egui::ThemePreference::Light,
            Appearance::ScreenForge | Appearance::Dark | Appearance::OsakaJade => egui::ThemePreference::Dark,
        }
    }

    /// Make the theme module use this preference's dark palette, then set egui's theme.
    pub fn apply(self, ctx: &egui::Context) {
        use crate::theme::{DARK, FORGE_DARK, FORGE_LIGHT, LIGHT, OSAKA_JADE};
        let (dark, light) = match self {
            Appearance::ScreenForge | Appearance::ScreenForgeLight | Appearance::System => (FORGE_DARK, FORGE_LIGHT),
            Appearance::OsakaJade => (OSAKA_JADE, LIGHT),
            Appearance::Light | Appearance::Dark => (DARK, LIGHT),
        };
        crate::theme::set_palettes(dark, light);
        ctx.set_theme(self.preference());
        crate::theme::sync(ctx);
    }
}

/// What the user asked the picker to do.
#[derive(Debug, Clone)]
pub enum PickerAction {
    OpenProject(PathBuf),
    OpenProjectDialog,
    StartRecording(RecordRequest),
    /// Open the drag-to-select overlay; the app starts the recording with the chosen rect.
    SelectArea(RecordRequest),
    /// Open the click-a-window overlay; the app starts the recording with the chosen window.
    SelectWindow(RecordRequest),
    SetAppearance(Appearance),
    /// Turn the automatic update check on or off.
    SetCheckUpdates(bool),
    /// Check for a newer release now.
    CheckUpdates,
    /// Move a recent project to the Trash and forget it.
    RemoveProject(PathBuf),
    Quit,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RecordRequest {
    pub mode: Mode,
    pub display_id: Option<String>,
    pub window_id: Option<String>,
    /// Region to record in global screen points (Area and Window modes).
    pub area: Option<sf_project::schema::common::Rect>,
    pub camera: Option<String>,
    pub microphone: Option<String>,
    pub system_audio: bool,
    pub countdown_ms: u64,
    /// Capture cursor + keystrokes (needs Input Monitoring; set by the permission check).
    pub capture_input: bool,
    /// Device mode: the iPhone or iPad to record (`sf_recording::device::DeviceInfo::id`).
    pub device: Option<String>,
}

/// Thumbnails decoded on a background thread (the QuickLook previews are full-size PNGs).
/// A project whose preview is still being written (just saved in the editor) is decoded
/// again once the write finishes.
pub struct ThumbCache {
    tx: std::sync::mpsc::Sender<(PathBuf, egui::ColorImage)>,
    rx: std::sync::mpsc::Receiver<(PathBuf, egui::ColorImage)>,
    pub textures: std::collections::HashMap<PathBuf, egui::TextureHandle>,
    /// Projects whose preview is being written; reloaded when the write finishes.
    pub refreshing: Vec<PathBuf>,
    /// Decode threads still running.
    decoding: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl ThumbCache {
    pub fn start(paths: Vec<PathBuf>) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let refreshing = paths.iter().filter(|p| sf_session::preview::is_pending(p)).cloned().collect();
        let cache = ThumbCache { tx, rx, textures: Default::default(), refreshing, decoding: Default::default() };
        cache.decode(paths);
        cache
    }

    fn decode(&self, paths: Vec<PathBuf>) {
        let tx = self.tx.clone();
        let decoding = self.decoding.clone();
        decoding.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::thread::spawn(move || {
            for p in paths {
                let preview = p.join("QuickLook").join("Preview.png");
                let Ok(img) = image::open(&preview) else { continue };
                let thumb = img.thumbnail(352, 220).to_rgba8();
                let (w, h) = thumb.dimensions();
                let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &thumb);
                if tx.send((p, ci)).is_err() {
                    break;
                }
            }
            decoding.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        });
    }

    /// True while thumbnails are still decoding or previews are still being written.
    pub fn busy(&self) -> bool {
        self.decoding.load(std::sync::atomic::Ordering::SeqCst) > 0 || !self.refreshing.is_empty()
    }

    /// Upload any thumbnails that finished decoding, and reload those whose preview was rewritten.
    pub fn pump(&mut self, ctx: &egui::Context) {
        let (pending, done): (Vec<PathBuf>, Vec<PathBuf>) = std::mem::take(&mut self.refreshing).into_iter().partition(|p| sf_session::preview::is_pending(p));
        self.refreshing = pending;
        if !done.is_empty() {
            self.decode(done);
        }
        while let Ok((p, ci)) = self.rx.try_recv() {
            let name = format!("thumb:{}", p.display());
            self.textures.insert(p, ctx.load_texture(name, ci, egui::TextureOptions::LINEAR));
        }
    }
}

pub struct Picker {
    pub thumbs: Option<ThumbCache>,
    pub inputs: Inputs,
    pub mode: Option<Mode>,
    pub camera: Option<String>,
    pub microphone: Option<String>,
    /// Live level of the selected microphone (fills the microphone icon on the picker).
    mic_meter: Option<crate::mic_meter::MicMeter>,
    /// Bar width that fits its contents (measured each frame; long device names widen it).
    bar_w: f32,
    pub system_audio: bool,
    pub countdown_ms: u64,
    /// Short notice above the bar ("Moved … to the Trash") and when it was shown; it fades
    /// out after [`STATUS_FOR`] or when the recent panel closes, whichever comes first.
    status: Option<(String, std::time::Instant)>,
    pub show_recent: bool,
    /// When the recent panel last closed: the window stays tall for a moment so a quick
    /// re-open does not resize it (and lose the click) again.
    recent_closed_at: Option<std::time::Instant>,
    /// Page dot clicked in the recent panel: the card to scroll to on the next frame.
    recent_scroll_to: Option<usize>,
    /// Screen rect of the recent panel (last frame), for click-outside detection.
    pub recent_rect: egui::Rect,
    /// The bar, in window points (last frame).
    pub bar_rect: egui::Rect,
    /// Height the window should have this frame (bar only, or bar + open panel/menu).
    pub wanted_height: f32,
    /// Cameras, microphones and windows being listed on a background thread.
    devices_rx: Option<std::sync::mpsc::Receiver<Inputs>>,
    /// Menu text has been laid out once (glyphs are in the font atlas).
    menus_warm: bool,
}

impl Default for Picker {
    fn default() -> Self {
        Picker { thumbs: None, inputs: Inputs::default(), mode: None, camera: None, microphone: None, mic_meter: None, bar_w: PILL_SIZE.x, system_audio: false, countdown_ms: 5000, status: None, show_recent: false, recent_closed_at: None, recent_scroll_to: None, recent_rect: egui::Rect::NOTHING, bar_rect: egui::Rect::NOTHING, wanted_height: BAR_WINDOW_H, devices_rx: None, menus_warm: false }
    }
}

pub const PILL_SIZE: egui::Vec2 = vec2(950.0, 62.0);
pub const PILL_MARGIN: f32 = 10.0;
/// Window height when only the bar is shown (no transparent dead zone that would eat clicks).
pub const BAR_WINDOW_H: f32 = PILL_SIZE.y + PILL_MARGIN * 2.0;
/// Height of the status notice above the bar, plus its gap.
const STATUS_H: f32 = 28.0 + 8.0;
/// Window height that fits the bar and a status notice.
const STATUS_WINDOW_H: f32 = BAR_WINDOW_H + STATUS_H;
/// How long a status notice stays up.
const STATUS_FOR: std::time::Duration = std::time::Duration::from_secs(4);
pub const MENU_WINDOW_H: f32 = 440.0;
/// Project card in the recent panel (thumbnail, name, age).
const CARD_SIZE: egui::Vec2 = vec2(176.0, 150.0);
/// Page dots sit this far above the recent panel's bottom edge (centre of the dots).
const DOTS_FROM_BOTTOM: f32 = 18.0;
/// Recent panel sized to its content: padding, header row and one row of cards (with layout
/// spacing), then bottom padding...
const RECENT_PANEL_H: f32 = 14.0 + 28.0 + 6.0 + CARD_SIZE.y + 8.0 + 14.0;
/// ...or, when the cards overflow, room for the page dots with clear space around them.
const RECENT_PANEL_DOTS_H: f32 = RECENT_PANEL_H - 14.0 + 12.0 + DOTS_FROM_BOTTOM + 3.0;
/// Window height above the bar's margin+pill while the recent panel is open (8 px gap, 4 px slack).
pub const RECENT_WINDOW_H: f32 = RECENT_PANEL_DOTS_H + 8.0 + 4.0;
pub const PILL_RADIUS: f32 = 10.0;
/// The recent-projects panel above the bar: same corners as the bar.
const RECENT_RADIUS: u8 = 10;

impl Picker {
    /// Fill `inputs`: displays now (cheap), the rest from a background thread so the toolbar
    /// appears without waiting for device listing (cameras run ffmpeg).
    pub fn load_inputs(&mut self) {
        self.inputs = crate::recorder::display_inputs();
        self.load_device_inputs();
    }

    /// List cameras, microphones, windows and connected iPhones/iPads on a background thread.
    fn load_device_inputs(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::recorder::device_inputs());
        });
        self.devices_rx = Some(rx);
        self.menus_warm = false;
    }

    /// Lay out the menus' and the recent panel's text once so its glyphs are rasterized before
    /// a menu or the panel first opens (a new glyph or size rebuilds the font atlas mid-frame).
    fn warm_menu_text(&self, ui: &Ui, prefs: &Prefs) {
        let color = theme::p().on_glass;
        let fonts = [egui::TextStyle::Body.resolve(ui.style()), egui::TextStyle::Button.resolve(ui.style()), egui::TextStyle::Small.resolve(ui.style()), FontId::proportional(11.0), FontId::proportional(12.0), FontId::proportional(13.0)];
        let mut texts: Vec<String> = [
            "Don't record camera", "Don't record microphone", "Don't record system audio", "Record system audio from all apps",
            "Recording countdown", "None", "3 seconds", "5 seconds", "10 seconds", "Appearance", "Match system", "Light", "Dark", "Osaka Jade", "Quit ScreenForge",
            "RECENT PROJECTS", "Open", "No projects yet. Record something or open a project.", "Loading…", "No preview", "0123456789 min h d ago", "Move to Trash",
        ]
        .map(String::from)
        .to_vec();
        texts.extend(self.inputs.cameras.iter().chain(&self.inputs.microphones).map(|d| d.name.clone()));
        texts.extend(prefs.recent.iter().filter_map(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()));
        ui.fonts_mut(|f| {
            for t in &texts {
                for font in &fonts {
                    let _ = f.layout(t.clone(), font.clone(), color, f32::INFINITY);
                }
            }
        });
    }

    pub fn ui(&mut self, ui: &mut Ui, prefs: &Prefs) -> Option<PickerAction> {
        let mut action = None;
        let was_recent = self.show_recent;
        if let Some(rx) = &self.devices_rx {
            match rx.try_recv() {
                Ok(d) => {
                    self.inputs.merge_devices(d);
                    self.devices_rx = None;
                    self.menus_warm = false;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(30)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.devices_rx = None,
            }
        }
        if !self.menus_warm {
            self.warm_menu_text(ui, prefs);
            self.menus_warm = true;
        }
        if self.thumbs.is_none() {
            self.thumbs = Some(ThumbCache::start(prefs.recent.iter().filter(|p| p.exists()).cloned().collect()));
        }
        if let Some(t) = self.thumbs.as_mut() {
            t.pump(ui.ctx());
            if t.busy() {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
        let full = ui.max_rect();
        // transparent window anchored to the bottom of the screen: the pill sits at the
        // bottom, menus and the recent-projects panel open upwards
        let pill = egui::Rect::from_center_size(pos2(full.center().x, full.bottom() - PILL_MARGIN - PILL_SIZE.y / 2.0), vec2(self.bar_w.min(full.width() - 2.0 * PILL_MARGIN), PILL_SIZE.y));
        self.bar_rect = pill;
        crate::vibrancy::set(Some([pill.left(), pill.top(), pill.width(), pill.height()]), full.height(), PILL_RADIUS);
        let pal = theme::p();
        let tint = if crate::vibrancy::is_active() { pal.glass } else { pal.glass_solid };
        let painter = ui.painter();
        painter.rect(pill, CornerRadius::same(PILL_RADIUS as u8), tint, Stroke::new(1.0, pal.glass_border), egui::StrokeKind::Inside);

        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(pill.shrink2(vec2(14.0, 8.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        let ui = &mut child;
        ui.spacing_mut().item_spacing.x = 10.0;

        // close
        let (r, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
        ui.painter().circle_filled(r.center(), 13.0, pal.glass_button);
        widgets::paint_icon(ui.painter(), egui::Rect::from_center_size(r.center(), vec2(11.0, 11.0)), widgets::Icon::Close, pal.glass_button_fg);
        if resp.clicked() {
            action = Some(PickerAction::Quit);
        }
        divider(ui);

        for (mode, icon, label) in [(Mode::Display, widgets::Icon::Display, "Display"), (Mode::Window, widgets::Icon::Window, "Window"), (Mode::Area, widgets::Icon::Area, "Area"), (Mode::Device, widgets::Icon::Device, "Device")] {
            if mode == Mode::Device {
                let tip = "Record an iPhone or iPad connected by cable";
                let b = mode_button(ui, icon, label, self.mode == Some(mode)).on_hover_text(tip);
                // several devices: choose one from a menu; one: record it; none: say how
                if self.inputs.phones.len() > 1 {
                    let phones = self.inputs.phones.clone();
                    egui::Popup::menu(&b).show(|ui| {
                        for p in &phones {
                            if ui.button(&p.name).clicked() {
                                action = Some(self.record_device(p));
                            }
                        }
                    });
                } else if b.clicked() {
                    match self.inputs.phones.first().cloned() {
                        Some(p) => action = Some(self.record_device(&p)),
                        None => {
                            self.set_status("Connect an iPhone or iPad with a cable, unlock it and tap Trust, then click Device again");
                            // list again: the device appears once connected and trusted
                            if self.devices_rx.is_none() {
                                self.load_device_inputs();
                            }
                        }
                    }
                }
                continue;
            }
            let b = mode_button(ui, icon, label, self.mode == Some(mode));
            if b.clicked() {
                self.mode = Some(mode);
                action = self.start(mode);
            }
        }
        divider(ui);

        // inputs
        let cam_label = self.camera.as_ref().and_then(|id| self.inputs.cameras.iter().find(|c| &c.id == id)).map(|c| c.name.clone()).unwrap_or_else(|| "No camera".into());
        let cam_menu = input_menu(ui, widgets::Icon::Camera, &cam_label, self.camera.is_some(), |ui| {
            if ui.selectable_label(self.camera.is_none(), "Don't record camera").clicked() {
                self.camera = None;
            }
            for c in &self.inputs.cameras {
                if ui.selectable_label(self.camera.as_deref() == Some(&c.id), &c.name).clicked() {
                    self.camera = Some(c.id.clone());
                    // ask now so the system prompt is answered before the recording starts
                    let _ = sf_recording::permissions::request_camera();
                }
            }
        });
        let mic_label = self.microphone.as_ref().and_then(|id| self.inputs.microphones.iter().find(|c| &c.id == id)).map(|c| c.name.clone()).unwrap_or_else(|| "No microphone".into());
        if self.microphone.is_none() {
            // SF_TEST_MIC=<device name substring>: UI tests preselect a microphone
            if let Ok(want) = std::env::var("SF_TEST_MIC") {
                self.microphone = self.inputs.microphones.iter().find(|c| c.name.contains(&want)).map(|c| c.id.clone());
            }
        }
        // the meter opens the device by id (unique) and the name is kept for the hover label
        let mic_name = self.microphone.as_ref().filter(|id| self.inputs.microphones.iter().any(|c| &c.id == *id)).cloned();
        match (&mic_name, &self.mic_meter) {
            (Some(n), Some(m)) if &m.device_name == n => {}
            (Some(n), _) => self.mic_meter = crate::mic_meter::MicMeter::start(n),
            (None, Some(_)) => self.mic_meter = None,
            _ => {}
        }
        let mic_level = self.mic_meter.as_ref().map(|m| m.level());
        if mic_level.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        }
        let mic_menu = input_menu_level(ui, widgets::Icon::Mic, &mic_label, self.microphone.is_some(), mic_level, |ui| {
            if ui.selectable_label(self.microphone.is_none(), "Don't record microphone").clicked() {
                self.microphone = None;
            }
            for c in &self.inputs.microphones {
                if ui.selectable_label(self.microphone.as_deref() == Some(&c.id), &c.name).clicked() {
                    self.microphone = Some(c.id.clone());
                }
            }
        });
        let sys_label = if self.system_audio { "System audio" } else { "No system audio" };
        let sys_menu = input_menu(ui, widgets::Icon::Audio, sys_label, self.system_audio, |ui| {
            if ui.selectable_label(!self.system_audio, "Don't record system audio").clicked() {
                self.system_audio = false;
            }
            let supported = self.inputs.system_audio_supported;
            let r = ui.add_enabled_ui(supported, |ui| ui.selectable_label(self.system_audio, "Record system audio from all apps")).inner;
            if r.clicked() {
                self.system_audio = true;
            }
        });
        divider(ui);

        // recent projects panel toggle
        let folder = widgets::icon_button(ui, widgets::Icon::Folder, 18.0, "Recent projects");
        let folder_clicked = folder.clicked();
        if folder_clicked {
            self.show_recent = !self.show_recent;
            if !self.show_recent {
                self.recent_closed_at = Some(std::time::Instant::now());
            }
            ui.ctx().request_repaint();
        }
        if self.show_recent {
            ui.painter().circle_filled(pos2(folder.rect.right() - 4.0, folder.rect.top() + 4.0), 3.0, theme::p().accent_hover);
        }
        // settings (a dot when a newer release is out)
        let update = crate::update::current();
        let gear = widgets::icon_text_button(ui, Some(widgets::Icon::Gear), "", true);
        if matches!(update, crate::update::State::Available { .. }) {
            ui.painter().circle_filled(pos2(gear.rect.left() + 20.0, gear.rect.top() + 7.0), 3.0, theme::p().accent_hover);
        }
        // fit the bar to what it holds (14 px inner margin each side)
        let fit = (gear.rect.right() - pill.left() + 14.0).ceil();
        if (fit - self.bar_w).abs() > 0.5 {
            self.bar_w = fit;
            ui.ctx().request_repaint();
        }
        bar_menu(&gear, egui::RectAlign::TOP_END, |ui| {
            ui.set_min_width(240.0);
            ui.label(RichText::new("Recording countdown").small().color(theme::p().text_dim));
            for (ms, l) in [(0u64, "None"), (3000, "3 seconds"), (5000, "5 seconds"), (10000, "10 seconds")] {
                if ui.selectable_label(self.countdown_ms == ms, l).clicked() {
                    self.countdown_ms = ms;
                }
            }
            ui.separator();
            ui.label(RichText::new("Appearance").small().color(theme::p().text_dim));
            for (choice, l) in [(Appearance::ScreenForge, "ScreenForge Dark"), (Appearance::ScreenForgeLight, "ScreenForge Light"), (Appearance::System, "Match system"), (Appearance::Dark, "Catppuccin Mocha"), (Appearance::Light, "Solarized Light"), (Appearance::OsakaJade, "Osaka Jade")] {
                if ui.selectable_label(prefs.appearance == choice, l).clicked() {
                    action = Some(PickerAction::SetAppearance(choice));
                }
            }
            ui.separator();
            if sf_recording::permissions::accessibility().is_granted() {
                ui.label(RichText::new("Zooms follow typing: on").small().color(theme::p().text_dim));
            } else if ui.button("Let zooms follow typing…").on_hover_text("Allow Accessibility so automatic zooms follow the text caret while you type").clicked() {
                if sf_recording::permissions::request_accessibility() != sf_recording::PermissionStatus::Granted {
                    let _ = sf_recording::permissions::open_settings(sf_recording::Permission::Accessibility);
                }
            }
            ui.separator();
            ui.label(RichText::new(format!("Updates · v{}", crate::update::version())).small().color(theme::p().text_dim));
            match &update {
                crate::update::State::Available { version, url } => {
                    if ui.button(RichText::new(format!("Update to v{version}…")).color(theme::p().accent)).on_hover_text("Open the release page to download it").clicked() {
                        crate::os::open(url);
                    }
                }
                crate::update::State::Checking => {
                    ui.label(RichText::new("Checking…").small().color(theme::p().text_dim));
                }
                crate::update::State::UpToDate => {
                    ui.label(RichText::new("ScreenForge is up to date").small().color(theme::p().text_dim));
                }
                crate::update::State::Failed(_) => {
                    ui.label(RichText::new("Could not reach GitHub").small().color(theme::p().text_dim));
                }
                crate::update::State::Idle => {}
            }
            if !matches!(update, crate::update::State::Checking) && ui.button("Check for updates now").clicked() {
                action = Some(PickerAction::CheckUpdates);
            }
            let mut auto = prefs.check_updates;
            if ui.checkbox(&mut auto, "Check automatically").on_hover_text("Once a day at launch: one request to GitHub's public releases API, nothing about you is sent").changed() {
                action = Some(PickerAction::SetCheckUpdates(auto));
            }
            ui.separator();
            if ui.button("Quit ScreenForge").clicked() {
                action = Some(PickerAction::Quit);
            }
        });

        // Menus open above the bar, so the window must be tall first. Grow it while the pointer
        // is on a menu button: a click then opens the menu at once instead of waiting a few
        // frames for the window to resize. Tested against this frame's button rects: egui's
        // hover state uses last frame's layout, which is off by the resize just made.
        let pointer = ui.input(|i| i.pointer.latest_pos());
        let menu_hover = pointer.is_some_and(|p| [&cam_menu, &mic_menu, &sys_menu, &gear, &folder].iter().any(|r| r.rect.contains(p)));
        let any_menu = egui::Popup::is_any_open(ui.ctx()) || menu_hover;
        if self.show_recent {
            // only make room for page dots when the cards overflow the panel
            let n = prefs.recent.iter().filter(|p| p.exists()).count() as f32;
            let gap = ui.spacing().item_spacing.x;
            let overflows = n * (CARD_SIZE.x + gap) - gap > pill.width() - 28.0;
            let h = if overflows { RECENT_PANEL_DOTS_H } else { RECENT_PANEL_H };
            let panel = egui::Rect::from_min_size(pos2(pill.left(), pill.top() - 8.0 - h), vec2(pill.width(), h));
            // same frosted glass as the bar, so the two read as one surface
            crate::vibrancy::set_panel(Some([panel.left(), panel.top(), panel.width(), panel.height()]), full.height(), RECENT_RADIUS as f32);
            self.recent_panel(ui, panel, prefs, &mut action);
        } else {
            crate::vibrancy::set_panel(None, 0.0, 0.0);
        }
        // after the panel closes the window keeps its height while the pointer stays on the bar
        // (and briefly after): toggling the folder button would otherwise grow and shrink the
        // window on every click, and a click landing mid-resize falls through to the app behind
        const LINGER: std::time::Duration = std::time::Duration::from_millis(450);
        let over_bar = pointer.is_some_and(|p| pill.contains(p));
        let lingering = self.recent_closed_at.is_some_and(|t| t.elapsed() < LINGER || over_bar);
        if lingering {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            self.recent_closed_at = None;
        }
        // the status notice expires on its own
        if self.status.as_ref().is_some_and(|(_, at)| at.elapsed() >= STATUS_FOR) {
            self.status = None;
        }
        let status_h = if self.status.is_some() { STATUS_WINDOW_H } else { 0.0 };
        let wanted = if self.show_recent || lingering { PILL_MARGIN + PILL_SIZE.y + RECENT_WINDOW_H } else if any_menu { MENU_WINDOW_H } else { BAR_WINDOW_H.max(status_h) };
        // never shrink mid-click: resizing moves the bar in window coordinates, and the release
        // would land somewhere else and lose the click
        self.wanted_height = if ui.input(|i| i.pointer.any_down()) { wanted.max(self.wanted_height) } else { wanted };
        if let Some((s, at)) = &self.status {
            let w = 520.0;
            let r = egui::Rect::from_center_size(pos2(pill.center().x, pill.top() - 8.0 - 14.0), vec2(w, 28.0));
            ui.painter().rect_filled(r, CornerRadius::same(8), theme::p().glass_solid);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, s, FontId::proportional(13.0), theme::p().text_dim);
            ui.ctx().request_repaint_after(STATUS_FOR.saturating_sub(at.elapsed()));
        }
        // Esc, clicking anything else on the bar (mode buttons, input menus, settings) or
        // picking a project hides the recent panel; moving one to the Trash keeps it open
        if self.show_recent && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_recent = false;
            self.recent_closed_at = Some(std::time::Instant::now());
            ui.ctx().request_repaint();
        }
        if self.show_recent && !folder_clicked {
            let other_click = ui.input(|i| i.pointer.any_click()) && !self.pointer_in_recent(ui);
            let closes = action.is_some() && !matches!(action, Some(PickerAction::RemoveProject(_)));
            if closes || egui::Popup::is_any_open(ui.ctx()) || other_click {
                self.show_recent = false;
                self.recent_closed_at = Some(std::time::Instant::now());
            }
        }
        // the notice belongs to the panel it was shown over: closing the panel takes it along
        if was_recent && !self.show_recent {
            self.status = None;
        }
        action
    }

    /// Show a short notice above the bar; it goes away on its own.
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), std::time::Instant::now()));
    }

    fn pointer_in_recent(&self, ui: &Ui) -> bool {
        let rect = self.recent_rect;
        ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| rect.contains(p))
    }

    fn recent_panel(&mut self, ui: &mut Ui, rect: egui::Rect, prefs: &Prefs, action: &mut Option<PickerAction>) {
        self.recent_rect = rect;
        let p = ui.painter();
        let tint = if crate::vibrancy::is_active() { theme::p().glass } else { theme::p().glass_solid };
        p.rect(rect, CornerRadius::same(RECENT_RADIUS), tint, Stroke::new(1.0, theme::p().glass_border), egui::StrokeKind::Inside);
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(14.0)).layout(egui::Layout::top_down(egui::Align::Min)));
        inner.horizontal(|ui| {
            ui.label(RichText::new("RECENT PROJECTS").small().color(theme::p().text_dim));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if open_button(ui).clicked() {
                    *action = Some(PickerAction::OpenProjectDialog);
                }
            });
        });
        inner.add_space(6.0);
        let recent: Vec<PathBuf> = prefs.recent.iter().filter(|p| p.exists()).cloned().collect();
        if recent.is_empty() {
            inner.label(RichText::new("No projects yet. Record something or open a project.").color(theme::p().text_dim));
            return;
        }
        let thumbs = self.thumbs.as_ref();
        let scroll_to = self.recent_scroll_to.take();
        let gap = inner.spacing().item_spacing.x;
        // no scrollbar: trackpad / wheel scrolling, with page dots underneath when it overflows
        let out = egui::ScrollArea::horizontal().id_salt("recent_cards").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(&mut inner, |ui| {
            ui.horizontal(|ui| {
                for (i, path) in recent.iter().enumerate() {
                    let tex = thumbs.and_then(|t| t.textures.get(path));
                    let refreshing = thumbs.is_some_and(|t| t.refreshing.contains(path));
                    let (card, trash) = project_card(ui, path, tex, refreshing);
                    if scroll_to == Some(i) {
                        card.scroll_to_me(Some(egui::Align::Min));
                    }
                    if trash {
                        *action = Some(PickerAction::RemoveProject(path.clone()));
                    } else if card.clicked() {
                        *action = Some(PickerAction::OpenProject(path.clone()));
                    }
                }
            });
        });
        let view = out.inner_rect.width();
        let max_offset = (out.content_size.x - view).max(0.0);
        if max_offset > 1.0 {
            let step = CARD_SIZE.x + gap;
            let per_page = ((view + gap) / step).floor().max(1.0) as usize;
            let pages = recent.len().div_ceil(per_page);
            let offset = out.state.offset.x;
            let current = if offset >= max_offset - 1.0 { pages - 1 } else { ((offset / (step * per_page as f32)).round() as usize).min(pages - 1) };
            if let Some(page) = page_dots(&mut inner, rect.center().x, rect.bottom() - DOTS_FROM_BOTTOM, pages, current) {
                self.recent_scroll_to = Some(page * per_page);
                inner.ctx().request_repaint();
            }
        }
    }

    /// Build a request for `mode` with the current input choices.
    pub fn request(&self, mode: Mode, area: Option<sf_project::schema::common::Rect>, window_id: Option<String>) -> RecordRequest {
        let display_id = area
            .and_then(|a| self.inputs.displays.iter().find(|d| a.x + a.width / 2.0 >= d.bounds.x && a.x + a.width / 2.0 < d.bounds.x + d.bounds.width && a.y + a.height / 2.0 >= d.bounds.y && a.y + a.height / 2.0 < d.bounds.y + d.bounds.height))
            .or_else(|| self.inputs.displays.iter().find(|d| d.is_main))
            .or_else(|| self.inputs.displays.first())
            .map(|d| d.id.to_string());
        RecordRequest {
            mode,
            display_id,
            window_id,
            area,
            camera: self.camera.clone(),
            microphone: self.microphone.clone(),
            system_audio: self.system_audio,
            countdown_ms: self.countdown_ms,
            capture_input: true,
            device: None,
        }
    }

    fn record_device(&mut self, phone: &InputDevice) -> PickerAction {
        self.mode = Some(Mode::Device);
        let mut req = self.request(Mode::Device, None, None);
        req.device = Some(phone.id.clone());
        PickerAction::StartRecording(req)
    }

    fn start(&mut self, mode: Mode) -> Option<PickerAction> {
        match mode {
            Mode::Display => Some(PickerAction::StartRecording(self.request(Mode::Display, None, None))),
            Mode::Area => Some(PickerAction::SelectArea(self.request(Mode::Area, None, None))),
            Mode::Window => Some(PickerAction::SelectWindow(self.request(Mode::Window, None, None))),
            Mode::Device => None,
        }
    }

}

fn divider(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(1.0, 40.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, theme::glass_alpha(30));
}

fn mode_button(ui: &mut Ui, icon: widgets::Icon, label: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(66.0, 46.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() || selected {
        p.rect_filled(rect, CornerRadius::same(2), theme::glass_alpha(if selected { 44 } else { 24 }));
    }
    widgets::paint_icon(p, egui::Rect::from_center_size(pos2(rect.center().x, rect.top() + 16.0), vec2(22.0, 22.0)), icon, theme::p().on_glass);
    p.text(pos2(rect.center().x, rect.bottom() - 9.0), Align2::CENTER_CENTER, label, FontId::proportional(11.0), theme::p().text_dim);
    resp
}

/// Input picker pill on the bar: icon · label · chevron, with a subtle glass fill. `active`
/// (a device is selected) draws the icon in the accent colour.
fn input_menu(ui: &mut Ui, icon: widgets::Icon, label: &str, active: bool, add: impl FnOnce(&mut Ui)) -> egui::Response {
    input_menu_level(ui, icon, label, active, None, add)
}

/// `input_menu` with an optional live level (0..1) that fills the icon.
fn input_menu_level(ui: &mut Ui, icon: widgets::Icon, label: &str, active: bool, level: Option<f32>, add: impl FnOnce(&mut Ui)) -> egui::Response {
    let pal = theme::p();
    let font = FontId::proportional(13.0);
    // long device names end in "…" instead of being cut mid-letter
    let mut job = egui::text::LayoutJob::simple_singleline(label.to_string(), font.clone(), pal.on_glass);
    job.wrap = egui::text::TextWrapping { max_width: 170.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let text_w = galley.size().x;
    let w = 12.0 + 16.0 + 8.0 + text_w + 6.0 + 10.0 + 10.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
    let p = ui.painter();
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    if resp.hovered() || open {
        p.rect_filled(rect, CornerRadius::same(2), theme::glass_alpha(36));
    }
    let icon_color = if active { pal.accent_hover } else { pal.on_glass };
    let icon_rect = egui::Rect::from_center_size(pos2(rect.left() + 12.0 + 8.0, rect.center().y), vec2(16.0, 16.0));
    match level {
        // the microphone itself fills green as you talk
        Some(l) => widgets::paint_mic_level(p, icon_rect, l, icon_color, pal.meter),
        None => widgets::paint_icon(p, icon_rect, icon, icon_color),
    }
    let text_rect = egui::Rect::from_min_size(pos2(rect.left() + 12.0 + 16.0 + 8.0, rect.center().y - galley.size().y / 2.0), vec2(text_w, galley.size().y));
    p.with_clip_rect(text_rect.expand2(vec2(0.0, 4.0))).galley(text_rect.min, galley, pal.on_glass);
    widgets::paint_icon(p, egui::Rect::from_center_size(pos2(rect.right() - 10.0 - 5.0, rect.center().y + 1.0), vec2(10.0, 10.0)), widgets::Icon::ChevronDown, pal.text_dim);
    bar_menu(&resp, egui::RectAlign::TOP_START, |ui| {
        ui.set_min_width(260.0);
        add(ui);
    });
    resp
}

/// Popup menu styled like the bar: glass panel with 10 px corners, rows with 2 px corners, a
/// faint highlight on hover and accent text (not a filled block) for the current choice.
pub(crate) fn bar_menu(resp: &egui::Response, align: egui::RectAlign, add: impl FnOnce(&mut Ui)) {
    let pal = theme::p();
    let frame = egui::Frame::NONE
        .fill(pal.glass_solid)
        .stroke(Stroke::new(1.0, pal.glass_border))
        .corner_radius(CornerRadius::same(PILL_RADIUS as u8))
        .inner_margin(egui::Margin::same(6));
    egui::Popup::menu(resp).align(align).frame(frame).show(|ui| {
        let st = ui.style_mut();
        st.spacing.button_padding = vec2(10.0, 6.0);
        st.spacing.item_spacing.y = 2.0;
        let v = &mut st.visuals;
        v.selection.bg_fill = theme::glass_alpha(22);
        v.selection.stroke = Stroke::new(1.0, pal.accent_hover);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
            w.corner_radius = CornerRadius::same(2);
            w.bg_stroke = Stroke::NONE;
            w.expansion = 0.0;
        }
        v.widgets.hovered.weak_bg_fill = theme::glass_alpha(36);
        v.widgets.hovered.bg_fill = theme::glass_alpha(36);
        v.widgets.active.weak_bg_fill = theme::glass_alpha(48);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, pal.on_glass);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, pal.on_glass);
        add(ui);
    });
}


/// Thumbnail card for a recent project (uses the package's QuickLook preview).
/// `refreshing`: a new preview is being written (the project was just saved); a spinner sits
/// over the old one. Returns the card response and whether its trash button was clicked.
fn project_card(ui: &mut Ui, path: &std::path::Path, tex: Option<&egui::TextureHandle>, refreshing: bool) -> (egui::Response, bool) {
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let size = CARD_SIZE;
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let thumb = egui::Rect::from_min_size(rect.min, vec2(size.x, 110.0));
    // trash button in the thumbnail's top-right corner, only while the card is hovered
    let trash_rect = egui::Rect::from_min_size(pos2(thumb.right() - 30.0, thumb.top() + 6.0), vec2(24.0, 24.0));
    let trash = ui.interact(trash_rect, resp.id.with("trash"), Sense::click());
    let card_hovered = resp.hovered() || trash.hovered();
    let p = ui.painter();
    if card_hovered {
        p.rect_filled(rect.expand(4.0), CornerRadius::same(12), theme::glass_alpha(14));
    }
    let mut drawn = false;
    if let Some(texture) = tex {
        {
            let (w, h) = (texture.size()[0] as f32, texture.size()[1] as f32);
            let target = thumb.width() / thumb.height();
            let uv = if w / h > target {
                let f = (h * target) / w;
                egui::Rect::from_min_max(pos2((1.0 - f) / 2.0, 0.0), pos2((1.0 + f) / 2.0, 1.0))
            } else {
                let f = (w / target) / h;
                egui::Rect::from_min_max(pos2(0.0, (1.0 - f) / 2.0), pos2(1.0, (1.0 + f) / 2.0))
            };
            let mut mesh = egui::Mesh::with_texture(texture.id());
            mesh.add_rect_with_uv(thumb, uv, Color32::WHITE);
            p.add(egui::Shape::mesh(mesh));
            drawn = true;
        }
    }
    if !drawn {
        p.rect_filled(thumb, CornerRadius::same(8), theme::p().thumb_empty);
        p.text(thumb.center(), Align2::CENTER_CENTER, if path.join("QuickLook/Preview.png").is_file() { "Loading…" } else { "No preview" }, FontId::proportional(12.0), theme::p().text_dim);
    }
    p.rect_stroke(thumb, 8.0, Stroke::new(3.0, theme::p().glass_solid), egui::StrokeKind::Outside);
    if refreshing && drawn {
        let badge = egui::Rect::from_min_size(pos2(thumb.left() + 6.0, thumb.bottom() - 30.0), vec2(24.0, 24.0));
        p.rect_filled(badge, CornerRadius::same(2), theme::p().glass_solid);
        egui::Spinner::new().size(14.0).color(theme::p().text_dim).paint_at(ui, badge.shrink(5.0));
    }
    if card_hovered {
        p.rect_filled(trash_rect, CornerRadius::same(2), if trash.hovered() { theme::p().danger } else { theme::p().glass_solid });
        widgets::paint_icon(p, trash_rect.shrink(5.0), widgets::Icon::Trash, if trash.hovered() { Color32::WHITE } else { theme::p().on_glass });
    }
    let mut job = egui::text::LayoutJob::simple_singleline(name, FontId::proportional(13.0), theme::p().text);
    job.wrap = egui::text::TextWrapping { max_width: size.x, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = p.layout_job(job);
    p.galley(pos2(rect.left(), thumb.bottom() + 8.0), galley, theme::p().text);
    let modified = std::fs::metadata(path.join("project.json")).and_then(|m| m.modified()).ok().map(|t| {
        let secs = std::time::SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
        if secs < 3600 { format!("{} min ago", secs / 60) } else if secs < 86400 { format!("{} h ago", secs / 3600) } else { format!("{} d ago", secs / 86400) }
    });
    if let Some(m) = modified {
        p.text(pos2(rect.left(), rect.bottom() - 6.0), Align2::LEFT_BOTTOM, m, FontId::proportional(11.0), theme::p().text_dim);
    }
    let trash_clicked = trash.clicked();
    let _ = trash.on_hover_text("Move to Trash");
    (resp.on_hover_cursor(egui::CursorIcon::PointingHand), trash_clicked)
}

/// "Open" with a folder icon for the recent-projects panel: quiet on the glass, filled on hover.
fn open_button(ui: &mut Ui) -> egui::Response {
    let pal = theme::p();
    let galley = ui.painter().layout_no_wrap("Open".into(), egui::FontId::proportional(13.0), Color32::PLACEHOLDER);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(galley.size().x + 44.0, 28.0), egui::Sense::click());
    let fg = if resp.hovered() { pal.glass_button_fg } else { pal.on_glass };
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(2), pal.glass_button);
    }
    widgets::paint_icon(ui.painter(), egui::Rect::from_center_size(pos2(rect.left() + 17.0, rect.center().y), egui::vec2(14.0, 14.0)), widgets::Icon::Folder, fg);
    ui.painter().galley(pos2(rect.left() + 32.0, rect.center().y - galley.size().y / 2.0), galley, fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Open a project, or import a Screen Studio project")
}

/// A row of page dots centred on (`cx`, `cy`); returns the page whose dot was clicked.
fn page_dots(ui: &mut Ui, cx: f32, cy: f32, pages: usize, current: usize) -> Option<usize> {
    const PITCH: f32 = 14.0;
    let pal = theme::p();
    let x0 = cx - PITCH * (pages as f32 - 1.0) / 2.0;
    let mut clicked = None;
    for i in 0..pages {
        let c = pos2(x0 + PITCH * i as f32, cy);
        let hit = ui.interact(egui::Rect::from_center_size(c, egui::vec2(PITCH, PITCH)), ui.id().with(("page_dot", i)), egui::Sense::click());
        let color = if i == current { pal.on_glass } else if hit.hovered() { theme::glass_alpha(120) } else { theme::glass_alpha(60) };
        ui.painter().circle_filled(c, 3.0, color);
        if hit.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            clicked = Some(i);
        }
    }
    clicked
}
