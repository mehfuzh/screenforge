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
    /// "Already bought? Sign in": the email and code form is open.
    signing_in: bool,
    /// Coupon for the checkout, and whether its field is open.
    promo: String,
    promo_open: bool,
    /// A checkout being opened: its URL, or why it failed (an unknown coupon).
    checkout: Option<Receiver<Result<String, String>>>,
    /// A checkout started here, polled silently until then: once paid it activates this
    /// computer by itself (see `sf_license::claim_checkout`).
    claim_until: Option<std::time::Instant>,
    next_claim: Option<std::time::Instant>,
    claim: Option<Receiver<Result<sf_license::Claim, String>>>,
    /// Free exports left on this computer without a license: asked each time the dialog opens
    /// (`Err`: the server could not be reached, so there is none).
    free_left: Option<Result<u32, String>>,
    free_rx: Option<Receiver<Result<u32, String>>>,
    /// The dialog was open last frame (opening it refreshes the free exports).
    was_open: bool,
    /// `sf_license::license_generation` the license was last read at: a key saved or removed
    /// elsewhere (the agent panel's upgrade) reads it again.
    license_seen: u64,
    /// The export running now is a free one: counted once it finishes.
    free_job: bool,
    progress: Option<sf_export::Progress>,
    result: Option<Result<PathBuf, String>>,
    started: Option<std::time::Instant>,
    /// A share to make (an export with "Make a share link too" finished): the app takes this
    /// and starts the share job.
    pub share_request: Option<crate::share::Source>,
    /// This project's share job, while one runs: 0..1 and what it is doing (set by the app
    /// every frame).
    pub share_running: Option<(f32, String)>,
    /// Set by Cancel while a share job runs; the app stops the job.
    pub share_cancel: bool,
    /// This project's last share link, or why it failed (set by the app).
    pub last_link: Option<Result<String, String>>,
    /// "Make a share link too": Export uploads what it wrote (Pro; `share::with_export_enabled`).
    share_with_export: bool,
    /// The running or finished export makes a link too: step 2 of the progress dialog.
    export_shared: bool,
    /// This project's link as the server has it (one per project), shown whenever the dialog
    /// is open; asked again each time it opens. `project_link_for`: the project asked about.
    project_link: Option<String>,
    /// Whether the server has answered for this project (so "nothing uploaded yet" is known).
    project_link_known: bool,
    project_link_for: Option<String>,
    /// The editor's project id (set by the editor every frame): its link is known without
    /// asking the server (`share::link_url`).
    pub project_id: String,
    /// The QR code under the link is showing.
    qr_open: bool,
    project_link_rx: Option<Receiver<Result<Option<String>, String>>>,
}

/// An empty dialog: no file reads, no network. `std::mem::take` puts one in place every frame
/// while the real dialog is borrowed, so this must stay free; [`ExportDialog::new`] is the one
/// that loads the license and talks to the backend.
impl Default for ExportDialog {
    fn default() -> Self {
        ExportDialog { open: false, license: sf_license::Status::Invalid("no key".into()), email: String::new(), activation: None, check_in: None, activation_msg: None, code_sent_to: None, code: String::new(), signing_in: false, promo: String::new(), promo_open: false, checkout: None, claim_until: None, next_claim: None, claim: None, free_left: None, free_rx: None, was_open: false, license_seen: 0, free_job: false, format: Format::Mp4, fps: 60.0, height: 1080, quality: QualityPreset::SocialMedia, codec: VideoCodec::H264, audio: true, job: None, progress: None, result: None, started: None, share_request: None, share_running: None, share_cancel: false, last_link: None, share_with_export: false, export_shared: false, project_link: None, project_link_known: false, project_link_for: None, project_id: String::new(), qr_open: false, project_link_rx: None }
    }
}

impl ExportDialog {
    /// The dialog for a newly opened editor: loads the saved license and starts the one quiet
    /// check-in with the backend.
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
        // a checkout from an earlier session, paid after quitting: look for it a while
        let mut d = ExportDialog { license, email, check_in, license_seen: sf_license::license_generation(), share_with_export: crate::share::with_export_enabled(), ..Default::default() };
        // known before the dialog first opens
        if !d.license.is_valid() {
            d.fetch_free_exports();
        }
        if !d.license.is_valid() && sf_license::pending_claim() {
            d.watch_claim(10 * 60);
            d.next_claim = Some(std::time::Instant::now());
        }
        d
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

    fn watch_claim(&mut self, secs: u64) {
        let now = std::time::Instant::now();
        self.claim_until = Some(now + std::time::Duration::from_secs(secs));
        self.next_claim = Some(now + std::time::Duration::from_secs(3));
    }

    /// Open checkout for `plan`; once paid this computer is activated by itself.
    fn buy(&mut self, plan: &str) {
        let (tx, rx) = channel();
        let (plan, promo) = (plan.to_string(), self.promo.trim().to_string());
        let email = sf_license::load_email();
        std::thread::spawn(move || {
            let _ = tx.send(sf_license::start_checkout(&plan, email.as_deref(), Some(&promo)));
        });
        self.checkout = Some(rx);
        self.activation_msg = None;
    }

    /// Checkout links and the silent check for a paid checkout.
    fn poll_checkout(&mut self, ctx: &egui::Context) {
        let now = std::time::Instant::now();
        if let Some(rx) = &self.checkout {
            match rx.try_recv() {
                Ok(Ok(url)) => {
                    crate::os::open(url);
                    self.checkout = None;
                    self.watch_claim(30 * 60);
                }
                Ok(Err(e)) => {
                    self.checkout = None;
                    self.activation_msg = Some((false, e));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.checkout = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
            }
        }
        if let Some(rx) = &self.claim {
            let r = match rx.try_recv() {
                Ok(r) => Some(r),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("no checkout".into())),
            };
            if let Some(r) = r {
                self.claim = None;
                match r {
                    Ok(sf_license::Claim::Done(status)) => {
                        self.claim_until = None;
                        self.promo.clear();
                        self.promo_open = false;
                        self.signing_in = false;
                        self.activation_msg = Some(if status.is_valid() { (true, "Payment received. Export is unlocked on this computer.".into()) } else { (false, format!("That license is not valid here: {status:?}")) });
                        self.license = status;
                    }
                    Ok(sf_license::Claim::Pending) => self.next_claim = Some(now + std::time::Duration::from_secs(3)),
                    Ok(sf_license::Claim::Verify(email)) => {
                        // the address had a license before: confirm it with the emailed code
                        self.claim_until = None;
                        self.signing_in = true;
                        self.email = email.clone();
                        let (tx, rx) = channel();
                        std::thread::spawn(move || {
                            let _ = tx.send(sf_license::activate(&email, true));
                        });
                        self.activation = Some(rx);
                    }
                    Err(e) if e == "no checkout" => self.claim_until = None,
                    Err(e) if e.contains("already used") => {
                        self.claim_until = None;
                        self.activation_msg = Some((false, e));
                    }
                    // offline or a server hiccup: keep trying, slower
                    Err(_) => self.next_claim = Some(now + std::time::Duration::from_secs(10)),
                }
            }
        }
        // the agent panel's sign-in or its own claim may have unlocked export meanwhile
        if self.claim_until.is_some_and(|t| now > t) || self.license.is_valid() || self.signing_in {
            self.claim_until = None;
        }
        if self.claim_until.is_some() {
            if self.claim.is_none() && self.next_claim.is_some_and(|t| now >= t) {
                self.next_claim = None;
                let (tx, rx) = channel();
                std::thread::spawn(move || {
                    let _ = tx.send(sf_license::claim_checkout());
                });
                self.claim = Some(rx);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }

    /// Free exports left: asked when the dialog opens without a license, forgotten when it
    /// closes so the next open asks again.
    fn poll_free_exports(&mut self, ctx: &egui::Context) {
        // asked when the editor opens and again on each open, keeping the last answer
        // meanwhile so the dialog does not change size as it appears
        let opened = self.open && !self.was_open;
        self.was_open = self.open;
        if !self.license.is_valid() && self.free_rx.is_none() && (opened || (self.open && self.free_left.is_none())) {
            self.fetch_free_exports();
        }
        if let Some(rx) = &self.free_rx {
            match rx.try_recv() {
                Ok(r) => {
                    self.free_rx = None;
                    self.free_left = Some(r);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.free_rx = None,
            }
        }
    }

    fn fetch_free_exports(&mut self) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(sf_license::free_exports_left());
        });
        self.free_rx = Some(rx);
    }

    /// Exporting is allowed: a license, or a free export left (known from the server).
    fn can_export(&self) -> bool {
        self.license.is_valid() || matches!(self.free_left, Some(Ok(n)) if n > 0)
    }

    /// Drain background messages (export progress, activation) and handle Escape.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let generation = sf_license::license_generation();
        if generation != self.license_seen {
            self.license_seen = generation;
            self.license = sf_license::load_saved().map(|k| sf_license::verify(&k)).unwrap_or_else(|| sf_license::Status::Invalid("no key".into()));
        }
        self.poll_checkout(ctx);
        if self.open && !self.was_open {
            // asked again (the link may have been made or deleted elsewhere); the last answer
            // shows meanwhile
            self.project_link_for = None;
        }
        self.poll_free_exports(ctx);
        if let Some(rx) = &self.project_link_rx {
            match rx.try_recv() {
                Ok(r) => {
                    self.project_link_rx = None;
                    // offline or a server error: keep what was shown
                    if let Ok(link) = r {
                        // a link made before ids were derived from the project counts as
                        // nothing uploaded: the next share moves the project to its own link
                        let asked = self.project_link_for.clone().unwrap_or_default();
                        let own = crate::share::link_url(&asked);
                        self.project_link = link.filter(|l| *l == own);
                        crate::share::remember_uploaded(&asked, self.project_link.is_some());
                        self.project_link_known = true;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.project_link_rx = None,
            }
        }
        if let Some(rx) = &self.check_in {
            match rx.try_recv() {
                Ok(r) => {
                    self.check_in = None;
                    let had_valid = self.license.is_valid();
                    // the key file was renewed or removed: read it again
                    self.license = sf_license::load_saved().map(|k| sf_license::verify(&k)).unwrap_or_else(|| sf_license::Status::Invalid("no key".into()));
                    if matches!(r, Ok(Some(false))) && had_valid && !self.license.is_valid() {
                        // the license form shows whenever there is no valid license
                        self.signing_in = true;
                        self.activation_msg = Some((false, "This license was moved to another computer. Sign in to move it back here.".into()));
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
                    Err(e) if e == "no license" => self.activation_msg = Some((false, "No license for that email. Buy Pro or Creator above, or use the email you paid with.".into())),
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
                        // a finished free export is counted (kept on disk until online)
                        // the server's answer replaces the count (a cancelled or failed one
                        // gives back the export taken off when it started)
                        if std::mem::take(&mut self.free_job) {
                            let ok = r.is_ok();
                            let (tx, rx) = channel();
                            std::thread::spawn(move || {
                                let _ = tx.send(if ok { sf_license::record_free_export() } else { sf_license::free_exports_left() });
                            });
                            self.free_rx = Some(rx);
                        }
                        // "Make a share link too": the file just written goes up as it is
                        // when browsers play it (H.264 MP4); a GIF, ProRes or VP9 export is
                        // rendered for the link instead
                        if let Ok(out) = &r {
                            if self.export_shared {
                                let as_is = self.format == Format::Mp4 && self.codec == VideoCodec::H264;
                                self.share_request = Some(if as_is { crate::share::Source::File(out.clone()) } else { crate::share::Source::Render(self.share_settings()) });
                                self.share_running = Some((0.0, "Starting…".into()));
                            }
                        }
                        // a cancelled export just returns to the dialog
                        self.result = match r {
                            Err(e) if e.contains("cancelled") => {
                                self.export_shared = false;
                                None
                            }
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

    /// Progress / result shown in the export dialog while `is_busy()`: a preview of the video,
    /// then the export and (with "Make a share link too") its share link, as two numbered steps.
    /// `preview`: the editor's frame (texture, width / height).
    pub fn compact_ui(&mut self, ui: &mut egui::Ui, preview: Option<(egui::TextureId, f32)>) {
        let pal = theme::p();
        if let Some(Err(e)) = self.result.clone() {
            ui.colored_label(pal.danger, RichText::new("Export failed").size(15.0).strong());
            ui.add_space(2.0);
            ui.label(e);
            ui.add_space(10.0);
            buttons_row(ui, |ui| {
                if widgets::accent_button(ui, "Close").clicked() {
                    self.result = None;
                    self.export_shared = false;
                }
            });
            return;
        }
        let exporting = self.job.is_some();
        let sharing = self.export_shared;
        let title = if exporting {
            "Exporting…"
        } else if sharing && self.share_running.is_some() {
            "Making the share link…"
        } else {
            "Export complete"
        };
        ui.label(RichText::new(title).size(15.0).strong().color(pal.text));
        ui.add_space(8.0);
        if let Some((tex, aspect)) = preview {
            // what is being exported, as wide as the box (at most 220 px tall)
            let w = ui.available_width();
            let h = (w / aspect.max(0.1)).min(220.0);
            let size = egui::vec2(h * aspect, h).min(egui::vec2(w, h));
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::new((tex, size)).corner_radius(4));
            });
            ui.add_space(12.0);
        }

        // 1. the video file
        let p = self.progress;
        let frac = p.map(|p| p.frame as f32 / p.total_frames.max(1) as f32).unwrap_or(0.0);
        step_header(ui, 1, "Video", if exporting { StepState::Active } else { StepState::Done }, exporting.then(|| format!("{:.0}%", frac * 100.0)));
        step_body(ui, |ui| {
            if exporting {
                progress_bar(ui, frac);
                ui.add_space(2.0);
                match (p, self.started) {
                    (Some(_), Some(t)) if frac > 0.02 => {
                        let el = t.elapsed().as_secs_f64();
                        widgets::hint(ui, &format!("About {:.0}s left", el / frac as f64 - el));
                    }
                    _ => widgets::hint(ui, "Preparing…"),
                }
            } else if let Some(Ok(out)) = self.result.clone() {
                ui.horizontal(|ui| {
                    let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    ui.label(RichText::new(name).size(13.0).color(pal.text)).on_hover_text(out.display().to_string());
                    if widgets::icon_button(ui, widgets::Icon::Folder, 13.0, &format!("Show in {}", crate::os::FILE_MANAGER)).clicked() {
                        crate::os::reveal(&out);
                    }
                });
                // written after the video by this export (an older list of a project that
                // has no chapters now is not mentioned)
                let txt = sf_export::chapters_txt_path(&out);
                let modified = |f: &std::path::Path| std::fs::metadata(f).and_then(|m| m.modified()).ok();
                if modified(&txt).zip(modified(&out)).is_some_and(|(t, v)| t >= v) {
                    let txt_name = txt.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    widgets::hint(ui, &format!("Chapters are in the video, and listed for a YouTube description in {txt_name}."));
                }
            }
        });

        // 2. its share link: made by this export, or the project's link from before (shown
        // so it is always at hand, with its QR code)
        if sharing || (crate::share::allowed() && self.uploaded() == Some(true)) {
            ui.add_space(10.0);
            let state = match (&self.share_running, &self.last_link) {
                _ if !sharing => StepState::Done,
                _ if exporting => StepState::Waiting,
                (Some(_), _) => StepState::Active,
                (None, Some(Err(_))) => StepState::Failed,
                _ => StepState::Done,
            };
            let pct = self.share_running.as_ref().map(|(f, _)| format!("{:.0}%", f * 100.0));
            step_header(ui, 2, "Share link", state, pct);
            step_body(ui, |ui| {
                if sharing && exporting {
                    // the link is known already: it can be copied (or scanned) while this runs
                    if let Some(url) = self.shown_link() {
                        widgets::link_field_qr(ui, &url, &mut self.qr_open);
                        ui.add_space(6.0);
                    }
                    widgets::hint(ui, "Uploads when the video is ready.");
                } else {
                    self.link_status_ui(ui);
                    if !sharing {
                        widgets::hint(ui, "Plays the last video shared from this project: export with \u{201c}Make a share link too\u{201d} to update it.");
                    }
                }
            });
        }

        ui.add_space(10.0);
        buttons_row(ui, |ui| {
            if exporting {
                if widgets::secondary_button(ui, "Cancel").on_hover_text("Esc").clicked() {
                    if let Some((_, _, cancel)) = &self.job {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
            } else if widgets::accent_button(ui, "Done").clicked() {
                // a link still uploading carries on; a notification says when it is ready
                self.result = None;
                self.export_shared = false;
            }
        });
    }

    /// Export settings, drawn in the export dialog over the editor. Escape, Cancel or a click
    /// outside dismisses it.
    /// Without a license: the free exports line. Returns true when Pro, Creator and sign-in
    /// should show below it (always, once the free exports are known).
    fn free_upgrade_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let pal = theme::p();
        widgets::title(ui, "License");
        let free = sf_license::FREE_EXPORTS;
        let show = match &self.free_left {
            // same layout as the answer (the upgrade buttons below), so nothing jumps
            None => {
                ui.add(egui::Label::new(RichText::new("Checking your free exports…").size(13.0).color(pal.text_dim)).wrap());
                true
            }
            Some(Ok(n)) if *n > 0 => {
                // the upgrade buttons show right under it, as when none are left
                let line = if *n == free { format!("You have {free} free exports. Upgrade for unlimited export.") } else { format!("{n} of {free} free exports left on this computer. Upgrade for unlimited export.") };
                ui.add(egui::Label::new(RichText::new(line).size(13.0).color(pal.text)).wrap());
                true
            }
            Some(Ok(_)) => {
                widgets::hint(ui, &format!("Your {free} free exports are used. Pro or Creator keeps you exporting."));
                true
            }
            Some(Err(_)) => {
                ui.add(egui::Label::new(RichText::new("Free exports need an internet connection. Connect and try again, or use a license (it works offline).").size(13.0).color(pal.danger)).wrap());
                ui.add_space(4.0);
                if ui.link(RichText::new("Try again").color(pal.text_dim)).clicked() {
                    self.free_left = None;
                }
                true
            }
        };
        if show {
            ui.add_space(4.0);
        } else {
            ui.add_space(6.0);
            ui.separator();
        }
        show
    }

    /// The video quality a share link of a GIF, ProRes or VP9 export is rendered at: its frame rate,
    /// resolution and compression, always as H.264 MP4 (what browsers play).
    fn share_settings(&self) -> crate::share::ShareSettings {
        let quality = match self.quality {
            QualityPreset::Studio => 0.95,
            QualityPreset::SocialMedia => 0.75,
            QualityPreset::WebHigh => 0.45,
            QualityPreset::WebLow => 0.2,
        };
        let (fps, max_height) = if self.format == Format::Gif { (30.0, 1080) } else { (self.fps, self.height) };
        crate::share::ShareSettings { fps, max_height, quality, include_audio: self.audio }
    }

    /// Ask the server for this project's link, once per open of the dialog.
    fn ask_project_link(&mut self, project: &str) {
        if self.project_link_for.as_deref() == Some(project) {
            return;
        }
        if self.project_link_for.as_deref().is_some_and(|p| p != project) {
            // another project's link must not show while this one's is asked for
            self.project_link = None;
            self.project_link_known = false;
        }
        self.project_link_for = Some(project.to_string());
        let (tx, rx) = channel();
        let id = project.to_string();
        std::thread::spawn(move || {
            let _ = tx.send(crate::share::project_link(&id));
        });
        self.project_link_rx = Some(rx);
    }


    /// The share being made (progress bar and Cancel), or the project's link in a text box
    /// (copy, open), or why it failed.
    fn link_status_ui(&mut self, ui: &mut egui::Ui) {
        let pal = theme::p();
        if let Some(url) = self.shown_link() {
            widgets::link_field_qr(ui, &url, &mut self.qr_open);
            ui.add_space(6.0);
        }
        if let Some((frac, label)) = self.share_running.clone() {
            progress_bar(ui, frac);
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                widgets::hint(ui, &label);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.link(RichText::new("Cancel").size(13.0).color(pal.text_dim)).on_hover_text("Stop uploading; the link keeps what it had").clicked() {
                        self.share_cancel = true;
                    }
                });
            });
            widgets::hint(ui, if self.uploaded() == Some(true) { "The link plays the previous video until this upload finishes." } else { "Copy the link now: it plays the video once the upload finishes." });
        } else if let Some(Err(e)) = self.last_link.clone() {
            ui.add(egui::Label::new(RichText::new(format!("The upload failed: {e}")).size(13.0).color(pal.danger)).wrap());
        } else if self.uploaded() == Some(false) {
            widgets::hint(ui, "Nothing is uploaded yet: export with \u{201c}Make a share link too\u{201d} to put this video behind the link.");
        }
    }

    /// This project's link: derived from its id, so it is the same every time (the server
    /// gives the project this id; `share::link_id`).
    fn shown_link(&self) -> Option<String> {
        if !self.project_id.is_empty() {
            return Some(crate::share::link_url(&self.project_id));
        }
        self.last_link.clone().and_then(Result::ok)
    }

    /// Whether a video is behind the link: the server's answer, or until it comes what this
    /// computer last knew (so the dialog does not change as it opens).
    fn uploaded(&self) -> Option<bool> {
        if matches!(self.last_link, Some(Ok(_))) || self.project_link.is_some() {
            Some(true)
        } else if self.project_link_known {
            Some(false)
        } else if self.project_id.is_empty() {
            None
        } else {
            Some(crate::share::known_uploaded(&self.project_id))
        }
    }

    /// Cloud share links (Pro): made by Export with "Make a share link too"; others see what
    /// it is.
    fn share_ui(&mut self, ui: &mut egui::Ui) {
        let pal = theme::p();
        widgets::title(ui, "Share link");
        if !crate::share::allowed() {
            widgets::hint(ui, "Pro uploads your export and puts a link on your clipboard, ready to paste anywhere.");
            ui.add_space(2.0);
            let opening = self.checkout.is_some();
            if ui.add_enabled(!opening, egui::Link::new(RichText::new(format!("Get Pro · {}", sf_license::PRICE_PRO)).color(pal.accent))).clicked() {
                self.buy("pro");
            }
            ui.add_space(4.0);
            ui.separator();
            return;
        }
        // the project's one link, always: exporting with the box ticked puts the video behind it
        self.link_status_ui(ui);
        if self.share_running.is_none() && self.uploaded() == Some(true) && !matches!(self.last_link, Some(Err(_))) {
            widgets::hint(ui, "This project's link. Export with \u{201c}Make a share link too\u{201d} to update its video.");
        }
        ui.add_space(2.0);
        if ui.checkbox(&mut self.share_with_export, "Make a share link too").on_hover_text("Export uploads the video it writes and copies a link anyone can watch").changed() {
            crate::share::set_with_export(self.share_with_export);
        }
        ui.add_space(4.0);
        ui.separator();
    }

    pub fn settings_ui(&mut self, ed: &mut Editor, ui: &mut egui::Ui) {
            ui.label(RichText::new("Export").size(18.0).strong());
            ui.add_space(4.0);
            // the settings scroll on a short window; Export and Cancel stay in view below
            let max_h = (ui.ctx().content_rect().height() - 200.0).max(160.0);
            egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                if !self.license.is_valid() && self.free_upgrade_ui(ui) {
                    let opening = self.checkout.is_some();
                    ui.horizontal_wrapped(|ui| {
                        // both ways to export: Pro with the agent, or Creator for export alone
                        if ui.add_enabled_ui(!opening, |ui| widgets::accent_button(ui, &format!("Pro · {}", sf_license::PRICE_PRO))).inner.on_hover_text("Unlimited export and the AI agent with 1,000 credits a month").clicked() {
                            self.buy("pro");
                        }
                        if ui.add_enabled_ui(!opening, |ui| widgets::secondary_button(ui, &format!("Creator · {}", sf_license::PRICE_LIFETIME))).inner.on_hover_text("Unlimited export, without the AI agent").clicked() {
                            self.buy("lifetime");
                        }
                    });
                    ui.add_space(6.0);
                    if self.promo_open {
                        ui.label("Coupon code");
                        ui.add_space(2.0);
                        let f = widgets::text_field(ui, &mut self.promo, "CODE");
                        // Enter: Pro's checkout with the code (Creator by its button)
                        if widgets::submitted(ui, &f) && !opening && !self.promo.trim().is_empty() {
                            self.buy("pro");
                        }
                    } else if ui.link(RichText::new("Have a coupon code?").color(theme::p().text_dim)).clicked() {
                        self.promo_open = true;
                    }
                    ui.add_space(4.0);
                    widgets::hint(ui, "No key or code to type: once you pay, export turns on here by itself. It works on one computer at a time.");
                    ui.add_space(6.0);
                    if !self.signing_in && self.code_sent_to.is_none() {
                        if ui.link(RichText::new("Already bought? Sign in").color(theme::p().text_dim)).clicked() {
                            self.signing_in = true;
                        }
                    } else {
                        ui.label("Email used at checkout");
                        ui.add_space(2.0);
                        let f = widgets::text_field(ui, &mut self.email, "you@example.com");
                        let mut enter = widgets::submitted(ui, &f);
                        // a code belongs to the address it was sent to
                        let awaiting_code = self.code_sent_to.as_deref() == Some(self.email.trim());
                        if awaiting_code {
                            ui.add_space(6.0);
                            ui.label("Code from the email");
                            ui.add_space(2.0);
                            let f = widgets::text_field(ui, &mut self.code, "123456");
                            enter |= widgets::submitted(ui, &f);
                        }
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
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
                                if ui.add_enabled_ui(!busy, |ui| widgets::accent_button(ui, if busy { "Checking…" } else { "Confirm" })).inner.clicked() || (enter && !busy) {
                                    let (email, code) = (self.email.clone(), self.code.clone());
                                    start(self, Box::new(move || sf_license::confirm_code(&email, &code).map(sf_license::Activation::Done)));
                                }
                                if ui.add_enabled_ui(!busy, |ui| widgets::secondary_button(ui, "Send again")).inner.clicked() {
                                    let email = self.email.clone();
                                    start(self, Box::new(move || sf_license::activate(&email, true)));
                                }
                            } else if ui.add_enabled_ui(!busy && self.email.contains('@'), |ui| widgets::accent_button(ui, if busy { "Checking…" } else { "Email me a code" })).inner.clicked() || (enter && !busy && self.email.contains('@')) {
                                let email = self.email.clone();
                                start(self, Box::new(move || sf_license::activate(&email, true)));
                            }
                            if widgets::secondary_button(ui, "Cancel").clicked() {
                                self.signing_in = false;
                                self.code_sent_to = None;
                                self.code.clear();
                                self.activation_msg = None;
                            }
                        });
                    }
                    if let Some((ok, msg)) = &self.activation_msg {
                        ui.add_space(4.0);
                        ui.colored_label(if *ok { theme::p().accent_hover } else { theme::p().danger }, msg);
                    }
                    ui.add_space(6.0);
                    ui.separator();
                }
                if self.license.is_valid() {
                    if crate::share::allowed() {
                        self.ask_project_link(&ed.session.pkg.project.id);
                    }
                    self.share_ui(ui);
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
            });
            ui.add_space(6.0);
            let secs = ed.duration_ms() / 1000.0;
            widgets::hint(ui, &format!("{:.0}s of video at {} fps", secs, self.fps));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let btn = ui.add_enabled_ui(self.can_export(), |ui| widgets::accent_button(ui, "Export")).inner;
                    let ext = match (self.format, self.codec) {
                        (Format::Gif, _) => "gif",
                        (_, VideoCodec::ProRes4444) => "mov",
                        (_, VideoCodec::Vp9) => "webm",
                        _ => "mp4",
                    };
                    // into the projects folder, next to the project and named after its package
                    // (unique there); exporting again replaces it. SF_EXPORT_OUT: UI tests pick
                    // the file
                    let out = std::env::var_os("SF_EXPORT_OUT").map(PathBuf::from).unwrap_or_else(|| {
                        let stem = ed.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| ed.session.pkg.project.name.clone());
                        sf_session::create::default_projects_folder().join(format!("{stem}.{ext}"))
                    });
                    let hover = format!("Saves {} in {}", out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), out.parent().map(|d| d.display().to_string()).unwrap_or_default());
                    if btn.on_hover_text(hover).clicked() {
                        if let Some(dir) = out.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        self.start(ed, out);
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
        self.free_job = !self.license.is_valid();
        if self.free_job {
            // one used, as far as this dialog knows (the server counts it once it finishes)
            if let Some(Ok(n)) = &mut self.free_left {
                *n = n.saturating_sub(1);
            }
        }
        self.started = Some(std::time::Instant::now());
        self.progress = None;
        self.result = None;
        // step 2 of the progress dialog: the link, once the file is written
        self.export_shared = self.share_with_export && crate::share::allowed();
        if self.export_shared {
            self.last_link = None;
        }
        self.job = Some((rx, handle, cancel));
        self.open = false;
    }
}

#[derive(Clone, Copy, PartialEq)]
enum StepState {
    Waiting,
    Active,
    Done,
    Failed,
}

/// A numbered step of the export dialog: its marker (number, or a check once done), its name,
/// and on the right its percentage while it runs.
fn step_header(ui: &mut egui::Ui, n: u32, name: &str, state: StepState, right: Option<String>) {
    let pal = theme::p();
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
        let c = rect.center();
        let painter = ui.painter();
        match state {
            StepState::Done => {
                painter.circle_filled(c, 9.0, pal.accent);
                widgets::paint_icon(painter, rect.shrink(4.5), widgets::Icon::Check, pal.bg);
            }
            StepState::Failed => {
                painter.circle_filled(c, 9.0, pal.danger);
                widgets::paint_icon(painter, rect.shrink(5.0), widgets::Icon::Close, pal.bg);
            }
            StepState::Active | StepState::Waiting => {
                let color = if state == StepState::Active { pal.accent } else { pal.text_faint };
                painter.circle_stroke(c, 8.25, egui::Stroke::new(1.5, color));
                painter.text(c, egui::Align2::CENTER_CENTER, n.to_string(), egui::FontId::proportional(11.0), color);
            }
        }
        let text = if state == StepState::Waiting { pal.text_dim } else { pal.text };
        ui.label(RichText::new(name).size(14.0).color(text));
        if let Some(r) = right {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(r).size(13.0).color(pal.text_dim));
            });
        }
    });
    ui.add_space(4.0);
}

/// What a step shows under its header, lined up with the step's name (past the 18 px marker)
/// and as wide as the box from there.
fn step_body(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let left = 18.0 + ui.spacing().item_spacing.x;
    egui::Frame::NONE.inner_margin(egui::Margin { left: left as i8, right: 0, top: 0, bottom: 0 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

/// A full-width bar painted by hand (egui's ProgressBar shrinks to its label in these dialogs).
fn progress_bar(ui: &mut egui::Ui, frac: f32) {
    let pal = theme::p();
    let (bar, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
    ui.painter().rect_filled(bar, egui::CornerRadius::same(2), pal.card_3);
    let fill = egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * frac.clamp(0.0, 1.0), bar.height()));
    ui.painter().rect_filled(fill, egui::CornerRadius::same(2), pal.accent);
}

/// Buttons along the bottom of the progress box, right-aligned in one row. A bare
/// right-to-left layout would centre itself in all the height left in the box, which keeps the
/// settings' (taller) size: a band of empty space above the buttons.
fn buttons_row(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
}
