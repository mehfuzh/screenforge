//! Export dialog styled after Screen Studio's, plus the background export thread.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;

use egui::RichText;
use sf_media::{QualityPreset, VideoCodec};

use crate::editor::Editor;
use crate::theme;
use crate::widgets;

/// Content width of the export dialog.
pub const DIALOG_W: f32 = 440.0;
/// Content width of the export progress / result dialog.
pub const PROGRESS_W: f32 = 340.0;

pub enum Msg {
    Progress(sf_export::Progress),
    Done(Result<PathBuf, String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mp4,
    Gif,
}

pub struct ExportDialog {
    pub open: bool,
    pub license: sf_license::Status,
    pub format: Format,
    pub fps: f64,
    pub height: u32,
    pub quality: QualityPreset,
    pub codec: VideoCodec,
    pub audio: bool,
    job: Option<(Receiver<Msg>, JoinHandle<()>, Arc<AtomicBool>)>,
    /// Email used at checkout; the backend looks the key up by it.
    pub email: String,
    /// Result of the last activation attempt (background thread).
    activation: Option<Receiver<Result<sf_license::Activation, String>>>,
    /// The quiet check-in started when the dialog was created (see `sf_license::check_in`).
    check_in: Option<Receiver<Result<Option<bool>, String>>>,
    activation_msg: Option<(bool, String)>,
    /// A code was emailed for this address; the dialog asks for it.
    code_sent_to: Option<String>,
    code: String,
    /// User opened the license section while free exports remain.
    show_license: bool,
    /// The free-export count was reconciled with the backend since the dialog last opened.
    free_exports_synced: bool,
    progress: Option<sf_export::Progress>,
    result: Option<Result<PathBuf, String>>,
    started: Option<std::time::Instant>,
}

/// An empty dialog: no file reads, no network. `std::mem::take` puts one in place every frame
/// while the real dialog is borrowed, so this must stay free; [`ExportDialog::new`] is the one
/// that loads the license and talks to the backend.
impl Default for ExportDialog {
    fn default() -> Self {
        ExportDialog { open: false, license: sf_license::Status::Invalid("no key".into()), email: String::new(), activation: None, check_in: None, activation_msg: None, code_sent_to: None, code: String::new(), show_license: false, free_exports_synced: false, format: Format::Mp4, fps: 60.0, height: 1080, quality: QualityPreset::SocialMedia, codec: VideoCodec::H264, audio: true, job: None, progress: None, result: None, started: None }
    }
}

impl ExportDialog {
    /// The dialog for a newly opened editor: loads the saved license and starts the one quiet
    /// check-in with the backend (or the free-export count sync without a license).
    pub fn new() -> Self {
        let license_key = sf_license::load_saved().unwrap_or_default();
        let license = if license_key.is_empty() { sf_license::Status::Invalid("no key".into()) } else { sf_license::verify(&license_key) };
        let email = sf_license::load_email().unwrap_or_default();
        // a key on this computer: check in quietly (never emails a code). A renewed monthly key
        // is saved; a license moved to another computer is removed here. Offline, nothing
        // changes.
        let check_in = if !license_key.is_empty() && !email.is_empty() {
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let _ = tx.send(sf_license::check_in());
            });
            Some(rx)
        } else {
            None
        };
        if !license.is_valid() {
            std::thread::spawn(sf_license::sync_free_exports);
        }
        ExportDialog { license, email, check_in, ..Default::default() }
    }
}

const MP4_FPS: [f64; 7] = [60.0, 50.0, 30.0, 25.0, 24.0, 20.0, 10.0];
const GIF_FPS: [f64; 6] = [50.0, 30.0, 25.0, 20.0, 15.0, 10.0];
const MP4_RES: [(u32, &str); 3] = [(720, "720p"), (1080, "1080p"), (2160, "4K")];
const GIF_RES: [(u32, &str); 3] = [(480, "480p"), (720, "720p"), (1080, "1080p")];

impl ExportDialog {
    /// Export or result pending: the app shows the compact export window instead of the editor.
    pub fn is_busy(&self) -> bool {
        self.job.is_some() || self.result.is_some()
    }

    /// Drain background messages (export progress, activation) and handle Escape.
    pub fn poll(&mut self, ctx: &egui::Context) {
        // each time the dialog opens, pick up free exports used on this device from elsewhere
        // (a previous install, a wiped support folder) before the count is shown
        if self.open && !self.free_exports_synced {
            self.free_exports_synced = true;
            if !self.license.is_valid() {
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    sf_license::sync_free_exports();
                    ctx.request_repaint();
                });
            }
        } else if !self.open {
            self.free_exports_synced = false;
        }
        if let Some(rx) = &self.check_in {
            match rx.try_recv() {
                Ok(r) => {
                    self.check_in = None;
                    let had_valid = self.license.is_valid();
                    // the key file was renewed or removed: read it again
                    self.license = sf_license::load_saved().map(|k| sf_license::verify(&k)).unwrap_or_else(|| sf_license::Status::Invalid("no key".into()));
                    if matches!(r, Ok(Some(false))) && had_valid && !self.license.is_valid() {
                        self.activation_msg = Some((false, "This license was moved to another computer. Press Activate to move it back here.".into()));
                        self.show_license = true;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(500)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.check_in = None,
            }
        }
        if let Some(rx) = &self.activation {
            if let Ok(r) = rx.try_recv() {
                match r {
                    Ok(sf_license::Activation::CodeSent) => {
                        self.code_sent_to = Some(self.email.trim().to_string());
                        self.code.clear();
                        self.activation_msg = Some((true, format!("We emailed a 6-digit code to {}. Enter it below to activate this computer.", self.email.trim())));
                    }
                    Ok(sf_license::Activation::NeedsCode) => {}
                    Ok(sf_license::Activation::Done(status)) => {
                        self.code_sent_to = None;
                        self.code.clear();
                        self.license = status.clone();
                        self.activation_msg = Some(match status {
                            sf_license::Status::Valid(_) => (true, "License activated on this Mac.".into()),
                            sf_license::Status::Expired(p) => (false, format!("Your monthly key ended on {}. A lifetime key ({}) keeps exporting.", p.expires.map(|d| d.to_string()).unwrap_or_default(), sf_license::PRICE_LIFETIME)),
                            sf_license::Status::Invalid(e) => (false, format!("The server sent a key this build cannot verify ({e}).")),
                        });
                    }
                    Err(e) if e == "no license" => self.activation_msg = Some((false, "No license for that email yet. Buy one below, then activate again.".into())),
                    Err(e) => self.activation_msg = Some((false, e)),
                }
                self.activation = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
        if (self.open || self.job.is_some()) && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Some((_, _, cancel)) = &self.job {
                cancel.store(true, Ordering::Relaxed);
            } else {
                self.open = false;
                self.result = None;
            }
        }
        if let Some((rx, _, _)) = &self.job {
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Progress(p) => self.progress = Some(p),
                    Msg::Done(r) => {
                        if r.is_ok() && !self.license.is_valid() {
                            let _ = sf_license::record_free_export();
                        }
                        // a cancelled export just returns to the dialog
                        self.result = match r {
                            Err(e) if e.contains("cancelled") => None,
                            r => Some(r),
                        };
                    }
                }
            }
            if self.result.is_some() || self.job.as_ref().is_some_and(|(_, h, _)| h.is_finished()) {
                self.job = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }

    /// Progress / result shown in the export dialog while `is_busy()`.
    pub fn compact_ui(&mut self, ui: &mut egui::Ui) {
        let pal = theme::p();
        if self.job.is_some() {
            let p = self.progress;
            let frac = p.map(|p| p.frame as f32 / p.total_frames.max(1) as f32).unwrap_or(0.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Exporting…").size(15.0).strong().color(pal.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{:.0}%", frac * 100.0)).size(13.0).color(pal.text_dim));
                });
            });
            ui.add_space(6.0);
            // full-width bar painted by hand (egui's ProgressBar shrinks to its label here)
            let (bar, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
            ui.painter().rect_filled(bar, egui::CornerRadius::same(2), pal.card_3);
            let fill = egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * frac.clamp(0.0, 1.0), bar.height()));
            ui.painter().rect_filled(fill, egui::CornerRadius::same(2), pal.accent);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                match (p, self.started) {
                    (Some(_), Some(t)) if frac > 0.02 => {
                        let el = t.elapsed().as_secs_f64();
                        widgets::hint(ui, &format!("About {:.0}s left", el / frac as f64 - el));
                    }
                    _ => widgets::hint(ui, "Preparing…"),
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::secondary_button(ui, "Cancel").on_hover_text("Esc").clicked() {
                        if let Some((_, _, cancel)) = &self.job {
                            cancel.store(true, Ordering::Relaxed);
                        }
                    }
                });
            });
            return;
        }
        if let Some(r) = self.result.clone() {
            match &r {
                Ok(p) => {
                    ui.label(RichText::new("Export complete").size(15.0).strong().color(pal.text));
                    ui.add_space(2.0);
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    ui.label(RichText::new(name).size(13.0).color(pal.text_dim)).on_hover_text(p.display().to_string());
                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::accent_button(ui, "Done").clicked() {
                            self.result = None;
                        }
                        if widgets::secondary_button(ui, &format!("Show in {}", crate::os::FILE_MANAGER)).clicked() {
                            crate::os::reveal(p);
                        }
                    });
                }
                Err(e) => {
                    ui.colored_label(pal.danger, RichText::new("Export failed").size(15.0).strong());
                    ui.add_space(2.0);
                    ui.label(e);
                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::accent_button(ui, "Close").clicked() {
                            self.result = None;
                        }
                    });
                }
            }
        }
    }

    /// Export settings, drawn in the export dialog over the editor. Escape, Cancel or a click
    /// outside dismisses it.
    pub fn settings_ui(&mut self, ed: &mut Editor, ui: &mut egui::Ui) {
            ui.label(RichText::new("Export").size(18.0).strong());
            ui.add_space(4.0);
            let left = sf_license::free_exports_left();
            if !self.license.is_valid() && left > 0 && !self.show_license {
                // free exports remain: one line, no form
                ui.horizontal(|ui| {
                    widgets::hint(ui, &format!("{left} of {} free exports left.", sf_license::FREE_EXPORTS));
                    if ui.link(RichText::new("Get a license").size(12.5).color(theme::p().accent_hover)).clicked() {
                        self.show_license = true;
                    }
                });
            }
            if !self.license.is_valid() && (left == 0 || self.show_license) {
                widgets::title(ui, "License");
                if left > 0 {
                    widgets::hint(ui, &format!("{left} free exports left. A license removes the limit ({}).", sf_license::PRICE_LABEL));
                } else {
                    widgets::hint(ui, &format!("Your {} free exports are used up. Exporting needs a ScreenForge license ({}).", sf_license::FREE_EXPORTS, sf_license::PRICE_LABEL));
                }
                ui.add_space(4.0);
                ui.label("Email used at checkout");
                ui.add_space(2.0);
                widgets::text_field(ui, &mut self.email, "you@example.com");
                // a code belongs to the address it was sent to
                let awaiting_code = self.code_sent_to.as_deref() == Some(self.email.trim());
                if awaiting_code {
                    ui.add_space(6.0);
                    ui.label("Code from the email");
                    ui.add_space(2.0);
                    widgets::text_field(ui, &mut self.code, "123456");
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let busy = self.activation.is_some();
                    let start = |this: &mut Self, f: Box<dyn FnOnce() -> Result<sf_license::Activation, String> + Send>| {
                        let (tx, rx) = channel();
                        std::thread::spawn(move || {
                            let _ = tx.send(f());
                        });
                        this.activation = Some(rx);
                        this.activation_msg = None;
                    };
                    if awaiting_code {
                        if ui.add_enabled_ui(!busy, |ui| widgets::secondary_button(ui, if busy { "Checking…" } else { "Confirm" })).inner.clicked() {
                            let (email, code) = (self.email.clone(), self.code.clone());
                            start(self, Box::new(move || sf_license::confirm_code(&email, &code).map(sf_license::Activation::Done)));
                        }
                        if ui.add_enabled_ui(!busy, |ui| widgets::secondary_button(ui, "Send again")).inner.clicked() {
                            let email = self.email.clone();
                            start(self, Box::new(move || sf_license::activate(&email, true)));
                        }
                    } else if ui.add_enabled_ui(!busy, |ui| widgets::secondary_button(ui, if busy { "Activating…" } else { "Activate" })).inner.clicked() {
                        let email = self.email.clone();
                        start(self, Box::new(move || sf_license::activate(&email, true)));
                    }
                    if widgets::accent_button(ui, &format!("Buy · {}", sf_license::PRICE_LIFETIME)).clicked() {
                        open_checkout(&self.email, "lifetime");
                    }
                });
                if let Some((ok, msg)) = &self.activation_msg {
                    ui.colored_label(if *ok { theme::p().accent_hover } else { theme::p().danger }, msg);
                }
                widgets::hint(ui, "After paying, press Activate: we email you a code to confirm this computer, then the key is stored on it. Nothing to copy. A license works on one computer at a time; activating another computer moves it there.");
                ui.add_space(6.0);
                ui.separator();
            }
            widgets::title(ui, "Format");
            if let Some(i) = widgets::segmented(ui, if self.format == Format::Mp4 { 0 } else { 1 }, &["MP4", "GIF"]) {
                self.format = if i == 0 { Format::Mp4 } else { Format::Gif };
                if self.format == Format::Gif {
                    self.fps = 15.0;
                    self.height = 480;
                    self.quality = QualityPreset::Studio;
                } else {
                    self.fps = 60.0;
                    self.height = 1080;
                    self.quality = QualityPreset::SocialMedia;
                }
            }
            widgets::title(ui, "Frame rate");
            let fps_list: &[f64] = if self.format == Format::Mp4 { &MP4_FPS } else { &GIF_FPS };
            let labels: Vec<String> = fps_list.iter().map(|f| format!("{f}")).collect();
            let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            if let Some(i) = widgets::segmented(ui, fps_list.iter().position(|f| *f == self.fps).unwrap_or(0), &label_refs) {
                self.fps = fps_list[i];
            }
            widgets::title(ui, "Resolution");
            let res: &[(u32, &str)] = if self.format == Format::Mp4 { &MP4_RES } else { &GIF_RES };
            let rl: Vec<&str> = res.iter().map(|r| r.1).collect();
            if let Some(i) = widgets::segmented(ui, res.iter().position(|r| r.0 == self.height).unwrap_or(1), &rl) {
                self.height = res[i].0;
            }
            let size = ed.session.output_size(self.height);
            widgets::hint(ui, &format!("{} px × {} px", size.width, size.height));
            widgets::title(ui, "Compression");
            let qs: &[QualityPreset] = if self.format == Format::Mp4 { &QualityPreset::ALL } else { &QualityPreset::ALL[..2] };
            let ql: Vec<&str> = qs.iter().map(|q| q.label()).collect();
            if let Some(i) = widgets::segmented(ui, qs.iter().position(|q| *q == self.quality).unwrap_or(1), &ql) {
                self.quality = qs[i];
            }
            if self.format == Format::Mp4 {
                egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
                    widgets::title(ui, "Codec");
                    let codecs = [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::ProRes4444, VideoCodec::Vp9];
                    if let Some(i) = widgets::segmented(ui, codecs.iter().position(|c| *c == self.codec).unwrap_or(0), &["H.264", "HEVC", "ProRes", "WebM"]) {
                        self.codec = codecs[i];
                    }
                    ui.checkbox(&mut self.audio, "Include audio");
                });
            }
            ui.add_space(6.0);
            let secs = ed.duration_ms() / 1000.0;
            widgets::hint(ui, &format!("{:.0}s of video at {} fps", secs, self.fps));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let licensed = self.license.is_valid() || sf_license::free_exports_left() > 0;
                    let btn = ui.add_enabled_ui(licensed, |ui| widgets::accent_button(ui, "Export to file…")).inner;
                    if btn.clicked() {
                        let ext = match (self.format, self.codec) {
                            (Format::Gif, _) => "gif",
                            (_, VideoCodec::ProRes4444) => "mov",
                            (_, VideoCodec::Vp9) => "webm",
                            _ => "mp4",
                        };
                        let default_name = format!("{}.{ext}", ed.session.pkg.project.name);
                        // SF_EXPORT_OUT: UI tests skip the native save panel
                        let out = match std::env::var_os("SF_EXPORT_OUT") {
                            Some(p) => Some(PathBuf::from(p)),
                            None => rfd::FileDialog::new().set_file_name(&default_name).add_filter(ext, &[ext]).save_file(),
                        };
                        if let Some(out) = out {
                            self.start(ed, out);
                        }
                    }
                    if widgets::secondary_button(ui, "Cancel").clicked() {
                        self.open = false;
                    }
                });
            });
    }

    fn start(&mut self, ed: &mut Editor, out: PathBuf) {
        let path = ed.path.clone();
        ed.save();
        let codec = if self.format == Format::Gif { VideoCodec::Gif } else { self.codec };
        let quality = match self.quality {
            QualityPreset::Studio => 0.95,
            QualityPreset::SocialMedia => 0.75,
            QualityPreset::WebHigh => 0.45,
            QualityPreset::WebLow => 0.2,
        };
        let opts = sf_export::ExportOptions { output: out, fps: self.fps, max_height: self.height, codec, quality, include_audio: self.audio, range_ms: None };
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel2 = cancel.clone();
        let handle = std::thread::spawn(move || {
            let run = || -> anyhow::Result<PathBuf> {
                let gpu = sf_render::Gpu::headless()?;
                let mut session = sf_session::Session::open(&path, gpu.clone())?;
                let mut comp = sf_render::Compositor::new(gpu);
                let tx2 = tx.clone();
                sf_export::export_cancellable(&mut session, &mut comp, &opts, &cancel2, move |p| {
                    let _ = tx2.send(Msg::Progress(p));
                })
            };
            let _ = tx.send(Msg::Done(run().map_err(|e| format!("{e:#}"))));
        });
        self.started = Some(std::time::Instant::now());
        self.progress = None;
        self.result = None;
        self.job = Some((rx, handle, cancel));
        self.open = false;
    }
}

/// Open Stripe checkout in the browser; falls back to the pricing page when the backend is not
/// configured or unreachable.
fn open_checkout(email: &str, plan: &str) {
    let email = email.to_string();
    let plan = plan.to_string();
    std::thread::spawn(move || {
        let url = sf_license::checkout_url(&email, &plan).unwrap_or_else(|_| sf_license::PURCHASE_URL.to_string());
        crate::os::open(url);
    });
}
