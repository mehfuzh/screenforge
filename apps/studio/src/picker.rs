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
    pub microphones: Vec<InputDevice>,
    pub displays: Vec<DisplayEntry>,
    pub windows: Vec<WindowEntry>,
    pub system_audio_supported: bool,
}

/// Theme preference stored in prefs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    System,
    Light,
    /// Catppuccin Mocha (the default).
    #[default]
    Dark,
    /// Dark, with the Osaka Jade palette.
    #[serde(rename = "osaka-jade")]
    OsakaJade,
}

impl Appearance {
    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Appearance::System => egui::ThemePreference::System,
            Appearance::Light => egui::ThemePreference::Light,
            Appearance::Dark | Appearance::OsakaJade => egui::ThemePreference::Dark,
        }
    }

    /// Make the theme module use this preference's dark palette, then set egui's theme.
    pub fn apply(self, ctx: &egui::Context) {
        crate::theme::set_dark_palette(if self == Appearance::OsakaJade { crate::theme::OSAKA_JADE } else { crate::theme::DARK });
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
}

/// Thumbnails decoded on a background thread (the QuickLook previews are full-size PNGs).
pub struct ThumbCache {
    rx: std::sync::mpsc::Receiver<(PathBuf, egui::ColorImage)>,
    pub textures: std::collections::HashMap<PathBuf, egui::TextureHandle>,
    done: bool,
}

impl ThumbCache {
    pub fn start(paths: Vec<PathBuf>) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
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
        });
        ThumbCache { rx, textures: Default::default(), done: false }
    }

    /// True once the decoding thread has hung up.
    pub fn rx_done(&self) -> bool {
        self.done
    }

    /// Upload any thumbnails that finished decoding.
    pub fn pump(&mut self, ctx: &egui::Context) {
        loop {
            match self.rx.try_recv() {
                Ok((p, ci)) => {
                    let name = format!("thumb:{}", p.display());
                    self.textures.insert(p, ctx.load_texture(name, ci, egui::TextureOptions::LINEAR));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.done = true;
                    break;
                }
            }
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
    pub status: Option<String>,
    pub show_recent: bool,
    /// Screen rect of the recent panel (last frame), for click-outside detection.
    recent_rect: egui::Rect,
    /// Height the window should have this frame (bar only, or bar + open panel/menu).
    pub wanted_height: f32,
}

impl Default for Picker {
    fn default() -> Self {
        Picker { thumbs: None, inputs: Inputs::default(), mode: None, camera: None, microphone: None, mic_meter: None, bar_w: PILL_SIZE.x, system_audio: false, countdown_ms: 5000, status: None, show_recent: false, recent_rect: egui::Rect::NOTHING, wanted_height: BAR_WINDOW_H }
    }
}

pub const PILL_SIZE: egui::Vec2 = vec2(950.0, 62.0);
pub const PILL_MARGIN: f32 = 10.0;
/// Window height when only the bar is shown (no transparent dead zone that would eat clicks).
pub const BAR_WINDOW_H: f32 = PILL_SIZE.y + PILL_MARGIN * 2.0;
pub const MENU_WINDOW_H: f32 = 440.0;
pub const RECENT_WINDOW_H: f32 = 300.0;
pub const PILL_RADIUS: f32 = 10.0;

impl Picker {
    pub fn ui(&mut self, ui: &mut Ui, prefs: &Prefs) -> Option<PickerAction> {
        let mut action = None;
        if self.thumbs.is_none() {
            self.thumbs = Some(ThumbCache::start(prefs.recent.iter().filter(|p| p.exists()).cloned().collect()));
        }
        if let Some(t) = self.thumbs.as_mut() {
            t.pump(ui.ctx());
            if !t.rx_done() {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
        let full = ui.max_rect();
        // transparent window anchored to the bottom of the screen: the pill sits at the
        // bottom, menus and the recent-projects panel open upwards
        let pill = egui::Rect::from_center_size(pos2(full.center().x, full.bottom() - PILL_MARGIN - PILL_SIZE.y / 2.0), vec2(self.bar_w.min(full.width() - 2.0 * PILL_MARGIN), PILL_SIZE.y));
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
                widgets::roadmap(ui, |ui| mode_button(ui, icon, label, false));
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
        // camera recording is not implemented: greyed out until it is
        widgets::roadmap(ui, |ui| input_menu(ui, widgets::Icon::Camera, &cam_label, false, |_| {}));
        let mic_label = self.microphone.as_ref().and_then(|id| self.inputs.microphones.iter().find(|c| &c.id == id)).map(|c| c.name.clone()).unwrap_or_else(|| "No microphone".into());
        if self.microphone.is_none() {
            // SF_TEST_MIC=<device name substring>: UI tests preselect a microphone
            if let Ok(want) = std::env::var("SF_TEST_MIC") {
                self.microphone = self.inputs.microphones.iter().find(|c| c.name.contains(&want)).map(|c| c.id.clone());
            }
        }
        let mic_name = self.microphone.as_ref().and_then(|id| self.inputs.microphones.iter().find(|c| &c.id == id)).map(|c| c.name.clone());
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
        input_menu_level(ui, widgets::Icon::Mic, &mic_label, self.microphone.is_some(), mic_level, |ui| {
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
        input_menu(ui, widgets::Icon::Audio, sys_label, self.system_audio, |ui| {
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
            ui.ctx().request_repaint();
        }
        if self.show_recent {
            ui.painter().circle_filled(pos2(folder.rect.right() - 4.0, folder.rect.top() + 4.0), 3.0, theme::p().accent_hover);
        }
        // settings
        let gear = widgets::icon_text_button(ui, Some(widgets::Icon::Gear), "", true);
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
            for (choice, l) in [(Appearance::System, "Match system"), (Appearance::Light, "Light"), (Appearance::Dark, "Dark"), (Appearance::OsakaJade, "Osaka Jade")] {
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
            if ui.button("Quit ScreenForge").clicked() {
                action = Some(PickerAction::Quit);
            }
        });

        let any_menu = egui::Popup::is_any_open(ui.ctx());
        if self.show_recent {
            let h = RECENT_WINDOW_H - 8.0 - 4.0;
            let panel = egui::Rect::from_min_size(pos2(pill.left(), pill.top() - 8.0 - h), vec2(pill.width(), h));
            self.recent_panel(ui, panel, prefs, &mut action);
        }
        self.wanted_height = if self.show_recent { PILL_MARGIN + PILL_SIZE.y + RECENT_WINDOW_H } else if any_menu { MENU_WINDOW_H } else { BAR_WINDOW_H };
        if let Some(s) = &self.status {
            let w = 520.0;
            let r = egui::Rect::from_center_size(pos2(pill.center().x, pill.top() - 22.0), vec2(w, 28.0));
            ui.painter().rect_filled(r, CornerRadius::same(8), theme::p().glass_solid);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, s, FontId::proportional(13.0), theme::p().text_dim);
        }
        // clicking anything else on the bar (mode buttons, input menus, settings) or picking a
        // project hides the recent panel
        if self.show_recent && !folder_clicked {
            let other_click = ui.input(|i| i.pointer.any_click()) && !self.pointer_in_recent(ui);
            if action.is_some() || egui::Popup::is_any_open(ui.ctx()) || other_click {
                self.show_recent = false;
            }
        }
        action
    }

    fn pointer_in_recent(&self, ui: &Ui) -> bool {
        let rect = self.recent_rect;
        ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| rect.contains(p))
    }

    fn recent_panel(&mut self, ui: &mut Ui, rect: egui::Rect, prefs: &Prefs, action: &mut Option<PickerAction>) {
        self.recent_rect = rect;
        let p = ui.painter();
        p.rect(rect, CornerRadius::same(14), theme::p().glass_solid, Stroke::new(1.0, theme::p().glass_border), egui::StrokeKind::Inside);
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(14.0)).layout(egui::Layout::top_down(egui::Align::Min)));
        inner.horizontal(|ui| {
            ui.label(RichText::new("RECENT PROJECTS").small().color(theme::p().text_dim));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Open project…").clicked() {
                    *action = Some(PickerAction::OpenProjectDialog);
                }
            });
        });
        inner.add_space(6.0);
        let recent: Vec<PathBuf> = prefs.recent.iter().filter(|p| p.exists()).cloned().collect();
        if recent.is_empty() {
            inner.label(RichText::new("No projects yet. Record something or open a .screenstudio package.").color(theme::p().text_dim));
            return;
        }
        let thumbs = self.thumbs.as_ref().map(|t| &t.textures);
        egui::ScrollArea::horizontal().show(&mut inner, |ui| {
            ui.horizontal(|ui| {
                for path in &recent {
                    let tex = thumbs.and_then(|t| t.get(path));
                    let (card, trash) = project_card(ui, path, tex);
                    if trash {
                        *action = Some(PickerAction::RemoveProject(path.clone()));
                    } else if card.clicked() {
                        *action = Some(PickerAction::OpenProject(path.clone()));
                    }
                }
            });
        });
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
        }
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
fn input_menu(ui: &mut Ui, icon: widgets::Icon, label: &str, active: bool, add: impl FnOnce(&mut Ui)) {
    input_menu_level(ui, icon, label, active, None, add)
}

/// `input_menu` with an optional live level (0..1) that fills the icon.
fn input_menu_level(ui: &mut Ui, icon: widgets::Icon, label: &str, active: bool, level: Option<f32>, add: impl FnOnce(&mut Ui)) {
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
}

/// Popup menu styled like the bar: glass panel with 10 px corners, rows with 2 px corners, a
/// faint highlight on hover and accent text (not a filled block) for the current choice.
fn bar_menu(resp: &egui::Response, align: egui::RectAlign, add: impl FnOnce(&mut Ui)) {
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
/// Returns the card response and whether its trash button was clicked.
fn project_card(ui: &mut Ui, path: &std::path::Path, tex: Option<&egui::TextureHandle>) -> (egui::Response, bool) {
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let size = vec2(176.0, 150.0);
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

