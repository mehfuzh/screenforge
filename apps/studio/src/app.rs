use std::path::PathBuf;
use std::sync::Arc;

use eframe::CreationContext;
use sf_render::Gpu;

use crate::editor::Editor;
use crate::picker::{Picker, PickerAction};
use crate::recorder::{RecordingOutcome, RecordingScreen};

pub enum Screen {
    /// Floating recording toolbar (frameless window).
    Picker(Picker),
    /// Countdown / recording HUD in the floating window.
    Recording(RecordingScreen),
    Editor(Box<Editor>),
    /// The editor window is up and shows "Opening …" while the project loads; the count is
    /// the frames left before loading starts (the resized window paints first).
    Opening(PathBuf, u8),
}

pub struct App {
    pub gpu: Gpu,
    pub screen: Screen,
    pub error: Option<String>,
    pub prefs: Prefs,
    window_mode: WindowMode,
    picker_height: f32,
    placed_once: bool,
    tray: Option<crate::tray::Tray>,
    tray_tried: bool,
    /// Area selection overlay in progress, with the request to start once a rect is chosen.
    area_select: Option<(crate::area_select::AreaSelect, crate::picker::RecordRequest)>,
    window_select: Option<(crate::window_select::WindowSelect, crate::picker::RecordRequest)>,
    /// Frames to wait before restyling the native title bar after `Decorations(true)`.
    titlebar_fix: u8,
    /// Project passed on the command line (`screenforge My.screenstudio`), opened on the first frame.
    pending_open: Option<PathBuf>,
    /// First-launch permissions window.
    perm_setup: Option<crate::permissions::PermissionsSetup>,
    /// Closing a just-recorded project: ask keep / move to Trash. `true` = quit afterwards.
    confirm_close: Option<bool>,
    /// Set when the user chose Quit (toolbar or menu bar icon); any other window close in
    /// the editor goes back to the recording toolbar instead of quitting.
    quitting: bool,
    /// Path of the project opened right after recording (marks the editor as fresh).
    fresh_open: Option<PathBuf>,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum WindowMode {
    Picker,
    Editor,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Prefs {
    pub recent: Vec<PathBuf>,
    #[serde(default)]
    pub appearance: crate::picker::Appearance,
}

impl Prefs {
    fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.config_dir().join("prefs.json"))
    }
    pub fn load() -> Self {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        if let Some(p) = Self::path() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(p, serde_json::to_string_pretty(self).unwrap_or_default());
        }
    }
    pub fn touch(&mut self, path: &std::path::Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(12);
        self.save();
    }
}

pub const EDITOR_SIZE: [f32; 2] = [1440.0, 900.0];
/// Gap between the picker bar and the bottom of the screen (clears the Dock).
pub const PICKER_BOTTOM_GAP: f32 = 96.0;
pub const PICKER_SIZE: [f32; 2] = [1200.0, crate::picker::BAR_WINDOW_H];

impl App {
    pub fn new(cc: &CreationContext<'_>) -> Self {
        crate::theme::apply(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let rs = cc.wgpu_render_state.as_ref().expect("wgpu renderer required");
        let gpu = Gpu::from_parts(Arc::new(rs.device.clone()), Arc::new(rs.queue.clone()));
        let prefs = Prefs::load();
        prefs.appearance.apply(&cc.egui_ctx);
        crate::theme::apply(&cc.egui_ctx);
        let mut picker = Picker::default();
        picker.load_inputs();
        let perm_setup = (crate::permissions::PermissionsSetup::needed() || std::env::var_os("SF_SHOW_PERMISSIONS").is_some()).then(crate::permissions::PermissionsSetup::start);
        let app = App { gpu, screen: Screen::Picker(picker), error: None, prefs, window_mode: WindowMode::Picker, picker_height: crate::picker::BAR_WINDOW_H, placed_once: false, tray: None, tray_tried: false, area_select: None, window_select: None, titlebar_fix: 0, perm_setup, confirm_close: None, quitting: false, fresh_open: None, pending_open: std::env::args().nth(1).map(PathBuf::from).filter(|p| p.is_dir() || p.extension().is_some_and(|e| e == "screenstudio")) };
        // a project on the command line opens on the first frame (`pending_open`)
        app
    }

    fn set_window_mode(&mut self, ctx: &egui::Context, mode: WindowMode) {
        if self.window_mode == mode {
            return;
        }
        self.window_mode = mode;
        match mode {
            WindowMode::Editor => {
                crate::vibrancy::set(None, 0.0, 0.0);
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
                // winit keeps the fixed-size window's limits and restores them on Resizable(true)
                #[cfg(target_os = "linux")]
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::Vec2::ZERO));
                    ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(egui::Vec2::INFINITY));
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(true));
                // size the window to fit the screen and centre it explicitly (the generic
                // centre helper would use the picker's old size)
                let mon = ctx.input(|i| i.viewport().monitor_size).unwrap_or(egui::vec2(1920.0, 1080.0));
                let w = EDITOR_SIZE[0].min(mon.x - 40.0);
                let h = EDITOR_SIZE[1].min(mon.y - 80.0);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize([w, h].into()));
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition([((mon.x - w) / 2.0).max(0.0), ((mon.y - h) / 2.0 - 10.0).max(0.0)].into()));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.titlebar_fix = 3;
            }
            WindowMode::Picker => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
                ctx.send_viewport_cmd(egui::ViewportCommand::Title("ScreenForge".into()));
                Self::place_picker(ctx, PICKER_SIZE[0], self.picker_height);
            }
        }
    }

    /// Size the floating window and pin its bottom edge above the Dock, centred horizontally.
    pub fn place_picker(ctx: &egui::Context, width: f32, height: f32) {
        Self::set_fixed_size(ctx, width, height);
        if let Some(mon) = ctx.input(|i| i.viewport().monitor_size) {
            let x = ((mon.x - width) / 2.0).max(0.0);
            let y = (mon.y - height - PICKER_BOTTOM_GAP).max(0.0);
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition([x, y].into()));
        }
    }

    /// Resize a window that is not user-resizable. On X11, winit pins the min/max size hints
    /// to the window's size when it stops being resizable, and the window manager then ignores
    /// any other size; move the hints along with the size.
    pub fn set_fixed_size(ctx: &egui::Context, width: f32, height: f32) {
        #[cfg(target_os = "linux")]
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize([width, height].into()));
            ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize([width, height].into()));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize([width, height].into()));
    }

    fn tray_tick(&mut self, ctx: &egui::Context, open_request: &mut Option<PathBuf>) {
        // The status item is created on the first frame, after the app has finished launching:
        // creating it inside the launch callback crashed inside ImageIO on some systems.
        if !self.tray_tried {
            self.tray_tried = true;
            if std::env::var_os("SF_NO_TRAY").is_none() {
                self.tray = crate::tray::Tray::new();
            }
        }
        // menu bar icon: events and the recording timer
        if let Some(tray) = self.tray.as_mut() {
            let title = match &self.screen {
                Screen::Recording(r) => match &r.phase {
                    crate::recorder::Phase::Recording(rec) => {
                        let secs = (rec.elapsed_ms() / 1000.0) as u64;
                        Some(format!("● {}:{:02}", secs / 60, secs % 60))
                    }
                    _ => None,
                },
                _ => None,
            };
            tray.set_title(title);
            for a in tray.poll() {
                match a {
                    crate::tray::TrayAction::ShowPicker => {
                        if !matches!(self.screen, Screen::Recording(_)) {
                            self.show_picker(ctx);
                        }
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    crate::tray::TrayAction::OpenProject => {
                        if let Some(p) = rfd::FileDialog::new().add_filter("ScreenForge project", &["screenstudio"]).pick_folder() {
                            *open_request = Some(p);
                        }
                    }
                    crate::tray::TrayAction::Quit => {
                        self.quitting = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            // tray events arrive outside egui's input loop: keep polling at a low rate
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Switch to the editor window and show "Opening …"; the project loads a couple of frames
    /// later (`open_project`), so the window never sits empty while it does.
    fn begin_open(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.leave_editor();
        self.screen = Screen::Opening(path, 2);
        self.set_window_mode(ctx, WindowMode::Editor);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(String::new()));
        ctx.request_repaint();
    }

    pub fn open_project(&mut self, path: PathBuf, ctx: &egui::Context, rs: &egui_wgpu::RenderState) {
        match Editor::open(&path, self.gpu.clone(), rs) {
            Ok(mut ed) => {
                ed.fresh_recording = self.fresh_open.take().is_some_and(|f| f == path);
                self.prefs.touch(&path);
                self.screen = Screen::Editor(Box::new(ed));
                self.set_window_mode(ctx, WindowMode::Editor);
                // the editor paints its own centred title like Screen Studio
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(String::new()));
                let _ = ed_name(&path);
            }
            Err(e) => {
                if matches!(self.screen, Screen::Opening(..)) {
                    self.show_picker(ctx);
                }
                self.error = Some(format!("Could not open {}:\n{e:#}", path.display()));
            }
        }
    }

    /// Save the open project's unsaved changes before the editor is replaced or the app quits.
    fn leave_editor(&mut self) {
        if let Screen::Editor(ed) = &mut self.screen {
            ed.save_if_dirty();
        }
    }

    fn show_picker(&mut self, ctx: &egui::Context) {
        self.leave_editor();
        let mut picker = Picker::default();
        picker.load_inputs();
        self.screen = Screen::Picker(picker);
        self.picker_height = crate::picker::BAR_WINDOW_H;
        self.set_window_mode(ctx, WindowMode::Picker);
        // coming back from the (smaller) recording HUD keeps the picker mode, so re-place explicitly
        Self::place_picker(ctx, PICKER_SIZE[0], crate::picker::BAR_WINDOW_H);
    }
}

/// Placeholder while a project loads: its name and a spinner, centred on the editor background.
fn opening_ui(ui: &mut egui::Ui, path: &std::path::Path) {
    let pal = crate::theme::p();
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0.0, pal.bg);
    let c = rect.center();
    egui::Spinner::new().size(22.0).color(pal.text_dim).paint_at(ui, egui::Rect::from_center_size(egui::pos2(c.x, c.y - 22.0), egui::vec2(22.0, 22.0)));
    ui.painter().text(egui::pos2(c.x, c.y + 10.0), egui::Align2::CENTER_CENTER, format!("Opening {}", ed_name(path)), egui::FontId::proportional(14.0), pal.text_dim);
}

fn ed_name(p: &std::path::Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

impl eframe::App for App {
    fn on_exit(&mut self) {
        self.leave_editor();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        match self.window_mode {
            WindowMode::Picker => [0.0, 0.0, 0.0, 0.0],
            WindowMode::Editor => {
                let c = crate::theme::p().bg;
                [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0]
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        let rs = frame.wgpu_render_state().expect("wgpu").clone();

        let mut open_request: Option<PathBuf> = self.pending_open.take();
        crate::theme::sync(ctx);
        if let Some(setup) = self.perm_setup.as_mut() {
            if setup.show(ctx) {
                self.perm_setup = None;
                if let Screen::Picker(p) = &mut self.screen {
                    p.load_inputs();
                }
            }
        }
        if self.titlebar_fix > 0 && self.window_mode == WindowMode::Editor {
            self.titlebar_fix -= 1;
            if self.titlebar_fix == 0 {
                crate::vibrancy::transparent_titlebar();
            }
            ctx.request_repaint();
        }
        self.tray_tick(ctx, &mut open_request);
        ctx.input(|i| {
            for f in &i.raw.dropped_files {
                open_request = Some(f.path().to_path_buf());
            }
        });

        if !self.placed_once && matches!(self.screen, Screen::Picker(_)) {
            self.placed_once = true;
            Self::place_picker(ctx, PICKER_SIZE[0], self.picker_height);
        }
        let mut go_home = false;
        let mut action: Option<PickerAction> = None;
        let mut outcome: Option<RecordingOutcome> = None;
        let mut load_now: Option<PathBuf> = None;
        match &mut self.screen {
            Screen::Picker(p) => {
                // allow dragging the frameless window by its background
                let resp = ui.interact(ui.max_rect(), ui.id().with("drag"), egui::Sense::drag());
                if resp.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                action = p.ui(ui, &self.prefs);
                if (p.wanted_height - self.picker_height).abs() > 0.5 {
                    self.picker_height = p.wanted_height;
                    Self::place_picker(ctx, PICKER_SIZE[0], p.wanted_height);
                }
            }
            Screen::Recording(r) => {
                let resp = ui.interact(ui.max_rect(), ui.id().with("drag"), egui::Sense::drag());
                if resp.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                outcome = Some(r.ui(ui, ctx));
            }
            Screen::Editor(ed) => {
                if ed.ui(ui, &rs) {
                    if ed.fresh_recording {
                        self.confirm_close = Some(false);
                    } else {
                        go_home = true;
                    }
                }
                if let Some(p) = ed.renamed_to.take() {
                    self.prefs.recent.retain(|r| r.exists());
                    self.prefs.touch(&p);
                }
            }
            Screen::Opening(path, frames) => {
                opening_ui(ui, path);
                if *frames == 0 {
                    load_now = Some(path.clone());
                } else {
                    *frames -= 1;
                    ctx.request_repaint();
                }
            }
        }
        // Closing the editor window goes back to the recording toolbar (like its home button);
        // the app quits only from the toolbar or the menu bar icon.
        if !self.quitting && ctx.input(|i| i.viewport().close_requested()) {
            if let Screen::Editor(ed) = &self.screen {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if ed.export.is_busy() {
                    // keep the editor while an export runs
                } else if ed.fresh_recording {
                    self.confirm_close = Some(false);
                } else {
                    go_home = true;
                }
            }
        }
        match outcome {
            Some(RecordingOutcome::Finished(p)) => {
                self.fresh_open = Some(p.clone());
                open_request = Some(p);
            }
            Some(RecordingOutcome::Cancelled(err)) => {
                self.show_picker(ctx);
                if let Some(e) = err {
                    self.error = Some(e);
                }
            }
            _ => {}
        }
        if let (Some(quit), Screen::Editor(ed)) = (self.confirm_close, &mut self.screen) {
            let mut choice: Option<bool> = None; // Some(true) = keep, Some(false) = delete
            let mut cancel = false;
            egui::Window::new("Keep this recording?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.set_max_width(380.0);
                ui.label(egui::RichText::new(format!("“{}” was just recorded. Keep it in your projects, or move it to the Trash?", ed.session.pkg.project.name)).color(crate::theme::p().text));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if crate::widgets::accent_button(ui, "Keep").clicked() { choice = Some(true); }
                    if crate::widgets::secondary_button(ui, "Move to Trash").clicked() { choice = Some(false); }
                    if crate::widgets::secondary_button(ui, "Cancel").clicked() { cancel = true; }
                });
            });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { cancel = true; }
            if cancel {
                self.confirm_close = None;
            } else if let Some(keep) = choice {
                self.confirm_close = None;
                if keep {
                    ed.save();
                } else {
                    let path = ed.path.clone();
                    match crate::os::move_to_trash(&path) {
                        Ok(()) => {
                            self.prefs.recent.retain(|r| r != &path);
                            self.prefs.save();
                        }
                        Err(e) => self.error = Some(format!("Could not move the recording to the Trash: {e}")),
                    }
                }
                if let Screen::Editor(ed) = &mut self.screen { ed.fresh_recording = false; }
                if quit {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    go_home = true;
                }
            }
        }
        // editor Trash button: confirm, move the package to the Trash, back to the toolbar
        let mut removed: Option<String> = None;
        if let Screen::Editor(ed) = &mut self.screen {
            if ed.remove_requested {
                let name = ed.session.pkg.project.name.clone();
                let (mut confirm, mut cancel) = (false, false);
                egui::Window::new("Move to Trash?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                    ui.set_max_width(380.0);
                    ui.label(egui::RichText::new(format!("“{name}” and its recording will be moved to the Trash. You can put it back from the Trash in Finder.")).color(crate::theme::p().text));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if crate::widgets::accent_button(ui, "Move to Trash").clicked() { confirm = true; }
                        if crate::widgets::secondary_button(ui, "Cancel").clicked() { cancel = true; }
                    });
                });
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { cancel = true; }
                if cancel {
                    ed.remove_requested = false;
                } else if confirm {
                    ed.remove_requested = false;
                    let path = ed.path.clone();
                    match crate::os::move_to_trash(&path) {
                        Ok(()) => {
                            self.prefs.recent.retain(|r| r != &path);
                            self.prefs.save();
                            removed = Some(name);
                            go_home = true;
                        }
                        Err(e) => self.error = Some(format!("Could not move “{name}” to the Trash: {e}")),
                    }
                }
            }
        }
        if go_home {
            self.show_picker(ctx);
            if let (Some(name), Screen::Picker(p)) = (removed, &mut self.screen) {
                p.status = Some(format!("Moved “{name}” to the Trash"));
            }
        }
        match action {
            Some(PickerAction::OpenProject(p)) => open_request = Some(p),
            Some(PickerAction::OpenProjectDialog) => {
                if let Some(p) = rfd::FileDialog::new().add_filter("ScreenForge project", &["screenstudio"]).pick_folder() {
                    open_request = Some(p);
                }
            }
            Some(PickerAction::StartRecording(mut req)) => {
                if let Some(msg) = crate::recorder::check_permissions(&mut req) {
                    self.error = Some(msg);
                } else {
                    Self::place_picker(ctx, 520.0, 110.0);
                    self.screen = Screen::Recording(RecordingScreen::new(req));
                    crate::vibrancy::set(None, 0.0, 0.0);
                }
            }
            Some(PickerAction::SelectArea(mut req)) => {
                if let Some(msg) = crate::recorder::check_permissions(&mut req) {
                    self.error = Some(msg);
                } else {
                    let d = sf_recording::discover();
                    let display = d.main_display().or_else(|| d.displays.first());
                    if let Some(disp) = display {
                        let b = disp.bounds_points;
                        let screen = egui::Rect::from_min_size(egui::pos2(b.x as f32, b.y as f32), egui::vec2(b.width as f32, b.height as f32));
                        req.display_id = Some(disp.id.to_string());
                        self.area_select = Some((crate::area_select::AreaSelect::new(disp.id, screen), req));
                    }
                }
            }
            Some(PickerAction::SelectWindow(mut req)) => {
                if let Some(msg) = crate::recorder::check_permissions(&mut req) {
                    self.error = Some(msg);
                } else if crate::recorder::picks_window_in_portal() {
                    // Wayland: other windows cannot be listed; the screen-sharing dialog picks one
                    Self::place_picker(ctx, 520.0, 110.0);
                    self.screen = Screen::Recording(RecordingScreen::new(req));
                    crate::vibrancy::set(None, 0.0, 0.0);
                } else {
                    let d = sf_recording::discover();
                    if let Some(disp) = d.main_display().or_else(|| d.displays.first()) {
                        let b = disp.bounds_points;
                        let screen = egui::Rect::from_min_size(egui::pos2(b.x as f32, b.y as f32), egui::vec2(b.width as f32, b.height as f32));
                        req.display_id = Some(disp.id.to_string());
                        self.window_select = Some((crate::window_select::WindowSelect::new(screen), req));
                    }
                }
            }
            Some(PickerAction::SetAppearance(a)) => {
                self.prefs.appearance = a;
                self.prefs.save();
                a.apply(ctx);
            }
            Some(PickerAction::RemoveProject(p)) => {
                let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                match crate::os::move_to_trash(&p) {
                    Ok(()) => {
                        self.prefs.recent.retain(|r| r != &p);
                        self.prefs.save();
                        if let Screen::Picker(pk) = &mut self.screen {
                            pk.thumbs = None;
                            pk.status = Some(format!("Moved “{name}” to the Trash"));
                        }
                    }
                    Err(e) => self.error = Some(format!("Could not move “{name}” to the Trash: {e}")),
                }
            }
            Some(PickerAction::Quit) => {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            None => {}
        }
        if let Some((sel, _)) = self.window_select.as_mut() {
            match sel.show(ctx) {
                crate::window_select::WindowOutcome::Pending => {}
                crate::window_select::WindowOutcome::Cancelled => self.window_select = None,
                crate::window_select::WindowOutcome::Done(w) => {
                    let (_, mut req) = self.window_select.take().unwrap();
                    let bounds = crate::recorder::activate_and_locate(w.pid, w.id).unwrap_or(w.bounds);
                    req.area = Some(bounds);
                    req.window_id = Some(w.id.to_string());
                    Self::place_picker(ctx, 520.0, 110.0);
                    self.screen = Screen::Recording(RecordingScreen::new(req));
                    crate::vibrancy::set(None, 0.0, 0.0);
                }
            }
        }
        if let Some((sel, _)) = self.area_select.as_mut() {
            match sel.show(ctx) {
                crate::area_select::AreaOutcome::Pending => {}
                crate::area_select::AreaOutcome::Cancelled => self.area_select = None,
                crate::area_select::AreaOutcome::Done(r) => {
                    let (_, mut req) = self.area_select.take().unwrap();
                    let area = sf_project::schema::common::Rect { x: r.min.x as f64, y: r.min.y as f64, width: r.width() as f64, height: r.height() as f64 };
                    req.area = Some(area);
                    Self::place_picker(ctx, 520.0, 110.0);
                    self.screen = Screen::Recording(RecordingScreen::new(req));
                    crate::vibrancy::set(None, 0.0, 0.0);
                }
            }
        }
        if let Some(p) = load_now {
            self.open_project(p, ctx, &rs);
        }
        if let Some(p) = open_request {
            self.begin_open(p, ctx);
        }

        if let Some(err) = self.error.clone() {
            let mut open = true;
            let help = crate::recorder::permission_help(&err);
            let title = if help.is_some() { "Permission needed" } else { "Error" };
            egui::Window::new(title).collapsible(false).resizable(false).open(&mut open).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.set_max_width(420.0);
                match help {
                    Some((text, perm)) => {
                        ui.label(text);
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Open System Settings").clicked() {
                                let _ = sf_recording::permissions::open_settings(perm);
                                self.error = None;
                            }
                            if ui.button("Cancel").clicked() {
                                self.error = None;
                            }
                        });
                    }
                    None => {
                        ui.label(err);
                        if ui.button("OK").clicked() {
                            self.error = None;
                        }
                    }
                }
            });
            if !open {
                self.error = None;
            }
        }
    }
}
